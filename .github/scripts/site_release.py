#!/usr/bin/env python3
"""Turn the latest release into the download data the website is built from.

site.yml fetches the latest release and its SHA256SUMS with `gh` and calls this to write the JSON that `site/` reads.
It works offline from those files and takes the platform table (which file belongs to which platform) from
release_assets.py. Unit tests: test_site_release.py.

Usage:
  site_release.py --release-json FILE --sums FILE --repo OWNER/NAME --output FILE
  site_release.py --none --output FILE

--release-json is the output of `gh release view --json tagName,name,publishedAt,url,isDraft,isPrerelease,assets`,
--sums the release's SHA256SUMS. --none writes {"schema": 1, "release": null} for a repository with no release yet.

The output (schema 1) is {"schema": 1, "release": {"tag", "version", "published_at", "url", "sums_url", "platforms"}}.
`platforms` lists, in the order of release_assets.PLATFORMS, each platform that has files: {"name", "label", "files"}.
A file is {"name", "kind", "url", "size", "sha256"}. `kind` is setup-exe, dmg, appimage, deb, rpm or flatpak, taken
from the extension; the files of a platform come in that order (Linux: AppImage, deb, rpm), then by name. The same
input always gives the same bytes.

It fails (exit 1, `::error::` annotations, no output written) when the release is a draft or a prerelease, has no
SHA256SUMS, has an asset without a line in it (any asset, also one that belongs to no platform: publish-release.yml
lists every file it attaches, so a file without a line was not put there by a release), or SHA256SUMS lists a file the
release doesn't have or has a malformed line. The build then fails and the previous site stays live. An asset that
belongs to no platform but is listed in SHA256SUMS is left out with a `::warning::`. The update files of a release
made with an update key (the .sig files, latest-*.json and the macOS .app.tar.gz, see release_assets.py) are left out
without one: they are for the app's updater, not for people.
"""

import argparse
import json
import os
import re
import sys
from pathlib import Path
from urllib.parse import quote

from release_assets import (
    PLATFORMS,
    SUMS_NAME,
    TAG,
    ReleaseError,
    is_update_file,
    owner,
    parse_sums,
    read_json,
    read_text,
)

SCHEMA = 1
NO_RELEASE = {"schema": SCHEMA, "release": None}
# Extension -> kind. The order is the order of the files inside a platform.
KINDS = {
    ".exe": "setup-exe",
    ".dmg": "dmg",
    ".AppImage": "appimage",
    ".deb": "deb",
    ".rpm": "rpm",
    ".flatpak": "flatpak",
}
KIND_ORDER = list(KINDS.values())
REPO = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")
# What `gh release view --json ...` has to give, and as what type. A draft has no publishedAt yet.
FIELDS = {"tagName": str, "publishedAt": str, "url": str, "isDraft": bool, "isPrerelease": bool, "assets": list}


def kind_of(name):
    """The download kind of a file, from its extension, or None."""
    return KINDS.get(os.path.splitext(name)[1])


def is_size(value):
    return isinstance(value, int) and not isinstance(value, bool) and value >= 0


