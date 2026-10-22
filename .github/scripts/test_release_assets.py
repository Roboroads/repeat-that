"""Unit tests for release_assets.py. Run with: python3 -m unittest discover -s .github/scripts

File names and hashes are made up in memory; only the CLI tests at the bottom touch a temporary folder (with a few
bytes of text, nothing that looks like audio).
"""

import base64
import contextlib
import dataclasses
import hashlib
import io
import json
import os
import tempfile
import unittest
import unittest.mock
from pathlib import Path

import release_assets as ra
from release_assets import ReleaseError

EXE = "Repeat-That_0.2.0_x64-setup.exe"
DEB = "Repeat-That_0.2.0_amd64.deb"
RPM = "Repeat-That-0.2.0-1.x86_64.rpm"
APPIMAGE = "Repeat-That_0.2.0_amd64.AppImage"
FLATPAK = "Repeat-That_0.2.0_x86_64.flatpak"
DMG = "Repeat-That_0.2.0_aarch64.dmg"
APP = "Repeat-That_0.2.0_aarch64.app.tar.gz"  # the macOS update bundle
LINUX = [DEB, RPM, APPIMAGE]
# Everything a release run with today's table produces: Windows, Linux and the Flatpak are required.
REQUIRED_FILES = [EXE, *LINUX, FLATPAK]
# The update files, which only exist once an update key does: a signature for each payload, a manifest for each
# platform, and for macOS the bundle itself.
EXE_SIG, APPIMAGE_SIG, APP_SIG = f"{EXE}.sig", f"{APPIMAGE}.sig", f"{APP}.sig"
WINDOWS_MANIFEST, LINUX_MANIFEST, MACOS_MANIFEST = (
    "latest-windows-x86_64.json",
    "latest-linux-x86_64.json",
    "latest-darwin-aarch64.json",
)
REQUIRED_UPDATE_FILES = [EXE_SIG, WINDOWS_MANIFEST, APPIMAGE_SIG, LINUX_MANIFEST]
MACOS_UPDATE_FILES = [APP, APP_SIG, MACOS_MANIFEST]

H1, H2, H3, H4, HA = ("1" * 64, "2" * 64, "3" * 64, "4" * 64, "a" * 64)


def line(name, digest):
    return f"{digest}  {name}"


def sums_text(*pairs):
    """A SHA256SUMS body from (name, hash) pairs, in the order given."""
    return "".join(line(name, digest) + "\n" for name, digest in pairs)


def all_built(platforms=ra.PLATFORMS):
    """The platform table with a build job for every platform in publish-release.yml (today's table has that)."""
    return tuple(dataclasses.replace(platform, build=True) for platform in platforms)


def without_job(name, platforms=ra.PLATFORMS):
    """The platform table as it was before platform `name` got its build job."""
    return tuple(dataclasses.replace(p, build=False) if p.name == name else p for p in platforms)


class PlatformTableTests(unittest.TestCase):
    def test_required_and_optional(self):
        required = {platform.name: platform.required for platform in ra.PLATFORMS}
        self.assertEqual(required, {"windows": True, "linux": True, "flatpak": True, "macos": False})

    def test_every_platform_has_a_build_job(self):
        built = [platform.name for platform in ra.PLATFORMS if platform.build]
        self.assertEqual(built, ["windows", "linux", "flatpak", "macos"])

    def test_the_flatpak_is_built_by_its_own_workflow(self):
        flatpak = next(platform for platform in ra.PLATFORMS if platform.name == "flatpak")
        self.assertEqual(flatpak.workflow, "build-flatpak.yml")

    def test_no_two_platforms_claim_the_same_file(self):
        every_file = (EXE, DEB, RPM, APPIMAGE, FLATPAK, DMG, *REQUIRED_UPDATE_FILES, *MACOS_UPDATE_FILES)
        for name in every_file:
            matches = [p.name for p in ra.PLATFORMS if any(ra.matches(pattern, name) for pattern in p.all_patterns)]
            self.assertEqual(len(matches), 1, f"{name} matched {matches}")

    def test_the_update_files_of_each_platform(self):
        update = {platform.name: platform.update_patterns for platform in ra.PLATFORMS}
        self.assertEqual(
            update,
            {
                "windows": ("*-setup.exe.sig", "latest-windows-x86_64.json"),
                "linux": ("*.AppImage.sig", "latest-linux-x86_64.json"),
                "flatpak": (),  # Flathub updates it, so it never has update files
                "macos": ("*.app.tar.gz", "*.app.tar.gz.sig", "latest-darwin-aarch64.json"),
            },
        )

    def test_the_update_patterns_are_not_release_patterns(self):
        # check_assets_full requires every pattern of a required platform to be produced, which would break a
        # release made without an update key.
        for platform in ra.PLATFORMS:
            self.assertFalse(set(platform.patterns) & set(platform.update_patterns), platform.name)


class ClassifyTests(unittest.TestCase):
    def test_every_asset_kind_maps_to_its_platform(self):
        expected = {
            EXE: "windows",
            DEB: "linux",
            RPM: "linux",
            APPIMAGE: "linux",
            FLATPAK: "flatpak",
            DMG: "macos",
        }
        for name, platform in expected.items():
            self.assertEqual(ra.classify(name), platform, name)

    def test_every_update_file_maps_to_its_platform(self):
        expected = {
            EXE_SIG: "windows",
            WINDOWS_MANIFEST: "windows",
            APPIMAGE_SIG: "linux",
            LINUX_MANIFEST: "linux",
            APP: "macos",
            APP_SIG: "macos",
            MACOS_MANIFEST: "macos",
        }
        for name, platform in expected.items():
            self.assertEqual(ra.classify(name), platform, name)

    def test_the_update_files_of_a_package_or_the_flatpak_belong_to_nobody(self):
        # Only the AppImage updates itself on Linux, and the Flatpak never does.
        for name in (f"{DEB}.sig", f"{RPM}.sig", f"{FLATPAK}.sig", "latest-flatpak-x86_64.json", "latest.json"):
            self.assertIsNone(ra.classify(name), name)

    def test_a_manifest_of_a_system_without_updates_belongs_to_nobody(self):
        for name in ("latest-windows-aarch64.json", "latest-darwin-x86_64.json", "latest-linux-aarch64.json"):
            self.assertIsNone(ra.classify(name), name)

    def test_uses_the_file_name_not_the_folder(self):
        self.assertEqual(ra.classify(f"assets/sub/{DEB}"), "linux")

    def test_unknown_files_have_no_platform(self):
        for name in ("notes.txt", "Repeat-That_0.2.0_x64.msi", "Repeat-That_0.2.0_x64.exe", "SHA256SUMS", "a.deb.sig"):
            self.assertIsNone(ra.classify(name), name)

    def test_matching_is_case_sensitive(self):
        self.assertIsNone(ra.classify("Repeat-That_0.2.0_amd64.appimage"))

    def test_classify_files_groups_by_platform(self):
        grouped = ra.classify_files([f"out/{EXE}", DEB, RPM, DMG])
        self.assertEqual(grouped, {"windows": [EXE], "linux": [DEB, RPM], "macos": [DMG]})

    def test_classify_files_rejects_an_unknown_file_and_names_it(self):
        with self.assertRaises(ReleaseError) as caught:
            ra.classify_files([EXE, "installer.zip"])
        self.assertIn("installer.zip", str(caught.exception))
        self.assertNotIn(EXE, str(caught.exception))


class ParseSumsTests(unittest.TestCase):
    def test_parses_text_mode_lines(self):
        parsed = ra.parse_sums(sums_text((EXE, H1), (DMG, H2)))
        self.assertEqual(parsed, {EXE: line(EXE, H1), DMG: line(DMG, H2)})

    def test_rejects_lines_it_could_not_keep_byte_for_byte(self):
        for bad in (
            f"{H1} *{EXE}\n",  # binary mode marker
            f"{HA.upper()}  {EXE}\n",  # uppercase hash
            f"{H1[:-1]}  {EXE}\n",  # short hash
            "not a checksum line\n",
        ):
            with self.assertRaises(ReleaseError, msg=bad):
                ra.parse_sums(bad)

    def test_rejects_the_same_name_twice(self):
        with self.assertRaises(ReleaseError):
            ra.parse_sums(sums_text((EXE, H1), (EXE, H2)))

    def test_blank_text_is_an_empty_list(self):
        self.assertEqual(ra.parse_sums(""), {})


