"""Unit tests for updater_manifest.py. Run with: python3 -m unittest discover -s .github/scripts

Names, signatures and notes are made up in memory; only the CLI tests at the bottom touch a temporary folder (with a few
bytes of text, nothing that looks like audio).
"""

import base64
import contextlib
import errno
import io
import json
import tempfile
import unittest
import unittest.mock
from pathlib import Path

import release_assets as ra
import updater_manifest as um
from release_assets import ReleaseError

REPO = "Roboroads/repeat-that"
TAG = "v0.2.0"
BASE = f"https://github.com/{REPO}/releases/download/{TAG}"
PUB_DATE = "2026-10-21T13:50:34Z"

EXE = "Repeat-That_0.2.0_x64-setup.exe"
APP = "Repeat-That_0.2.0_aarch64.app.tar.gz"
APPIMAGE = "Repeat-That_0.2.0_amd64.AppImage"
DMG = "Repeat-That_0.2.0_aarch64.dmg"
DEB = "Repeat-That_0.2.0_amd64.deb"
PAYLOADS = {"windows": EXE, "macos": APP, "linux": APPIMAGE}


def signature(label="signature from tauri secret key"):
    """What `tauri signer sign` writes to the .sig file: a minisign signature file, base64 encoded, on one line."""
    minisign = (
        f"untrusted comment: {label}\n"
        "RUQqzR3dX1h0eVSyY2dGZUVqYy9FZ2pPdTJ0cHRVbWFuZGl2ZXJzaXR5VGF1cmlTaWduYXR1cmU=\n"
        "trusted comment: timestamp:1761054634\tfile:Repeat-That_0.2.0_x64-setup.exe\tprehashed\n"
        "d2VsbCB0aGlzIGlzIG5vdCBhIHJlYWwgc2lnbmF0dXJlIGJ1dCBpdCBpcyBiYXNlNjQ=\n"
    )
    return base64.b64encode(minisign.encode()).decode()


SIG = signature()


def build(platform="windows", payload=None, sig=SIG + "\n", notes="", tag=TAG, repo=REPO, pub_date=PUB_DATE):
    """(manifest file name, data) with the usual input, any part of which a test can change."""
    return um.build_manifest(
        tag=tag, platform=platform, payload=payload or PAYLOADS[platform], signature=sig, notes=notes, repo=repo,
        pub_date=pub_date,
    )  # fmt: skip


def errors_of(*args, **kwargs):
    """The error messages build() fails with; fails the test when it doesn't fail."""
    try:
        build(*args, **kwargs)
    except ReleaseError as error:
        return error.errors
    raise AssertionError(f"build({args}, {kwargs}) did not fail")


