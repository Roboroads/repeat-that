"""Tests that each called workflow gets its secrets. Run with: python3 -m unittest discover -s .github/scripts

A called workflow only sees the secrets its caller passes, Environment secrets included. build-macos.yml sets the
apple-signing Environment on its job, yet without `secrets: inherit` in the calling job every secret is an empty string,
with no error from GitHub (the macOS build of Publish release on main failed that way). Secrets also go only one hop:
for release-please.yml -> publish-release.yml -> build-macos.yml, each call must pass them on.

The workflow files are read line by line, so the tests need only the standard library, which is all the runner's
python3 is sure to have. The reader knows block-style YAML: keys by their indentation, comments, and block scalars
(`run: |`), whose lines are text, not keys. Flow style (`jobs: {build: {uses: ...}}`) isn't read.
"""

import re
import unittest
from pathlib import Path

WORKFLOWS = Path(__file__).resolve().parent.parent / "workflows"

# secrets.NAME, or secrets['NAME'] / secrets["NAME"].
SECRET = re.compile(r"\bsecrets(?:\.([A-Za-z_][A-Za-z0-9_]*)|\[\s*(['\"])([A-Za-z_][A-Za-z0-9_]*)\2\s*\])")
# A key whose value is a block scalar: `run: |`, `script: >-`, `- run: |2`.
BLOCK = re.compile(r":\s*[|>][-+0-9]*$")
CALLED = "./.github/workflows/"


def cut_comment(line):
    """The line without its comment: a # at the start or after a space, outside quotes."""
    quote = None
    for i, char in enumerate(line):
        if quote:
            if char == quote:
                quote = None
        elif char in "'\"" and (not line[:i].strip() or line[:i].rstrip()[-1] in ":-[{,"):
            quote = char  # only at the start of a value: in `echo "a # b"` the # starts a comment
        elif char == "#" and (i == 0 or line[i - 1] in " \t"):
            return line[:i]
    return line


def unquote(value):
    """A scalar without its quotes: 'a', "a" and a are the same value."""
    if len(value) >= 2 and value[0] == value[-1] and value[0] in "'\"":
        return value[1:-1]
    return value


def lines(text):
    """The lines that hold YAML, as (indent, text, in a block scalar). Comments are cut and empty lines left out, except
    in a block scalar, whose lines are kept whole: a # there is part of the text (a shell comment in `run:`)."""
    block = None  # the indent of the key that opened a block scalar
    for raw in text.splitlines():
        indent = len(raw) - len(raw.lstrip(" "))
        if block is not None:
            if not raw.strip() or indent > block:
                yield indent, raw.strip(), True
                continue
            block = None
        line = cut_comment(raw).strip()
        if not line:
            continue
        yield indent, line, False
        if BLOCK.search(line):
            block = indent


def calls(text):
    """Each job that calls a workflow of this repository, as (job id, called file name, passes `secrets: inherit`)."""
    found = []
    in_jobs = False
    job_indent = key_indent = None
    job = None
    for indent, line, in_block in lines(text):
        if in_block:
            continue
        if indent == 0:
            in_jobs = line == "jobs:"
            job_indent = job = None
            continue
        if not in_jobs:
            continue
        key, _, value = line.partition(":")
        key, value = unquote(key.strip()), unquote(value.strip())
        if job_indent is None:
            job_indent = indent
        if indent == job_indent:
            job = [key, None, False]
            found.append(job)
            key_indent = None
            continue
        if job is None:
            continue
        if key_indent is None:
            key_indent = indent
        if indent != key_indent:
            continue
        if key == "uses" and value.startswith(CALLED):
            job[1] = value.removeprefix(CALLED)
        elif key == "secrets":
            job[2] = value == "inherit"
    return [tuple(job) for job in found if job[1]]


def secrets_read(text):
    """The secrets the workflow reads itself, upper case (names ignore case), not counting comments. A line of a block
    scalar counts even when it is a shell comment: GitHub fills in its expressions before the shell sees it.
    GITHUB_TOKEN doesn't count, it is there without being passed."""
    found = set()
    for _indent, line, _in_block in lines(text):
        for dotted, _quote, indexed in SECRET.findall(line):
            name = (dotted or indexed).upper()
            if name != "GITHUB_TOKEN":
                found.add(name)
    return found


def problems(workflows):
    """Every call that leaves a secret empty. `workflows` maps file names (build-macos.yml) to their text."""
    # A workflow needs secrets when it reads one, or calls a workflow that needs them (which must be passed through).
    needs = {name: secrets_read(text) for name, text in workflows.items()}
    changed = True
    while changed:
        changed = False
        for name, text in workflows.items():
            for _job, called, _inherit in calls(text):
                missing = needs.get(called, set()) - needs[name]
                if missing:
                    needs[name] |= missing
                    changed = True

    found = []
    for name, text in sorted(workflows.items()):
        for job, called, inherit in calls(text):
            if called not in workflows:
                found.append(f"{name}: job {job} calls {called}, which doesn't exist")
            elif needs[called] and not inherit:
                found.append(
                    f"{name}: job {job} calls {called}, which needs {', '.join(sorted(needs[called]))}, without "
                    "`secrets: inherit`; the called workflow would see empty values"
                )
    return found


def read_workflows(folder):
    return {path.name: path.read_text(encoding="utf-8") for path in sorted(folder.glob("*.yml"))}


CALLER = """\
on:
  workflow_dispatch:

jobs:
  first:
    uses: ./.github/workflows/build.yml
    with:
      tag: v0.1.0
{secrets}
  other:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
"""

