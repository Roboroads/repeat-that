"""Unit tests for site_release.py. Run with: python3 -m unittest discover -s .github/scripts

File names, sizes and hashes are made up in memory; only the CLI tests at the bottom touch a temporary folder (with a
few bytes of text, nothing that looks like audio).
"""

import contextlib
import errno
import io
import json
import random
import tempfile
import unittest
import unittest.mock
from pathlib import Path

import release_assets as ra
import site_release as sr
from release_assets import ReleaseError

REPO = "Roboroads/repeat-that"
TAG = "v0.2.0"
BASE = f"https://github.com/{REPO}/releases/download/{TAG}"

EXE = "Repeat-That_0.2.0_x64-setup.exe"
DMG = "Repeat-That_0.2.0_aarch64.dmg"
APPIMAGE = "Repeat-That_0.2.0_amd64.AppImage"
DEB = "Repeat-That_0.2.0_amd64.deb"
RPM = "Repeat-That-0.2.0-1.x86_64.rpm"
FLATPAK = "Repeat-That_0.2.0_x86_64.flatpak"

SIZES = {EXE: 9500925, DMG: 8123456, APPIMAGE: 71000111, DEB: 6100222, RPM: 6100333, FLATPAK: 12000444}
HASHES = {name: format(index, "x") * 64 for index, name in enumerate(SIZES, 1)}


def sums_text(*names):
    """A SHA256SUMS body (sorted, as the workflow writes it) over the made-up hashes of `names`."""
    return "".join(f"{HASHES[name]}  {name}\n" for name in sorted(names))


def release(*names, sums=True, **overrides):
    """What `gh release view --json tagName,name,publishedAt,url,isDraft,isPrerelease,assets` prints."""
    assets = [
        {"name": name, "size": SIZES[name], "url": f"{BASE}/{name}", "digest": f"sha256:{HASHES[name]}"}
        for name in names
    ]
    if sums:
        assets.append({"name": "SHA256SUMS", "size": 98, "url": f"{BASE}/SHA256SUMS"})
    info = {
        "tagName": TAG,
        "name": TAG,
        "publishedAt": "2026-10-21T13:50:34Z",
        "url": f"https://github.com/{REPO}/releases/tag/{TAG}",
        "isDraft": False,
        "isPrerelease": False,
        "assets": assets,
    }
    info.update(overrides)
    return info


def build(*names, sums=None, **overrides):
    """(data, warnings) for a release holding `names`, whose SHA256SUMS lists `sums` (default: all of them)."""
    return sr.build_release_data(release(*names, **overrides), sums_text(*(names if sums is None else sums)), REPO)


def file_entry(name, kind):
    return {"name": name, "kind": kind, "url": f"{BASE}/{name}", "size": SIZES[name], "sha256": HASHES[name]}


def failure(testcase, *args, **kwargs):
    """The error messages of a build that must fail."""
    with testcase.assertRaises(ReleaseError) as caught:
        sr.build_release_data(*args, **kwargs)
    return caught.exception.errors