class ManifestFormatTests(unittest.TestCase):
    """The static format of the Tauri updater, one file per platform (plan 3.1)."""

    def test_windows(self):
        name, data = build("windows")
        self.assertEqual(name, "latest-windows-x86_64.json")
        self.assertEqual(
            data,
            {
                "version": "0.2.0",
                "notes": "",
                "pub_date": PUB_DATE,
                "platforms": {
                    "windows-x86_64": {"url": f"{BASE}/{EXE}", "signature": SIG},
                    "windows-x86_64-nsis": {"url": f"{BASE}/{EXE}", "signature": SIG},
                },
            },
        )

    def test_macos(self):
        name, data = build("macos")
        self.assertEqual(name, "latest-darwin-aarch64.json")
        self.assertEqual(
            data["platforms"],
            {
                "darwin-aarch64": {"url": f"{BASE}/{APP}", "signature": SIG},
                "darwin-aarch64-app": {"url": f"{BASE}/{APP}", "signature": SIG},
            },
        )

    def test_linux(self):
        name, data = build("linux")
        self.assertEqual(name, "latest-linux-x86_64.json")
        self.assertEqual(
            data["platforms"],
            {
                "linux-x86_64": {"url": f"{BASE}/{APPIMAGE}", "signature": SIG},
                "linux-x86_64-appimage": {"url": f"{BASE}/{APPIMAGE}", "signature": SIG},
            },
        )

    def test_the_version_is_the_tag_without_the_v(self):
        for tag, version in (("v0.2.0", "0.2.0"), ("v10.20.30", "10.20.30")):
            payload = f"Repeat-That_{version}_x64-setup.exe"
            self.assertEqual(build(tag=tag, payload=payload)[1]["version"], version)

    def test_the_url_is_built_from_the_repo_and_the_tag_and_the_payload_name(self):
        _, data = build("linux", repo="someone/fork")
        self.assertEqual(
            data["platforms"]["linux-x86_64"]["url"],
            f"https://github.com/someone/fork/releases/download/v0.2.0/{APPIMAGE}",
        )

    def test_only_the_name_of_the_payload_goes_into_the_url(self):
        _, data = build("windows", payload=f"/tmp/some folder/artifacts/{EXE}")
        self.assertEqual(data["platforms"]["windows-x86_64"]["url"], f"{BASE}/{EXE}")

    def test_characters_that_do_not_belong_in_a_url_are_escaped(self):
        _, data = build("windows", payload="Repeat That_0.2.0_x64-setup.exe")
        self.assertEqual(data["platforms"]["windows-x86_64"]["url"], f"{BASE}/Repeat%20That_0.2.0_x64-setup.exe")

    def test_the_signature_is_the_sig_file_without_the_line_end(self):
        for text in (SIG, SIG + "\n", SIG + "\r\n", SIG + "\n\n", f"  {SIG}  \n"):
            _, data = build(sig=text)
            self.assertEqual(data["platforms"]["windows-x86_64"]["signature"], SIG, repr(text))

    def test_both_keys_of_a_platform_carry_the_same_entry(self):
        for platform in PAYLOADS:
            first, second = build(platform)[1]["platforms"].values()
            self.assertEqual(first, second)

    def test_a_pub_date_with_an_offset_or_fractional_seconds_is_kept_as_given(self):
        for date in ("2026-10-21T13:50:34Z", "2026-10-21T15:50:34+02:00", "2026-10-21T13:50:34.123Z"):
            self.assertEqual(build(pub_date=date)[1]["pub_date"], date)

    def test_the_notes_are_cleaned(self):
        notes = "## 0.2.0\n\n<!-- macos-untested -->\n- fixed it\r\n"
        self.assertEqual(build(notes=notes)[1]["notes"], "## 0.2.0\n\n- fixed it")

    def test_every_updater_platform_is_a_platform_of_the_release(self):
        names = {platform.name for platform in ra.PLATFORMS}
        self.assertEqual({target.platform for target in um.TARGETS}, set(PAYLOADS))
        self.assertLessEqual({target.platform for target in um.TARGETS}, names)

    def test_the_payload_patterns_of_the_files_that_are_release_files_already_are_theirs(self):
        # The Windows installer and the AppImage are release files already; the macOS tarball is new in the updater.
        for platform in ("windows", "linux"):
            self.assertIn(um.TARGETS_BY_PLATFORM[platform].pattern, ra.find_platform(platform).patterns)
            self.assertEqual(ra.classify(PAYLOADS[platform]), platform)