class SumsTests(unittest.TestCase):
    def test_output_is_sorted_text_mode_with_a_final_newline(self):
        text = ra.build_sums({EXE: H1, DEB: H2, RPM: H3})
        # Plain byte order: "Repeat-That-" sorts before "Repeat-That_", and the two .. names by their next letter.
        self.assertEqual(text, sums_text((RPM, H3), (DEB, H2), (EXE, H1)))

    def test_matches_sha256sum_text_format(self):
        text = ra.build_sums({"a.deb": H1})
        self.assertEqual(text, f"{H1}  a.deb\n")  # two spaces: `sha256sum --text`

    def test_existing_lines_are_kept_byte_for_byte(self):
        existing = sums_text((EXE, H1), (DMG, H2))
        text = ra.build_sums({DEB: H3}, existing=existing)
        self.assertEqual(text, sums_text((DMG, H2), (DEB, H3), (EXE, H1)))
        for old_line in existing.splitlines():
            self.assertIn(old_line + "\n", text)

    def test_existing_lines_in_any_order_come_out_sorted_but_unchanged(self):
        existing = sums_text((EXE, H1), (DEB, H2))  # not in sorted order
        text = ra.build_sums({}, existing=existing)
        self.assertEqual(text, sums_text((DEB, H2), (EXE, H1)))

    def test_adding_a_file_with_the_same_hash_is_fine(self):
        existing = sums_text((EXE, H1))
        self.assertEqual(ra.build_sums({EXE: H1}, existing=existing), existing)

    def test_fails_when_an_existing_line_would_change(self):
        existing = sums_text((DMG, H1))
        with self.assertRaises(ReleaseError) as caught:
            ra.build_sums({DMG: H2}, existing=existing)
        self.assertIn(DMG, str(caught.exception))

    def test_replace_lets_an_added_file_change_its_own_line_only(self):
        existing = sums_text((EXE, H1), (DMG, H2))
        text = ra.build_sums({DMG: H3}, existing=existing, replace=True)
        self.assertEqual(text, sums_text((DMG, H3), (EXE, H1)))

    def test_without_replace_one_changed_hash_fails_among_other_added_files(self):
        existing = sums_text((EXE, H1), (DMG, H2))
        with self.assertRaises(ReleaseError):
            ra.build_sums({EXE: H4, DMG: H2}, existing=existing, replace=False)

    def test_keep_carries_only_the_named_lines(self):
        existing = sums_text((EXE, H1), (DEB, H2), (DMG, H3))
        text = ra.build_sums({EXE: H4}, existing=existing, keep=[DMG])
        # The old .exe line is dropped (the run produced a new .exe), the .dmg line is carried unchanged.
        self.assertEqual(text, sums_text((DMG, H3), (EXE, H4)))

    def test_keep_with_no_names_carries_nothing(self):
        text = ra.build_sums({EXE: H4}, existing=sums_text((EXE, H1)), keep=[])
        self.assertEqual(text, sums_text((EXE, H4)))

    def test_keep_fails_for_a_name_without_a_line(self):
        with self.assertRaises(ReleaseError) as caught:
            ra.build_sums({EXE: H4}, existing=sums_text((EXE, H1)), keep=[DMG])
        self.assertIn(DMG, str(caught.exception))

    def test_keep_fails_when_the_run_also_produced_that_name(self):
        with self.assertRaises(ReleaseError):
            ra.build_sums({DMG: H4}, existing=sums_text((DMG, H3)), keep=[DMG])

    def test_check_preserved_fails_when_an_old_line_changes(self):
        old = ra.parse_sums(sums_text((EXE, H1), (DMG, H2)))
        new = ra.parse_sums(sums_text((EXE, H1), (DMG, H3)))
        with self.assertRaises(ReleaseError) as caught:
            ra.check_preserved(old, new)
        self.assertIn(DMG, str(caught.exception))

    def test_check_preserved_fails_when_an_old_line_vanishes(self):
        old = ra.parse_sums(sums_text((EXE, H1), (DMG, H2)))
        new = ra.parse_sums(sums_text((EXE, H1)))
        with self.assertRaises(ReleaseError) as caught:
            ra.check_preserved(old, new)
        self.assertIn(DMG, str(caught.exception))

    def test_check_preserved_accepts_added_lines(self):
        old = ra.parse_sums(sums_text((EXE, H1)))
        new = ra.parse_sums(sums_text((EXE, H1), (DMG, H2)))
        ra.check_preserved(old, new)

    def test_check_preserved_allows_listed_names_to_change(self):
        old = ra.parse_sums(sums_text((EXE, H1), (DMG, H2)))
        new = ra.parse_sums(sums_text((EXE, H1), (DMG, H3)))
        ra.check_preserved(old, new, replaceable={DMG})

    def test_check_preserved_still_fails_for_a_listed_name_that_vanishes(self):
        old = ra.parse_sums(sums_text((EXE, H1), (DMG, H2)))
        new = ra.parse_sums(sums_text((EXE, H1)))
        with self.assertRaises(ReleaseError):
            ra.check_preserved(old, new, replaceable={DMG})

    def test_drop_removes_the_line_of_a_file_that_leaves_the_release(self):
        old_dmg = "Repeat-That_0.1.9_aarch64.dmg"
        existing = sums_text((EXE, H1), (old_dmg, H2))
        text = ra.build_sums({DMG: H3}, existing=existing, replace=True, drop=[old_dmg])
        self.assertEqual(text, sums_text((DMG, H3), (EXE, H1)))

    def test_drop_of_a_name_without_a_line_is_fine(self):
        # An interrupted replace already dropped the line; running again must still work.
        text = ra.build_sums({DMG: H3}, existing=sums_text((EXE, H1)), drop=["Repeat-That_0.1.9_aarch64.dmg"])
        self.assertEqual(text, sums_text((DMG, H3), (EXE, H1)))

    def test_drop_cannot_name_an_added_file(self):
        with self.assertRaises(ReleaseError):
            ra.build_sums({DMG: H3}, existing=sums_text((EXE, H1)), drop=[DMG])

    def test_without_drop_a_line_cannot_vanish(self):
        old = ra.parse_sums(sums_text((EXE, H1), (DMG, H2)))
        new = ra.parse_sums(sums_text((EXE, H1)))
        ra.check_preserved(old, new, removable={DMG})
        with self.assertRaises(ReleaseError):
            ra.check_preserved(old, new)


class VersionTests(unittest.TestCase):
    def test_every_file_name_must_carry_the_tags_version(self):
        ra.check_versions([EXE, DEB, RPM, APPIMAGE, FLATPAK, DMG], "v0.2.0")

    def test_another_version_fails_and_names_the_file(self):
        with self.assertRaises(ReleaseError) as caught:
            ra.check_versions([DMG, "Repeat-That_0.1.9_aarch64.dmg"], "v0.2.0")
        self.assertIn("Repeat-That_0.1.9_aarch64.dmg", str(caught.exception))
        self.assertNotIn(DMG, str(caught.exception))

    def test_a_longer_version_does_not_match_a_shorter_tag(self):
        for name in ("Repeat-That_0.2.0.1_aarch64.dmg", "Repeat-That_10.2.0_aarch64.dmg"):
            with self.assertRaises(ReleaseError, msg=name):
                ra.check_versions([name], "v0.2.0")

    def test_a_file_without_a_version_fails(self):
        with self.assertRaises(ReleaseError):
            ra.check_versions(["Repeat-That_aarch64.dmg"], "v0.2.0")

    def test_the_update_files_carry_the_version_but_the_manifests_do_not_have_to(self):
        # latest-<target>-<arch>.json is the name the app asks for, so it can't say which release it is of.
        ra.check_versions([EXE_SIG, APPIMAGE_SIG, APP, APP_SIG], "v0.2.0")
        ra.check_versions([WINDOWS_MANIFEST, LINUX_MANIFEST, MACOS_MANIFEST], "v0.2.0")
        with self.assertRaises(ReleaseError) as caught:
            ra.check_versions([f"{APP}.sig".replace("0.2.0", "0.1.9"), MACOS_MANIFEST], "v0.2.0")
        self.assertEqual(len(caught.exception.errors), 1)
        self.assertIn("0.1.9", str(caught.exception))


RUN = {
    "path": ".github/workflows/build-macos.yml",
    "event": "workflow_dispatch",
    "status": "completed",
    "conclusion": "success",
    "head_branch": "main",
    "head_sha": "a" * 40,
    "head_repository": {"full_name": "Roboroads/repeat-that"},
}


class SourceRunTests(unittest.TestCase):
    """A late attach from `artifacts_run_id` trusts files built by another run, so that run is checked."""

    def check(self, compare="ahead", platform="macos", **changes):
        run = {**RUN, **changes}
        return ra.check_source_run(run, {"status": compare}, platform, "Roboroads/repeat-that", "main")

    def test_a_successful_dispatched_build_on_main_passes(self):
        self.check()

    def test_a_commit_equal_to_the_tag_passes(self):
        self.check(compare="identical")

    def test_a_commit_that_is_not_in_the_tags_history_fails(self):
        for state in ("behind", "diverged", None):
            with self.assertRaises(ReleaseError, msg=state):
                self.check(compare=state)

    def test_another_workflow_fails(self):
        paths = [f".github/workflows/{name}" for name in ("build-windows.yml", "publish-release.yml")]
        for path in [*paths, "x/build-macos.yml"]:
            with self.assertRaises(ReleaseError, msg=path) as caught:
                self.check(path=path)
            self.assertIn("build-macos.yml", str(caught.exception))

    def test_a_run_that_did_not_succeed_fails(self):
        unfinished = {"status": "in_progress", "conclusion": None}
        for changes in ({"conclusion": "failure"}, {"conclusion": "cancelled"}, unfinished):
            with self.assertRaises(ReleaseError, msg=str(changes)):
                self.check(**changes)

    def test_a_pull_request_build_fails(self):
        # Pull request builds are not notarized.
        for event in ("pull_request", "pull_request_target", "push", "schedule"):
            with self.assertRaises(ReleaseError, msg=event):
                self.check(event=event)

    def test_a_run_from_another_branch_fails(self):
        with self.assertRaises(ReleaseError):
            self.check(head_branch="ci/other")

    def test_a_run_from_a_fork_fails_even_on_a_branch_called_main(self):
        with self.assertRaises(ReleaseError):
            self.check(head_repository={"full_name": "someone/repeat-that"})

    def test_a_platform_that_is_required_or_unknown_fails(self):
        for platform in ("windows", "beos"):
            with self.assertRaises(ReleaseError, msg=platform):
                self.check(platform=platform)

    def test_every_problem_is_reported_at_once(self):
        with self.assertRaises(ReleaseError) as caught:
            self.check(compare="diverged", conclusion="failure", head_branch="dev")
        self.assertEqual(len(caught.exception.errors), 3)