def build_release_data(info, sums, repo, platforms=PLATFORMS):
    """(data, warnings) for the release `info` (`gh release view --json` output) and the text of its SHA256SUMS.
    Raises ReleaseError listing every problem found."""
    errors, warnings = [], []
    if not REPO.match(repo):
        raise ReleaseError(f"the repository must look like OWNER/NAME, got {repo!r}")
    if not isinstance(info, dict):
        raise ReleaseError("the release JSON is not an object")

    draft = info.get("isDraft") is True
    if draft:
        errors.append("the latest release is a draft, which the website must not link to")
    if info.get("isPrerelease") is True:
        errors.append("the latest release is a prerelease, which the website must not link to")
    unusable = [
        field
        for field, kind in FIELDS.items()
        if not isinstance(info.get(field), kind) and not (draft and field == "publishedAt")
    ]
    if unusable:
        errors.append(f"the release JSON has no usable {', '.join(unusable)}")
        raise ReleaseError(errors)

    tag = info["tagName"]
    if not TAG.match(tag):
        errors.append(f"the release tag must look like v0.1.0, got {tag!r}")

    assets = {}
    for asset in info["assets"]:
        name = asset.get("name") if isinstance(asset, dict) else None
        if not isinstance(name, str) or not name:
            raise ReleaseError(errors + ["the release JSON has an asset without a name"])
        if name != SUMS_NAME and not is_size(asset.get("size")):
            errors.append(f"{name} has no usable size in the release JSON")
        assets[name] = asset

    listed = None
    if SUMS_NAME not in assets:
        errors.append(f"the release has no {SUMS_NAME} asset")
    else:
        try:
            listed = parse_sums(sums)
        except ReleaseError as error:
            errors += error.errors
    if listed is not None:
        errors += [
            f"{name} is on the release but has no line in {SUMS_NAME}"
            for name in assets
            if name != SUMS_NAME and name not in listed
        ]
        errors += [
            f"{SUMS_NAME} lists {name}, which is not on the release"
            for name in listed
            if name != SUMS_NAME and name not in assets
        ]

    grouped = {}
    for name, asset in assets.items():
        if name == SUMS_NAME or is_update_file(name, platforms):
            continue  # the update files (signatures, manifests, the macOS bundle) are for the app, not for people
        platform = owner(name, platforms)
        if platform is None:
            warnings.append(f"{name} belongs to no platform, so the website leaves it out")
            continue
        kind = kind_of(name)
        if kind is None:
            errors.append(f"{name} is a {platform.label} file, but site_release.py has no kind for it: add it to KINDS")
            continue
        if listed is None or name not in listed:
            continue  # reported above
        url = f"https://github.com/{repo}/releases/download/{tag}/{quote(name, safe='')}"
        sha256 = listed[name].partition("  ")[0]
        grouped.setdefault(platform.name, []).append(
            {"name": name, "kind": kind, "url": url, "size": asset["size"], "sha256": sha256}
        )

    if errors:
        raise ReleaseError(errors)
    if not grouped:
        warnings.append("the release has no file to download")

    out = []
    for platform in platforms:
        files = sorted(grouped.get(platform.name, []), key=lambda f: (KIND_ORDER.index(f["kind"]), f["name"]))
        if files:
            out.append({"name": platform.name, "label": platform.label, "files": files})
    data = {
        "schema": SCHEMA,
        "release": {
            "tag": tag,
            "version": tag[1:],
            "published_at": info["publishedAt"],
            "url": info["url"],
            "sums_url": f"https://github.com/{repo}/releases/download/{tag}/{SUMS_NAME}",
            "platforms": out,
        },
    }
    return data, warnings


def render(data):
    """The text of the output file: the same data always gives the same bytes."""
    return json.dumps(data, indent=2, ensure_ascii=False) + "\n"


def read(reader, path):
    """reader(path), where reader is read_json or read_text of release_assets.py, with a failure reported as a
    ReleaseError that names the file and the cause."""
    try:
        return reader(path)
    except OSError as error:
        raise ReleaseError(f"cannot read {path}: {error.strerror}")
    except json.JSONDecodeError as error:
        raise ReleaseError(f"{path} is not valid JSON: {error}")
    except ValueError as error:  # not UTF-8
        raise ReleaseError(f"cannot read {path}: {error}")


def write_output(path, text):
    """Write `text` to a temporary file next to `path`, then move it over `path`: a failed write leaves the old file
    (or none) instead of a partial one."""
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + ".tmp")
    try:
        with open(temporary, "w", encoding="utf-8", newline="\n") as stream:
            stream.write(text)
        os.replace(temporary, path)
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise


def build_parser():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--release-json", help="output of `gh release view --json ...`")
    parser.add_argument("--sums", help="the release's SHA256SUMS")
    parser.add_argument("--repo", help="OWNER/NAME, for the download URLs")
    parser.add_argument("--none", action="store_true", help="write the data for a repository with no release")
    parser.add_argument("--output", required=True, help="the JSON file to write")
    return parser


def main(argv=None):
    parser = build_parser()
    args = parser.parse_args(argv)
    given = (args.release_json, args.sums, args.repo)
    if args.none and any(given):
        parser.error("--none takes only --output")
    if not args.none and not all(given):
        parser.error("--release-json, --sums and --repo are all required (or use --none)")

    try:
        if args.none:
            data, warnings = NO_RELEASE, []
        else:
            info = read(read_json, args.release_json)
            data, warnings = build_release_data(info, read(read_text, args.sums), args.repo)
    except ReleaseError as error:
        for message in error.errors:
            print(f"::error::{message}", file=sys.stderr)
        return 1
    for warning in warnings:
        print(f"::warning::{warning}")
    try:
        write_output(args.output, render(data))
    except OSError as error:
        print(f"::error::cannot write {args.output}: {error.strerror}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