class ErrorTests(unittest.TestCase):
    def test_a_tag_of_the_wrong_form_fails(self):
        for tag in ("0.2.0", "v0.2", "v0.2.0-rc.1", "v0.2.0.1", "V0.2.0", "latest", "", "v0.2.0\n", " v0.2.0"):
            errors = errors_of(tag=tag)
            self.assertTrue(any("tag must look like v0.1.0" in error for error in errors), (tag, errors))

    def test_a_payload_that_does_not_carry_the_version_of_the_tag_fails(self):
        names = ("Repeat-That_0.1.0_x64-setup.exe", "Repeat-That_10.2.0_x64-setup.exe", "Repeat-That_x64-setup.exe")
        for name in names:
            errors = errors_of(payload=name)
            self.assertEqual(len(errors), 1, errors)
            self.assertIn(f"{name} does not carry version 0.2.0", errors[0])

    def test_a_payload_of_another_platform_fails(self):
        wrong = (("windows", APPIMAGE), ("windows", APP), ("linux", EXE), ("macos", DMG), ("macos", EXE))
        for platform, other in wrong:
            errors = errors_of(platform, payload=other)
            self.assertEqual(len(errors), 1, (platform, other, errors))
            self.assertIn(other, errors[0])
            self.assertIn("update file", errors[0])

    def test_a_package_or_a_sig_file_is_no_payload(self):
        for name in (DEB, "Repeat-That_0.2.0_amd64.rpm", f"{EXE}.sig", f"{APPIMAGE}.sig", "SHA256SUMS"):
            for platform in PAYLOADS:
                self.assertTrue(errors_of(platform, payload=name), (platform, name))

    def test_the_error_names_the_pattern_a_payload_should_match(self):
        self.assertIn("*-setup.exe", errors_of("windows", payload=APP)[0])
        self.assertIn("*.app.tar.gz", errors_of("macos", payload=EXE)[0])
        self.assertIn("*.AppImage", errors_of("linux", payload=EXE)[0])

    def test_a_platform_without_updates_fails(self):
        for platform in ("flatpak", "freebsd", "", "Windows"):
            with self.assertRaises(ReleaseError) as caught:
                build(platform, payload=EXE)
            self.assertIn("windows, macos, linux", str(caught.exception))

    def test_a_bad_signature_fails(self):
        raw_minisign = base64.b64decode(SIG).decode()
        not_minisign = base64.b64encode(b"hello world, this is no signature").decode()
        for text in ("", "\n", "   ", "not a signature!", f"{SIG}\n{SIG}", raw_minisign, not_minisign, SIG[:-3]):
            errors = errors_of(sig=text)
            self.assertEqual(len(errors), 1, (text, errors))
            self.assertIn("signature", errors[0], text)

    def test_a_signature_file_cut_short_fails(self):
        first_lines = "\n".join(base64.b64decode(SIG).decode().split("\n")[:2])
        errors = errors_of(sig=base64.b64encode(first_lines.encode()).decode())
        self.assertEqual(len(errors), 1, errors)

    def test_a_signature_with_a_line_break_inside_fails(self):
        half = len(SIG) // 2
        self.assertTrue(errors_of(sig=f"{SIG[:half]}\n{SIG[half:]}\n"))

    def test_a_bad_pub_date_fails(self):
        bad = (
            "", "yesterday", "2026-10-21", "2026-10-21 13:50:34", "2026-13-40T00:00:00Z", "2026-10-21T13:50:34",
            "2026-10-21T13:50:34+25:00", "2026-10-21T13:50:34+02:61",
        )  # fmt: skip
        for date in bad:
            errors = errors_of(pub_date=date)
            self.assertEqual(len(errors), 1, (date, errors))
            self.assertIn("pub_date", errors[0])

    def test_a_bad_repo_fails(self):
        for repo in ("", "repeat-that", "a/b/c", "a b/c", "a/b\n"):
            errors = errors_of(repo=repo)
            self.assertEqual(len(errors), 1, (repo, errors))
            self.assertIn("OWNER/NAME", errors[0])

    def test_every_problem_is_reported(self):
        errors = errors_of("linux", payload="Repeat-That_0.1.0_amd64.AppImage", sig="", pub_date="soon", repo="x")
        self.assertEqual(len(errors), 4, errors)

    def test_the_payload_of_the_wrong_platform_does_not_hide_the_other_problems(self):
        errors = errors_of("macos", payload=EXE, sig="")
        self.assertEqual(len(errors), 2, errors)