class SelectArtifactsTests(unittest.TestCase):
    """Publishing only uses the artifacts of platform jobs that succeeded."""

    @staticmethod
    def results(**states):
        return {job: {"result": state} for job, state in states.items()}

    def select(self, names, platforms=None, **states):
        return ra.select_artifacts(names, self.results(**states), platforms=platforms or all_built())

    def test_artifacts_of_succeeded_jobs_are_selected(self):
        names = [f"release-windows-{EXE}", f"release-macos-{DMG}"]
        selected, ignored = self.select(names, windows="success", macos="success")
        self.assertEqual((selected, ignored), (names, []))

    def test_an_optional_platform_that_failed_after_uploading_contributes_nothing(self):
        names = [f"release-windows-{EXE}", f"release-macos-{DMG}"]
        selected, ignored = self.select(names, windows="success", macos="failure")
        self.assertEqual(selected, [f"release-windows-{EXE}"])
        self.assertEqual([name for name, _ in ignored], [f"release-macos-{DMG}"])
        self.assertIn("macOS", ignored[0][1])
        self.assertIn("failure", ignored[0][1])

    def test_a_skipped_or_cancelled_job_contributes_nothing(self):
        for state in ("skipped", "cancelled"):
            selected, _ = self.select([f"release-macos-{DMG}"], windows="success", macos=state)
            self.assertEqual(selected, [], state)

    def test_an_artifact_that_belongs_to_no_platform_fails(self):
        for name in ("release-beos-x.zip", "release-windows", "SHA256SUMS", "release-"):
            with self.assertRaises(ReleaseError, msg=name):
                self.select([name], windows="success")

    def test_an_artifact_of_a_platform_without_a_job_fails(self):
        with self.assertRaises(ReleaseError) as caught:
            ra.select_artifacts([f"release-macos-{DMG}"], self.results(windows="success"), platforms=all_built())
        self.assertIn("macos", str(caught.exception))

    def test_the_table_decides_which_platforms_have_jobs(self):
        # Before the Flatpak had a build job, a flatpak artifact was out of step.
        with self.assertRaises(ReleaseError):
            ra.select_artifacts(
                [f"release-flatpak-{FLATPAK}"],
                self.results(windows="success", flatpak="success"),
                platforms=without_job("flatpak"),
            )

    def test_with_todays_table_the_flatpak_artifact_is_selected(self):
        names = [f"release-windows-{EXE}", f"release-flatpak-{FLATPAK}"]
        selected, ignored = ra.select_artifacts(names, self.results(windows="success", flatpak="success"))
        self.assertEqual((selected, ignored), (names, []))

    def test_with_todays_table_a_failed_flatpak_build_contributes_nothing(self):
        names = [f"release-windows-{EXE}", f"release-flatpak-{FLATPAK}"]
        selected, ignored = ra.select_artifacts(names, self.results(windows="success", flatpak="failure"))
        self.assertEqual(selected, [f"release-windows-{EXE}"])
        self.assertEqual(len(ignored), 1)

    def test_with_todays_table_the_linux_artifacts_are_selected(self):
        names = [f"release-windows-{EXE}"] + [f"release-linux-{name}" for name in LINUX]
        selected, ignored = ra.select_artifacts(names, self.results(windows="success", linux="success"))
        self.assertEqual((selected, ignored), (names, []))

    def test_with_todays_table_a_failed_linux_build_contributes_nothing(self):
        names = [f"release-windows-{EXE}"] + [f"release-linux-{name}" for name in LINUX]
        selected, ignored = ra.select_artifacts(names, self.results(windows="success", linux="failure"))
        self.assertEqual(selected, [f"release-windows-{EXE}"])
        self.assertEqual(len(ignored), 3)

    def test_with_todays_table_a_macos_artifact_is_selected(self):
        names = [f"release-windows-{EXE}", f"release-macos-{DMG}"]
        selected, ignored = ra.select_artifacts(names, self.results(windows="success", macos="success"))
        self.assertEqual((selected, ignored), (names, []))


class CheckAssetsFullTests(unittest.TestCase):
    """Full mode: what is already on the release against what this run produced."""

    # What a release run produces with today's table: Windows, Linux and the Flatpak are required and have build jobs.
    produced = REQUIRED_FILES

    def test_a_fresh_release_with_the_produced_files_passes(self):
        self.assertEqual(ra.check_assets_full([], self.produced), [])

    def test_assets_from_this_runs_files_and_sha256sums_pass(self):
        self.assertEqual(ra.check_assets_full([*REQUIRED_FILES, "SHA256SUMS"], self.produced), [])

    def test_an_unexpected_asset_fails_and_is_named(self):
        with self.assertRaises(ReleaseError) as caught:
            ra.check_assets_full([EXE, "debug-symbols.zip"], self.produced)
        self.assertIn("debug-symbols.zip", str(caught.exception))

    def test_an_asset_of_a_required_platform_that_was_not_produced_fails(self):
        old = "Repeat-That_0.1.9_amd64.deb"
        with self.assertRaises(ReleaseError) as caught:
            ra.check_assets_full([EXE, old], self.produced)
        self.assertIn(old, str(caught.exception))

    def test_a_dmg_from_an_earlier_late_attach_is_kept_when_macos_was_not_produced(self):
        kept = ra.check_assets_full([EXE, DMG, "SHA256SUMS"], self.produced)
        self.assertEqual(kept, [DMG])

    def test_a_dmg_is_replaced_not_kept_when_macos_was_produced(self):
        produced = [*REQUIRED_FILES, DMG]
        self.assertEqual(ra.check_assets_full([EXE, DMG, "SHA256SUMS"], produced, platforms=all_built()), [])

    def test_a_dmg_with_another_name_than_the_produced_one_fails(self):
        with self.assertRaises(ReleaseError) as caught:
            ra.check_assets_full(
                [EXE, "Repeat-That_0.1.9_aarch64.dmg"], [EXE, *LINUX, FLATPAK, DMG], platforms=all_built()
            )
        self.assertEqual(len(caught.exception.errors), 1)
        self.assertIn("0.1.9", str(caught.exception))

    def test_linux_must_produce_the_deb_the_rpm_and_the_appimage(self):
        for missing in LINUX:
            produced = [EXE, FLATPAK] + [name for name in LINUX if name != missing]
            with self.assertRaises(ReleaseError, msg=missing) as caught:
                ra.check_assets_full([], produced)
            self.assertEqual(len(caught.exception.errors), 1, missing)
            self.assertIn(f"Linux produced no *.{missing.rsplit('.', 1)[1]} file", str(caught.exception))

    def test_linux_that_produced_nothing_fails_once(self):
        with self.assertRaises(ReleaseError) as caught:
            ra.check_assets_full([], [EXE, FLATPAK])
        self.assertEqual(caught.exception.errors, ["Linux produced no files"])

    def test_a_flatpak_that_produced_nothing_fails_once(self):
        with self.assertRaises(ReleaseError) as caught:
            ra.check_assets_full([], [EXE, *LINUX])
        self.assertEqual(caught.exception.errors, ["Flatpak produced no files"])

    def test_a_built_required_platform_that_produced_nothing_fails(self):
        with self.assertRaises(ReleaseError) as caught:
            ra.check_assets_full([], [EXE], platforms=all_built())
        self.assertIn("Linux produced no files", str(caught.exception))
        self.assertIn("Flatpak produced no files", str(caught.exception))
        self.assertNotIn("macOS", str(caught.exception))  # optional

    def test_a_platform_without_a_build_job_may_produce_nothing(self):
        produced = [EXE, *LINUX]
        self.assertEqual(ra.check_assets_full([], produced, platforms=without_job("flatpak")), [])

    def test_a_produced_file_of_no_platform_fails(self):
        with self.assertRaises(ReleaseError):
            ra.check_assets_full([], [*self.produced, "stray.log"])

    def test_every_problem_is_reported_at_once(self):
        with self.assertRaises(ReleaseError) as caught:
            ra.check_assets_full(["one.zip", "two.zip"], self.produced)
        self.assertEqual(len(caught.exception.errors), 2)


