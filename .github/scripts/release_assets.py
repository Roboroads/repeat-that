#!/usr/bin/env python3
"""The one place that knows which platforms a release has and which files belong to each.

publish-release.yml calls this to decide whether a release may be published, to write its SHA256SUMS and to attach a
late optional file (the macOS .dmg) to a release that is already out. Unit tests: test_release_assets.py.

Usage: release_assets.py <command> ...
  classify FILE...             print "<platform> <file>" for each file; fail on a file that belongs to no platform
  sums [--existing FILE [--keep NAME...]] --add FILE... [--replace] [--drop NAME...] [--output FILE]
                               write SHA256SUMS (sorted, `sha256sum --text` format) over the added files plus the
                               existing lines. With --keep only those existing lines are carried over. Existing lines
                               never change (unless --replace lets an added file change its own line) or vanish
                               (unless --drop names a file that leaves the release).
  check-assets --mode full --produced FILE... [--release-json FILE] [--updater]
                               every required platform with a build job produced each kind of file it has, and every
                               asset on the release must be produced by this run, except an optional platform's asset
                               this run did not produce; print the names to keep. A platform's update files (see
                               below) go together: with --updater every platform that produced files has all of them
  check-assets --mode late --platform P --produced FILE... --release-json FILE --sums FILE [--replace] [--tag TAG]
                               [--updater]
                               the files may be added to the release: only platform P's, P optional, built for TAG,
                               all existing assets listed in the release's SHA256SUMS. Prints what the job must do:
                               `pending NAME` (on the release already, from an attach that stopped half way),
                               `overwrite NAME` and `remove NAME` (with --replace: every file of P is replaced)
  check-manifests --tag TAG --repo OWNER/NAME FILE...
                               every latest-*.json among the files is of TAG, and its urls and signatures are those
                               of files among them: the payloads and the .sig files next to the manifests
  check-updater-keys [--summary FILE]
                               check-release's look at the update key: the variable TAURI_UPDATER_PUBKEY and the
                               secrets TAURI_SIGNING_PRIVATE_KEY and TAURI_SIGNING_PRIVATE_KEY_PASSWORD, read from
                               the environment (never printed). The variable without the private key is an error (the
                               password is optional), the secrets without the variable a warning, neither a notice
  check-source-run --run-json FILE [--compare-json FILE] --platform P --repo OWNER/NAME --branch B
                               the earlier run whose files are attached: a successful dispatched build workflow of
                               P on branch B of this repository, its commit part of the release
  select-artifacts --needs FILE --artifacts FILE [--github-output FILE]
                               from the run's artifacts (lines "<id> <name>") pick the `release-<platform>-<file>`
                               ones whose platform job succeeded and print `ids=<id>,<id>` for download-artifact; the
                               rest contributes nothing
  check-release --tag TAG --mode full|late [--platform P] [--replace] [--release-json FILE]
                               the early check, before anything is built or downloaded
  gate --needs FILE|- [--release] [--summary FILE]
                               read `toJSON(needs)` of the publish job: required platforms must have succeeded,
                               optional ones only warn

The update files are what the app's updater needs once an update key exists (publish-release.yml signs the payload of
each platform and writes its manifest): for each platform that updates itself, the `.sig` of its payload and a manifest
`latest-<target>-<arch>.json` (macOS: the `.app.tar.gz` update bundle too, which is no download of its own). They belong
to their platform like its other files (late attach, the files kept from an earlier attach) but are not in the rule
that a required platform produces each of its patterns: a release made before there was a key has none of them.

--release-json is the output of `gh release view TAG --json isDraft,assets`, --run-json that of
`gh api repos/OWNER/NAME/actions/runs/ID` and --compare-json that of `gh api repos/OWNER/NAME/compare/SHA...REF`.
Problems are printed as `::error::` annotations and exit 1.
"""

import argparse
import base64
import binascii
import dataclasses
import fnmatch
import hashlib
import json
import os
import re
import sys
from urllib.parse import unquote

SUMS_NAME = "SHA256SUMS"
TAG = re.compile(r"^v\d+\.\d+\.\d+$")
# `sha256sum --text` output. Strict on purpose: a line that is not exactly this can't be kept byte for byte.
SUMS_LINE = re.compile(r"^([0-9a-f]{64})  ([^\\\r\n/]+)$")
HASH = re.compile(r"^[0-9a-f]{64}$")