BUILD = """\
on:
  workflow_call:

jobs:
  build:
    runs-on: macos-latest
    environment: signing
    steps:
      # The token below is secrets.IN_A_COMMENT, which doesn't count.
      - run: ./sign.sh
        env:
          TOKEN: ${{ secrets.GITHUB_TOKEN }}
          KEY: ${{ secrets.SIGNING_KEY }}
"""

NO_SECRETS = """\
on:
  workflow_call:

jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - run: echo ${{ secrets.GITHUB_TOKEN }}
"""


class RepositoryWorkflowsTest(unittest.TestCase):
    def test_every_call_passes_the_secrets_it_needs(self):
        workflows = read_workflows(WORKFLOWS)
        self.assertIn("build-macos.yml", workflows, f"no workflows found in {WORKFLOWS}")
        self.assertEqual(problems(workflows), [])

    def test_the_macos_build_gets_the_signing_secrets(self):
        # The chain a release takes, so a change to the parser can't make the check above pass by finding nothing.
        workflows = read_workflows(WORKFLOWS)
        self.assertIn("APPLE_CERTIFICATE", secrets_read(workflows["build-macos.yml"]))
        self.assertIn(("macos", "build-macos.yml", True), calls(workflows["publish-release.yml"]))
        self.assertIn(("publish-release", "publish-release.yml", True), calls(workflows["release-please.yml"]))


class ProblemsTest(unittest.TestCase):
    def test_call_without_inherit_is_reported(self):
        found = problems({"caller.yml": CALLER.format(secrets=""), "build.yml": BUILD})
        self.assertEqual(len(found), 1)
        self.assertIn("caller.yml: job first calls build.yml, which needs SIGNING_KEY", found[0])

    def test_call_with_inherit_passes(self):
        caller = CALLER.format(secrets="    secrets: inherit")
        self.assertEqual(calls(caller), [("first", "build.yml", True)])
        self.assertEqual(problems({"caller.yml": caller, "build.yml": BUILD}), [])

    def test_inherit_with_a_comment_counts(self):
        caller = CALLER.format(secrets="    secrets: inherit # the signing key")
        self.assertEqual(problems({"caller.yml": caller, "build.yml": BUILD}), [])

    def test_github_token_and_comments_are_not_secrets_to_pass(self):
        self.assertEqual(secrets_read(BUILD), {"SIGNING_KEY"})
        self.assertEqual(problems({"caller.yml": CALLER.format(secrets=""), "build.yml": NO_SECRETS}), [])

    def test_secrets_read(self):
        text = """\
jobs:
  build:
    env:
      A: ${{ secrets['FIRST'] }} # was secrets.IN_A_COMMENT
      B: ${{ secrets[ "SECOND" ] || secrets.third }}
      C: ${{ secrets.github_token }} "# not a comment" ${{ secrets.FOURTH }}
    steps:
      - run: |
          # a shell comment: ${{ secrets.FIFTH }}
          echo "a # b ${{ secrets.SIXTH }}"
      - run: echo "a # b ${{ secrets.IN_A_COMMENT }}"
"""
        self.assertEqual(secrets_read(text), {"FIRST", "SECOND", "THIRD", "FOURTH", "FIFTH", "SIXTH"})

    def test_each_hop_must_pass_them_on(self):
        middle = CALLER.format(secrets="").replace("first:", "middle:")
        top = CALLER.format(secrets="    secrets: inherit").replace("build.yml", "middle.yml")
        found = problems({"top.yml": top, "middle.yml": middle, "build.yml": BUILD})
        self.assertEqual(len(found), 1)
        self.assertIn("middle.yml: job middle calls build.yml", found[0])

        # The middle one reads nothing itself, but needs the secrets to pass on, so its caller must pass them too.
        middle = CALLER.format(secrets="    secrets: inherit").replace("first:", "middle:")
        top = CALLER.format(secrets="").replace("build.yml", "middle.yml")
        found = problems({"top.yml": top, "middle.yml": middle, "build.yml": BUILD})
        self.assertEqual(len(found), 1)
        self.assertIn("top.yml: job first calls middle.yml, which needs SIGNING_KEY", found[0])

    def test_missing_workflow_is_reported(self):
        found = problems({"caller.yml": CALLER.format(secrets="")})
        self.assertEqual(found, ["caller.yml: job first calls build.yml, which doesn't exist"])

    def test_comments_after_keys(self):
        text = """\
jobs: # the jobs
  build:
    runs-on: ubuntu-latest
  macos: # optional
    uses: ./.github/workflows/build.yml # the build
    secrets: inherit # all of them
"""
        self.assertEqual(calls(text), [("macos", "build.yml", True)])

    def test_quoted_values(self):
        text = """\
jobs:
  "macos":
    'uses': './.github/workflows/build.yml'
    secrets: "inherit"
"""
        self.assertEqual(calls(text), [("macos", "build.yml", True)])

    def test_any_indentation(self):
        text = "jobs:\n    macos:\n        uses: ./.github/workflows/build.yml\n        secrets: inherit\n"
        self.assertEqual(calls(text), [("macos", "build.yml", True)])

    def test_block_scalars_are_text(self):
        text = """\
jobs:
  first:
    runs-on: ubuntu-latest
    steps:
      - run: |
          # shell, not YAML
          uses: ./.github/workflows/build.yml
  second:
    uses: ./.github/workflows/other.yml
"""
        self.assertEqual(calls(text), [("second", "other.yml", False)])

    def test_keys_outside_jobs_are_not_jobs(self):
        text = "on:\n  workflow_call:\n    uses: ./.github/workflows/build.yml\njobs:\n  a:\n    runs-on: x\n"
        self.assertEqual(calls(text), [])


if __name__ == "__main__":
    unittest.main()