class CheckAssetsFullUpdaterTests(unittest.TestCase):
    """Full mode with an update key: the update files go with their platform's files."""

    produced = REQUIRED_FILES
    signed = [*REQUIRED_FILES, *REQUIRED_UPDATE_FILES]

    def check(self, produced, release=(), updater=True, platforms=None):
        return ra.check_assets_full(list(release), list(produced), platforms=platforms or all_built(), updater=updater)

    def test_a_release_without_update_files_passes_without_the_updater_flag(self):
        # No update key yet: the files of today's releases are all there is.
        self.assertEqual(self.check(self.produced, updater=False), [])

    def test_a_release_with_every_update_file_passes_with_the_flag_and_without(self):
        self.assertEqual(self.check(self.signed), [])
        self.assertEqual(self.check(self.signed, updater=False), [])

    def test_the_flag_requires_every_update_file_of_every_built_platform_and_names_the_missing_one(self):
        patterns = {
            EXE_SIG: "*-setup.exe.sig",
            WINDOWS_MANIFEST: WINDOWS_MANIFEST,
            APPIMAGE_SIG: "*.AppImage.sig",
            LINUX_MANIFEST: LINUX_MANIFEST,
        }
        for missing, pattern in patterns.items():
            produced = [name for name in self.signed if name != missing]
            with self.assertRaises(ReleaseError, msg=missing) as caught:
                self.check(produced)
            self.assertIn(f"produced no {pattern} file", str(caught.exception))

    def test_a_missing_signature_is_named_by_its_pattern_and_its_payload(self):
        with self.assertRaises(ReleaseError) as caught:
            self.check([name for name in self.signed if name != EXE_SIG])
        self.assertEqual(
            caught.exception.errors,
            ["Windows produced no *-setup.exe.sig file", f"{EXE} has no signature: {EXE_SIG} is missing"],
        )

    def test_a_missing_manifest_is_named(self):
        with self.assertRaises(ReleaseError) as caught:
            self.check([name for name in self.signed if name != LINUX_MANIFEST])
        self.assertEqual(caught.exception.errors, [f"Linux produced no {LINUX_MANIFEST} file"])

    def test_no_update_files_at_all_fails_with_the_flag_for_each_platform(self):
        with self.assertRaises(ReleaseError) as caught:
            self.check(self.produced)
        self.assertEqual(
            caught.exception.errors,
            [
                "Windows produced no *-setup.exe.sig file",
                f"Windows produced no {WINDOWS_MANIFEST} file",
                "Linux produced no *.AppImage.sig file",
                f"Linux produced no {LINUX_MANIFEST} file",
                f"{EXE} has no signature: {EXE_SIG} is missing",
                f"{APPIMAGE} has no signature: {APPIMAGE_SIG} is missing",
            ],
        )

    def test_the_flatpak_has_no_update_files_and_is_never_asked_for_any(self):
        self.assertEqual(self.check(self.signed), [])  # FLATPAK is in REQUIRED_FILES and has none

    def test_macos_that_was_not_built_needs_no_update_files(self):
        self.assertEqual(self.check(self.signed), [])

    def test_macos_that_was_built_needs_the_bundle_its_signature_and_its_manifest(self):
        self.assertEqual(self.check([*self.signed, DMG, *MACOS_UPDATE_FILES]), [])
        for missing in MACOS_UPDATE_FILES:
            produced = [*self.signed, DMG, *[name for name in MACOS_UPDATE_FILES if name != missing]]
            with self.assertRaises(ReleaseError, msg=missing) as caught:
                self.check(produced)
            self.assertIn(missing.replace(APP, "*.app.tar.gz"), str(caught.exception))

    def test_update_files_that_are_only_partly_there_fail_even_without_the_flag(self):
        # An update bundle without its signature must not be attached just because the key is off: it would sit on
        # the release without anything that uses it.
        with self.assertRaises(ReleaseError) as caught:
            self.check([*self.produced, DMG, APP], updater=False, platforms=all_built())
        self.assertIn("*.app.tar.gz.sig", str(caught.exception))
        self.assertIn(MACOS_MANIFEST, str(caught.exception))

    def test_a_signature_without_its_payload_fails(self):
        with self.assertRaises(ReleaseError) as caught:
            self.check([*self.signed, DMG, *MACOS_UPDATE_FILES[1:]])
        self.assertIn(f"{APP_SIG} is the signature of {APP}, which is missing", str(caught.exception))

    def test_a_dmg_and_its_update_files_from_an_earlier_late_attach_are_kept_when_macos_was_not_built(self):
        release = [*self.signed, DMG, *MACOS_UPDATE_FILES, "SHA256SUMS"]
        self.assertEqual(self.check(self.signed, release=release), [DMG, *MACOS_UPDATE_FILES])

    def test_every_macos_file_is_replaced_not_kept_when_macos_was_built(self):
        release = [*self.signed, DMG, *MACOS_UPDATE_FILES, "SHA256SUMS"]
        self.assertEqual(self.check([*self.signed, DMG, *MACOS_UPDATE_FILES], release=release), [])

    def test_an_update_file_of_a_required_platform_that_was_not_produced_fails(self):
        # A draft that an earlier attempt gave signatures, and this run has no key: nothing lists them.
        with self.assertRaises(ReleaseError) as caught:
            self.check(self.produced, release=[EXE, EXE_SIG], updater=False)
        self.assertIn(EXE_SIG, str(caught.exception))

    def test_the_table_decides_which_platforms_have_update_files(self):
        # Without a build job for a platform nothing is asked of it.
        platforms = without_job("linux")
        produced = [EXE, FLATPAK, EXE_SIG, WINDOWS_MANIFEST]
        self.assertEqual(self.check(produced, platforms=platforms), [])