class CleanNotesTests(unittest.TestCase):
    clean = staticmethod(um.clean_notes)

    def test_plain_text_is_left_alone(self):
        text = "## 0.2.0\n\n- one\n- two\n\n\tindented\n\nGrüße, 日本語, emoji \U0001f600"
        self.assertEqual(self.clean(text), text)

    def test_html_comments_are_stripped(self):
        self.assertEqual(self.clean("before <!-- macos-untested --> after"), "before  after")
        self.assertEqual(self.clean("<!-- a -->x<!-- b -->y<!---->z"), "xyz")

    def test_a_comment_over_several_lines_is_stripped(self):
        self.assertEqual(self.clean("one\n<!-- two\nthree\n\nfour -->\nfive"), "one\n\nfive")

    def test_a_comment_on_a_line_of_its_own_leaves_no_gap(self):
        text = "## 0.2.0\n\n<!-- macos-untested -->\n\n- fixed it\n"
        self.assertEqual(self.clean(text), "## 0.2.0\n\n- fixed it")

    def test_a_comment_that_is_not_closed_stays_as_text(self):
        text = "visible\n<!-- never closed\nstill visible"
        self.assertEqual(self.clean(text), text)

    def test_only_the_comments_that_are_there_are_taken_out(self):
        # What is left over is shown as text, so a leftover that looks like a comment hides nothing and drops nothing.
        self.assertEqual(self.clean("<!<!-- x -->-- y -->after"), "<!-- y -->after")
        self.assertEqual(self.clean("<<!-- x -->!-- y -->after"), "<!-- y -->after")

    def test_code_is_left_as_it_is(self):
        for text in (
            "strips `<!-- like this -->` from the notes",
            "```html\n<!-- a block -->\n```",
            "```\n<!-- a block that is never closed\n-->",
            "two `<!-- spans -->` and `<!-- more -->`",
            "empty `` span <!--",
        ):
            self.assertEqual(self.clean(text), text, text)

    def test_a_comment_next_to_code_is_still_taken_out(self):
        self.assertEqual(self.clean("x <!-- a --> `<!-- b -->`"), "x  `<!-- b -->`")
        self.assertEqual(self.clean("`code` <!-- a --> `more`"), "`code`  `more`")
        self.assertEqual(self.clean("```\ncode\n```\n<!-- a -->\nafter"), "```\ncode\n```\n\nafter")

    def test_a_comment_that_holds_a_backtick_is_taken_out_whole(self):
        self.assertEqual(self.clean("<!-- it`s -->after`"), "after`")
        self.assertEqual(self.clean("<!-- `x` -->after"), "after")
        self.assertEqual(self.clean("it`s <!-- c --> done"), "it`s  done")

    def test_a_comment_cannot_be_hidden_from_the_stripping_by_a_control_character(self):
        self.assertEqual(self.clean("<!\x00-- x -->after"), "after")
        self.assertEqual(self.clean("a<\x1b!-- x --\x07>b"), "ab")

    def test_other_html_is_left_as_text(self):
        text = "<b>bold</b> <script>alert(1)</script> --> <!- not a comment ->"
        self.assertEqual(self.clean(text), text)

    def test_control_characters_are_removed_but_tab_and_newline_stay(self):
        self.assertEqual(self.clean("a\x00b\x07c\x08d\x7fe\x85f\x9bg"), "abcdefg")
        self.assertEqual(self.clean("a\tb\nc"), "a\tb\nc")
        self.assertEqual(self.clean("x\x1b[31mred\x1b[0m"), "x[31mred[0m")

    def test_windows_and_old_mac_line_ends_become_newlines(self):
        self.assertEqual(self.clean("one\r\ntwo\rthree\n"), "one\ntwo\nthree")

    def test_surrounding_white_space_and_runs_of_blank_lines_are_trimmed(self):
        self.assertEqual(self.clean("\n\n  one  \n\n\n\n\ntwo \t \n\n"), "  one\n\ntwo")

    def test_nothing_left_is_an_empty_string(self):
        for text in ("", "\n", "<!-- only a comment -->", " \x00 \n<!-- x -->\n "):
            self.assertEqual(self.clean(text), "", repr(text))

    def test_the_cap_is_4000_characters(self):
        self.assertEqual(um.NOTES_LIMIT, 4000)

    def test_text_of_exactly_4000_characters_is_not_touched(self):
        text = "a" * 4000
        self.assertEqual(self.clean(text), text)

    def test_text_over_4000_characters_is_cut_to_4000_with_an_ellipsis(self):
        cut = self.clean("a" * 4001)
        self.assertEqual(len(cut), 4000)
        self.assertEqual(cut, "a" * 3999 + "…")
        self.assertEqual(self.clean("b" * 100_000), "b" * 3999 + "…")

    def test_the_cap_counts_characters_not_bytes(self):
        self.assertEqual(self.clean("é" * 4000), "é" * 4000)
        self.assertEqual(len(self.clean("日" * 4001)), 4000)

    def test_white_space_before_the_cut_does_not_stay_in_front_of_the_ellipsis(self):
        text = "a" * 3997 + "\n\n\n" + "b" * 100
        self.assertEqual(self.clean(text), "a" * 3997 + "…")

    def test_the_cap_applies_to_what_is_left_after_the_cleaning(self):
        text = "a" * 2000 + "<!-- " + "c" * 5000 + " -->" + "b" * 2000
        self.assertEqual(self.clean(text), "a" * 2000 + "b" * 2000)

    def test_cleaning_twice_changes_nothing(self):
        for text in ("a" * 5000, "<!-- x -->\n\n\n\nb\r\n\x00", "x" * 3999 + "\n\n" + "y" * 10):
            once = self.clean(text)
            self.assertEqual(self.clean(once), once)