@dataclasses.dataclass(frozen=True)
class Platform:
    name: str
    label: str
    required: bool  # a required platform must be built and succeed before a release is published
    patterns: tuple  # file name patterns (fnmatch, case-sensitive) of the release assets
    build: bool = False  # True once publish-release.yml has a job named `name` that builds it
    workflow: str = ""  # the build workflow file whose runs may be attached late
    # The update files, which only a release with an update key has: they belong to the platform but are not among
    # `patterns`, which a required platform must all produce. The payload of Windows and Linux (the installer, the
    # AppImage) is in `patterns` already, so these are what is new: the .sig and manifest names and, for macOS, the
    # update bundle.
    update_patterns: tuple = ()

    @property
    def all_patterns(self):
        return self.patterns + self.update_patterns


# A step that adds a platform flips `build` and adds a job of the same name to publish-release.yml (and to the
# `needs` of its publish job); `gate` fails when the two are out of step.
PLATFORMS = (
    Platform(
        "windows", "Windows", True, ("*-setup.exe",), build=True, workflow="build-windows.yml",
        update_patterns=("*-setup.exe.sig", "latest-windows-x86_64.json"),
    ),
    Platform(
        "linux", "Linux", True, ("*.deb", "*.rpm", "*.AppImage"), build=True, workflow="build-linux.yml",
        # Only the AppImage updates itself; the .deb and .rpm only announce a new version.
        update_patterns=("*.AppImage.sig", "latest-linux-x86_64.json"),
    ),
    # Flathub updates the Flatpak, so it never has update files.
    Platform("flatpak", "Flatpak", True, ("*.flatpak",), build=True, workflow="build-flatpak.yml"),
    Platform(
        "macos", "macOS", False, ("*.dmg",), build=True, workflow="build-macos.yml",
        update_patterns=("*.app.tar.gz", "*.app.tar.gz.sig", "latest-darwin-aarch64.json"),
    ),
)  # fmt: skip


class ReleaseError(Exception):
    """One or more problems that stop a release step. `errors` holds one message each."""

    def __init__(self, errors):
        self.errors = [errors] if isinstance(errors, str) else list(errors)
        super().__init__("; ".join(self.errors))


@dataclasses.dataclass
class GateResult:
    errors: list
    warnings: list
    missing_optional: list

    @property
    def ok(self):
        return not self.errors


@dataclasses.dataclass
class LatePlan:
    """What a late attach must do on top of uploading the new files and SHA256SUMS."""

    pending: list  # on the release but not in SHA256SUMS, as new files: an attach that stopped half way
    overwrite: list  # listed and on the release, replaced by a new file of the same name (replace)
    remove: list  # of the platform, on the release, not among the new files: deleted (replace)


def matches(pattern, name):
    return fnmatch.fnmatchcase(name, pattern)


def owner(path, platforms=PLATFORMS):
    """The Platform a file belongs to (by file name), or None."""
    name = os.path.basename(path)
    for platform in platforms:
        if any(matches(pattern, name) for pattern in platform.all_patterns):
            return platform
    return None


def is_update_file(path, platforms=PLATFORMS):
    """True for a file that only exists for the updater of its platform (a .sig, a manifest, the macOS bundle): it is
    no download for people."""
    name = os.path.basename(path)
    return any(matches(pattern, name) for platform in platforms for pattern in platform.update_patterns)


def is_manifest(path):
    """True for the name of an update manifest, latest-<target>-<arch>.json."""
    return matches("latest-*.json", os.path.basename(path))


def is_required(path, platforms=PLATFORMS):
    platform = owner(path, platforms)
    return platform is not None and platform.required


def classify(path, platforms=PLATFORMS):
    """The name of the platform a file belongs to, or None."""
    platform = owner(path, platforms)
    return platform.name if platform else None


def classify_files(paths, platforms=PLATFORMS):
    """{platform: [file name, ...]} for the files; fails naming every file that belongs to no platform."""
    grouped, unknown = {}, []
    for path in paths:
        platform = classify(path, platforms)
        if platform is None:
            unknown.append(os.path.basename(path))
        else:
            grouped.setdefault(platform, []).append(os.path.basename(path))
    if unknown:
        raise ReleaseError(f"no platform has files like {name}" for name in unknown)
    return grouped


def find_platform(name, platforms=PLATFORMS):
    for platform in platforms:
        if platform.name == name:
            return platform
    raise ReleaseError(f"unknown platform {name!r}; known: {', '.join(p.name for p in platforms)}")


def optional_platform(name, platforms=PLATFORMS):
    """The platform called `name`, which must be optional: only those can be attached after a release is out."""
    platform = find_platform(name, platforms)
    if platform.required:
        raise ReleaseError(
            f"{platform.label} is a required platform: its files are published with the release, not attached later"
        )
    return platform