class CheckAssetsLateTests(unittest.TestCase):
    """Late mode: add one optional platform's files to a release that has its own SHA256SUMS."""

    release = [EXE, "SHA256SUMS"]
    sums = sums_text((EXE, H1))

    def check(self, produced=(DMG,), release=None, sums=None, platform="macos", replace=False, tag=None):
        return ra.check_assets_late(
            self.release if release is None else release,
            list(produced),
            platform,
            self.sums if sums is None else sums,
            replace=replace,
            tag=tag,
        )

    def test_adding_the_platforms_files_passes(self):
        plan = self.check()
        self.assertEqual((plan.pending, plan.overwrite, plan.remove), ([], [], []))

    def test_a_file_whose_version_is_not_the_tags_fails(self):
        with self.assertRaises(ReleaseError) as caught:
            self.check(produced=["Repeat-That_0.1.9_aarch64.dmg"], tag="v0.2.0")
        self.assertIn("0.1.9", str(caught.exception))
        self.check(tag="v0.2.0")  # DMG is 0.2.0

    def test_the_update_files_are_added_with_the_dmg_as_new_lines_only(self):
        # The release was published with Windows and Linux files (and their update files); macOS comes later.
        release = [EXE, EXE_SIG, WINDOWS_MANIFEST, "SHA256SUMS"]
        sums = sums_text((EXE, H1), (EXE_SIG, H2), (WINDOWS_MANIFEST, H3))
        produced = [DMG, *MACOS_UPDATE_FILES]
        plan = self.check(produced=produced, release=release, sums=sums, tag="v0.2.0")
        self.assertEqual((plan.pending, plan.overwrite, plan.remove), ([], [], []))
        text = ra.build_sums({name: H4 for name in produced}, existing=sums)
        for old_line in sums.splitlines():
            self.assertIn(old_line + "\n", text)
        self.assertEqual(len(text.splitlines()), len(sums.splitlines()) + len(produced))

    def test_the_manifest_is_exempt_from_the_version_check_but_the_bundle_and_signature_are_not(self):
        self.check(produced=[DMG, *MACOS_UPDATE_FILES], tag="v0.2.0")
        old = [name.replace("0.2.0", "0.1.9") for name in (DMG, APP, APP_SIG)]
        with self.assertRaises(ReleaseError) as caught:
            self.check(produced=[*old, MACOS_MANIFEST], tag="v0.2.0")
        self.assertEqual(len(caught.exception.errors), 3)
        self.assertNotIn(MACOS_MANIFEST, str(caught.exception))

    def test_with_the_updater_flag_a_missing_signature_or_manifest_fails_and_is_named(self):
        self.assertEqual(self.check_updater([DMG, *MACOS_UPDATE_FILES]), ([], [], []))
        for missing in MACOS_UPDATE_FILES:
            produced = [DMG, *[name for name in MACOS_UPDATE_FILES if name != missing]]
            with self.assertRaises(ReleaseError, msg=missing) as caught:
                self.check_updater(produced)
            self.assertIn(missing.replace(APP, "*.app.tar.gz"), str(caught.exception))

    def check_updater(self, produced):
        plan = ra.check_assets_late(
            self.release, produced, "macos", self.sums, updater=True, platforms=all_built()
        )
        return plan.pending, plan.overwrite, plan.remove

    def test_without_the_updater_flag_a_dmg_alone_still_passes(self):
        self.assertEqual(self.check(produced=[DMG]).pending, [])

    def test_the_update_files_that_are_on_the_release_are_replaced_with_replace(self):
        release = [EXE, DMG, *MACOS_UPDATE_FILES, "SHA256SUMS"]
        sums = sums_text((EXE, H1), (DMG, H2), (APP, H3), (APP_SIG, H4), (MACOS_MANIFEST, HA))
        plan = self.check(produced=[DMG, *MACOS_UPDATE_FILES], release=release, sums=sums, replace=True)
        self.assertEqual(plan.overwrite, [DMG, *MACOS_UPDATE_FILES])
        with self.assertRaises(ReleaseError) as caught:  # without replace they are not touched
            self.check(produced=[DMG, *MACOS_UPDATE_FILES], release=release, sums=sums)
        for name in (DMG, *MACOS_UPDATE_FILES):
            self.assertIn(name, str(caught.exception))

    def test_an_update_file_of_an_interrupted_attach_is_pending(self):
        # The bundle, its signature and the manifest were uploaded; SHA256SUMS was not.
        release = [EXE, DMG, *MACOS_UPDATE_FILES, "SHA256SUMS"]
        plan = self.check(produced=[DMG, *MACOS_UPDATE_FILES], release=release, sums=sums_text((EXE, H1)))
        self.assertEqual(plan.pending, [DMG, *MACOS_UPDATE_FILES])

    def test_a_second_platforms_update_file_fails(self):
        with self.assertRaises(ReleaseError) as caught:
            self.check(produced=[DMG, EXE_SIG])
        self.assertIn(EXE_SIG, str(caught.exception))

    def test_a_second_platforms_file_fails(self):
        with self.assertRaises(ReleaseError) as caught:
            self.check(produced=[DMG, DEB])
        self.assertIn(DEB, str(caught.exception))

    def test_a_required_platform_fails(self):
        with self.assertRaises(ReleaseError) as caught:
            self.check(produced=[DEB], platform="linux")
        self.assertIn("Linux is a required platform", str(caught.exception))

    def test_an_unknown_platform_fails(self):
        with self.assertRaises(ReleaseError):
            self.check(platform="beos")

    def test_nothing_to_attach_fails(self):
        with self.assertRaises(ReleaseError):
            self.check(produced=[])

    def test_an_existing_dmg_without_replace_fails(self):
        with self.assertRaises(ReleaseError) as caught:
            self.check(release=[EXE, DMG, "SHA256SUMS"], sums=sums_text((EXE, H1), (DMG, H2)))
        self.assertIn(DMG, str(caught.exception))
        self.assertIn("replace", str(caught.exception))

    def test_an_existing_dmg_with_replace_is_overwritten(self):
        plan = self.check(release=[EXE, DMG, "SHA256SUMS"], sums=sums_text((EXE, H1), (DMG, H2)), replace=True)
        self.assertEqual((plan.pending, plan.overwrite, plan.remove), ([], [DMG], []))

    def test_replace_removes_an_existing_dmg_with_another_name(self):
        old = "Repeat-That_0.1.9_aarch64.dmg"
        release, sums = [EXE, old, "SHA256SUMS"], sums_text((EXE, H1), (old, H2))
        plan = self.check(release=release, sums=sums, replace=True)
        self.assertEqual((plan.pending, plan.overwrite, plan.remove), ([], [], [old]))
        with self.assertRaises(ReleaseError) as caught:
            self.check(release=release, sums=sums)  # without replace the release is not touched
        self.assertIn(old, str(caught.exception))

    def test_replace_covers_every_existing_dmg(self):
        old, older = "Repeat-That_0.1.9_aarch64.dmg", "Repeat-That_0.1.8_aarch64.dmg"
        plan = self.check(
            release=[EXE, old, DMG, older, "SHA256SUMS"],
            sums=sums_text((EXE, H1), (old, H2), (DMG, H3), (older, H4)),
            replace=True,
        )
        self.assertEqual((plan.overwrite, plan.remove), ([DMG], [old, older]))

    def test_a_dmg_the_release_has_but_sha256sums_lacks_is_an_interrupted_attach(self):
        # The .dmg was uploaded and the upload of SHA256SUMS failed: the same file is produced again.
        plan = self.check(release=[EXE, DMG, "SHA256SUMS"])
        self.assertEqual((plan.pending, plan.overwrite, plan.remove), ([DMG], [], []))

    def test_an_unlisted_dmg_this_run_did_not_produce_fails_without_replace(self):
        stray = "Repeat-That_0.1.9_aarch64.dmg"
        with self.assertRaises(ReleaseError) as caught:
            self.check(release=[EXE, stray, "SHA256SUMS"])
        self.assertIn(stray, str(caught.exception))
        self.assertIn("replace", str(caught.exception))

    def test_an_unlisted_dmg_this_run_did_not_produce_is_removed_with_replace(self):
        # What an interrupted replace leaves: the old name dropped from SHA256SUMS but not yet deleted.
        stray = "Repeat-That_0.1.9_aarch64.dmg"
        plan = self.check(release=[EXE, stray, "SHA256SUMS"], replace=True)
        self.assertEqual((plan.pending, plan.remove), ([], [stray]))

    def test_an_unlisted_file_of_another_platform_still_fails(self):
        with self.assertRaises(ReleaseError) as caught:
            self.check(release=[EXE, DEB, "SHA256SUMS"], replace=True)
        self.assertIn(DEB, str(caught.exception))

    def test_an_existing_asset_missing_from_sha256sums_fails(self):
        with self.assertRaises(ReleaseError) as caught:
            self.check(release=[EXE, "extra.bin", "SHA256SUMS"])
        self.assertIn("extra.bin", str(caught.exception))

    def test_a_release_without_sha256sums_fails(self):
        with self.assertRaises(ReleaseError) as caught:
            self.check(release=[EXE])
        self.assertIn("SHA256SUMS", str(caught.exception))

    def test_sha256sums_naming_a_file_the_release_lacks_fails(self):
        with self.assertRaises(ReleaseError) as caught:
            self.check(sums=sums_text((EXE, H1), (DEB, H2)))
        self.assertIn(DEB, str(caught.exception))


class CheckReleaseTests(unittest.TestCase):
    """The early check, before anything is built or downloaded."""

    def test_a_draft_may_hold_any_known_assets(self):
        replaced = ra.check_release("v0.2.0", [EXE, "SHA256SUMS"], draft=True, mode="full")
        self.assertEqual(replaced, [])

    def test_a_published_release_with_installers_fails_without_replace(self):
        with self.assertRaises(ReleaseError) as caught:
            ra.check_release("v0.2.0", [EXE, "SHA256SUMS"], draft=False, mode="full")
        self.assertIn(EXE, str(caught.exception))
        self.assertIn("replace", str(caught.exception))

    def test_replace_lists_what_will_be_replaced(self):
        replaced = ra.check_release("v0.2.0", [EXE, DMG, "SHA256SUMS"], draft=False, mode="full", replace=True)
        self.assertEqual(replaced, [EXE, "SHA256SUMS"])  # the late .dmg stays unless macOS is rebuilt

    def test_a_published_release_with_only_a_late_dmg_passes(self):
        self.assertEqual(ra.check_release("v0.2.0", [DMG], draft=False, mode="full"), [])

    def test_a_published_release_with_only_late_update_files_passes(self):
        self.assertEqual(ra.check_release("v0.2.0", MACOS_UPDATE_FILES, draft=False, mode="full"), [])

    def test_the_update_files_of_a_required_platform_are_protected_like_its_installer(self):
        # Replacing a signature or a manifest changes what installed apps fetch, so a published release keeps them.
        for name in (EXE_SIG, WINDOWS_MANIFEST, APPIMAGE_SIG, LINUX_MANIFEST):
            with self.assertRaises(ReleaseError, msg=name) as caught:
                ra.check_release("v0.2.0", [name, "SHA256SUMS"], draft=False, mode="full")
            self.assertIn(name, str(caught.exception))
            replaced = ra.check_release("v0.2.0", [name, "SHA256SUMS"], draft=False, mode="full", replace=True)
            self.assertEqual(replaced, [name, "SHA256SUMS"])
        self.assertEqual(ra.check_release("v0.2.0", [EXE_SIG], draft=True, mode="full"), [])

    def test_an_update_file_nobody_produces_fails_early(self):
        with self.assertRaises(ReleaseError) as caught:
            ra.check_release("v0.2.0", [f"{DEB}.sig", "latest.json"], draft=True, mode="full")
        self.assertIn(f"{DEB}.sig", str(caught.exception))
        self.assertIn("latest.json", str(caught.exception))

    def test_an_asset_of_a_platform_without_a_build_job_fails_early(self):
        with self.assertRaises(ReleaseError) as caught:
            ra.check_release("v0.2.0", [EXE, FLATPAK], draft=True, mode="full", platforms=without_job("flatpak"))
        self.assertIn(FLATPAK, str(caught.exception))

    def test_an_asset_of_a_platform_with_a_build_job_passes_early(self):
        ra.check_release("v0.2.0", [EXE, FLATPAK], draft=True, mode="full", platforms=all_built())

    def test_with_todays_table_the_linux_packages_and_the_flatpak_pass_early_on_a_draft(self):
        self.assertEqual(ra.check_release("v0.2.0", [*REQUIRED_FILES, "SHA256SUMS"], draft=True, mode="full"), [])

    def test_a_published_release_with_linux_packages_fails_without_replace(self):
        with self.assertRaises(ReleaseError) as caught:
            ra.check_release("v0.2.0", [*LINUX, "SHA256SUMS"], draft=False, mode="full")
        for name in LINUX:
            self.assertIn(name, str(caught.exception))

    def test_an_unknown_asset_fails_early_even_on_a_draft(self):
        with self.assertRaises(ReleaseError) as caught:
            ra.check_release("v0.2.0", [EXE, "notes.txt"], draft=True, mode="full")
        self.assertIn("notes.txt", str(caught.exception))

    def test_the_tag_must_be_plain_x_y_z(self):
        for tag in ("0.2.0", "v0.2", "v0.2.0-rc.1", "v0.2.0 ", ""):
            with self.assertRaises(ReleaseError, msg=tag):
                ra.check_release(tag, [], draft=True, mode="full")

    def test_late_mode_needs_an_optional_platform(self):
        with self.assertRaises(ReleaseError):
            ra.check_release("v0.2.0", [EXE, "SHA256SUMS"], draft=False, mode="late", platform="linux")

    def test_late_mode_leaves_the_existing_file_to_check_assets(self):
        # Whether the file is attached, or an attach that stopped half way, needs SHA256SUMS: check_assets_late.
        replaced = ra.check_release("v0.2.0", [EXE, DMG, "SHA256SUMS"], draft=False, mode="late", platform="macos")
        self.assertEqual(replaced, [])

    def test_late_mode_with_replace_lists_every_existing_file(self):
        other = "Repeat-That_0.1.9_aarch64.dmg"
        replaced = ra.check_release(
            "v0.2.0", [EXE, other, DMG, "SHA256SUMS"], draft=False, mode="late", platform="macos", replace=True
        )
        self.assertEqual(replaced, [other, DMG])

    def test_late_mode_with_replace_allows_the_existing_file(self):
        replaced = ra.check_release(
            "v0.2.0", [EXE, DMG, "SHA256SUMS"], draft=False, mode="late", platform="macos", replace=True
        )
        self.assertEqual(replaced, [DMG])

    def test_late_mode_with_replace_lists_the_update_files_too(self):
        replaced = ra.check_release(
            "v0.2.0", [EXE, DMG, *MACOS_UPDATE_FILES, "SHA256SUMS"], draft=False, mode="late", platform="macos",
            replace=True,
        )  # fmt: skip
        self.assertEqual(replaced, [DMG, *MACOS_UPDATE_FILES])

    def test_late_mode_works_on_a_published_release(self):
        replaced = ra.check_release("v0.2.0", [EXE, "SHA256SUMS"], draft=False, mode="late", platform="macos")
        self.assertEqual(replaced, [])

    def test_unknown_mode_fails(self):
        with self.assertRaises(ReleaseError):
            ra.check_release("v0.2.0", [], draft=True, mode="partial")