class DeterminismTests(unittest.TestCase):
    def test_the_same_input_gives_the_same_bytes(self):
        for platform in PAYLOADS:
            first = um.render(build(platform, notes="x\n- y")[1])
            second = um.render(build(platform, notes="x\n- y")[1])
            self.assertEqual(first, second)

    def test_the_keys_are_sorted_at_every_level(self):
        text = um.render(build("windows", notes="n")[1])
        data = json.loads(text)
        self.assertEqual(list(data), ["notes", "platforms", "pub_date", "version"])
        self.assertEqual(list(data["platforms"]), ["windows-x86_64", "windows-x86_64-nsis"])
        for entry in data["platforms"].values():
            self.assertEqual(list(entry), ["signature", "url"])

    def test_the_text_is_indented_ends_with_one_newline_and_keeps_unicode(self):
        text = um.render(build(notes="Grüße 日本")[1])
        self.assertTrue(text.startswith('{\n  "notes": "Grüße 日本",\n'), text)
        self.assertTrue(text.endswith("}\n") and not text.endswith("\n\n"), repr(text[-5:]))
        self.assertNotIn("\\u", text)

    def test_the_rendered_text_is_the_same_data_when_read_back(self):
        _, data = build("macos", notes="a\nb")
        self.assertEqual(json.loads(um.render(data)), data)