def parse_sums(text):
    """{file name: its line without the newline}. Fails on a line that is not exactly `<sha256>  <name>`."""
    lines = text.split("\n")
    if lines and lines[-1] == "":
        lines.pop()
    parsed, errors = {}, []
    for number, raw in enumerate(lines, 1):
        found = SUMS_LINE.match(raw)
        if not found:
            errors.append(f"{SUMS_NAME} line {number} is not '<sha256>  <name>' in `sha256sum --text` format: {raw!r}")
        elif found.group(2) in parsed:
            errors.append(f"{SUMS_NAME} lists {found.group(2)} twice")
        else:
            parsed[found.group(2)] = raw
    if errors:
        raise ReleaseError(errors)
    return parsed


def check_preserved(old, new, replaceable=frozenset(), removable=frozenset()):
    """Fails when a line of `old` is missing from `new` or differs, unless the name is in `replaceable` (which may
    differ, but not vanish) or in `removable` (which may vanish). Both are {name: line}, as parse_sums returns them."""
    errors = []
    for name, old_line in old.items():
        if name not in new:
            if name not in removable:
                errors.append(f"the {SUMS_NAME} line for {name} would disappear")
        elif new[name] != old_line and name not in replaceable:
            errors.append(f"the {SUMS_NAME} line for {name} would change; lines already published never change")
    if errors:
        raise ReleaseError(errors)


def build_sums(files, existing=None, keep=None, replace=False, drop=()):
    """The text of a SHA256SUMS over `files` ({name: sha256}) and the lines of `existing` (a SHA256SUMS text).

    keep: None carries every existing line over; a list carries only those names (each must have a line and must not
    be among the files). replace: an added file may change its own existing line. drop: files that leave the release;
    their lines are removed (a name without a line is fine, an earlier run may have removed it already)."""
    for name, digest in files.items():
        if not HASH.match(digest):
            raise ReleaseError(f"{name}: {digest!r} is not a lowercase sha256")
    old = parse_sums(existing) if existing else {}
    if keep is not None:
        errors = [f"cannot keep {name}: {SUMS_NAME} has no line for it" for name in keep if name not in old]
        errors += [f"cannot keep {name}: this run produced it" for name in keep if name in files]
        if errors:
            raise ReleaseError(errors)
        old = {name: old[name] for name in keep}
    if any(name in files for name in drop):
        raise ReleaseError(f"cannot both add and drop {', '.join(name for name in drop if name in files)}")
    new = {name: line for name, line in old.items() if name not in drop}
    for name, digest in files.items():
        new[name] = f"{digest}  {name}"
    check_preserved(old, new, replaceable=set(files) if replace else frozenset(), removable=set(drop))
    return "".join(new[name] + "\n" for name in sorted(new))


def _names(paths):
    return [os.path.basename(path) for path in paths]


def check_versions(files, tag):
    """Fails for a file whose name does not carry the version of `tag` (v0.2.0 -> 0.2.0, between _ or - on both
    sides: Repeat-That_0.2.0_aarch64.dmg, Repeat-That-0.2.0-1.x86_64.rpm). The update manifests are exempt: the app
    asks for `latest-<target>-<arch>.json`, so the name can't say which release it is of (check_manifests reads it)."""
    if not TAG.match(tag):
        raise ReleaseError(f"tag must look like v0.1.0, got {tag!r}")
    version = re.escape(tag[1:])
    carries = re.compile(rf"(?<=[_-]){version}(?=[_-])")
    bad = [name for name in _names(files) if not is_manifest(name) and not carries.search(name)]
    if bad:
        raise ReleaseError(f"{name} does not carry version {tag[1:]}, so it was not built for {tag}" for name in bad)


def check_updater_files(produced, platforms=PLATFORMS, required=False):
    """The update files of a platform go together: a .sig for its payload and its manifest (macOS: the update bundle
    too), and a payload for each .sig. A platform that produced any of them must have all of them; with `required`
    (the release has an update key) every platform that produced files must. Returns the errors."""
    names = _names(produced)
    errors, wanted = [], []
    for platform in platforms:
        if not platform.update_patterns:
            continue
        ours = [name for name in names if owner(name, platforms) is platform]
        has_update = any(matches(pattern, name) for name in ours for pattern in platform.update_patterns)
        if ours and (required or has_update):
            wanted.append(platform)
            errors += [
                f"{platform.label} produced no {pattern} file"
                for pattern in platform.update_patterns
                if not any(matches(pattern, name) for name in names)
            ]
    for platform in wanted:
        for sig_pattern in platform.update_patterns:
            payload_pattern = sig_pattern.removesuffix(".sig")
            if payload_pattern == sig_pattern or payload_pattern not in platform.all_patterns:
                continue
            for name in names:
                if matches(payload_pattern, name) and f"{name}.sig" not in names:
                    errors.append(f"{name} has no signature: {name}.sig is missing")
                elif matches(sig_pattern, name) and name.removesuffix(".sig") not in names:
                    errors.append(f"{name} is the signature of {name.removesuffix('.sig')}, which is missing")
    return errors


