"""Tests that each called workflow gets its secrets. Run with: python3 -m unittest discover -s .github/scripts

A called workflow only sees the secrets its caller passes, Environment secrets included. build-macos.yml sets the
apple-signing Environment on its job, yet without `secrets: inherit` in the calling job every secret is an empty string,
with no error from GitHub (the macOS build of Publish release on main failed that way). Secrets also go only one hop:
for release-please.yml -> publish-release.yml -> build-macos.yml, each call must pass them on.

The workflow files are read as plain text, so the tests need nothing beyond the standard library. That is enough for the
shape they all have: jobs two spaces in under `jobs:`, their keys four spaces in.
"""

import re
import unittest
from pathlib import Path

WORKFLOWS = Path(__file__).resolve().parent.parent / "workflows"

JOB = re.compile(r"^  ([A-Za-z0-9_-]+):\s*$")
USES = re.compile(r"^    uses:\s*\./\.github/workflows/([^\s#]+)")
INHERIT = re.compile(r"^    secrets:\s*inherit\s*(#.*)?$")
# secrets.GITHUB_TOKEN is there without being passed.
SECRET = re.compile(r"\bsecrets\.(?!GITHUB_TOKEN\b)([A-Za-z_][A-Za-z0-9_]*)")


def calls(text):
    """Each job that calls a workflow of this repository, as (job id, called file name, passes `secrets: inherit`)."""
    found = []
    in_jobs = False
    job = None
    for line in text.splitlines():
        if line.strip().startswith("#") or not line.strip():
            continue
        if not line.startswith(" "):
            in_jobs = line.rstrip() == "jobs:"
            job = None
            continue
        if not in_jobs:
            continue
        if match := JOB.match(line):
            job = [match[1], None, False]
            found.append(job)
        elif job and (match := USES.match(line)):
            job[1] = match[1]
        elif job and INHERIT.match(line):
            job[2] = True
    return [tuple(job) for job in found if job[1]]


def secrets_read(text):
    """The secrets the workflow reads itself, not counting comments."""
    lines = (line for line in text.splitlines() if not line.strip().startswith("#"))
    return {name for line in lines for name in SECRET.findall(line)}


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

    def test_keys_outside_jobs_are_not_jobs(self):
        text = "on:\n  workflow_call:\n    uses: ./.github/workflows/build.yml\njobs:\n  a:\n    runs-on: x\n"
        self.assertEqual(calls(text), [])


if __name__ == "__main__":
    unittest.main()
