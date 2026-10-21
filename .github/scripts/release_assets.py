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
  check-assets --mode full --produced FILE... [--release-json FILE]
                               every asset on the release must be produced by this run, except an optional platform's
                               asset this run did not produce; print the names to keep
  check-assets --mode late --platform P --produced FILE... --release-json FILE --sums FILE [--replace] [--tag TAG]
                               the files may be added to the release: only platform P's, P optional, built for TAG,
                               all existing assets listed in the release's SHA256SUMS. Prints what the job must do:
                               `pending NAME` (on the release already, from an attach that stopped half way),
                               `overwrite NAME` and `remove NAME` (with --replace: every file of P is replaced)
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

--release-json is the output of `gh release view TAG --json isDraft,assets`, --run-json that of
`gh api repos/OWNER/NAME/actions/runs/ID` and --compare-json that of `gh api repos/OWNER/NAME/compare/SHA...REF`.
Problems are printed as `::error::` annotations and exit 1.
"""

import argparse
import dataclasses
import fnmatch
import hashlib
import json
import os
import re
import sys

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


# A step that adds a platform flips `build` and adds a job of the same name to publish-release.yml (and to the
# `needs` of its publish job); `gate` fails when the two are out of step.
PLATFORMS = (
    Platform("windows", "Windows", True, ("*-setup.exe",), build=True, workflow="build-windows.yml"),
    Platform("linux", "Linux", True, ("*.deb", "*.rpm", "*.AppImage"), workflow="build-linux.yml"),
    Platform("flatpak", "Flatpak", True, ("*.flatpak",), workflow="build-flatpak.yml"),
    Platform("macos", "macOS", False, ("*.dmg",), workflow="build-macos.yml"),
)


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
        if any(matches(pattern, name) for pattern in platform.patterns):
            return platform
    return None


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
    sides: Repeat-That_0.2.0_universal.dmg, Repeat-That-0.2.0-1.x86_64.rpm)."""
    if not TAG.match(tag):
        raise ReleaseError(f"tag must look like v0.1.0, got {tag!r}")
    version = re.escape(tag[1:])
    carries = re.compile(rf"(?<=[_-]){version}(?=[_-])")
    bad = [name for name in _names(files) if not carries.search(name)]
    if bad:
        raise ReleaseError(f"{name} does not carry version {tag[1:]}, so it was not built for {tag}" for name in bad)


def check_assets_full(release_assets, produced, platforms=PLATFORMS):
    """Full mode: the assets already on the release against the files this run produced.

    Every asset must be one of the produced files (it gets replaced), except an optional platform's asset when this
    run produced nothing for that platform (a .dmg attached late earlier): that one is kept. Returns the kept names.
    Anything else would sit on the release without a line in the new SHA256SUMS."""
    produced = _names(produced)
    errors = []
    try:
        produced_platforms = set(classify_files(produced, platforms))
    except ReleaseError as error:
        errors += error.errors
        produced_platforms = {classify(name, platforms) for name in produced} - {None}
    for platform in platforms:
        if platform.build and platform.required and platform.name not in produced_platforms:
            errors.append(f"{platform.label} produced no files")
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


def check_assets_late(release_assets, produced, platform, sums_text, replace=False, tag=None, platforms=PLATFORMS):
    """Late mode: may these files be added to the release? Only an optional platform's own files, built for `tag`,
    onto a release whose assets are all listed in its current SHA256SUMS (which the new one will extend).

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
        for name in check_assets_full(assets, args.produced):
            print(name)
        return
    if not (args.platform and args.sums):
        raise ReleaseError("late mode needs --platform and --sums")
    plan = check_assets_late(
        assets, args.produced, args.platform, read_text(args.sums), replace=args.replace, tag=args.tag
    )
    for kind in ("pending", "overwrite", "remove"):
        for name in getattr(plan, kind):
            print(kind, name)


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
    sub.set_defaults(run=command_check_assets)

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
