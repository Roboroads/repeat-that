#!/usr/bin/env python3
"""Fills the <releases> element of the AppStream metainfo from CHANGELOG.md.

release-please writes CHANGELOG.md and bumps the version, but it cannot add a <release> to an XML file, and a Flatpak
needs one for the version it ships. So the metainfo in the repository has an empty <releases /> and the Flatpak build
runs this script on a copy of it: every changelog section up to the version being built (at most five) becomes a
<release> with its date and its notes.

Usage: metainfo_releases.py --metainfo FILE --changelog FILE --version X.Y.Z [--output FILE]
  Without --output the filled metainfo goes to the standard output.

Fails (exit 1) when the changelog has no section for the version, so a build of a version without release notes stops
instead of shipping a Flatpak whose newest release is another one. Unit tests: test_metainfo_releases.py.
"""

import argparse
import dataclasses
import re
import sys
from pathlib import Path
from xml.sax.saxutils import escape

REPO_URL = "https://github.com/Roboroads/repeat-that"
# The newest releases listed. AppStream only shows a few, and an old release's notes are of no use to anyone.
MAX_RELEASES = 5
# The changelog sections that are a note for users. Documentation, chores and the like are left out.
NOTE_SECTIONS = ("BREAKING CHANGES", "Features", "Bug Fixes", "Performance Improvements")

SECTION = re.compile(r"^## \[(?P<version>\d+\.\d+\.\d+)\]\([^)]*\) \((?P<date>\d{4}-\d{2}-\d{2})\)\s*$")
KIND = re.compile(r"^### (?:⚠ )?(?P<kind>.+?)\s*$")
BULLET = re.compile(r"^\* (?P<text>.+?)\s*$")
# `([#14](https://...))` and `([8792366](https://...))`, which release-please puts after each note.
LINK_GROUP = re.compile(r"\s*\(\[[^\]]*\]\([^)]*\)\)")
SCOPE = re.compile(r"^\*\*(?P<scope>[^*]+):\*\*\s*")
RELEASES = re.compile(r"^(?P<indent>[ \t]*)<releases\s*/>|^(?P<indent2>[ \t]*)<releases>.*?</releases>", re.M | re.S)


class ReleaseNotesError(Exception):
    """The metainfo can't be filled in."""


@dataclasses.dataclass
class Section:
    version: str
    date: str
    notes: list


def clean_note(text):
    """One changelog bullet as a sentence for users: no links, no commit hash, the scope as a prefix, a capital."""
    text = LINK_GROUP.sub("", text).strip()
    scope = SCOPE.match(text)
    if scope:
        text = f"{scope.group('scope')}: {text[scope.end():]}"
    return text[:1].upper() + text[1:]


def parse_changelog(text):
    """The sections of release-please's CHANGELOG.md, newest first, each with the notes of its user-facing parts."""
    sections, kind = [], None
    for raw in text.splitlines():
        heading = SECTION.match(raw)
        if heading:
            sections.append(Section(heading["version"], heading["date"], []))
            kind = None
            continue
        if not sections:
            continue
        part = KIND.match(raw)
        if part:
            kind = part["kind"]
            continue
        bullet = BULLET.match(raw)
        if bullet and kind in NOTE_SECTIONS:
            sections[-1].notes.append(clean_note(bullet["text"]))
    return sections


def render_release(section, indent):
    notes = "".join(f"{indent}        <li>{escape(note)}</li>\n" for note in section.notes)
    if notes:
        body = f"{indent}      <ul>\n{notes}{indent}      </ul>\n"
    else:
        body = f"{indent}      <p>Maintenance release.</p>\n"
    return (
        f'{indent}  <release version="{section.version}" date="{section.date}">\n'
        f'{indent}    <url type="details">{REPO_URL}/releases/tag/v{section.version}</url>\n'
        f"{indent}    <description>\n{body}{indent}    </description>\n"
        f"{indent}  </release>\n"
    )


def version_key(version):
    return tuple(int(part) for part in version.split("."))


def fill(metainfo, changelog, version):
    """`metainfo` with its <releases> filled in for a build of `version`."""
    sections = parse_changelog(changelog)
    if not any(section.version == version for section in sections):
        raise ReleaseNotesError(
            f"CHANGELOG.md has no section for {version}, so the metainfo would have no release for the version "
            "being built. release-please adds it in the release PR."
        )
    listed = [section for section in sections if version_key(section.version) <= version_key(version)]
    match = RELEASES.search(metainfo)
    if not match:
        raise ReleaseNotesError("the metainfo has no <releases> element")
    indent = match["indent"] if match["indent"] is not None else match["indent2"]
    body = "".join(render_release(section, indent) for section in listed[:MAX_RELEASES])
    block = f"{indent}<releases>\n{body}{indent}</releases>"
    return metainfo[: match.start()] + block + metainfo[match.end() :]


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--metainfo", required=True, type=Path)
    parser.add_argument("--changelog", required=True, type=Path)
    parser.add_argument("--version", required=True)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args(argv)
    try:
        filled = fill(
            args.metainfo.read_text(encoding="utf-8"), args.changelog.read_text(encoding="utf-8"), args.version
        )
    except (ReleaseNotesError, OSError) as error:
        print(f"::error::{error}", file=sys.stderr)
        return 1
    if args.output:
        args.output.write_text(filled, encoding="utf-8")
    else:
        sys.stdout.write(filled)
    return 0


if __name__ == "__main__":
    sys.exit(main())
