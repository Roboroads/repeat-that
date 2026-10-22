"""Unit tests for release_assets.py. Run with: python3 -m unittest discover -s .github/scripts

File names and hashes are made up in memory; only the CLI tests at the bottom touch a temporary folder (with a few
bytes of text, nothing that looks like audio).
"""

import contextlib
import dataclasses
import hashlib
import io
import json
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
LINUX = [DEB, RPM, APPIMAGE]

H1, H2, H3, H4, HA = ("1" * 64, "2" * 64, "3" * 64, "4" * 64, "a" * 64)


def line(name, digest):
    return f"{digest}  {name}"


def sums_text(*pairs):
    """A SHA256SUMS body from (name, hash) pairs, in the order given."""
    return "".join(line(name, digest) + "\n" for name, digest in pairs)


def all_built(platforms=ra.PLATFORMS):
    """The platform table as it will look once every platform has a build job in publish-release.yml."""
    return tuple(dataclasses.replace(platform, build=True) for platform in platforms)


class PlatformTableTests(unittest.TestCase):
    def test_required_and_optional(self):
        required = {platform.name: platform.required for platform in ra.PLATFORMS}
        self.assertEqual(required, {"windows": True, "linux": True, "flatpak": True, "macos": False})

    def test_only_flatpak_has_no_build_job_yet(self):
        built = [platform.name for platform in ra.PLATFORMS if platform.build]
        self.assertEqual(built, ["windows", "linux", "macos"])

    def test_no_two_platforms_claim_the_same_file(self):
        for name in (EXE, DEB, RPM, APPIMAGE, FLATPAK, DMG):
            matches = [p.name for p in ra.PLATFORMS if any(ra.matches(pattern, name) for pattern in p.patterns)]
            self.assertEqual(len(matches), 1, f"{name} matched {matches}")


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
        # With today's table the Flatpak has no build job yet, so a flatpak artifact is out of step.
        with self.assertRaises(ReleaseError):
            ra.select_artifacts([f"release-flatpak-{FLATPAK}"], self.results(windows="success", flatpak="success"))

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

    # What a release run produces with today's table: Windows and Linux are required and have build jobs.
    produced = [EXE, *LINUX]

    def test_a_fresh_release_with_the_produced_files_passes(self):
        self.assertEqual(ra.check_assets_full([], self.produced), [])

    def test_assets_from_this_runs_files_and_sha256sums_pass(self):
        self.assertEqual(ra.check_assets_full([EXE, *LINUX, "SHA256SUMS"], self.produced), [])

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
        produced = [EXE, *LINUX, FLATPAK, DMG]
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
            produced = [EXE] + [name for name in LINUX if name != missing]
            with self.assertRaises(ReleaseError, msg=missing) as caught:
                ra.check_assets_full([], produced)
            self.assertEqual(len(caught.exception.errors), 1, missing)
            self.assertIn(f"Linux produced no *.{missing.rsplit('.', 1)[1]} file", str(caught.exception))

    def test_linux_that_produced_nothing_fails_once(self):
        with self.assertRaises(ReleaseError) as caught:
            ra.check_assets_full([], [EXE])
        self.assertEqual(caught.exception.errors, ["Linux produced no files"])

    def test_a_built_required_platform_that_produced_nothing_fails(self):
        with self.assertRaises(ReleaseError) as caught:
            ra.check_assets_full([], [EXE], platforms=all_built())
        self.assertIn("Linux produced no files", str(caught.exception))
        self.assertIn("Flatpak produced no files", str(caught.exception))
        self.assertNotIn("macOS", str(caught.exception))  # optional

    def test_a_platform_without_a_build_job_may_produce_nothing(self):
        self.assertEqual(ra.check_assets_full([], self.produced), [])  # no Flatpak yet

    def test_a_produced_file_of_no_platform_fails(self):
        with self.assertRaises(ReleaseError):
            ra.check_assets_full([], [*self.produced, "stray.log"])

    def test_every_problem_is_reported_at_once(self):
        with self.assertRaises(ReleaseError) as caught:
            ra.check_assets_full(["one.zip", "two.zip"], self.produced)
        self.assertEqual(len(caught.exception.errors), 2)


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

    def test_an_asset_of_a_platform_without_a_build_job_fails_early(self):
        with self.assertRaises(ReleaseError) as caught:
            ra.check_release("v0.2.0", [EXE, FLATPAK], draft=True, mode="full")
        self.assertIn(FLATPAK, str(caught.exception))

    def test_an_asset_of_a_platform_with_a_build_job_passes_early(self):
        ra.check_release("v0.2.0", [EXE, FLATPAK], draft=True, mode="full", platforms=all_built())

    def test_with_todays_table_the_linux_packages_pass_early_on_a_draft(self):
        self.assertEqual(ra.check_release("v0.2.0", [EXE, *LINUX, "SHA256SUMS"], draft=True, mode="full"), [])

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
        # As the table is today the Flatpak has build=False, so a flatpak job in `needs` is out of step.
        needs = self.needs(check_release="success", windows="success", linux="success", flatpak="success")
        needs["macos"] = {"result": "success", "outputs": {}}
        result = ra.gate(needs, release=True)
        self.assertFalse(result.ok)
        self.assertIn("Flatpak", result.errors[0])

    def test_with_todays_table_windows_linux_and_macos_are_needed(self):
        needs = self.needs(check_release="success", windows="success", linux="success", macos="success")
        result = ra.gate(needs, release=True)
        self.assertTrue(result.ok)
        self.assertEqual(result.warnings, [])

    def test_with_todays_table_a_missing_linux_job_is_a_configuration_error(self):
        result = ra.gate(self.needs(check_release="success", windows="success", macos="success"), release=True)
        self.assertFalse(result.ok)
        self.assertIn("Linux has build=True", result.errors[0])

    def test_with_todays_table_a_missing_macos_job_is_a_configuration_error(self):
        result = ra.gate(self.needs(check_release="success", windows="success", linux="success"), release=True)
        self.assertFalse(result.ok)
        self.assertIn("macOS has build=True", result.errors[0])

    def test_with_todays_table_macos_failing_still_publishes_windows_and_linux(self):
        needs = self.needs(check_release="success", windows="success", linux="success", macos="failure")
        result = ra.gate(needs, release=True)
        self.assertTrue(result.ok)
        self.assertEqual(result.missing_optional, ["macos"])

    def test_with_todays_table_a_failing_linux_build_stops_publishing(self):
        for state in ("failure", "cancelled", "skipped"):
            needs = self.needs(check_release="success", windows="success", linux=state, macos="success")
            result = ra.gate(needs, release=True)
            self.assertFalse(result.ok, state)
            self.assertEqual(result.errors, [f"Linux build did not succeed ({state}); nothing is published"])


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
            produced = [f"a/{name}" for name in (EXE, *LINUX)]
            code, out, _ = self.run_main(
                "check-assets", "--mode", "full", "--release-json", str(path), "--produced", *produced
            )
        self.assertEqual(code, 0)
        self.assertEqual(out.splitlines(), [DMG])

    def test_check_assets_full_fails_when_a_linux_package_is_missing(self):
        code, _, err = self.run_main("check-assets", "--mode", "full", "--produced", EXE, DEB, APPIMAGE)
        self.assertEqual(code, 1)
        self.assertIn("::error::Linux produced no *.rpm file", err)

    def test_gate_reads_needs_from_stdin(self):
        needs = json.dumps(
            {
                "check-release": {"result": "skipped"},
                "windows": {"result": "success"},
                "linux": {"result": "success"},
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


if __name__ == "__main__":
    unittest.main()
