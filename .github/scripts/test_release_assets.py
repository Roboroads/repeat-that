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
DMG = "Repeat-That_0.2.0_universal.dmg"

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

    def test_only_windows_has_a_build_job_so_far(self):
        built = [platform.name for platform in ra.PLATFORMS if platform.build]
        self.assertEqual(built, ["windows"])

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
        self.assertEqual(text, sums_text((DEB, H3), (DMG, H2), (EXE, H1)))
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


class CheckAssetsFullTests(unittest.TestCase):
    """Full mode: what is already on the release against what this run produced."""

    def test_a_fresh_release_with_the_produced_files_passes(self):
        self.assertEqual(ra.check_assets_full([], [EXE]), [])

    def test_assets_from_this_runs_files_and_sha256sums_pass(self):
        self.assertEqual(ra.check_assets_full([EXE, "SHA256SUMS"], [EXE]), [])

    def test_an_unexpected_asset_fails_and_is_named(self):
        with self.assertRaises(ReleaseError) as caught:
            ra.check_assets_full([EXE, "debug-symbols.zip"], [EXE])
        self.assertIn("debug-symbols.zip", str(caught.exception))

    def test_an_asset_of_a_required_platform_that_was_not_produced_fails(self):
        with self.assertRaises(ReleaseError) as caught:
            ra.check_assets_full([EXE, DEB], [EXE])
        self.assertIn(DEB, str(caught.exception))

    def test_a_dmg_from_an_earlier_late_attach_is_kept_when_macos_was_not_produced(self):
        kept = ra.check_assets_full([EXE, DMG, "SHA256SUMS"], [EXE])
        self.assertEqual(kept, [DMG])

    def test_a_dmg_is_replaced_not_kept_when_macos_was_produced(self):
        produced = [EXE, DEB, FLATPAK, DMG]
        self.assertEqual(ra.check_assets_full([EXE, DMG, "SHA256SUMS"], produced, platforms=all_built()), [])

    def test_a_dmg_with_another_name_than_the_produced_one_fails(self):
        with self.assertRaises(ReleaseError) as caught:
            ra.check_assets_full(
                [EXE, "Repeat-That_0.1.9_universal.dmg"], [EXE, DEB, FLATPAK, DMG], platforms=all_built()
            )
        self.assertIn("0.1.9", str(caught.exception))

    def test_a_built_required_platform_that_produced_nothing_fails(self):
        with self.assertRaises(ReleaseError) as caught:
            ra.check_assets_full([], [EXE], platforms=all_built())
        self.assertIn("Linux produced no files", str(caught.exception))
        self.assertIn("Flatpak produced no files", str(caught.exception))
        self.assertNotIn("macOS", str(caught.exception))  # optional

    def test_a_platform_without_a_build_job_may_produce_nothing(self):
        self.assertEqual(ra.check_assets_full([], [EXE]), [])

    def test_a_produced_file_of_no_platform_fails(self):
        with self.assertRaises(ReleaseError):
            ra.check_assets_full([], [EXE, "stray.log"])

    def test_every_problem_is_reported_at_once(self):
        with self.assertRaises(ReleaseError) as caught:
            ra.check_assets_full(["one.zip", "two.zip"], [EXE])
        self.assertEqual(len(caught.exception.errors), 2)


class CheckAssetsLateTests(unittest.TestCase):
    """Late mode: add one optional platform's files to a release that has its own SHA256SUMS."""

    release = [EXE, "SHA256SUMS"]
    sums = sums_text((EXE, H1))

    def check(self, produced=(DMG,), release=None, sums=None, platform="macos", replace=False):
        return ra.check_assets_late(
            self.release if release is None else release,
            list(produced),
            platform,
            self.sums if sums is None else sums,
            replace=replace,
        )

    def test_adding_the_platforms_files_passes(self):
        self.check()

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

    def test_an_existing_dmg_with_replace_passes(self):
        self.check(release=[EXE, DMG, "SHA256SUMS"], sums=sums_text((EXE, H1), (DMG, H2)), replace=True)

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
            ra.check_release("v0.2.0", [EXE, DEB], draft=True, mode="full")
        self.assertIn(DEB, str(caught.exception))

    def test_an_asset_of_a_platform_with_a_build_job_passes_early(self):
        ra.check_release("v0.2.0", [EXE, DEB], draft=True, mode="full", platforms=all_built())

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

    def test_late_mode_fails_when_the_platform_is_already_on_the_release(self):
        with self.assertRaises(ReleaseError) as caught:
            ra.check_release("v0.2.0", [EXE, DMG, "SHA256SUMS"], draft=False, mode="late", platform="macos")
        self.assertIn(DMG, str(caught.exception))

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
        # As the table is today only windows has build=True, so a macos job in `needs` is out of step with the script.
        result = ra.gate(self.needs(check_release="success", windows="success", macos="success"), release=True)
        self.assertFalse(result.ok)
        self.assertIn("macOS", result.errors[0])

    def test_with_todays_table_only_windows_is_needed(self):
        result = ra.gate(self.needs(check_release="success", windows="success"), release=True)
        self.assertTrue(result.ok)


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
            code, out, _ = self.run_main(
                "check-assets", "--mode", "full", "--release-json", str(path), "--produced", f"a/{EXE}"
            )
        self.assertEqual(code, 0)
        self.assertEqual(out.splitlines(), [DMG])

    def test_gate_reads_needs_from_stdin(self):
        needs = json.dumps({"check-release": {"result": "skipped"}, "windows": {"result": "success"}})
        code, _, _ = self.run_main("gate", "--needs", "-", stdin=needs)
        self.assertEqual(code, 0)
        code, _, err = self.run_main("gate", "--needs", "-", "--release", stdin=needs)
        self.assertEqual(code, 1)
        self.assertIn("::error::", err)


if __name__ == "__main__":
    unittest.main()