class GateTests(unittest.TestCase):
    """The required/optional gate over `toJSON(needs)` of the publish job."""

    @staticmethod
    def needs(**results):
        return {job.replace("_", "-"): {"result": result, "outputs": {}} for job, result in results.items()}

    def gate(self, release=True, **results):
        """Every job succeeds unless `results` says otherwise; the table has a build job for every platform."""
        states = dict(check_release="success", windows="success", linux="success", flatpak="success", macos="success")
        states.update(results)
        return ra.gate(self.needs(**states), release=release, platforms=all_built())

    def test_everything_succeeding_is_clean(self):
        result = self.gate()
        self.assertTrue(result.ok)
        self.assertEqual((result.errors, result.warnings, result.missing_optional), ([], [], []))

    def test_macos_failing_still_publishes_with_a_warning(self):
        result = self.gate(macos="failure")
        self.assertTrue(result.ok)
        self.assertEqual(result.missing_optional, ["macos"])
        self.assertEqual(len(result.warnings), 1)
        self.assertIn("macOS not attached", result.warnings[0])
        self.assertIn("late attach", result.warnings[0])

    def test_macos_skipped_or_cancelled_also_only_warns(self):
        for state in ("skipped", "cancelled"):
            result = self.gate(macos=state)
            self.assertTrue(result.ok, state)
            self.assertEqual(result.missing_optional, ["macos"], state)

    def test_a_required_platform_failing_stops_publishing(self):
        result = self.gate(windows="failure")
        self.assertFalse(result.ok)
        self.assertEqual(len(result.errors), 1)
        self.assertIn("Windows", result.errors[0])

    def test_a_required_platform_that_was_skipped_stops_publishing(self):
        result = self.gate(linux="skipped")
        self.assertFalse(result.ok)
        self.assertIn("Linux", result.errors[0])

    def test_windows_failing_with_macos_failing_reports_the_error_and_the_warning(self):
        result = self.gate(windows="failure", macos="failure")
        self.assertFalse(result.ok)
        self.assertEqual(len(result.errors), 1)
        self.assertEqual(result.missing_optional, ["macos"])

    def test_a_failed_release_check_stops_publishing(self):
        self.assertFalse(self.gate(check_release="failure").ok)

    def test_a_skipped_release_check_is_fine_for_a_dry_run_but_not_for_a_release(self):
        self.assertTrue(self.gate(release=False, check_release="skipped").ok)
        self.assertFalse(self.gate(release=True, check_release="skipped").ok)

    def test_a_built_platform_missing_from_needs_is_a_configuration_error(self):
        needs = self.needs(check_release="success", windows="success", linux="success", macos="success")
        result = ra.gate(needs, release=True, platforms=all_built())
        self.assertFalse(result.ok)
        self.assertIn("Flatpak", result.errors[0])

    def test_a_job_for_a_platform_without_a_build_flag_is_a_configuration_error(self):
        # Before the Flatpak had its job it had build=False, so a flatpak job in `needs` was out of step.
        needs = self.needs(check_release="success", windows="success", linux="success", flatpak="success")
        needs["macos"] = {"result": "success", "outputs": {}}
        result = ra.gate(needs, release=True, platforms=without_job("flatpak"))
        self.assertFalse(result.ok)
        self.assertIn("Flatpak", result.errors[0])

    def todays(self, **results):
        """The needs of a publish job in which every job succeeds unless `results` says otherwise."""
        states = dict(check_release="success", windows="success", linux="success", flatpak="success", macos="success")
        states.update(results)
        return self.needs(**states)

    def test_with_todays_table_windows_linux_flatpak_and_macos_are_needed(self):
        result = ra.gate(self.todays(), release=True)
        self.assertTrue(result.ok)
        self.assertEqual(result.warnings, [])

    def test_with_todays_table_a_missing_linux_job_is_a_configuration_error(self):
        needs = self.todays()
        del needs["linux"]
        result = ra.gate(needs, release=True)
        self.assertFalse(result.ok)
        self.assertIn("Linux has build=True", result.errors[0])

    def test_with_todays_table_a_missing_flatpak_job_is_a_configuration_error(self):
        needs = self.todays()
        del needs["flatpak"]
        result = ra.gate(needs, release=True)
        self.assertFalse(result.ok)
        self.assertIn("Flatpak has build=True", result.errors[0])

    def test_with_todays_table_a_missing_macos_job_is_a_configuration_error(self):
        needs = self.todays()
        del needs["macos"]
        result = ra.gate(needs, release=True)
        self.assertFalse(result.ok)
        self.assertIn("macOS has build=True", result.errors[0])

    def test_with_todays_table_macos_failing_still_publishes_the_required_platforms(self):
        result = ra.gate(self.todays(macos="failure"), release=True)
        self.assertTrue(result.ok)
        self.assertEqual(result.missing_optional, ["macos"])

    def test_with_todays_table_a_failing_linux_build_stops_publishing(self):
        for state in ("failure", "cancelled", "skipped"):
            result = ra.gate(self.todays(linux=state), release=True)
            self.assertFalse(result.ok, state)
            self.assertEqual(result.errors, [f"Linux build did not succeed ({state}); nothing is published"])

    def test_with_todays_table_a_failing_flatpak_build_stops_publishing(self):
        for state in ("failure", "cancelled", "skipped"):
            result = ra.gate(self.todays(flatpak=state), release=True)
            self.assertFalse(result.ok, state)
            self.assertEqual(result.errors, [f"Flatpak build did not succeed ({state}); nothing is published"])


REPO = "Roboroads/repeat-that"
SIGNATURE = "c2lnbmF0dXJlIGZyb20gdGF1cmkgc2VjcmV0IGtleQ=="  # what a .sig holds; its content is only compared


def manifest_text(payload, tag="v0.2.0", signature=SIGNATURE, repo=REPO, keys=("windows-x86_64",), **changes):
    """The text of a manifest for `payload`, valid unless `changes` replace a top-level field."""
    entry = {"signature": signature, "url": f"https://github.com/{repo}/releases/download/{tag}/{payload}"}
    data = {"version": tag[1:], "notes": "", "pub_date": "2026-10-22T10:00:00Z"}
    data["platforms"] = {key: dict(entry) for key in keys}
    data.update(changes)
    return json.dumps(data)


