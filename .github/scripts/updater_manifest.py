#!/usr/bin/env python3
"""Write the updater manifest of one platform of a release.

The app checks https://github.com/OWNER/NAME/releases/latest/download/latest-<target>-<arch>.json for a newer version
(Tauri's static update format). publish-release.yml signs a platform's update file and calls this to write that
platform's manifest. It works offline from files and stdlib only. Like site_release.py it imports the modules next to
it, so run it as a script, not with `python3 -I`. Unit tests: test_updater_manifest.py.

Usage:
  updater_manifest.py --tag TAG --platform windows|macos|linux --payload FILE --sig FILE --notes FILE
                      --repo OWNER/NAME --pub-date TIMESTAMP --output-dir DIR

--payload is the update file (windows: the NSIS `*-setup.exe`, macos: the `*.app.tar.gz` of the .app, linux: the
`*.AppImage`) and --sig the `<payload name>.sig` that `tauri signer sign` wrote next to it. Only the name of the payload
is used: the file itself must exist (a typo in a path should not go unnoticed) but is not read here.
--notes is the release body (`gh release view TAG --json body --jq .body`), --pub-date an RFC 3339 timestamp such as
`publishedAt` of `gh release view` (2026-10-21T13:50:34Z), passed in so the same input always gives the same bytes.

It writes DIR/latest-<target>-<arch>.json (latest-windows-x86_64.json, latest-darwin-aarch64.json,
latest-linux-x86_64.json), prints its path and has this content:
  {"notes", "platforms", "pub_date", "version"}, keys sorted at every level, two spaces of indent, one final newline.
`version` is the tag without the v. `platforms` has two keys, both with the same {"signature", "url"} entry: the
`<target>-<arch>` key and the `<target>-<arch>-<installer>` key (windows-x86_64 and windows-x86_64-nsis, darwin-aarch64
and darwin-aarch64-app, linux-x86_64 and linux-x86_64-appimage). `url` is the download URL of the payload on the release
of the tag and `signature` the content of the .sig file without its line end.

The plain `<target>-<arch>` key is there on purpose. The app looks up `<target>-<arch>-<installer>` first and falls back
to it, and a Linux .deb or .rpm install (installers `deb` and `rpm`) has no key of its own: with the fallback, its check
finds the new version, which it only announces. Such an install must never download the entry: it is the AppImage, and
the plugin refuses those bytes as "not a valid deb package" after fetching them.

`notes` is plain text for the app to show as text: line ends become \\n, control characters (not tab and newline) and
HTML comments (`<!-- macos-untested -->`) are removed, except in code (a fenced block or `inline code`) and when never
closed, runs of blank lines shrink to one, and the result is cut to 4000 bytes of UTF-8 (at most 4000 characters, UTF-16
units or bytes, whichever way the app counts), ending in an ellipsis when something was cut.

It fails (exit 1, `::error::` annotations, nothing written) when a file is missing, the tag isn't v0.1.0-shaped, the
payload doesn't carry the version of the tag or doesn't look like an update file of the platform, the .sig isn't named
after the payload or isn't a base64 minisign signature made for that file, or the repository or the timestamp is
malformed.
"""

import argparse
import base64
import dataclasses
import datetime
import json
import os
import re
import sys
from pathlib import Path
from urllib.parse import quote

from release_assets import TAG, ReleaseError, check_versions, find_platform, matches, read_text
from site_release import REPO, read, write_output

# The notes are cut to this many bytes of UTF-8. The app caps what it gets at 4000 as well, counting characters, UTF-16
# units or bytes. Bytes are the most of the three for any text, so notes that fit 4000 bytes fit 4000 of whatever the
# app counts: it never cuts them again, which would cost the ellipsis that marks the cut.
NOTES_MAX_BYTES = 4000
ELLIPSIS = "\u2026"
# A whole HTML comment (`<!--` up to the next `-->`) or, to keep it, a piece of code: a fenced block (also one that is
# never closed, as in markdown) or an inline span. A comment that is never closed is not one and stays as text.
COMMENT_OR_CODE = re.compile(r"(?P<code>```.*?(?:```|\Z)|`[^`\n]*`)|<!--.*?-->", re.DOTALL)
# Every control character (category Cc) except tab and newline.
CONTROL = re.compile("[\x00-\x08\x0b-\x1f\x7f-\x9f]")
BLANK_LINES = re.compile(r"\n{3,}")
PUB_DATE = re.compile(
    r"(?P<stamp>\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2})(?:\.\d+)?(?:Z|[+-](?P<hours>\d{2}):(?P<minutes>\d{2}))"
)