class CommandLineTests(unittest.TestCase):
    def run_main(self, *argv):
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            try:
                code = um.main(list(argv))
            except SystemExit as stop:  # argparse usage errors
                code = stop.code
        return code, out.getvalue(), err.getvalue()

    def files(self, folder, payload=EXE, sig=SIG + "\n", notes="Release notes\n", sig_name=None):
        """Write the input files into `folder`; a sig or notes of None leaves that file out. Returns the paths."""
        folder = Path(folder)
        paths = {
            "payload": folder / payload,
            "sig": folder / (sig_name or payload + ".sig"),
            "notes": folder / "notes.md",
        }
        paths["payload"].write_bytes(b"not really an installer")
        if sig is not None:
            paths["sig"].write_text(sig, encoding="utf-8")
        if notes is not None:
            paths["notes"].write_text(notes, encoding="utf-8")
        return paths

    def args(self, folder, paths, platform="windows", **overrides):
        options = {
            "--tag": TAG, "--platform": platform, "--payload": str(paths["payload"]), "--sig": str(paths["sig"]),
            "--notes": str(paths["notes"]), "--repo": REPO, "--pub-date": PUB_DATE,
            "--output-dir": str(Path(folder) / "out"),
        }  # fmt: skip
        options.update(overrides)
        return [item for pair in options.items() for item in pair]

    def test_a_manifest_is_written_to_a_file_named_after_the_platform(self):
        with tempfile.TemporaryDirectory() as folder:
            paths = self.files(folder)
            code, out, err = self.run_main(*self.args(folder, paths))
            written = Path(folder) / "out" / "latest-windows-x86_64.json"
            self.assertEqual((code, err), (0, ""))
            self.assertEqual(out, f"{written}\n")
            data = json.loads(written.read_text(encoding="utf-8"))
            self.assertEqual(data["version"], "0.2.0")
            self.assertEqual(data["notes"], "Release notes")
            self.assertEqual(data["platforms"]["windows-x86_64-nsis"], {"url": f"{BASE}/{EXE}", "signature": SIG})

    def test_every_platform_gets_its_own_file(self):
        for platform, payload in PAYLOADS.items():
            with tempfile.TemporaryDirectory() as folder:
                paths = self.files(folder, payload=payload)
                code, out, err = self.run_main(*self.args(folder, paths, platform))
                self.assertEqual((code, err), (0, ""), platform)
                written = Path(out.strip())
                self.assertEqual(written.parent, Path(folder) / "out")
                self.assertEqual(written.name, um.TARGETS_BY_PLATFORM[platform].manifest)

    def test_the_file_is_the_rendered_manifest(self):
        with tempfile.TemporaryDirectory() as folder:
            paths = self.files(folder, notes="Release notes\n<!-- macos-untested -->\n")
            self.run_main(*self.args(folder, paths))
            name, data = build("windows", notes="Release notes")
            self.assertEqual((Path(folder) / "out" / name).read_text(encoding="utf-8"), um.render(data))

    def test_running_twice_gives_the_same_file(self):
        with tempfile.TemporaryDirectory() as folder:
            paths = self.files(folder)
            target = Path(folder) / "out" / "latest-windows-x86_64.json"
            self.run_main(*self.args(folder, paths))
            first = target.read_bytes()
            self.run_main(*self.args(folder, paths))
            self.assertEqual(target.read_bytes(), first)

    def test_an_empty_release_body_gives_empty_notes(self):
        with tempfile.TemporaryDirectory() as folder:
            paths = self.files(folder, notes="")
            self.assertEqual(self.run_main(*self.args(folder, paths))[0], 0)
            data = json.loads((Path(folder) / "out" / "latest-windows-x86_64.json").read_text(encoding="utf-8"))
            self.assertEqual(data["notes"], "")

    def test_a_missing_sig_file_is_an_error_naming_the_file_and_nothing_is_written(self):
        with tempfile.TemporaryDirectory() as folder:
            paths = self.files(folder, sig=None)
            code, out, err = self.run_main(*self.args(folder, paths))
            self.assertEqual((code, out), (1, ""))
            self.assertTrue(err.startswith("::error::cannot read "), err)
            self.assertIn(f"{EXE}.sig: No such file or directory", err)
            self.assertFalse((Path(folder) / "out").exists())

    def test_a_missing_payload_is_an_error_naming_the_file(self):
        with tempfile.TemporaryDirectory() as folder:
            paths = self.files(folder, payload=EXE)
            paths["payload"].unlink()
            code, _, err = self.run_main(*self.args(folder, paths))
            self.assertEqual(code, 1)
            self.assertIn(f"::error::the update file {paths['payload']} does not exist", err)
            self.assertFalse((Path(folder) / "out").exists())

    def test_a_payload_that_is_a_folder_is_an_error(self):
        with tempfile.TemporaryDirectory() as folder:
            paths = self.files(folder)
            paths["payload"].unlink()
            paths["payload"].mkdir()
            code, _, err = self.run_main(*self.args(folder, paths))
            self.assertEqual(code, 1)
            self.assertIn("does not exist", err)

    def test_a_missing_notes_file_is_an_error(self):
        with tempfile.TemporaryDirectory() as folder:
            paths = self.files(folder, notes=None)
            code, _, err = self.run_main(*self.args(folder, paths))
            self.assertEqual(code, 1)
            self.assertIn("notes.md: No such file or directory", err)
            self.assertFalse((Path(folder) / "out").exists())

    def test_every_missing_file_is_reported(self):
        with tempfile.TemporaryDirectory() as folder:
            paths = self.files(folder, payload=EXE, sig=None, notes=None)
            paths["payload"].unlink()
            code, _, err = self.run_main(*self.args(folder, paths))
            self.assertEqual(code, 1)
            self.assertEqual(err.count("::error::"), 3, err)

    def test_a_file_that_is_not_text_is_an_error_naming_the_file(self):
        with tempfile.TemporaryDirectory() as folder:
            paths = self.files(folder)
            paths["sig"].write_bytes(b"\xff\xfe not utf-8")
            code, _, err = self.run_main(*self.args(folder, paths))
            self.assertEqual(code, 1)
            self.assertIn(f"cannot read {paths['sig']}: ", err)

    def test_a_sig_file_that_belongs_to_another_payload_is_an_error(self):
        with tempfile.TemporaryDirectory() as folder:
            paths = self.files(folder, sig_name=f"{APPIMAGE}.sig")
            code, _, err = self.run_main(*self.args(folder, paths))
            self.assertEqual(code, 1)
            self.assertIn(f"{APPIMAGE}.sig is not the signature of {EXE}: expected {EXE}.sig", err)
            self.assertFalse((Path(folder) / "out").exists())

    def test_a_bad_signature_is_an_error_and_the_old_manifest_stays(self):
        with tempfile.TemporaryDirectory() as folder:
            paths = self.files(folder, sig="this is not a signature\n")
            old = Path(folder) / "out" / "latest-windows-x86_64.json"
            old.parent.mkdir()
            old.write_text("old\n", encoding="utf-8")
            code, out, err = self.run_main(*self.args(folder, paths))
            self.assertEqual((code, out), (1, ""))
            self.assertTrue(err.startswith("::error::"), err)
            self.assertEqual(old.read_text(encoding="utf-8"), "old\n")

    def test_every_error_gets_its_own_annotation(self):
        with tempfile.TemporaryDirectory() as folder:
            paths = self.files(folder, sig="", payload=EXE)
            code, _, err = self.run_main(*self.args(folder, paths, **{"--tag": "v0.3.0", "--pub-date": "soon"}))
            self.assertEqual(code, 1)
            self.assertEqual(err.count("::error::"), 3, err)

    def test_a_payload_of_another_platform_is_an_error(self):
        with tempfile.TemporaryDirectory() as folder:
            paths = self.files(folder, payload=APPIMAGE)
            code, _, err = self.run_main(*self.args(folder, paths, "windows"))
            self.assertEqual(code, 1)
            self.assertIn(f"{APPIMAGE} is not an update file of Windows", err)

    def test_a_failed_write_is_an_error_and_leaves_no_temporary_file(self):
        with tempfile.TemporaryDirectory() as folder:
            paths = self.files(folder)
            with unittest.mock.patch("os.replace", side_effect=OSError(errno.EIO, "Input/output error")):
                code, out, err = self.run_main(*self.args(folder, paths))
            self.assertEqual((code, out), (1, ""))
            self.assertTrue(err.startswith("::error::cannot write "), err)
            self.assertIn("Input/output error", err)
            self.assertEqual(list((Path(folder) / "out").iterdir()), [])

    def test_the_options_must_be_complete_and_valid(self):
        with tempfile.TemporaryDirectory() as folder:
            paths = self.files(folder)
            full = self.args(folder, paths)
            for index in range(0, len(full), 2):
                code, _, _ = self.run_main(*full[:index], *full[index + 2 :])
                self.assertEqual(code, 2, full[index])
            code, _, _ = self.run_main(*self.args(folder, paths, "flatpak"))
            self.assertEqual(code, 2)
            self.assertFalse((Path(folder) / "out").exists())


if __name__ == "__main__":
    unittest.main()