class ContractTests(unittest.TestCase):
    def test_a_windows_only_release_has_one_platform_with_hash_and_url_from_the_sums(self):
        data, warnings = build(EXE)
        self.assertEqual(warnings, [])
        self.assertEqual(
            data,
            {
                "schema": 1,
                "release": {
                    "tag": TAG,
                    "version": "0.2.0",
                    "published_at": "2026-10-21T13:50:34Z",
                    "url": f"https://github.com/{REPO}/releases/tag/{TAG}",
                    "sums_url": f"{BASE}/SHA256SUMS",
                    "platforms": [
                        {"name": "windows", "label": "Windows", "files": [file_entry(EXE, "setup-exe")]},
                    ],
                },
            },
        )

    def test_the_hash_comes_from_sha256sums_not_from_the_asset_digest(self):
        info = release(EXE)
        info["assets"][0]["digest"] = "sha256:" + "f" * 64
        data, _ = sr.build_release_data(info, sums_text(EXE), REPO)
        self.assertEqual(data["release"]["platforms"][0]["files"][0]["sha256"], HASHES[EXE])

    def test_the_urls_are_built_from_the_repo_and_tag(self):
        info = release(EXE)
        del info["assets"][0]["url"]  # not every gh version prints it
        data, _ = sr.build_release_data(info, sums_text(EXE), "Other/fork")
        release_data = data["release"]
        self.assertEqual(release_data["sums_url"], "https://github.com/Other/fork/releases/download/v0.2.0/SHA256SUMS")
        self.assertEqual(
            release_data["platforms"][0]["files"][0]["url"],
            f"https://github.com/Other/fork/releases/download/v0.2.0/{EXE}",
        )

    def test_a_file_name_is_escaped_in_its_url(self):
        name = "Repeat-That_0.2.0+1_x64-setup.exe"
        info = release()
        info["assets"].append({"name": name, "size": 5})
        data, _ = sr.build_release_data(info, f"{'a' * 64}  {name}\n", REPO)
        url = data["release"]["platforms"][0]["files"][0]["url"]
        self.assertEqual(url, f"{BASE}/Repeat-That_0.2.0%2B1_x64-setup.exe")

    def test_the_kind_comes_from_the_file_extension(self):
        data, _ = build(EXE, DMG, APPIMAGE, DEB, RPM, FLATPAK)
        kinds = {f["name"]: f["kind"] for p in data["release"]["platforms"] for f in p["files"]}
        self.assertEqual(
            kinds,
            {EXE: "setup-exe", DMG: "dmg", APPIMAGE: "appimage", DEB: "deb", RPM: "rpm", FLATPAK: "flatpak"},
        )

    def test_all_platforms_come_in_the_order_of_the_platform_table(self):
        data, warnings = build(DMG, FLATPAK, RPM, EXE, DEB, APPIMAGE)
        self.assertEqual(warnings, [])
        platforms = data["release"]["platforms"]
        self.assertEqual([p["name"] for p in platforms], [p.name for p in ra.PLATFORMS])
        self.assertEqual([p["label"] for p in platforms], [p.label for p in ra.PLATFORMS])

    def test_linux_lists_the_appimage_then_the_deb_then_the_rpm(self):
        for names in ([RPM, DEB, APPIMAGE], [DEB, RPM, APPIMAGE], [APPIMAGE, RPM, DEB]):
            data, _ = build(*names)
            (linux,) = data["release"]["platforms"]
            self.assertEqual([f["name"] for f in linux["files"]], [APPIMAGE, DEB, RPM], names)

    def test_a_platform_without_files_is_left_out(self):
        data, _ = build(EXE, APPIMAGE)
        self.assertEqual([p["name"] for p in data["release"]["platforms"]], ["windows", "linux"])

    def test_a_release_without_a_dmg_has_no_macos_entry(self):
        """The macOS .dmg is attached late; until then the site shows no macOS card."""
        data, warnings = build(EXE, APPIMAGE, DEB, RPM)
        self.assertEqual(warnings, [])
        self.assertEqual([p["name"] for p in data["release"]["platforms"]], ["windows", "linux"])

    def test_every_file_has_the_same_fields(self):
        data, _ = build(EXE, DMG, APPIMAGE, DEB, RPM, FLATPAK)
        for platform in data["release"]["platforms"]:
            self.assertEqual(list(platform), ["name", "label", "files"])
            for entry in platform["files"]:
                self.assertEqual(list(entry), ["name", "kind", "url", "size", "sha256"])
                self.assertIsInstance(entry["size"], int)

    def test_every_pattern_in_the_platform_table_has_a_kind(self):
        """A file pattern added to release_assets.PLATFORMS must not slip through without a download kind."""
        for platform in ra.PLATFORMS:
            for pattern in platform.patterns:
                self.assertIsNotNone(sr.kind_of(pattern.replace("*", "Repeat-That_0.2.0_x")), pattern)


