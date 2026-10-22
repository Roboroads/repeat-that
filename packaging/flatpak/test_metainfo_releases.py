"""Tests for metainfo_releases.py. Run with: python3 -m unittest discover -s packaging/flatpak -p 'test_*.py'"""

import tempfile
import unittest
import xml.etree.ElementTree as ET
from pathlib import Path

import metainfo_releases as mr

CHANGELOG = """# Changelog

## [0.2.0](https://github.com/Roboroads/repeat-that/compare/v0.1.0...v0.2.0) (2026-11-03)


### ⚠ BREAKING CHANGES

* **core:** play through to live, including the sentence in progress ([#10](https://github.com/Roboroads/repeat-that/issues/10))

### Features

* **app:** add the settings window ([#14](https://github.com/Roboroads/repeat-that/issues/14)) ([8792366](https://github.com/Roboroads/repeat-that/commit/8792366c83d82bbbbe69e1941c8649299885496d))
* ship a Flatpak <build> & more ([#50](https://github.com/Roboroads/repeat-that/issues/50))


### Bug Fixes

* **capture:** keep the fade smooth ([#11](https://github.com/Roboroads/repeat-that/issues/11)) ([18016f3](https://github.com/Roboroads/repeat-that/commit/18016f3ce48c731237bc425cc089dfe47e24add8))


### Documentation

* **readme:** explain the hotkeys ([#12](https://github.com/Roboroads/repeat-that/issues/12))

## [0.1.0](https://github.com/Roboroads/repeat-that/compare/v0.0.1...v0.1.0) (2026-10-21)


### Features

* **core:** add VAD segmentation ([#2](https://github.com/Roboroads/repeat-that/issues/2)) ([f34f782](https://github.com/Roboroads/repeat-that/commit/f34f782f55eba410f83c99af25ae21d857bd06a7))
"""

METAINFO = """<?xml version="1.0" encoding="UTF-8"?>
<component type="desktop-application">
  <id>me.roboroads.RepeatThat</id>
  <!-- Filled in by the script. -->
  <releases />
</component>
"""


def releases_of(text):
    return ET.fromstring(text).find("releases")


class ParseTests(unittest.TestCase):
    def test_sections_come_newest_first_with_version_and_date(self):
        sections = mr.parse_changelog(CHANGELOG)
        self.assertEqual([(s.version, s.date) for s in sections], [("0.2.0", "2026-11-03"), ("0.1.0", "2026-10-21")])

    def test_the_notes_of_features_fixes_and_breaking_changes_are_kept_in_that_order(self):
        newest = mr.parse_changelog(CHANGELOG)[0]
        self.assertEqual(
            newest.notes,
            [
                "Core: play through to live, including the sentence in progress",
                "App: add the settings window",
                "Ship a Flatpak <build> & more",
                "Capture: keep the fade smooth",
            ],
        )

    def test_other_sections_such_as_documentation_are_left_out(self):
        newest = mr.parse_changelog(CHANGELOG)[0]
        self.assertFalse(any("hotkeys" in note for note in newest.notes))

    def test_links_and_commit_hashes_are_removed_from_a_note(self):
        self.assertEqual(
            mr.clean_note(
                "**app:** add the settings window ([#14](https://example.org/14)) ([8792366](https://example.org/c))"
            ),
            "App: add the settings window",
        )

    def test_a_note_without_a_scope_gets_a_capital(self):
        self.assertEqual(mr.clean_note("ship it ([#1](https://example.org/1))"), "Ship it")

    def test_a_changelog_without_sections_gives_none(self):
        self.assertEqual(mr.parse_changelog("# Changelog\n"), [])