def check_assets_full(release_assets, produced, platforms=PLATFORMS, updater=False):
    """Full mode: the assets already on the release against the files this run produced.

    Every required platform with a build job must have produced a file for each of its patterns. Every asset must be
    one of the produced files (it gets replaced), except an optional platform's asset when this run produced nothing
    for that platform (a .dmg attached late earlier, and its update files): that one is kept. Returns the kept names.
    Anything else would sit on the release without a line in the new SHA256SUMS. With `updater` (the release has an
    update key) every platform that produced files must have produced its update files too; without it they must
    still be all there or none (check_updater_files).
    """
    produced = _names(produced)
    errors = []
    try:
        produced_platforms = set(classify_files(produced, platforms))
    except ReleaseError as error:
        errors += error.errors
        produced_platforms = {classify(name, platforms) for name in produced} - {None}
    for platform in platforms:
        if not (platform.build and platform.required):
            continue
        if platform.name not in produced_platforms:
            errors.append(f"{platform.label} produced no files")
        else:
            # A required platform ships every kind of file it has (Linux: the .deb, the .rpm and the AppImage).
            errors += [
                f"{platform.label} produced no {pattern} file"
                for pattern in platform.patterns
                if not any(matches(pattern, name) for name in produced)
            ]
    errors += check_updater_files(produced, platforms, required=updater)
    kept = []
    for name in release_assets:
        if name == SUMS_NAME or name in produced:
            continue
        platform = owner(name, platforms)
        if platform and not platform.required and platform.name not in produced_platforms:
            kept.append(name)
        else:
            errors.append(
                f"the release has {name}, which this run did not produce, so {SUMS_NAME} would not list it; "
                "delete it (gh release delete-asset <tag> <name>) and run again"
            )
    if errors:
        raise ReleaseError(errors)
    return kept


def check_assets_late(
    release_assets, produced, platform, sums_text, replace=False, tag=None, platforms=PLATFORMS, updater=False
):
    """Late mode: may these files be added to the release? Only an optional platform's own files, built for `tag`,
    onto a release whose assets are all listed in its current SHA256SUMS (which the new one will extend). With
    `updater` the platform's update files must be among them.

    Returns a LatePlan. A file of the platform that is on the release but not in SHA256SUMS, with the name of a new
    file, is an attach that stopped between the upload of the file and that of SHA256SUMS: it is `pending`, and the
    job finishes it. With `replace` every file of the platform on the release is replaced, whatever its name, so the
    release never carries two."""
    target = optional_platform(platform, platforms)
    produced = _names(produced)
    errors, plan = [], LatePlan([], [], [])
    if not produced:
        errors.append(f"no {target.label} files to attach")
    errors += [
        f"{name} does not belong to {target.label}; late attach adds one platform's files only"
        for name in produced
        if classify(name, platforms) != target.name
    ]
    if tag:
        try:
            check_versions(produced, tag)
        except ReleaseError as error:
            errors += error.errors
    errors += check_updater_files(
        [name for name in produced if classify(name, platforms) == target.name], platforms, required=updater
    )
    if SUMS_NAME not in release_assets:
        errors.append(f"the release has no {SUMS_NAME}; publish it with a full run first")
    else:
        listed = parse_sums(sums_text)
        present = [name for name in release_assets if name != SUMS_NAME]
        attached = []
        for name in present:
            ours = classify(name, platforms) == target.name
            if name in listed:
                if ours:
                    attached.append(name)
                    if replace:
                        (plan.overwrite if name in produced else plan.remove).append(name)
            elif ours and name in produced:
                plan.pending.append(name)
            elif ours and replace:
                plan.remove.append(name)
            elif ours:
                errors.append(
                    f"the release has {name}, which {SUMS_NAME} does not list; "
                    "run again with 'replace' ticked to remove it"
                )
            else:
                errors.append(f"the release has {name}, which {SUMS_NAME} does not list")
        errors += [f"{SUMS_NAME} lists {name}, which is not on the release" for name in listed if name not in present]
        if attached and not replace:
            errors.append(
                f"the release already has {', '.join(attached)}; run again with 'replace' ticked to replace "
                f"{'it' if len(attached) == 1 else 'them'} (the line for it in {SUMS_NAME} changes)"
            )
    if errors:
        raise ReleaseError(errors)
    return plan