@dataclasses.dataclass(frozen=True)
class Target:
    """What the updater of one platform downloads and under which names the app looks it up."""

    platform: str  # name in release_assets.PLATFORMS
    key: str  # <target>-<arch>, the name the app looks its own system up by
    installer: str  # the second key adds this: <target>-<arch>-<installer>
    pattern: str  # the file name pattern (fnmatch) of the payload

    @property
    def label(self):
        return find_platform(self.platform).label

    @property
    def keys(self):
        # The plain key is the fallback of installs without a key of their own (the module docstring says why).
        return (self.key, f"{self.key}-{self.installer}")

    @property
    def manifest(self):
        return f"latest-{self.key}.json"


TARGETS = (
    Target("windows", "windows-x86_64", "nsis", "*-setup.exe"),
    Target("macos", "darwin-aarch64", "app", "*.app.tar.gz"),
    Target("linux", "linux-x86_64", "appimage", "*.AppImage"),
)
TARGETS_BY_PLATFORM = {target.platform: target for target in TARGETS}


def clean_notes(text):
    """The release body as the plain text the app shows (see the module docstring for what is done)."""
    text = text.replace("\r\n", "\n").replace("\r", "\n")
    text = CONTROL.sub("", text)  # before the comments: a control character can't hide a `<!--`
    text = COMMENT_OR_CODE.sub(lambda found: found["code"] or "", text)
    text = "\n".join(line.rstrip() for line in text.split("\n"))
    text = BLANK_LINES.sub("\n\n", text).strip("\n")
    encoded = text.encode("utf-8")
    if len(encoded) > NOTES_MAX_BYTES:
        room = NOTES_MAX_BYTES - len(ELLIPSIS.encode("utf-8"))
        # "ignore" drops the half of a character the cut may have split.
        text = encoded[:room].decode("utf-8", errors="ignore").rstrip() + ELLIPSIS
    return text


def read_signature(text, name):
    """The signature of a .sig file: its one line of base64 without the line end. Raises ReleaseError when the text is
    something else, or was made for another file than `name`, so a wrong file can't end up in a manifest (the app would
    only find out once it has downloaded the update). `tauri signer sign` writes the base64 of a minisign signature
    file: "untrusted comment: ...", signature, "trusted comment: timestamp:<seconds><tab>file:<name>", global signature.
    """
    line = text.strip()
    if not line:
        raise ReleaseError("the signature file is empty")
    try:
        decoded = base64.b64decode(line, validate=True).decode("utf-8")
    except ValueError:  # not base64 (also a line break inside), or not text
        decoded = ""
    _, found, trusted = decoded.partition("\ntrusted comment:")
    if not decoded.startswith("untrusted comment:") or not found:
        raise ReleaseError("the signature file is not the base64 of a minisign signature, as `tauri signer sign` makes")
    fields = trusted.split("\n", 1)[0].strip().split("\t")
    files = [field[len("file:") :] for field in fields if field.startswith("file:")]
    if not files:
        raise ReleaseError("the signature does not say which file it was made for (no file: in its trusted comment)")
    if files != [name]:
        raise ReleaseError(f"the signature was made for {files[0]!r}, not for {name}")
    return line


def check_pub_date(value):
    """True for an RFC 3339 timestamp with a time zone, such as 2026-10-21T13:50:34Z."""
    found = PUB_DATE.fullmatch(value)
    if not found:
        return False
    try:
        datetime.datetime.strptime(found["stamp"], "%Y-%m-%dT%H:%M:%S")
    except ValueError:
        return False
    return found["hours"] is None or (int(found["hours"]) <= 23 and int(found["minutes"]) <= 59)