class CheckManifestsTests(unittest.TestCase):
    """What publish-release.yml writes with updater_manifest.py is checked again against the release's own files."""

    def files(self, **texts):
        """Windows' files, with the text of any of them replaced by `texts` (keys: sig, manifest)."""
        return {
            EXE: None,
            EXE_SIG: texts.get("sig", SIGNATURE + "\n"),
            WINDOWS_MANIFEST: texts.get("manifest", manifest_text(EXE)),
        }

    def check(self, files=None, tag="v0.2.0", repo=REPO):
        return ra.check_manifests(self.files() if files is None else files, tag, repo)

    def test_a_manifest_of_this_release_passes(self):
        self.check()

    def test_all_three_platforms_are_checked(self):
        files = {
            **self.files(),
            APPIMAGE: None,
            APPIMAGE_SIG: SIGNATURE,
            LINUX_MANIFEST: manifest_text(APPIMAGE, keys=("linux-x86_64", "linux-x86_64-appimage")),
            APP: None,
            APP_SIG: SIGNATURE,
            MACOS_MANIFEST: manifest_text(APP, keys=("darwin-aarch64", "darwin-aarch64-app")),
        }
        self.check(files)
        files[MACOS_MANIFEST] = manifest_text(APP, version="0.2.1")
        with self.assertRaises(ReleaseError) as caught:
            self.check(files)
        self.assertTrue(all(MACOS_MANIFEST in error for error in caught.exception.errors))

    def test_a_version_that_is_not_the_tags_fails(self):
        with self.assertRaises(ReleaseError) as caught:
            self.check(self.files(manifest=manifest_text(EXE, version="0.1.9")))
        self.assertEqual(
            caught.exception.errors, [f"{WINDOWS_MANIFEST} has version 0.1.9, but the release is v0.2.0 (0.2.0)"]
        )

    def test_a_url_of_another_release_or_repository_fails(self):
        for changes in ({"tag": "v0.1.9"}, {"repo": "someone/repeat-that"}):
            with self.assertRaises(ReleaseError, msg=str(changes)) as caught:
                self.check(self.files(manifest=manifest_text(EXE, **{"version": "0.2.0", **changes})))
            self.assertIn("is not a file of v0.2.0 on Roboroads/repeat-that", str(caught.exception))

    def test_a_url_that_is_not_https_fails(self):
        text = manifest_text(EXE).replace("https://", "http://")
        with self.assertRaises(ReleaseError):
            self.check(self.files(manifest=text))

    def test_a_url_to_a_file_the_release_does_not_have_fails(self):
        with self.assertRaises(ReleaseError) as caught:
            self.check(self.files(manifest=manifest_text("Repeat-That_0.2.0_x86-setup.exe")))
        self.assertIn("Repeat-That_0.2.0_x86-setup.exe", str(caught.exception))

    def test_a_url_with_something_after_the_file_name_fails(self):
        for suffix in ("?x=1", "/../x", "#a"):
            with self.assertRaises(ReleaseError, msg=suffix):
                self.check(self.files(manifest=manifest_text(EXE + suffix)))

    def test_a_signature_that_is_not_the_one_of_the_sig_file_fails(self):
        with self.assertRaises(ReleaseError) as caught:
            self.check(self.files(manifest=manifest_text(EXE, signature="b3RoZXI=")))
        self.assertIn(f"is not the content of {EXE_SIG}", str(caught.exception))

    def test_the_line_end_of_the_sig_file_does_not_count(self):
        self.check(self.files(sig=f"{SIGNATURE}\r\n"))

    def test_a_manifest_that_is_not_json_or_has_no_platforms_fails(self):
        for text in ("", "{", "[]", "null", manifest_text(EXE, platforms={}), manifest_text(EXE, platforms=[])):
            with self.assertRaises(ReleaseError, msg=text):
                self.check(self.files(manifest=text))

    def test_an_entry_without_a_url_or_signature_fails(self):
        for entry in ({"signature": SIGNATURE}, {"url": "https://github.com"}, {}, "x", None):
            with self.assertRaises(ReleaseError, msg=str(entry)):
                self.check(self.files(manifest=manifest_text(EXE, platforms={"windows-x86_64": entry})))

    def test_a_signature_file_that_could_not_be_read_fails(self):
        files = self.files()
        files[EXE_SIG] = None
        with self.assertRaises(ReleaseError) as caught:
            self.check(files)
        self.assertIn(EXE_SIG, str(caught.exception))

    def test_no_manifest_at_all_fails(self):
        with self.assertRaises(ReleaseError) as caught:
            self.check({EXE: None, EXE_SIG: SIGNATURE})
        self.assertIn("no update manifest", str(caught.exception))

    def test_a_tag_of_the_wrong_form_fails(self):
        for tag in ("0.2.0", "v0.2", ""):
            with self.assertRaises(ReleaseError, msg=tag):
                self.check(tag=tag)


PUBLIC_KEY = base64.b64encode(b"untrusted comment: minisign public key: 3F9E6B1A2C4D5E70\nRWRwXkQsGhtunk\n").decode()
PRIVATE_KEY = base64.b64encode(b"untrusted comment: rsign encrypted secret key\nRWRTY0IyAAAAAAAAAAAA\n").decode()


class CheckUpdaterKeysTests(unittest.TestCase):
    """check-release looks at the key, the secrets and the variable before anything is built."""

    def check(self, pubkey=PUBLIC_KEY, private_key=PRIVATE_KEY, password="a password"):
        return ra.check_updater_keys(pubkey, private_key, password)

    def test_the_variable_and_both_secrets_pass(self):
        result = self.check()
        self.assertTrue(result.ok)
        self.assertEqual((result.errors, result.warnings, result.notices), ([], [], []))

    def test_the_variable_without_the_private_key_fails_and_names_what_to_do(self):
        result = self.check(private_key="")
        self.assertFalse(result.ok)
        self.assertEqual(len(result.errors), 1)
        self.assertIn("TAURI_SIGNING_PRIVATE_KEY", result.errors[0])
        self.assertIn("updater-signing", result.errors[0])
        self.assertIn("docs/MAINTAINING.md", result.errors[0])

    def test_a_key_without_a_password_passes(self):
        # GitHub can't store an empty secret, so a key made without a password has no password secret at all.
        result = self.check(password="")
        self.assertTrue(result.ok)
        self.assertEqual((result.errors, result.warnings, result.notices), ([], [], []))

    def test_a_private_key_of_blanks_counts_as_not_set(self):
        self.assertFalse(self.check(private_key=" \n").ok)

    def test_a_variable_that_is_not_a_public_key_fails(self):
        raw = "untrusted comment: minisign public key: 3F9E6B1A2C4D5E70\nRWRwXkQsGhtunk"  # the file, not its base64
        for text in ("~/.tauri/repeat-that-updater.key.pub", raw, "not base64!", base64.b64encode(b"hello").decode()):
            result = self.check(pubkey=text)
            self.assertFalse(result.ok, text)
            self.assertIn("TAURI_UPDATER_PUBKEY", result.errors[0])
            self.assertIn(".pub", result.errors[0])

    def test_a_private_key_that_is_not_a_key_file_fails(self):
        for text in ("~/.tauri/repeat-that-updater.key", "hello", base64.b64encode(b"hello").decode()):
            result = self.check(private_key=text)
            self.assertFalse(result.ok, text)
            self.assertIn("TAURI_SIGNING_PRIVATE_KEY", result.errors[0])

    def test_a_variable_of_blanks_is_set_but_not_a_key(self):
        # The workflows compare the variable with '' only, so they build for an updater; the app trims it to nothing.
        for text in (" ", "\n", " \n "):
            result = self.check(pubkey=text)
            self.assertFalse(result.ok, repr(text))
            self.assertIn("TAURI_UPDATER_PUBKEY", result.errors[0])
            self.assertEqual(result.warnings, [])

    def test_the_public_key_with_a_line_end_passes(self):
        self.assertTrue(self.check(pubkey=PUBLIC_KEY + "\n").ok)

    def test_secrets_without_the_variable_only_warn(self):
        result = self.check(pubkey="")
        self.assertTrue(result.ok)
        self.assertEqual(len(result.warnings), 1)
        self.assertIn("TAURI_UPDATER_PUBKEY", result.warnings[0])
        self.assertIn("no update files", result.warnings[0])

    def test_one_secret_without_the_variable_also_warns(self):
        for changes in ({"private_key": ""}, {"password": ""}):
            result = self.check(pubkey="", **changes)
            self.assertTrue(result.ok)
            self.assertEqual(len(result.warnings), 1)

    def test_nothing_at_all_is_a_notice(self):
        result = self.check(pubkey="", private_key="", password="")
        self.assertTrue(result.ok)
        self.assertEqual((result.errors, result.warnings), ([], []))
        self.assertEqual(len(result.notices), 1)
        self.assertIn("no update files", result.notices[0])