def check_source_run(run, compare, platform, repo, branch, platforms=PLATFORMS):
    """The earlier run whose artifacts a late attach uses (`gh api repos/R/actions/runs/ID`, and the comparison
    `gh api repos/R/compare/<its commit>...<tag>`). It must be the platform's build workflow, started by hand
    (pull request builds are not notarized), successful, on `branch` of this repository (not a fork), and its
    commit must be the tag's commit or one of its ancestors."""
    target = optional_platform(platform, platforms)
    errors = []
    expected = f".github/workflows/{target.workflow}"
    if run.get("path") != expected:
        errors.append(f"the run is of {run.get('path')!r}, not of {expected}")
    if run.get("event") != "workflow_dispatch":
        errors.append(f"the run was started by {run.get('event')!r}, not by hand (workflow_dispatch)")
    if run.get("status") != "completed" or run.get("conclusion") != "success":
        errors.append(f"the run did not succeed (status {run.get('status')}, conclusion {run.get('conclusion')})")
    if run.get("head_branch") != branch:
        errors.append(f"the run is of branch {run.get('head_branch')!r}, not {branch!r}")
    if (run.get("head_repository") or {}).get("full_name") != repo:
        errors.append(f"the run comes from {(run.get('head_repository') or {}).get('full_name')!r}, not from {repo}")
    state = (compare or {}).get("status")
    if state not in ("identical", "ahead"):
        errors.append(f"the run's commit {run.get('head_sha')} is not part of the release's history (compare: {state})")
    if errors:
        raise ReleaseError(errors)


def artifact_platform(name, platforms=PLATFORMS):
    """The platform of an artifact called release-<platform>-<file>, or None."""
    for platform in platforms:
        prefix = f"release-{platform.name}-"
        if name.startswith(prefix) and len(name) > len(prefix):
            return platform
    return None


def select_artifacts(names, needs, platforms=PLATFORMS):
    """Splits artifact names `release-<platform>-<file>` into those whose platform job succeeded and those that
    contribute nothing, as ([selected], [(ignored, reason)]). `needs` is `toJSON(needs)`. An optional platform that
    failed after uploading must not leak partial files into a release."""
    selected, ignored, errors = [], [], []
    for name in names:
        platform = artifact_platform(name, platforms)
        entry = needs.get(platform.name) if platform else None
        if platform is None:
            errors.append(f"artifact {name} is not named release-<platform>-<file> for a known platform")
        elif not platform.build:
            errors.append(f"artifact {name} is of {platform.label}, which has build=False in release_assets.py")
        elif entry is None:
            errors.append(f"artifact {name} is of {platform.label}, but there is no job '{platform.name}' in needs")
        elif entry.get("result") == "success":
            selected.append(name)
        else:
            result = entry.get("result")
            ignored.append((name, f"{platform.label} build did not succeed ({result}); its files are not used"))
    if errors:
        raise ReleaseError(errors)
    return selected, ignored


def check_release(tag, assets, draft, mode, platform=None, replace=False, platforms=PLATFORMS):
    """The early check. Returns the names of published assets that `replace` will replace (nothing for a draft)."""
    if not TAG.match(tag):
        raise ReleaseError(f"tag must look like v0.1.0, got {tag!r}")
    if mode == "full":
        errors = []
        for name in assets:
            kind = owner(name, platforms)
            # A required platform without a build job in the workflow can't produce its files; an optional one's
            # file may have been attached late.
            if name != SUMS_NAME and not (kind and (kind.build or not kind.required)):
                errors.append(
                    f"{tag} has {name}, which no build of this workflow produces, so {SUMS_NAME} would not list it; "
                    "delete it (gh release delete-asset <tag> <name>) and run again"
                )
        protected = [name for name in assets if name == SUMS_NAME or is_required(name, platforms)]
        if not draft and protected and not replace:
            errors.append(
                f"{tag} is published and already has {', '.join(protected)}. Replacing them changes files people may "
                "have downloaded and verified. Run again with 'replace' ticked if you really mean it"
            )
        if errors:
            raise ReleaseError(errors)
        return [] if draft else protected
    if mode == "late":
        # Whether a file of the platform is attached, or an attach that stopped half way, needs SHA256SUMS to tell:
        # check_assets_late decides. Here only the files `replace` will replace.
        target = optional_platform(platform or "", platforms)
        existing = [name for name in assets if classify(name, platforms) == target.name]
        return existing if replace else []
    raise ReleaseError(f"mode must be full or late, got {mode!r}")


