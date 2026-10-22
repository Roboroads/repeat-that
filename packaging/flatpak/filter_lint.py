#!/usr/bin/env python3
"""Reads the JSON that `flatpak-builder-lint` prints and fails on the errors that are not expected.

Usage: filter_lint.py LINT.json [--allow CODE ...]

flatpak-builder-lint is Flathub's own check, and what it reports is what a Flathub reviewer will see. A few errors are
expected for as long as the Flatpak is not on Flathub yet, and each is named with --allow where the workflow calls this
(build-flatpak.yml says why); any other error fails the build. Warnings and the info lines are printed. Prints every
error that is not allowed as an `::error::` annotation and exits 1. Unit tests: test_filter_lint.py.
"""

import argparse
import json
import sys
from pathlib import Path


def split(output, allowed):
    """(errors that are not allowed, allowed errors that came up) of a flatpak-builder-lint result."""
    errors = output.get("errors", [])
    return [code for code in errors if code not in allowed], [code for code in errors if code in allowed]


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("lint", type=Path, help="the output of flatpak-builder-lint")
    parser.add_argument("--allow", action="append", default=[], metavar="CODE", help="an error to let pass")
    args = parser.parse_args(argv)
    text = args.lint.read_text(encoding="utf-8")
    try:
        output = json.loads(text)
    except json.JSONDecodeError:
        print(f"::error::flatpak-builder-lint did not print JSON:\n{text}", file=sys.stderr)
        return 1
    errors, allowed = split(output, args.allow)
    for line in output.get("info", []):
        print(f"info: {line}")
    for code in output.get("warnings", []):
        print(f"warning: {code}")
    for code in allowed:
        print(f"allowed: {code}")
    for code in errors:
        print(f"::error::{code}", file=sys.stderr)
    if not errors and not allowed and not output.get("warnings"):
        print("flatpak-builder-lint: clean")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