class RenderTests(unittest.TestCase):
    def test_every_section_becomes_a_release_with_its_version_date_and_notes(self):
        releases = releases_of(mr.fill(METAINFO, CHANGELOG, "0.2.0"))
        self.assertEqual([(r.get("version"), r.get("date")) for r in releases], [("0.2.0", "2026-11-03"), ("0.1.0", "2026-10-21")])
        notes = [li.text for li in releases[0].iter("li")]
        self.assertEqual(notes[2], "Ship a Flatpak <build> & more")  # survives the XML escaping
        self.assertEqual(len(notes), 4)

    def test_a_release_links_to_its_page_on_github(self):
        release = releases_of(mr.fill(METAINFO, CHANGELOG, "0.2.0"))[0]
        self.assertEqual(release.find("url").text, "https://github.com/Roboroads/repeat-that/releases/tag/v0.2.0")
        self.assertEqual(release.find("url").get("type"), "details")

    def test_the_build_version_may_be_older_than_the_newest_section(self):
        # A build of v0.1.0 lists only what was released by then.
        releases = releases_of(mr.fill(METAINFO, CHANGELOG, "0.1.0"))
        self.assertEqual([r.get("version") for r in releases], ["0.1.0"])

    def test_a_version_the_changelog_lacks_fails_naming_it(self):
        with self.assertRaises(mr.ReleaseNotesError) as caught:
            mr.fill(METAINFO, CHANGELOG, "0.3.0")
        self.assertIn("0.3.0", str(caught.exception))

    def test_a_release_without_notes_still_has_a_description(self):
        changelog = "## [1.0.0](https://example.org) (2027-01-01)\n\n### Documentation\n\n* docs only\n"
        release = releases_of(mr.fill(METAINFO, changelog, "1.0.0"))[0]
        self.assertTrue(release.find("description/p").text)

    def test_at_most_five_releases_are_listed(self):
        changelog = "".join(
            f"## [0.{n}.0](https://example.org) (2026-10-{n:02d})\n\n### Features\n\n* feature {n}\n\n" for n in range(9, 0, -1)
        )
        releases = releases_of(mr.fill(METAINFO, changelog, "0.9.0"))
        self.assertEqual([r.get("version") for r in releases], ["0.9.0", "0.8.0", "0.7.0", "0.6.0", "0.5.0"])

    def test_the_rest_of_the_metainfo_is_untouched(self):
        text = mr.fill(METAINFO, CHANGELOG, "0.2.0")
        self.assertIn("<!-- Filled in by the script. -->", text)
        self.assertIn("<id>me.roboroads.RepeatThat</id>", text)

    def test_a_metainfo_without_a_releases_element_fails(self):
        with self.assertRaises(mr.ReleaseNotesError):
            mr.fill("<component><id>x</id></component>", CHANGELOG, "0.2.0")

    def test_filling_an_already_filled_metainfo_gives_the_same_result(self):
        once = mr.fill(METAINFO, CHANGELOG, "0.2.0")
        self.assertEqual(mr.fill(once, CHANGELOG, "0.2.0"), once)


class CommandLineTests(unittest.TestCase):
    def test_it_writes_the_filled_metainfo_to_the_output_file(self):
        with tempfile.TemporaryDirectory() as folder:
            folder = Path(folder)
            (folder / "in.xml").write_text(METAINFO, encoding="utf-8")
            (folder / "CHANGELOG.md").write_text(CHANGELOG, encoding="utf-8")
            code = mr.main(
                ["--metainfo", str(folder / "in.xml"), "--changelog", str(folder / "CHANGELOG.md"), "--version", "0.2.0",
                 "--output", str(folder / "out.xml")]
            )
            self.assertEqual(code, 0)
            self.assertEqual(releases_of((folder / "out.xml").read_text(encoding="utf-8"))[0].get("version"), "0.2.0")

    def test_a_missing_version_exits_with_one_and_writes_nothing(self):
        with tempfile.TemporaryDirectory() as folder:
            folder = Path(folder)
            (folder / "in.xml").write_text(METAINFO, encoding="utf-8")
            (folder / "CHANGELOG.md").write_text(CHANGELOG, encoding="utf-8")
            code = mr.main(
                ["--metainfo", str(folder / "in.xml"), "--changelog", str(folder / "CHANGELOG.md"), "--version", "9.9.9",
                 "--output", str(folder / "out.xml")]
            )
            self.assertEqual(code, 1)
            self.assertFalse((folder / "out.xml").exists())


if __name__ == "__main__":
    unittest.main()