def gate(needs, release, platforms=PLATFORMS):
    """Judge the jobs the publish job waited for. `needs` is `toJSON(needs)`: {job id: {"result": ...}}.

    Required platforms must have succeeded; an optional one that did not only warns. `release` is False for dry runs,
    where `check-release` is skipped."""
    errors, warnings, missing = [], [], []
    check = needs.get("check-release", {}).get("result")
    if check != "success" and not (check == "skipped" and not release):
        errors.append(f"the release check did not succeed ({check or 'missing'})")
    for platform in platforms:
        entry = needs.get(platform.name)
        if platform.build and entry is None:
            errors.append(
                f"{platform.label} has build=True in release_assets.py but no job '{platform.name}' in the publish "
                "job's needs"
            )
        elif not platform.build and entry is not None:
            errors.append(
                f"the publish job needs a job '{platform.name}', but {platform.label} has build=False in "
                "release_assets.py"
            )
        elif entry is not None and entry.get("result") != "success":
            result = entry.get("result")
            if platform.required:
                errors.append(f"{platform.label} build did not succeed ({result}); nothing is published")
            else:
                warnings.append(
                    f"{platform.label} not attached (its build {result}); run the late attach, see docs/MAINTAINING.md"
                )
                missing.append(platform.name)
    return GateResult(errors, warnings, missing)


def check_manifests(files, tag, repo):
    """The update manifests among `files` ({file name: text, or None for a file that is not read or could not be})
    must be of the release `tag` of `repo`: `version` is the tag's, every url is a file of that release that is among
    the files, and every signature is the content of that file's .sig. publish-release.yml writes them with
    updater_manifest.py right after signing; this is the second look, at what is about to be attached."""
    if not TAG.match(tag):
        raise ReleaseError(f"tag must look like v0.1.0, got {tag!r}")
    prefix = f"https://github.com/{repo}/releases/download/{tag}/"
    manifests = sorted(name for name in files if is_manifest(name))
    if not manifests:
        raise ReleaseError("there is no update manifest (latest-<target>-<arch>.json) among the files")
    errors = []
    for name in manifests:
        try:
            data = json.loads(files[name])
        except (TypeError, ValueError):
            errors.append(f"{name} is not JSON")
            continue
        if not isinstance(data, dict):
            errors.append(f"{name} is not a JSON object")
            continue
        if data.get("version") != tag[1:]:
            errors.append(f"{name} has version {data.get('version')}, but the release is {tag} ({tag[1:]})")
        platforms = data.get("platforms")
        if not isinstance(platforms, dict) or not platforms:
            errors.append(f"{name} has no platforms")
            continue
        for key, entry in platforms.items():
            url = entry.get("url") if isinstance(entry, dict) else None
            signature = entry.get("signature") if isinstance(entry, dict) else None
            if not isinstance(url, str) or not isinstance(signature, str):
                errors.append(f"{name}: {key} has no url and signature")
                continue
            payload = unquote(url[len(prefix) :]) if url.startswith(prefix) else None
            if payload not in files:
                errors.append(f"{name}: the url of {key} ({url}) is not a file of {tag} on {repo} among the files")
                continue
            sig_name = f"{payload}.sig"
            if sig_name not in files:
                errors.append(f"{name}: {payload} has no {sig_name} among the files")
            elif files[sig_name] is None:
                errors.append(f"{sig_name} could not be read")
            elif signature != files[sig_name].strip():
                errors.append(f"{name}: the signature of {key} is not the content of {sig_name}")
    if errors:
        raise ReleaseError(errors)


@dataclasses.dataclass
class KeyCheck:
    errors: list
    warnings: list
    notices: list

    @property
    def ok(self):
        return not self.errors


def is_key_file(text, kind):
    """True when `text` is the base64 of a minisign key file as `tauri signer generate` writes it: the content of the
    .pub file or of the private key file, whose first line is `untrusted comment: ...` and names the kind of key."""
    try:
        decoded = base64.b64decode(text.strip(), validate=True).decode("utf-8")
    except (binascii.Error, ValueError):
        return False
    first = decoded.partition("\n")[0]
    return first.startswith("untrusted comment:") and kind in first


