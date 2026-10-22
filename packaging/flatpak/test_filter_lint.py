"""Tests for filter_lint.py. Run with: python3 -m unittest discover -s packaging/flatpak -p 'test_*.py'"""

import contextlib
import io
import json
import tempfile
import unittest
from pathlib import Path

import filter_lint as fl

OUTPUT = {
    "errors": ["appid-url-not-reachable", "finish-args-arbitrary-xdg-run-access"],
    "warnings": ["appstream-no-flathub-manifest-key"],
    "info": ["appid-url-not-reachable: Tried https://roboroads.me | Request exception: ConnectionError"],
    "message": "See https://docs.flathub.org/linter for details and exceptions",
}


class SplitTests(unittest.TestCase):
    def test_an_allowed_error_is_set_aside_and_the_rest_stays(self):
        errors, allowed = fl.split(OUTPUT, ["appid-url-not-reachable"])
        self.assertEqual(errors, ["finish-args-arbitrary-xdg-run-access"])
        self.assertEqual(allowed, ["appid-url-not-reachable"])

    def test_nothing_allowed_leaves_every_error(self):
        errors, allowed = fl.split(OUTPUT, [])
        self.assertEqual(errors, OUTPUT["errors"])
        self.assertEqual(allowed, [])

    def test_a_clean_output_has_no_errors(self):
        self.assertEqual(fl.split({}, ["appid-url-not-reachable"]), ([], []))

    def test_an_allowed_code_that_does_not_come_up_is_not_listed(self):
        errors, allowed = fl.split({"errors": ["x"]}, ["y"])
        self.assertEqual((errors, allowed), (["x"], []))


class MainTests(unittest.TestCase):
    def run_main(self, payload, *args):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "lint.json"
            path.write_text(payload if isinstance(payload, str) else json.dumps(payload), encoding="utf-8")
            out, err = io.StringIO(), io.StringIO()
            with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                code = fl.main([str(path), *args])
        return code, out.getvalue(), err.getvalue()

    def test_errors_that_are_not_allowed_fail_with_an_annotation_each(self):
        code, _, err = self.run_main(OUTPUT, "--allow", "appid-url-not-reachable")
        self.assertEqual(code, 1)
        self.assertIn("::error::finish-args-arbitrary-xdg-run-access", err)
        self.assertNotIn("::error::appid-url-not-reachable", err)

    def test_only_allowed_errors_pass_and_are_named(self):
        code, out, _ = self.run_main({"errors": ["appid-url-not-reachable"]}, "--allow", "appid-url-not-reachable")
        self.assertEqual(code, 0)
        self.assertIn("appid-url-not-reachable", out)

    def test_a_clean_result_passes(self):
        code, _, _ = self.run_main({})
        self.assertEqual(code, 0)

    def test_warnings_are_shown_and_do_not_fail(self):
        code, out, _ = self.run_main({"warnings": ["some-warning"]})
        self.assertEqual(code, 0)
        self.assertIn("some-warning", out)

    def test_output_that_is_not_json_fails(self):
        code, _, err = self.run_main("Traceback (most recent call last): boom")
        self.assertEqual(code, 1)
        self.assertIn("::error::", err)

    def test_the_info_lines_are_shown_for_a_failure(self):
        _, out, _ = self.run_main(OUTPUT)
        self.assertIn("Tried https://roboroads.me", out)


if __name__ == "__main__":
    unittest.main()
