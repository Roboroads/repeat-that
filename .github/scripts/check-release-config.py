#!/usr/bin/env python3
"""Check that release-please is configured to bump every file that carries the product version.

Usage: check-release-config.py [--tags] [repo-root]

A release that bumps Cargo.toml but not app/package.json (or the other way round) ships installers whose version
disagrees with the release tag, so this fails when:
  * the config or manifest is not valid JSON, or the root package is missing from either
  * tags would carry a component prefix (the release workflow expects `vX.Y.Z`)
  * `release-as` is set but is not a plain X.Y.Z version
  * Cargo.toml (`workspace.package.version`), app/package.json (`version`) or one of their lockfiles (the workspace
    crates in Cargo.lock, `version` and `packages[""].version` in app/package-lock.json) is not listed in `extra-files`
  * an `extra-files` entry points at a missing file or a jsonpath that does not resolve to the manifest version
  * app/src-tauri/tauri.conf.json sets its own `version` (it must inherit the Cargo one)

A `release-as` below the manifest version only warns; remove it. A pin equal to the manifest version can't be judged
from the files: a release PR bumps the manifest to the pinned version before the release exists, and once the release
has shipped the pin is spent.

With --tags (release-please.yml, before release-please acts) it also fails when `release-as` names a version that
already has a `vX.Y.Z` tag on origin, which is a spent pin: left in, the next release PR would propose that version
again and collide with the tag.
"""

import json
import os
import re
import subprocess
import sys
import tomllib
from pathlib import Path

SEMVER = re.compile(r"^(\d+)\.(\d+)\.(\d+)$")
# Files that carry the product version: (path, type, jsonpath). The lockfiles are on the list so a release leaves
# them in step with the manifests; `$.package[?(!@.source)]` is every Cargo.lock entry that is not from a registry,
# that is, the workspace crates.
REQUIRED = [
    ("Cargo.toml", "toml", "$.workspace.package.version"),
    ("Cargo.lock", "toml", "$.package[?(!@.source)].version"),
    ("app/package.json", "json", "$.version"),
    ("app/package-lock.json", "json", "$.version"),
    ("app/package-lock.json", "json", "$.packages[''].version"),
]

# One step of a jsonpath: `.key`, `['']` (the empty key, as in package-lock.json) or `[?(!@.key)]` (array entries
# without that key).
PATH_STEP = re.compile(r"\.(?P<key>[A-Za-z0-9_-]+)|(?P<empty>\[''\])|\[\?\(!@\.(?P<missing>[A-Za-z0-9_-]+)\)\]")


def version_tuple(text):
    match = SEMVER.match(text) if isinstance(text, str) else None
    return tuple(int(part) for part in match.groups()) if match else None


def resolve(document, jsonpath):
    """Values at a jsonpath, as a list (empty when nothing matches). Supports only the steps PATH_STEP knows."""
    if not jsonpath.startswith("$"):
        raise ValueError(f"unsupported jsonpath {jsonpath!r}")
    nodes, position = [document], 1
    while position < len(jsonpath):
        step = PATH_STEP.match(jsonpath, position)
        if step is None:
            raise ValueError(f"unsupported jsonpath {jsonpath!r} (this check knows .key, [''] and [?(!@.key)])")
        position = step.end()
        if step.group("key") is not None or step.group("empty") is not None:
            key = step.group("key") if step.group("key") is not None else ""
            nodes = [node[key] for node in nodes if isinstance(node, dict) and key in node]
        else:
            missing = step.group("missing")
            nodes = [
                item
                for node in nodes
                if isinstance(node, list)
                for item in node
                if isinstance(item, dict) and missing not in item
            ]
    return nodes


def load(root, relative, kind):
    path = root / relative
    if not path.is_file():
        raise FileNotFoundError(f"{relative} does not exist")
    with path.open("rb") as handle:
        return tomllib.load(handle) if kind == "toml" else json.load(handle)


def remote_tag_exists(root, tag):
    """True when origin has the tag. Raises when origin can't be asked, so a flaky network can't pass the check."""
    result = subprocess.run(
        ["git", "ls-remote", "--exit-code", "--tags", "origin", f"refs/tags/{tag}"],
        cwd=root,
        capture_output=True,
        text=True,
    )
    if result.returncode not in (0, 2):  # --exit-code: 2 means no matching ref
        raise RuntimeError(f"git ls-remote failed: {result.stderr.strip()}")
    return result.returncode == 0


def check(root, tag_exists=None):
    """Returns (errors, warnings), both lists of messages. tag_exists(tag) is only given with --tags."""
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
        errors.append('"include-component-in-tag" must be false: publish-release.yml expects tags like v0.1.0')

    pinned = package.get("release-as", config.get("release-as"))
    if pinned is not None:
        if version_tuple(pinned) is None:
            errors.append(f'"release-as" {pinned!r} is not a plain X.Y.Z version')
        elif tag_exists is not None and tag_exists(f"v{pinned}"):
            errors.append(
                f'"release-as" {pinned} already has the tag v{pinned}: remove "release-as" from '
                "release-please-config.json, or the next release PR proposes that version again and collides with the tag"
            )
        elif version_tuple(current) and version_tuple(pinned) < version_tuple(current):
            warnings.append(
                f'"release-as" {pinned} is below the released version {current}: remove it, or the next release '
                "PR proposes that version again"
            )

    listed = set()
    for entry in package.get("extra-files", []):
        if not isinstance(entry, dict):
            errors.append(f"extra-files entry {entry!r} is a plain path; give it a type and a jsonpath")
            continue
        path, kind, jsonpath = entry.get("path"), entry.get("type"), entry.get("jsonpath")
        listed.add((path, kind, jsonpath))
        if kind not in ("json", "toml") or not path or not jsonpath:
            errors.append(f"extra-files entry {entry!r} needs type json|toml, path and jsonpath")
            continue
        try:
            values = resolve(load(root, path, kind), jsonpath)
        except (OSError, ValueError) as error:
            errors.append(f"extra-files {path}: {error}")
            continue
        if not values:
            errors.append(f"extra-files {path}: {jsonpath} does not resolve to a value")
        elif any(value != current for value in values):
            errors.append(f"extra-files {path}: {jsonpath} is {sorted(set(map(str, values)))}, the manifest says {current!r}")

    for path, kind, jsonpath in REQUIRED:
        if (path, kind, jsonpath) not in listed:
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
    args = sys.argv[1:]
    with_tags = "--tags" in args
    args = [arg for arg in args if arg != "--tags"]
    root = Path(args[0] if args else ".").resolve()
    tag_exists = (lambda tag: remote_tag_exists(root, tag)) if with_tags else None
    try:
        errors, warnings = check(root, tag_exists)
    except RuntimeError as error:
        errors, warnings = [str(error)], []
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