def check_updater_keys(pubkey, private_key, password):
    """check-release's look at the update key. `pubkey` is the repository variable TAURI_UPDATER_PUBKEY, `private_key`
    and `password` the secrets of the updater-signing Environment; each is the empty string when it is not set.
    The variable without the private key is an error (the apps of the release would have an updater that its own
    release can't feed), the secrets without the variable a warning, and neither a notice. The password is optional:
    GitHub can't store an empty secret, so a key without a password has no secret for it. The variable counts as set
    when it is not the empty string, like everywhere in the workflows (`!= ''`), so one of blanks is an error here:
    the builds would be checked for an updater that the app, which trims the key to nothing, does not have."""
    private_key, password = private_key.strip(), password.strip()
    guide = "See Update signing key in docs/MAINTAINING.md."
    secrets = (("TAURI_SIGNING_PRIVATE_KEY", private_key), ("TAURI_SIGNING_PRIVATE_KEY_PASSWORD", password))
    errors, warnings, notices = [], [], []
    if pubkey:
        if not private_key:
            errors.append(
                "The repository variable TAURI_UPDATER_PUBKEY is set, so the builds have an updater, but the "
                f"updater-signing Environment has no secret TAURI_SIGNING_PRIVATE_KEY to sign its updates with. {guide}"
            )
        if not is_key_file(pubkey, "public key"):
            errors.append(
                "TAURI_UPDATER_PUBKEY is not the content of the .pub file that `tauri signer generate` wrote "
                f"(one line of base64, not the path of the file). {guide}"
            )
        if private_key and not is_key_file(private_key, "secret key"):
            errors.append(
                "TAURI_SIGNING_PRIVATE_KEY is not the content of the private key file that `tauri signer generate` "
                f"wrote (one line of base64, not the path of the file). {guide}"
            )
    else:
        present = [name for name, value in secrets if value]
        if present:
            warnings.append(
                f"The updater-signing Environment has {' and '.join(present)}, but the repository variable "
                "TAURI_UPDATER_PUBKEY is not set, so the builds have no updater and this release has no update "
                f"files. Set the variable last, once the secrets are in. {guide}"
            )
        else:
            notices.append(
                "No update key is set up (the repository variable TAURI_UPDATER_PUBKEY and the secrets of the "
                f"updater-signing Environment), so this release has no update files. {guide}"
            )
    return KeyCheck(errors, warnings, notices)