def build_manifest(*, tag, platform, payload, signature, notes, repo, pub_date):
    """(file name, data) of the manifest of `platform` for the release of `tag`. `payload` is the path or name of the
    update file, `signature` the text of its .sig file and `notes` the release body. Raises ReleaseError listing every
    problem found."""
    errors = []
    name = os.path.basename(payload)

    target = TARGETS_BY_PLATFORM.get(platform)
    if target is None:
        known = ", ".join(TARGETS_BY_PLATFORM)
        errors.append(f"no updates for platform {platform!r}; the platforms with updates are {known}")
    elif not matches(target.pattern, name):
        errors.append(f"{name} is not an update file of {target.label}: expected a name like {target.pattern}")

    if not TAG.fullmatch(tag):
        errors.append(f"tag must look like v0.1.0, got {tag!r}")
    else:
        try:
            check_versions([name], tag)
        except ReleaseError as error:
            errors += error.errors

    try:
        signature = read_signature(signature, name)
    except ReleaseError as error:
        errors += error.errors
    if not REPO.fullmatch(repo):
        errors.append(f"the repository must look like OWNER/NAME, got {repo!r}")
    if not check_pub_date(pub_date):
        errors.append(f"pub_date must be an RFC 3339 timestamp like 2026-10-21T13:50:34Z, got {pub_date!r}")
    if errors:
        raise ReleaseError(errors)

    entry = {"signature": signature, "url": f"https://github.com/{repo}/releases/download/{tag}/{quote(name, safe='')}"}
    data = {
        "version": tag[1:],
        "notes": clean_notes(notes),
        "pub_date": pub_date,
        "platforms": {key: dict(entry) for key in target.keys},
    }
    return target.manifest, data


def render(data):
    """The text of the output file: the same data always gives the same bytes."""
    return json.dumps(data, indent=2, sort_keys=True, ensure_ascii=False) + "\n"


def read_inputs(payload, sig, notes):
    """(text of the .sig file, text of the notes file); raises ReleaseError naming every file that can't be used."""
    errors = []
    if not Path(payload).is_file():
        errors.append(f"the update file {payload} does not exist")
    expected = Path(payload).name + ".sig"
    if Path(sig).name != expected:
        errors.append(f"{Path(sig).name} is not the signature of {Path(payload).name}: expected {expected}")
    texts = []
    for path in (sig, notes):
        try:
            texts.append(read(read_text, path))
        except ReleaseError as error:
            errors += error.errors
    if errors:
        raise ReleaseError(errors)
    return texts


def build_parser():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--tag", required=True, help="the release tag, v0.2.0")
    parser.add_argument("--platform", required=True, choices=list(TARGETS_BY_PLATFORM), help="the platform to write")
    parser.add_argument("--payload", required=True, help="the update file the app downloads")
    parser.add_argument("--sig", required=True, help="the .sig file of the payload")
    parser.add_argument("--notes", required=True, help="the release body, as text")
    parser.add_argument("--repo", required=True, help="OWNER/NAME, for the download URL")
    parser.add_argument("--pub-date", required=True, help="RFC 3339 timestamp, such as 2026-10-21T13:50:34Z")
    parser.add_argument("--output-dir", required=True, help="the folder to write latest-<target>-<arch>.json to")
    return parser


def main(argv=None):
    args = build_parser().parse_args(argv)
    try:
        signature, notes = read_inputs(args.payload, args.sig, args.notes)
        name, data = build_manifest(
            tag=args.tag, platform=args.platform, payload=args.payload, signature=signature, notes=notes,
            repo=args.repo, pub_date=args.pub_date,
        )  # fmt: skip
    except ReleaseError as error:
        for message in error.errors:
            print(f"::error::{message}", file=sys.stderr)
        return 1
    path = Path(args.output_dir) / name
    try:
        write_output(path, render(data))
    except OSError as error:
        print(f"::error::cannot write {path}: {error.strerror}", file=sys.stderr)
        return 1
    print(path)
    return 0


if __name__ == "__main__":
    sys.exit(main())