class DeterminismTests(unittest.TestCase):
    NAMES = [EXE, DMG, APPIMAGE, DEB, RPM, FLATPAK]

    def test_the_same_input_gives_the_same_bytes(self):
        first = sr.render(build(*self.NAMES)[0])
        second = sr.render(build(*self.NAMES)[0])
        self.assertEqual(first, second)

    def test_the_order_of_the_assets_does_not_matter(self):
        expected = sr.render(build(*self.NAMES)[0])
        rng = random.Random(7)
        for _ in range(10):
            names = self.NAMES[:]
            rng.shuffle(names)
            info = release(*names)
            rng.shuffle(info["assets"])
            data, _ = sr.build_release_data(info, sums_text(*names), REPO)
            self.assertEqual(sr.render(data), expected)

    def test_the_output_has_the_layout_of_the_contract(self):
        expected = f"""{{
  "schema": 1,
  "release": {{
    "tag": "{TAG}",
    "version": "0.2.0",
    "published_at": "2026-10-21T13:50:34Z",
    "url": "https://github.com/{REPO}/releases/tag/{TAG}",
    "sums_url": "{BASE}/SHA256SUMS",
    "platforms": [
      {{
        "name": "windows",
        "label": "Windows",
        "files": [
          {{
            "name": "{EXE}",
            "kind": "setup-exe",
            "url": "{BASE}/{EXE}",
            "size": {SIZES[EXE]},
            "sha256": "{HASHES[EXE]}"
          }}
        ]
      }}
    ]
  }}
}}
"""
        self.assertEqual(sr.render(build(EXE)[0]), expected)

    def test_the_output_is_json_with_a_final_newline(self):
        text = sr.render(build(EXE)[0])
        self.assertTrue(text.endswith("}\n"))
        self.assertEqual(json.loads(text)["release"]["tag"], TAG)
        self.assertEqual(text, sr.render(json.loads(text)))  # reading and writing it again changes nothing


class NoReleaseTests(unittest.TestCase):
    def test_none_is_schema_1_with_a_null_release(self):
        self.assertEqual(json.loads(sr.render(sr.NO_RELEASE)), {"schema": 1, "release": None})


class SumsChecksTests(unittest.TestCase):
    def test_an_asset_missing_from_sha256sums_is_an_error(self):
        errors = failure(self, release(EXE, DMG), sums_text(EXE), REPO)
        self.assertEqual(len(errors), 1)
        self.assertIn(DMG, errors[0])
        self.assertIn("SHA256SUMS", errors[0])

    def test_every_asset_missing_from_sha256sums_is_reported(self):
        errors = failure(self, release(EXE, DMG, DEB), "", REPO)
        for name in (EXE, DMG, DEB):
            self.assertTrue(any(name in error for error in errors), name)

    def test_a_sha256sums_line_for_a_file_that_is_not_on_the_release_is_an_error(self):
        errors = failure(self, release(EXE), sums_text(EXE, DMG), REPO)
        self.assertEqual(len(errors), 1)
        self.assertIn(DMG, errors[0])

    def test_a_malformed_line_is_an_error(self):
        for bad in ("not a sums line\n", f"{'1' * 63}  {EXE}\n", f"{'G' * 64}  {EXE}\n", f"{HASHES[EXE]} {EXE}\n"):
            errors = failure(self, release(EXE), bad, REPO)
            self.assertTrue(errors, bad)
            self.assertIn("SHA256SUMS", errors[0], bad)

    def test_a_file_listed_twice_is_an_error(self):
        errors = failure(self, release(EXE), sums_text(EXE) * 2, REPO)
        self.assertIn(EXE, errors[0])

    def test_a_release_without_a_sha256sums_asset_is_an_error(self):
        errors = failure(self, release(EXE, sums=False), sums_text(EXE), REPO)
        self.assertEqual(len(errors), 1)
        self.assertIn("SHA256SUMS", errors[0])