def hash_file(path):
    digest = hashlib.sha256()
    with open(path, "rb") as stream:
        for chunk in iter(lambda: stream.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def load_release(path):
    """(is draft, asset names) from `gh release view --json isDraft,assets`; an empty release without a path."""
    if not path:
        return True, []
    with open(path, encoding="utf-8") as stream:
        info = json.load(stream)
    return bool(info.get("isDraft")), [asset["name"] for asset in info.get("assets", [])]


def read_text(path):
    with open(path, encoding="utf-8") as stream:
        return stream.read()


def command_classify(args):
    classify_files(args.files)
    for path in args.files:
        print(classify(path), os.path.basename(path))


def read_json(path):
    with open(path, encoding="utf-8") as stream:
        return json.load(stream)


def command_sums(args):
    files = {}
    for path in args.add:
        name = os.path.basename(path)
        if name in files:
            raise ReleaseError(f"{name} is added twice")
        files[name] = hash_file(path)
    existing = read_text(args.existing) if args.existing else None
    text = build_sums(files, existing=existing, keep=args.keep, replace=args.replace, drop=args.drop)
    if args.output:
        with open(args.output, "w", encoding="utf-8", newline="\n") as stream:
            stream.write(text)
    else:
        sys.stdout.write(text)


def command_check_assets(args):
    _, assets = load_release(args.release_json)
    if args.mode == "full":
        for name in check_assets_full(assets, args.produced, updater=args.updater):
            print(name)
        return
    if not (args.platform and args.sums):
        raise ReleaseError("late mode needs --platform and --sums")
    plan = check_assets_late(
        assets, args.produced, args.platform, read_text(args.sums), replace=args.replace, tag=args.tag,
        updater=args.updater,
    )  # fmt: skip
    for kind in ("pending", "overwrite", "remove"):
        for name in getattr(plan, kind):
            print(kind, name)


def command_check_manifests(args):
    files = {}
    for path in args.files:
        name = os.path.basename(path)
        files[name] = None
        if is_manifest(name) or name.endswith(".sig"):  # the payloads are big and not read
            try:
                files[name] = read_text(path)
            except (OSError, ValueError):
                pass
    check_manifests(files, args.tag, args.repo)


def command_check_updater_keys(args):
    result = check_updater_keys(
        os.environ.get("TAURI_UPDATER_PUBKEY", ""),
        os.environ.get("TAURI_SIGNING_PRIVATE_KEY", ""),
        os.environ.get("TAURI_SIGNING_PRIVATE_KEY_PASSWORD", ""),
    )
    for notice in result.notices:
        print(f"::notice::{notice}")
    for warning in result.warnings:
        print(f"::warning::{warning}")
    if args.summary and (result.notices or result.warnings or result.errors):
        with open(args.summary, "a", encoding="utf-8") as stream:
            for label, messages in (("Notice", result.notices), ("Warning", result.warnings), ("Error", result.errors)):
                for message in messages:
                    stream.write(f"> **{label}:** {message}\n\n")
    if not result.ok:
        raise ReleaseError(result.errors)  # main prints them as ::error::


def command_check_release(args):
    draft, assets = load_release(args.release_json)
    for name in check_release(args.tag, assets, draft, args.mode, platform=args.platform, replace=args.replace):
        print(f"will replace {name}")


def command_check_source_run(args):
    compare = read_json(args.compare_json) if args.compare_json else None
    check_source_run(read_json(args.run_json), compare, args.platform, args.repo, args.branch)


def command_select_artifacts(args):
    needs = read_json(args.needs)
    ids = {}
    for line in read_text(args.artifacts).splitlines():
        artifact_id, _, name = line.partition(" ")
        if name.startswith("release-"):  # the run may hold other artifacts
            ids[name] = artifact_id
    selected, ignored = select_artifacts(sorted(ids), needs)
    for artifact, reason in ignored:
        print(f"::warning::ignoring {artifact}: {reason}")
    if not selected:
        raise ReleaseError("no release artifact of a platform that succeeded")
    line = "ids=" + ",".join(ids[name] for name in selected)
    if args.github_output:
        with open(args.github_output, "a", encoding="utf-8") as stream:
            stream.write(line + "\n")
    else:
        print(line)


def command_gate(args):
    needs = json.load(sys.stdin if args.needs == "-" else open(args.needs, encoding="utf-8"))
    result = gate(needs, release=args.release)
    for warning in result.warnings:
        print(f"::warning::{warning}")
    for error in result.errors:
        print(f"::error::{error}", file=sys.stderr)
    if args.summary and (result.warnings or result.errors):
        with open(args.summary, "a", encoding="utf-8") as stream:
            for warning in result.warnings:
                stream.write(f"> **Warning:** {warning}\n\n")
            for error in result.errors:
                stream.write(f"> **Error:** {error}\n\n")
    if not result.ok:
        raise ReleaseError(result.errors)


def build_parser():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    commands = parser.add_subparsers(dest="command", required=True)

    sub = commands.add_parser("classify")
    sub.add_argument("files", nargs="+")
    sub.set_defaults(run=command_classify)

    sub = commands.add_parser("sums")
    sub.add_argument("--existing")
    sub.add_argument("--keep", nargs="*", default=None)
    sub.add_argument("--add", nargs="+", required=True)
    sub.add_argument("--replace", action="store_true")
    sub.add_argument("--drop", nargs="*", default=())
    sub.add_argument("--output")
    sub.set_defaults(run=command_sums)

    sub = commands.add_parser("check-assets")
    sub.add_argument("--mode", choices=["full", "late"], required=True)
    sub.add_argument("--release-json")
    sub.add_argument("--produced", nargs="*", default=[])
    sub.add_argument("--platform")
    sub.add_argument("--sums")
    sub.add_argument("--tag")
    sub.add_argument("--replace", action="store_true")
    sub.add_argument("--updater", action="store_true")
    sub.set_defaults(run=command_check_assets)

    sub = commands.add_parser("check-manifests")
    sub.add_argument("--tag", required=True)
    sub.add_argument("--repo", required=True)
    sub.add_argument("files", nargs="+")
    sub.set_defaults(run=command_check_manifests)

    sub = commands.add_parser("check-updater-keys")
    sub.add_argument("--summary")
    sub.set_defaults(run=command_check_updater_keys)

    sub = commands.add_parser("check-release")
    sub.add_argument("--tag", required=True)
    sub.add_argument("--mode", choices=["full", "late"], required=True)
    sub.add_argument("--platform")
    sub.add_argument("--release-json")
    sub.add_argument("--replace", action="store_true")
    sub.set_defaults(run=command_check_release)

    sub = commands.add_parser("check-source-run")
    sub.add_argument("--run-json", required=True)
    sub.add_argument("--compare-json")
    sub.add_argument("--platform", required=True)
    sub.add_argument("--repo", required=True)
    sub.add_argument("--branch", required=True)
    sub.set_defaults(run=command_check_source_run)

    sub = commands.add_parser("select-artifacts")
    sub.add_argument("--needs", required=True)
    sub.add_argument("--artifacts", required=True)
    sub.add_argument("--github-output")
    sub.set_defaults(run=command_select_artifacts)

    sub = commands.add_parser("gate")
    sub.add_argument("--needs", required=True)
    sub.add_argument("--release", action="store_true")
    sub.add_argument("--summary")
    sub.set_defaults(run=command_gate)
    return parser


def main(argv=None):
    args = build_parser().parse_args(argv)
    try:
        args.run(args)
    except ReleaseError as error:
        for message in error.errors:
            # The gate printed its own annotations already.
            if args.command != "gate":
                print(f"::error::{message}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
