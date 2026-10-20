#!/usr/bin/env python3
"""Check that release-please is configured to bump every file that carries the product version.

Usage: check-release-config.py [repo-root]

A release that bumps Cargo.toml but not app/package.json (or the other way round) ships installers whose version
disagrees with the release tag, so this fails when:
  * the config or manifest is not valid JSON, or the root package is missing from either
  * tags would carry a component prefix (the release workflow expects `vX.Y.Z`)
  * `release-as` is set but is not a plain X.Y.Z version
  * Cargo.toml (`workspace.package.version`) or app/package.json (`version`) is not listed in `extra-files`
  * an `extra-files` entry points at a missing file or a jsonpath that does not resolve to the manifest version
  * app/src-tauri/tauri.conf.json sets its own `version` (it must inherit the Cargo one)

A `release-as` that is not above the manifest version any more (the pinned release shipped) only warns; remove it.
"""

import json
import os
import re
import sys
import tomllib
from pathlib import Path

SEMVER = re.compile(r"^(\d+)\.(\d+)\.(\d+)$")
# Files that carry the product version: (path, type, jsonpath).
REQUIRED = [
    ("Cargo.toml", "toml", "$.workspace.package.version"),
    ("app/package.json", "json", "$.version"),
]


def version_tuple(text):
    match = SEMVER.match(text) if isinstance(text, str) else None
    return tuple(int(part) for part in match.groups()) if match else None


def resolve(document, jsonpath):
    """Resolve a plain `$.a.b.c` path, the only form this repo uses. Returns None when it does not resolve."""
    if not re.fullmatch(r"\$(\.[A-Za-z0-9_-]+)+", jsonpath):
        raise ValueError(f"unsupported jsonpath {jsonpath!r} (only plain $.a.b.c is checked)")
    node = document
    for key in jsonpath[2:].split("."):
        if not isinstance(node, dict) or key not in node:
            return None
        node = node[key]
    return node


def load(root, relative, kind):
    path = root / relative
    if not path.is_file():
        raise FileNotFoundError(f"{relative} does not exist")
    with path.open("rb") as handle:
        return tomllib.load(handle) if kind == "toml" else json.load(handle)


def check(root):
    """Returns (errors, warnings), both lists of messages."""
    errors, warnings = [], []

    try:
        config = load(root, "release-please-config.json", "json")
        manifest = load(root, ".release-please-manifest.json", "json")
    except (OSError, ValueError) as error:
        return [f"cannot read the release-please files: {error}"], warnings

    package = config.get("packages", {}).get(".")
    if package is None:
        return ['release-please-config.json has no packages["."] entry'], warnings
    if "." not in manifest:
        return ['.release-please-manifest.json has no "." entry'], warnings
    current = manifest["."]
    if version_tuple(current) is None:
        errors.append(f"manifest version {current!r} is not X.Y.Z")

    if package.get("include-component-in-tag", config.get("include-component-in-tag")) is not False:
        errors.append('"include-component-in-tag" must be false: build-windows.yml expects tags like v0.1.0')

    pinned = package.get("release-as", config.get("release-as"))
    if pinned is not None:
        if version_tuple(pinned) is None:
            errors.append(f'"release-as" {pinned!r} is not a plain X.Y.Z version (MSI versions must be numeric)')
        elif version_tuple(current) and version_tuple(pinned) <= version_tuple(current):
            warnings.append(
                f'"release-as" {pinned} is not above the released version {current}: remove it, or the next release '
                "PR proposes that version again"
            )

    listed = {}
    for entry in package.get("extra-files", []):
        if not isinstance(entry, dict):
            errors.append(f"extra-files entry {entry!r} is a plain path; give it a type and a jsonpath")
            continue
        path, kind, jsonpath = entry.get("path"), entry.get("type"), entry.get("jsonpath")
        listed[path] = (kind, jsonpath)
        if kind not in ("json", "toml") or not path or not jsonpath:
            errors.append(f"extra-files entry {entry!r} needs type json|toml, path and jsonpath")
            continue
        try:
            value = resolve(load(root, path, kind), jsonpath)
        except (OSError, ValueError) as error:
            errors.append(f"extra-files {path}: {error}")
            continue
        if value is None:
            errors.append(f"extra-files {path}: {jsonpath} does not resolve to a value")
        elif value != current:
            errors.append(f"extra-files {path}: {jsonpath} is {value!r}, the manifest says {current!r}")

    for path, kind, jsonpath in REQUIRED:
        if listed.get(path) != (kind, jsonpath):
            errors.append(f"extra-files must bump {jsonpath} in {path} (type {kind})")

    try:
        tauri = load(root, "app/src-tauri/tauri.conf.json", "json")
    except (OSError, ValueError) as error:
        errors.append(f"cannot read tauri.conf.json: {error}")
    else:
        if "version" in tauri:
            errors.append("app/src-tauri/tauri.conf.json sets `version`; remove it so the bundle uses the Cargo version")

    return errors, warnings


def main():
    root = Path(sys.argv[1] if len(sys.argv) > 1 else ".").resolve()
    errors, warnings = check(root)
    on_actions = os.environ.get("GITHUB_ACTIONS") == "true"
    for message in warnings:
        print(f"::warning::{message}" if on_actions else f"warning: {message}")
    for message in errors:
        print(f"::error::{message}" if on_actions else f"error: {message}")
    if errors:
        return 1
    print("release-please config ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