class ReleaseStateTests(unittest.TestCase):
    def test_a_draft_is_an_error(self):
        errors = failure(self, release(EXE, isDraft=True), sums_text(EXE), REPO)
        self.assertEqual(len(errors), 1)
        self.assertIn("draft", errors[0])

    def test_a_prerelease_is_an_error(self):
        errors = failure(self, release(EXE, isPrerelease=True), sums_text(EXE), REPO)
        self.assertEqual(len(errors), 1)
        self.assertIn("prerelease", errors[0])

    def test_a_tag_that_is_not_a_version_is_an_error(self):
        for tag in ("site-media", "0.2.0", "v0.2", "v0.2.0-rc.1"):
            errors = failure(self, release(EXE, tagName=tag), sums_text(EXE), REPO)
            self.assertIn(tag, errors[0])

    def test_missing_fields_are_an_error(self):
        for field in ("tagName", "publishedAt", "url", "isDraft", "isPrerelease", "assets"):
            info = release(EXE)
            del info[field]
            errors = failure(self, info, sums_text(EXE), REPO)
            self.assertTrue(any(field in error for error in errors), field)

    def test_an_asset_without_a_size_is_an_error(self):
        for size in (None, "9500925", True, -1):
            info = release(EXE)
            info["assets"][0]["size"] = size
            errors = failure(self, info, sums_text(EXE), REPO)
            self.assertIn(EXE, errors[0], size)

    def test_a_repo_that_is_not_owner_slash_name_is_an_error(self):
        for repo in ("repeat-that", "a/b/c", "", "a b/c"):
            errors = failure(self, release(EXE), sums_text(EXE), repo)
            self.assertIn("OWNER/NAME", errors[0], repo)


class UnknownAssetTests(unittest.TestCase):
    def test_an_asset_that_belongs_to_no_platform_is_left_out_with_a_warning(self):
        info = release(EXE)
        info["assets"].append({"name": "notes.txt", "size": 12, "url": f"{BASE}/notes.txt"})
        sums = sums_text(EXE) + f"{'9' * 64}  notes.txt\n"
        data, warnings = sr.build_release_data(info, sums, REPO)
        self.assertEqual(len(warnings), 1)
        self.assertIn("notes.txt", warnings[0])
        names = [f["name"] for p in data["release"]["platforms"] for f in p["files"]]
        self.assertEqual(names, [EXE])

    def test_an_unknown_asset_still_needs_a_line_in_sha256sums(self):
        info = release(EXE)
        info["assets"].append({"name": "notes.txt", "size": 12, "url": f"{BASE}/notes.txt"})
        errors = failure(self, info, sums_text(EXE), REPO)
        self.assertIn("notes.txt", errors[0])

    def test_a_release_with_nothing_to_download_warns(self):
        data, warnings = sr.build_release_data(release(), "", REPO)
        self.assertEqual(data["release"]["platforms"], [])
        self.assertEqual(len(warnings), 1)

    def test_a_platform_without_a_kind_is_an_error(self):
        """release_assets.py knows a new platform, this script does not: fail instead of guessing a kind."""
        new = ra.Platform("snap", "Snap", False, ("*.snap",))
        info = release(EXE)
        info["assets"].append({"name": "Repeat-That_0.2.0_amd64.snap", "size": 5, "url": f"{BASE}/x.snap"})
        sums = sums_text(EXE) + f"{'9' * 64}  Repeat-That_0.2.0_amd64.snap\n"
        errors = failure(self, info, sums, REPO, platforms=ra.PLATFORMS + (new,))
        self.assertIn("Repeat-That_0.2.0_amd64.snap", errors[0])
        self.assertIn("KINDS", errors[0])

    def test_several_problems_are_all_reported(self):
        info = release(EXE, DMG, isPrerelease=True)
        errors = failure(self, info, sums_text(EXE) + f"{'9' * 64}  ghost.exe\n", REPO)
        self.assertEqual(len(errors), 3)