class CommandLineTests(unittest.TestCase):
    def run_main(self, *argv, stdin=None):
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            if stdin is not None:
                with unittest.mock.patch("sys.stdin", io.StringIO(stdin)):
                    code = ra.main(list(argv))
            else:
                code = ra.main(list(argv))
        return code, out.getvalue(), err.getvalue()

    def test_classify_prints_platform_and_name(self):
        code, out, _ = self.run_main("classify", f"dir/{EXE}", DEB)
        self.assertEqual(code, 0)
        self.assertEqual(out.splitlines(), [f"windows {EXE}", f"linux {DEB}"])

    def test_classify_fails_with_an_annotation_for_an_unknown_file(self):
        code, _, err = self.run_main("classify", "mystery.bin")
        self.assertEqual(code, 1)
        self.assertIn("::error::", err)
        self.assertIn("mystery.bin", err)

    def test_sums_hashes_the_files_and_keeps_old_lines(self):
        with tempfile.TemporaryDirectory() as folder:
            folder = Path(folder)
            (folder / EXE).write_bytes(b"installer")
            existing = folder / "old-sums"
            existing.write_text(sums_text((DMG, H2)))
            code, out, _ = self.run_main("sums", "--existing", str(existing), "--add", str(folder / EXE))
        digest = hashlib.sha256(b"installer").hexdigest()
        self.assertEqual(code, 0)
        self.assertEqual(out, sums_text((DMG, H2), (EXE, digest)))

    def test_sums_fails_when_a_replaced_line_would_change(self):
        with tempfile.TemporaryDirectory() as folder:
            folder = Path(folder)
            (folder / DMG).write_bytes(b"other bytes")
            existing = folder / "old-sums"
            existing.write_text(sums_text((DMG, H2)))
            code, _, err = self.run_main("sums", "--existing", str(existing), "--add", str(folder / DMG))
        self.assertEqual(code, 1)
        self.assertIn(DMG, err)

    def test_check_assets_full_prints_the_names_to_keep(self):
        release = json.dumps({"isDraft": False, "assets": [{"name": EXE}, {"name": DMG}, {"name": "SHA256SUMS"}]})
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "release.json"
            path.write_text(release)
            produced = [f"a/{name}" for name in REQUIRED_FILES]
            code, out, _ = self.run_main(
                "check-assets", "--mode", "full", "--release-json", str(path), "--produced", *produced
            )
        self.assertEqual(code, 0)
        self.assertEqual(out.splitlines(), [DMG])

    def test_check_assets_full_fails_when_a_linux_package_is_missing(self):
        code, _, err = self.run_main("check-assets", "--mode", "full", "--produced", EXE, DEB, APPIMAGE, FLATPAK)
        self.assertEqual(code, 1)
        self.assertIn("::error::Linux produced no *.rpm file", err)

    def test_gate_reads_needs_from_stdin(self):
        needs = json.dumps(
            {
                "check-release": {"result": "skipped"},
                "windows": {"result": "success"},
                "linux": {"result": "success"},
                "flatpak": {"result": "success"},
                "macos": {"result": "success"},
            }
        )
        code, _, _ = self.run_main("gate", "--needs", "-", stdin=needs)
        self.assertEqual(code, 0)
        code, _, err = self.run_main("gate", "--needs", "-", "--release", stdin=needs)
        self.assertEqual(code, 1)
        self.assertIn("::error::", err)

    def test_sums_drops_the_line_of_a_file_that_leaves_the_release(self):
        old = "Repeat-That_0.1.9_aarch64.dmg"
        with tempfile.TemporaryDirectory() as folder:
            folder = Path(folder)
            (folder / DMG).write_bytes(b"new dmg")
            existing = folder / "old-sums"
            existing.write_text(sums_text((EXE, H1), (old, H2)))
            code, out, _ = self.run_main(
                "sums", "--existing", str(existing), "--add", str(folder / DMG), "--drop", old
            )
        self.assertEqual(code, 0)
        self.assertEqual(out, sums_text((DMG, hashlib.sha256(b"new dmg").hexdigest()), (EXE, H1)))

    def late_args(self, folder, release, sums, *extra):
        path = Path(folder)
        (path / "release.json").write_text(json.dumps({"isDraft": False, "assets": [{"name": n} for n in release]}))
        (path / "sums").write_text(sums)
        return (
            "check-assets", "--mode", "late", "--platform", "macos", "--release-json", str(path / "release.json"),
            "--sums", str(path / "sums"), "--produced", f"new/{DMG}", *extra,
        )  # fmt: skip

    def test_check_assets_late_prints_what_the_job_must_skip_and_remove(self):
        old = "Repeat-That_0.1.9_aarch64.dmg"
        with tempfile.TemporaryDirectory() as folder:
            args = self.late_args(folder, [EXE, old, DMG, "SHA256SUMS"], sums_text((EXE, H1), (old, H2)), "--replace")
            code, out, _ = self.run_main(*args)
        self.assertEqual(code, 0)
        self.assertEqual(out.splitlines(), [f"pending {DMG}", f"remove {old}"])

    def test_check_assets_late_checks_the_version_against_the_tag(self):
        with tempfile.TemporaryDirectory() as folder:
            args = self.late_args(folder, [EXE, "SHA256SUMS"], sums_text((EXE, H1)))
            self.assertEqual(self.run_main(*args, "--tag", "v0.2.0")[0], 0)
            code, _, err = self.run_main(*args, "--tag", "v0.3.0")
        self.assertEqual(code, 1)
        self.assertIn(DMG, err)

    def test_check_source_run_reads_the_run_and_the_comparison(self):
        with tempfile.TemporaryDirectory() as folder:
            folder = Path(folder)
            (folder / "run.json").write_text(json.dumps(RUN))
            (folder / "compare.json").write_text(json.dumps({"status": "ahead"}))
            args = (
                "check-source-run", "--run-json", str(folder / "run.json"),
                "--compare-json", str(folder / "compare.json"),
                "--platform", "macos", "--repo", "Roboroads/repeat-that", "--branch", "main",
            )  # fmt: skip
            self.assertEqual(self.run_main(*args)[0], 0)
            (folder / "compare.json").write_text(json.dumps({"status": "diverged"}))
            code, _, err = self.run_main(*args)
        self.assertEqual(code, 1)
        self.assertIn("::error::", err)

    def test_select_artifacts_picks_the_ids_of_succeeded_jobs(self):
        listing = f"11 release-windows-{EXE}\n12 SHA256SUMS\n"
        with tempfile.TemporaryDirectory() as folder:
            folder = Path(folder)
            (folder / "artifacts.txt").write_text(listing)  # SHA256SUMS: a dry run's artifact, not ours
            needs = folder / "needs.json"
            output = folder / "output"
            args = ("select-artifacts", "--needs", str(needs), "--artifacts", str(folder / "artifacts.txt"))

            needs.write_text(json.dumps({"windows": {"result": "success"}}))
            code, out, _ = self.run_main(*args, "--github-output", str(output))
            self.assertEqual((code, output.read_text()), (0, "ids=11\n"))
            self.assertNotIn("::warning::", out)
            code, out, _ = self.run_main(*args)
            self.assertEqual((code, out), (0, "ids=11\n"))

    def test_select_artifacts_fails_when_no_platform_succeeded(self):
        with tempfile.TemporaryDirectory() as folder:
            folder = Path(folder)
            (folder / "artifacts.txt").write_text(f"11 release-windows-{EXE}\n")
            (folder / "needs.json").write_text(json.dumps({"windows": {"result": "failure"}}))
            code, out, err = self.run_main(
                "select-artifacts", "--needs", str(folder / "needs.json"), "--artifacts", str(folder / "artifacts.txt")
            )
        self.assertEqual(code, 1)
        self.assertIn("::warning::", out)  # the ignored artifact is named
        self.assertIn(EXE, out)
        self.assertIn("::error::", err)

    def test_check_assets_full_with_the_updater_flag_names_the_missing_update_file(self):
        produced = [*REQUIRED_FILES, *REQUIRED_UPDATE_FILES]
        args = ("check-assets", "--mode", "full", "--produced")
        self.assertEqual(self.run_main(*args, *produced, "--updater")[0], 0)
        code, _, err = self.run_main(*args, *[name for name in produced if name != EXE_SIG], "--updater")
        self.assertEqual(code, 1)
        self.assertIn("::error::Windows produced no *-setup.exe.sig file", err)
        # Without the flag the same files are a release made before there was a key, and pass.
        self.assertEqual(self.run_main(*args, *REQUIRED_FILES)[0], 0)

    def test_check_assets_late_with_the_updater_flag(self):
        with tempfile.TemporaryDirectory() as folder:
            args = self.late_args(folder, [EXE, "SHA256SUMS"], sums_text((EXE, H1)), "--updater")
            code, _, err = self.run_main(*args)
        self.assertEqual(code, 1)
        self.assertIn("macOS produced no *.app.tar.gz file", err)

    def test_check_manifests_reads_the_manifests_and_signatures_next_to_the_files(self):
        with tempfile.TemporaryDirectory() as folder:
            folder = Path(folder)
            (folder / EXE).write_bytes(b"installer")
            (folder / EXE_SIG).write_text(SIGNATURE + "\n")
            (folder / WINDOWS_MANIFEST).write_text(manifest_text(EXE))
            paths = [str(folder / name) for name in (EXE, EXE_SIG, WINDOWS_MANIFEST)]
            args = ("check-manifests", "--tag", "v0.2.0", "--repo", REPO)
            self.assertEqual(self.run_main(*args, *paths)[0], 0)
            (folder / WINDOWS_MANIFEST).write_text(manifest_text(EXE, version="0.1.9"))
            code, _, err = self.run_main(*args, *paths)
        self.assertEqual(code, 1)
        self.assertIn(f"::error::{WINDOWS_MANIFEST} has version 0.1.9", err)

    def test_check_updater_keys_reads_the_environment_and_sets_the_exit_code(self):
        keys = {
            "TAURI_UPDATER_PUBKEY": PUBLIC_KEY,
            "TAURI_SIGNING_PRIVATE_KEY": PRIVATE_KEY,
            "TAURI_SIGNING_PRIVATE_KEY_PASSWORD": "a password",
        }
        with unittest.mock.patch.dict(os.environ, keys):
            self.assertEqual(self.run_main("check-updater-keys")[0], 0)
        with unittest.mock.patch.dict(os.environ, {**keys, "TAURI_SIGNING_PRIVATE_KEY": ""}):
            code, _, err = self.run_main("check-updater-keys")
        self.assertEqual(code, 1)
        self.assertIn("::error::", err)
        self.assertIn("TAURI_SIGNING_PRIVATE_KEY", err)
        self.assertNotIn(PRIVATE_KEY, err)

    def test_check_updater_keys_says_what_it_found_in_the_summary(self):
        none = {"TAURI_UPDATER_PUBKEY": "", "TAURI_SIGNING_PRIVATE_KEY": "", "TAURI_SIGNING_PRIVATE_KEY_PASSWORD": ""}
        with tempfile.TemporaryDirectory() as folder, unittest.mock.patch.dict(os.environ, none):
            summary = Path(folder) / "summary.md"
            code, out, _ = self.run_main("check-updater-keys", "--summary", str(summary))
            text = summary.read_text()
        self.assertEqual(code, 0)
        self.assertIn("::notice::", out)
        self.assertIn("no update files", text)


if __name__ == "__main__":
    unittest.main()