class CommandLineTests(unittest.TestCase):
    def run_main(self, *argv):
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            try:
                code = sr.main(list(argv))
            except SystemExit as stop:  # argparse usage errors
                code = stop.code
        return code, out.getvalue(), err.getvalue()

    def run_in_folder(self, info, sums, *extra):
        """main() over files in a temporary folder; (exit code, stdout, stderr, text of the output file or None)."""
        with tempfile.TemporaryDirectory() as folder:
            folder = Path(folder)
            (folder / "release.json").write_text(json.dumps(info), encoding="utf-8")
            if sums is not None:
                (folder / "SHA256SUMS").write_text(sums, encoding="utf-8")
            output = folder / "out" / "release.json"
            code, out, err = self.run_main(
                "--release-json", str(folder / "release.json"),
                "--sums", str(folder / "SHA256SUMS"),
                "--repo", REPO,
                "--output", str(output),
                *extra,
            )  # fmt: skip
            return code, out, err, output.read_text(encoding="utf-8") if output.exists() else None

    def test_a_release_is_written_to_the_output_file(self):
        code, out, err, text = self.run_in_folder(release(EXE, DMG), sums_text(EXE, DMG))
        self.assertEqual((code, out, err), (0, "", ""))
        data = json.loads(text)
        self.assertEqual(data["schema"], 1)
        self.assertEqual([p["name"] for p in data["release"]["platforms"]], ["windows", "macos"])

    def test_the_output_is_the_rendered_data(self):
        _, _, _, text = self.run_in_folder(release(EXE), sums_text(EXE))
        self.assertEqual(text, sr.render(build(EXE)[0]))

    def test_a_failed_write_leaves_the_old_output_alone_and_no_temporary_file(self):
        with tempfile.TemporaryDirectory() as folder:
            folder = Path(folder)
            (folder / "release.json").write_text(json.dumps(release(EXE)), encoding="utf-8")
            (folder / "SHA256SUMS").write_text(sums_text(EXE), encoding="utf-8")
            output = folder / "data.json"
            output.write_text("old\n", encoding="utf-8")
            with unittest.mock.patch("os.replace", side_effect=OSError(errno.EIO, "Input/output error")):
                code, _, err = self.run_main(
                    "--release-json", str(folder / "release.json"),
                    "--sums", str(folder / "SHA256SUMS"),
                    "--repo", REPO,
                    "--output", str(output),
                )  # fmt: skip
            self.assertEqual(code, 1)
            self.assertTrue(err.startswith("::error::"), err)
            self.assertIn(str(output), err)
            self.assertIn("Input/output error", err)
            self.assertEqual(output.read_text(encoding="utf-8"), "old\n")
            left = sorted(path.name for path in folder.iterdir())
            self.assertEqual(left, ["SHA256SUMS", "data.json", "release.json"])

    def test_a_write_leaves_no_temporary_file_behind(self):
        with tempfile.TemporaryDirectory() as folder:
            output = Path(folder) / "out" / "data.json"
            self.assertEqual(self.run_main("--none", "--output", str(output))[0], 0)
            self.assertEqual([path.name for path in output.parent.iterdir()], ["data.json"])

    def test_none_writes_a_null_release(self):
        with tempfile.TemporaryDirectory() as folder:
            output = Path(folder) / "release.json"
            code, out, err = self.run_main("--none", "--output", str(output))
            self.assertEqual((code, out, err), (0, "", ""))
            self.assertEqual(json.loads(output.read_text(encoding="utf-8")), {"schema": 1, "release": None})

    def test_a_warning_is_printed_as_an_annotation_and_the_run_still_succeeds(self):
        info = release(EXE)
        info["assets"].append({"name": "notes.txt", "size": 12, "url": f"{BASE}/notes.txt"})
        code, out, _, text = self.run_in_folder(info, sums_text(EXE) + f"{'9' * 64}  notes.txt\n")
        self.assertEqual(code, 0)
        self.assertTrue(out.startswith("::warning::"), out)
        self.assertIn("notes.txt", out)
        self.assertIsNotNone(text)

    def test_an_error_is_printed_as_an_annotation_and_nothing_is_written(self):
        code, out, err, text = self.run_in_folder(release(EXE, DMG), sums_text(EXE))
        self.assertEqual(code, 1)
        self.assertTrue(err.startswith("::error::"), err)
        self.assertIn(DMG, err)
        self.assertIsNone(text)

    def test_every_error_gets_its_own_annotation(self):
        code, _, err, _ = self.run_in_folder(release(EXE, DMG), "")
        self.assertEqual(code, 1)
        self.assertEqual(err.count("::error::"), 2)

    def test_a_missing_sums_file_is_an_error_naming_the_file_and_the_cause(self):
        code, _, err, text = self.run_in_folder(release(EXE), None)
        self.assertEqual(code, 1)
        self.assertTrue(err.startswith("::error::cannot read "), err)
        self.assertIn("SHA256SUMS: No such file or directory", err)
        self.assertIsNone(text)

    def test_a_sums_file_that_is_not_text_is_an_error_naming_the_file(self):
        with tempfile.TemporaryDirectory() as folder:
            folder = Path(folder)
            (folder / "release.json").write_text(json.dumps(release(EXE)), encoding="utf-8")
            (folder / "SHA256SUMS").write_bytes(b"\xff\xfe not utf-8")
            code, _, err = self.run_main(
                "--release-json", str(folder / "release.json"),
                "--sums", str(folder / "SHA256SUMS"),
                "--repo", REPO,
                "--output", str(folder / "out.json"),
            )  # fmt: skip
            self.assertEqual(code, 1)
            self.assertIn(f"cannot read {folder / 'SHA256SUMS'}: ", err)
            self.assertFalse((folder / "out.json").exists())

    def test_a_missing_sha256sums_asset_is_an_error_even_with_a_sums_file(self):
        code, _, err, text = self.run_in_folder(release(EXE, sums=False), sums_text(EXE))
        self.assertEqual(code, 1)
        self.assertIn("SHA256SUMS", err)
        self.assertIsNone(text)

    def test_a_draft_is_an_error(self):
        code, _, err, text = self.run_in_folder(release(EXE, isDraft=True), sums_text(EXE))
        self.assertEqual(code, 1)
        self.assertIn("draft", err)
        self.assertIsNone(text)

    def test_invalid_json_is_an_error(self):
        with tempfile.TemporaryDirectory() as folder:
            folder = Path(folder)
            (folder / "release.json").write_text("{not json", encoding="utf-8")
            (folder / "SHA256SUMS").write_text("", encoding="utf-8")
            code, _, err = self.run_main(
                "--release-json", str(folder / "release.json"),
                "--sums", str(folder / "SHA256SUMS"),
                "--repo", REPO,
                "--output", str(folder / "out.json"),
            )  # fmt: skip
            self.assertEqual(code, 1)
            self.assertIn("release.json", err)
            self.assertFalse((folder / "out.json").exists())

    def test_a_missing_release_file_is_an_error(self):
        with tempfile.TemporaryDirectory() as folder:
            code, _, err = self.run_main(
                "--release-json", str(Path(folder) / "nope.json"),
                "--sums", str(Path(folder) / "SHA256SUMS"),
                "--repo", REPO,
                "--output", str(Path(folder) / "out.json"),
            )  # fmt: skip
            self.assertEqual(code, 1)
            self.assertIn("nope.json", err)

    def test_the_options_must_be_complete(self):
        with tempfile.TemporaryDirectory() as folder:
            output = str(Path(folder) / "out.json")
            for argv in (
                ["--output", output],
                ["--release-json", "r.json", "--output", output],
                ["--release-json", "r.json", "--sums", "s", "--output", output],
                ["--none"],
                ["--none", "--release-json", "r.json", "--output", output],
                ["--none", "--repo", REPO, "--output", output],
            ):
                code, _, _ = self.run_main(*argv)
                self.assertEqual(code, 2, argv)
            self.assertFalse(Path(output).exists())


if __name__ == "__main__":
    unittest.main()
