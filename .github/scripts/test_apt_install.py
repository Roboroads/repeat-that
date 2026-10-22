"""Tests for apt-install.sh. Run with: python3 -m unittest discover -s .github/scripts

The script runs against fake `apt-get`, `dpkg` and `sudo` commands on a PATH of its own, so nothing is installed and the
network is never touched. A fake apt-get acts out a plan per subcommand (`update`, `install`, and `-f install` as
`fix`), one word per call, the last one repeating: `ok`, `fail`, `lock` (another apt holds the lock), `permanent` (a
package that doesn't exist), `warn` (a warning, exit 0), `hang`, `stubborn` (a hang that ignores SIGTERM, so it needs
the KILL after the grace period) or `killed` (killed by the system, as the out-of-memory killer does). Timeouts and the
grace period are shrunk to a second so a hung call costs the test no more.
"""

import os
import subprocess
import tempfile
import time
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent / "apt-install.sh"
STEP = "Install system packages (Linux)"
PACKAGES = ["pkg-config", "libgtk-3-dev", "clang"]

FAKE_APT_GET = """#!/usr/bin/env bash
echo "apt-get $*" >> "$FAKE_DIR/calls"
etc=${APT_ETC_DIR:-/nonexistent}
if grep -qs azure.archive.ubuntu.com "$etc/sources.list" "$etc"/sources.list.d/*; then
  echo "saw-azure" >> "$FAKE_DIR/calls"
fi
sub=
for arg in "$@"; do
  case $arg in update | install) sub=$arg ;; -f) fix=1 ;; esac
done
# `apt-get -f install` is the repair of a half-finished install, with a plan of its own.
if [ -n "${fix:-}" ]; then sub=fix; fi
plan=FAKE_PLAN_${sub^^}
read -ra steps <<< "${!plan:-ok}"
count=$(cat "$FAKE_DIR/count.$sub" 2>/dev/null || echo 0)
echo $((count + 1)) > "$FAKE_DIR/count.$sub"
step=${steps[$((count < ${#steps[@]} ? count : ${#steps[@]} - 1))]}
case $step in
  ok) exit 0 ;;
  fail) echo "E: Failed to fetch http://mirror.invalid/pool/x.deb" >&2; exit 100 ;;
  lock) echo "E: Could not get lock /var/lib/apt/lists/lock. It is held by process 42 (apt-get)" >&2; exit 100 ;;
  permanent) echo "E: Unable to locate package no-such-package" >&2; exit 100 ;;
  warn) echo "W: Some index files failed to download. They have been ignored, or old ones used instead." >&2; exit 0 ;;
  hang) sleep 60; exit 0 ;;
  stubborn) trap '' TERM; sleep 60; exit 0 ;;
  killed) kill -KILL $$ ;;
esac
"""
FAKE_DPKG = '#!/usr/bin/env bash\necho "dpkg $*" >> "$FAKE_DIR/calls"\nexit "${FAKE_DPKG_STATUS:-0}"\n'
FAKE_SUDO = """#!/usr/bin/env bash
echo "sudo $*" >> "$FAKE_DIR/calls"
if [ "$1" = "${FAKE_SUDO_FAIL:-}" ]; then exit 1; fi
exec "$@"
"""


class AptCase(unittest.TestCase):
    """The fake commands, the script run against them, and a look at what they were asked to do."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.dir = Path(self._tmp.name)
        bindir = self.dir / "bin"
        bindir.mkdir()
        for name, body in (("apt-get", FAKE_APT_GET), ("dpkg", FAKE_DPKG), ("sudo", FAKE_SUDO)):
            (bindir / name).write_text(body)
            (bindir / name).chmod(0o755)
        self.bindir = bindir
        # The script rewrites the apt sources in here, never in the real /etc/apt.
        self.etc = self.dir / "etc-apt"
        (self.etc / "sources.list.d").mkdir(parents=True)

    def run_script(self, *args, update="ok", install="ok", fix="ok", **env):
        """Runs the script; returns the finished process and how long it took."""
        environment = {
            **os.environ,
            "PATH": f"{self.bindir}{os.pathsep}{os.environ['PATH']}",
            "FAKE_DIR": str(self.dir),
            "FAKE_PLAN_UPDATE": update,
            "FAKE_PLAN_INSTALL": install,
            "FAKE_PLAN_FIX": fix,
            "APT_ETC_DIR": str(self.etc),
            "APT_UPDATE_TIMEOUT": "1",
            "APT_INSTALL_TIMEOUT": "1",
            "APT_RETRY_PAUSE": "0",
            "APT_LOCK_POLL": "0",
            "APT_KILL_AFTER": "1",
            **env,
        }
        start = time.monotonic()
        done = subprocess.run(
            ["bash", str(SCRIPT), *args], env=environment, capture_output=True, text=True, timeout=60, check=False
        )
        return done, time.monotonic() - start

    def logged(self):
        log = self.dir / "calls"
        return log.read_text().splitlines() if log.exists() else []

    def calls(self, prefix):
        """The calls of one command; for apt-get, the updates and installs, not the repairs."""
        lines = [line for line in self.logged() if line.startswith(prefix + " ")]
        return [line for line in lines if " -f install" not in line]

    def sequence(self):
        """What ran, in order: update, install, repair (apt-get -f install) and dpkg. Not the sudo lines."""
        names = []
        for line in self.logged():
            if line.startswith("dpkg "):
                names.append("dpkg")
            elif line.startswith("apt-get "):
                if " -f install" in line:
                    names.append("repair")
                else:
                    names.append("update" if line.endswith(" update") else "install")
        return names


class AptInstallTest(AptCase):
    def test_installs_the_packages_once_when_apt_works(self):
        done, _ = self.run_script("--step", STEP, *PACKAGES)
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertEqual(len(self.calls("apt-get")), 2)
        update, install = self.calls("apt-get")
        self.assertTrue(update.endswith(" update"), update)
        self.assertTrue(install.endswith(" install -y --no-install-recommends " + " ".join(PACKAGES)), install)
        self.assertNotIn("::warning::", done.stdout)
        self.assertEqual(self.calls("dpkg"), [])

    def test_every_apt_call_retries_downloads_and_waits_for_the_dpkg_lock(self):
        self.run_script("--step", STEP, *PACKAGES)
        for call in self.calls("apt-get"):
            self.assertIn("-o Acquire::Retries=3", call)
            self.assertIn("-o DPkg::Lock::Timeout=", call)

    def test_quiet_makes_both_calls_quiet(self):
        self.run_script("--quiet", "--step", STEP, "minisign")
        update, install = self.calls("apt-get")
        self.assertIn(" -qq ", update)
        self.assertIn(" -qq ", install)
        self.assertTrue(install.endswith(" install -y --no-install-recommends minisign"), install)

    def test_without_quiet_neither_call_is_quiet(self):
        self.run_script("--step", STEP, *PACKAGES)
        for call in self.calls("apt-get"):
            self.assertNotIn("-qq", call)

    def test_a_failed_download_is_retried(self):
        done, _ = self.run_script("--step", STEP, *PACKAGES, install="fail ok")
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertEqual(len(self.calls("apt-get")), 4, "update and install, twice")
        self.assertIn(f"::warning::{STEP}", done.stdout)
        self.assertIn("attempt 2 of 3", done.stdout)
        self.assertNotIn("::error::", done.stdout)

    def test_a_held_lock_is_waited_for_inside_the_call_not_counted_as_a_failed_attempt(self):
        done, _ = self.run_script("--step", STEP, *PACKAGES, update="lock lock ok")
        self.assertEqual(done.returncode, 0, done.stderr)
        calls = self.calls("apt-get")
        self.assertEqual([c.endswith(" update") for c in calls], [True, True, True, False])
        self.assertNotIn("::warning::", done.stdout)
        self.assertEqual(self.calls("dpkg"), [])

    def test_a_lock_held_during_the_install_is_waited_for_too(self):
        done, _ = self.run_script("--step", STEP, *PACKAGES, install="lock ok")
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertEqual(len(self.calls("apt-get")), 3, "update, install (lock), install")
        self.assertNotIn("::warning::", done.stdout)

    def test_a_lock_that_stays_held_fails_the_call_when_its_time_is_up(self):
        done, took = self.run_script("--step", STEP, *PACKAGES, update="lock", APT_ATTEMPTS="1", APT_LOCK_POLL="1")
        self.assertEqual(done.returncode, 1)
        self.assertGreaterEqual(took, 1)
        self.assertLess(took, 20)
        self.assertIn("::error::", done.stdout)
        self.assertIn("apt-get update could not get the apt lock within 1s", done.stdout)

    def test_a_retry_after_a_failed_install_repairs_what_it_left_behind_first(self):
        done, _ = self.run_script("--step", STEP, *PACKAGES, install="fail ok")
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertEqual(self.sequence(), ["update", "install", "dpkg", "repair", "update", "install"])
        self.assertIn("--configure -a", self.calls("dpkg")[0])
        repair = [line for line in self.logged() if line.startswith("apt-get ") and " -f install" in line]
        self.assertEqual(len(repair), 1)
        self.assertIn("-y", repair[0].split())

    def test_a_retry_after_a_failed_update_has_nothing_to_repair(self):
        done, _ = self.run_script("--step", STEP, *PACKAGES, update="fail ok")
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertEqual(self.sequence(), ["update", "update", "install"])

    def test_a_repair_that_fails_is_reported_and_the_retry_goes_on(self):
        done, _ = self.run_script("--step", STEP, *PACKAGES, install="fail ok", fix="fail", FAKE_DPKG_STATUS="1")
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertIn(f"::warning::{STEP}: dpkg --configure -a exited with status 1", done.stdout)
        self.assertIn(f"::warning::{STEP}: apt-get -f install exited with status 100", done.stdout)
        self.assertEqual(self.sequence(), ["update", "install", "dpkg", "repair", "update", "install"])

    def test_a_hung_update_is_killed_and_retried(self):
        done, took = self.run_script("--step", STEP, *PACKAGES, update="hang ok")
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertLess(took, 30, "the hung apt-get was waited for instead of killed")
        self.assertIn("apt-get update timed out after 1s", done.stdout)
        self.assertEqual(len(self.calls("apt-get")), 3, "update (hung), update, install")

    def test_a_hung_install_is_killed_and_retried(self):
        done, took = self.run_script("--step", STEP, *PACKAGES, install="hang hang ok")
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertLess(took, 30)
        self.assertIn("apt-get install timed out after 1s", done.stdout)
        self.assertEqual(len(self.calls("apt-get")), 6, "update and install, three times")

    def test_gives_up_after_three_attempts_and_names_the_step(self):
        done, _ = self.run_script("--step", STEP, *PACKAGES, install="fail")
        self.assertEqual(done.returncode, 1)
        self.assertEqual(len(self.calls("apt-get")), 6, "update and install, three times")
        errors = [line for line in done.stdout.splitlines() if line.startswith("::error::")]
        self.assertEqual(len(errors), 1, done.stdout)
        self.assertIn(STEP, errors[0])
        self.assertIn("apt-get install exited with status 100", errors[0])
        self.assertIn("3 attempts", errors[0])

    def test_a_package_that_does_not_exist_fails_at_once_with_apts_own_error(self):
        done, _ = self.run_script("--step", STEP, "no-such-package", install="permanent")
        self.assertEqual(done.returncode, 1)
        self.assertEqual(self.sequence(), ["update", "install"], "no retry, no repair")
        self.assertNotIn("::warning::", done.stdout)
        errors = [line for line in done.stdout.splitlines() if line.startswith("::error::")]
        self.assertEqual(len(errors), 1, done.stdout)
        self.assertIn(STEP, errors[0])
        self.assertIn("E: Unable to locate package no-such-package", errors[0])
        self.assertIn("not retried", errors[0])
        self.assertNotIn("attempts", errors[0])

    def test_a_missing_package_after_an_update_that_lost_an_index_is_retried(self):
        # The lists may just lack the index the package is in, which a new update can fix.
        done, _ = self.run_script("--step", STEP, "libgtk-3-dev", update="warn", install="permanent ok")
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertEqual(self.sequence(), ["update", "install", "dpkg", "repair", "update", "install"])
        self.assertIn("attempt 2 of 3", done.stdout)

    def test_a_failure_that_may_pass_names_the_error_apt_gave(self):
        done, _ = self.run_script("--step", STEP, *PACKAGES, install="fail", APT_ATTEMPTS="1")
        errors = [line for line in done.stdout.splitlines() if line.startswith("::error::")]
        self.assertIn("E: Failed to fetch http://mirror.invalid/pool/x.deb", errors[0])

    def test_a_hang_that_ignores_sigterm_is_killed_and_still_reported_as_a_timeout(self):
        done, took = self.run_script("--step", STEP, *PACKAGES, install="stubborn ok")
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertLess(took, 30)
        self.assertIn("apt-get install timed out after 1s", done.stdout)

    def test_an_install_killed_by_the_system_is_not_reported_as_a_timeout(self):
        done, _ = self.run_script("--step", STEP, *PACKAGES, install="killed", APT_ATTEMPTS="1")
        self.assertEqual(done.returncode, 1)
        errors = [line for line in done.stdout.splitlines() if line.startswith("::error::")]
        self.assertNotIn("timed out", errors[0])
        self.assertIn("apt-get install was killed (exit status 137", errors[0])
        self.assertIn("out of memory", errors[0])

    def test_gives_up_on_an_update_that_always_hangs(self):
        done, took = self.run_script("--step", STEP, *PACKAGES, update="hang")
        self.assertEqual(done.returncode, 1)
        self.assertLess(took, 40)
        self.assertEqual(len(self.calls("apt-get")), 3)
        self.assertTrue(all(call.endswith(" update") for call in self.calls("apt-get")))
        self.assertIn("::error::", done.stdout)
        self.assertIn("apt-get update timed out after 1s", done.stdout)

    def test_the_number_of_attempts_can_be_changed(self):
        done, _ = self.run_script("--step", STEP, *PACKAGES, install="fail", APT_ATTEMPTS="2")
        self.assertEqual(done.returncode, 1)
        self.assertEqual(len(self.calls("apt-get")), 4)
        self.assertIn("2 attempts", done.stdout)

    def test_the_step_name_is_optional(self):
        done, _ = self.run_script(*PACKAGES, install="fail", APT_ATTEMPTS="1")
        self.assertEqual(done.returncode, 1)
        self.assertIn("::error::apt install:", done.stdout)

    def test_no_packages_is_a_usage_error_and_runs_nothing(self):
        done, _ = self.run_script("--step", STEP)
        self.assertEqual(done.returncode, 2)
        self.assertIn("usage", done.stderr.lower())
        self.assertEqual(self.calls("apt-get"), [])

    def test_an_unknown_option_is_a_usage_error(self):
        done, _ = self.run_script("--bogus", "clang")
        self.assertEqual(done.returncode, 2)
        self.assertEqual(self.calls("apt-get"), [])

    @unittest.skipIf(os.geteuid() == 0, "root doesn't need sudo")
    def test_runs_apt_through_sudo_when_not_root(self):
        self.run_script("--step", STEP, *PACKAGES)
        self.assertEqual(len(self.calls("sudo")), 2)
        self.assertTrue(all("apt-get" in call for call in self.calls("sudo")))

    @unittest.skipUnless(os.geteuid() == 0, "only root runs apt directly")
    def test_runs_apt_directly_as_root(self):
        self.run_script("--step", STEP, *PACKAGES)
        self.assertEqual(self.calls("sudo"), [])


# What a GitHub-hosted runner has: apt reads its mirrors from a list file, and that file starts with the azure mirror.
# (The path of the list is the etc dir of the test, where a runner has /etc/apt.)
AZURE_MIRRORS = (
    "http://azure.archive.ubuntu.com/ubuntu/\tpriority:1\n"
    "https://archive.ubuntu.com/ubuntu/\tpriority:2\n"
    "https://security.ubuntu.com/ubuntu/\tpriority:3\n"
)
UBUNTU_SOURCES = """Types: deb
URIs: {uri}
Suites: noble noble-updates noble-backports
Components: main universe restricted multiverse
Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg

Types: deb
URIs: {uri}
Suites: noble-security
Components: main universe restricted multiverse
Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg
"""
MICROSOFT_LIST = (
    "deb [arch=amd64 signed-by=/etc/apt/keyrings/microsoft-prod.gpg] "
    "https://packages.microsoft.com/ubuntu/24.04/prod noble main\n"
)


class AptMirrorTest(AptCase):
    """On a hosted runner `apt-get update` is pointed away from azure.archive.ubuntu.com, which has been very slow."""

    def put(self, name, text):
        path = self.etc / name
        path.write_text(text)
        return path

    def hosted_runner_24_04(self):
        """Ubuntu 24.04 as GitHub ships it: deb822 sources that name the mirror list."""
        self.put("apt-mirrors.txt", AZURE_MIRRORS)
        self.put("sources.list.d/ubuntu.sources", UBUNTU_SOURCES.format(uri=f"mirror+file:{self.etc}/apt-mirrors.txt"))
        self.put("sources.list.d/microsoft-prod.list", MICROSOFT_LIST)

    def mirrors(self):
        """The mirror list the script wrote: (url, priority) in the order apt tries them."""
        lines = (self.etc / "ci-apt-mirrors.txt").read_text().splitlines()
        return [(line.split("\t")[0], line.split("\t")[1]) for line in lines]

    def test_the_sources_of_ubuntu_24_04_get_a_mirror_list_without_azure(self):
        self.hosted_runner_24_04()
        done, _ = self.run_script("--step", STEP, *PACKAGES)
        self.assertEqual(done.returncode, 0, done.stderr)
        sources = (self.etc / "sources.list.d/ubuntu.sources").read_text()
        self.assertEqual(sources, UBUNTU_SOURCES.format(uri=f"mirror+file:{self.etc}/ci-apt-mirrors.txt"))
        self.assertNotIn("azure", "".join(path.read_text() for path in self.etc.rglob("ci-*")))
        mirrors = self.mirrors()
        self.assertGreaterEqual(len(mirrors), 3, "apt needs somewhere to fall over to")
        self.assertEqual(mirrors[0], ("http://archive.ubuntu.com/ubuntu/", "priority:1"))
        self.assertEqual([priority for _, priority in mirrors], [f"priority:{n}" for n in range(1, len(mirrors) + 1)])
        for url, _ in mirrors:
            self.assertRegex(url, r"^https?://[^/]+/.*ubuntu/$")

    def test_other_repositories_are_left_alone(self):
        self.hosted_runner_24_04()
        self.run_script("--step", STEP, *PACKAGES)
        self.assertEqual((self.etc / "sources.list.d/microsoft-prod.list").read_text(), MICROSOFT_LIST)
        self.assertEqual((self.etc / "apt-mirrors.txt").read_text(), AZURE_MIRRORS, "the image's list stays as it was")

    def test_the_one_line_sources_of_ubuntu_22_04_are_rewritten_too(self):
        self.put("apt-mirrors.txt", AZURE_MIRRORS)
        self.put("sources.list", f"deb mirror+file:{self.etc}/apt-mirrors.txt jammy main restricted universe\n"
                 f"deb mirror+file:{self.etc}/apt-mirrors.txt jammy-updates main restricted universe\n")
        self.run_script("--step", STEP, *PACKAGES)
        self.assertEqual(
            (self.etc / "sources.list").read_text(),
            f"deb mirror+file:{self.etc}/ci-apt-mirrors.txt jammy main restricted universe\n"
            f"deb mirror+file:{self.etc}/ci-apt-mirrors.txt jammy-updates main restricted universe\n",
        )

    def test_sources_that_name_the_azure_mirror_directly_are_rewritten(self):
        self.put("sources.list", "deb http://azure.archive.ubuntu.com/ubuntu/ jammy main\n"
                 "deb [arch=amd64] http://azure.archive.ubuntu.com/ubuntu jammy-security main\n")
        self.put("sources.list.d/ubuntu.sources", UBUNTU_SOURCES.format(uri="http://azure.archive.ubuntu.com/ubuntu/"))
        self.run_script("--step", STEP, *PACKAGES)
        self.assertEqual(
            (self.etc / "sources.list").read_text(),
            f"deb mirror+file:{self.etc}/ci-apt-mirrors.txt jammy main\n"
            f"deb [arch=amd64] mirror+file:{self.etc}/ci-apt-mirrors.txt jammy-security main\n",
        )
        deb822 = (self.etc / "sources.list.d/ubuntu.sources").read_text()
        self.assertIn(f"URIs: mirror+file:{self.etc}/ci-apt-mirrors.txt\n", deb822)
        self.assertNotIn("azure", deb822)

    def test_apt_update_runs_only_after_the_sources_are_switched(self):
        self.hosted_runner_24_04()
        self.run_script("--step", STEP, *PACKAGES)
        self.assertNotIn("saw-azure", self.logged())
        self.assertEqual(self.sequence(), ["update", "install"])

    def test_the_container_image_with_its_own_mirrors_is_left_alone(self):
        # ubuntu:24.04 as the Linux package build uses it: archive.ubuntu.com and security.ubuntu.com.
        ubuntu = UBUNTU_SOURCES.format(uri="http://archive.ubuntu.com/ubuntu/")
        self.put("sources.list.d/ubuntu.sources", ubuntu)
        done, _ = self.run_script("--step", STEP, *PACKAGES)
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertEqual((self.etc / "sources.list.d/ubuntu.sources").read_text(), ubuntu)
        self.assertFalse((self.etc / "ci-apt-mirrors.txt").exists())
        self.assertEqual([call for call in self.calls("sudo") if "apt-get" not in call], [])

    def test_a_mirror_list_without_azure_is_left_alone(self):
        self.put("apt-mirrors.txt", "https://archive.ubuntu.com/ubuntu/\tpriority:1\n")
        sources = UBUNTU_SOURCES.format(uri=f"mirror+file:{self.etc}/apt-mirrors.txt")
        self.put("sources.list.d/ubuntu.sources", sources)
        self.run_script("--step", STEP, *PACKAGES)
        self.assertEqual((self.etc / "sources.list.d/ubuntu.sources").read_text(), sources)
        self.assertFalse((self.etc / "ci-apt-mirrors.txt").exists())

    def test_the_sources_are_switched_once_not_once_per_attempt(self):
        self.hosted_runner_24_04()
        done, _ = self.run_script("--step", STEP, *PACKAGES, install="fail fail ok")
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertEqual(len([call for call in self.calls("sudo") if " sed " in call]), 1)

    def test_a_second_run_on_switched_sources_changes_nothing(self):
        self.hosted_runner_24_04()
        self.run_script("--step", STEP, *PACKAGES)
        before = {path: path.read_text() for path in self.etc.rglob("*") if path.is_file()}
        (self.dir / "calls").unlink()
        self.run_script("--step", STEP, *PACKAGES)
        self.assertEqual({path: path.read_text() for path in self.etc.rglob("*") if path.is_file()}, before)
        self.assertEqual([call for call in self.calls("sudo") if "apt-get" not in call], [])

    def test_a_switch_that_fails_is_a_warning_and_the_install_goes_on(self):
        self.hosted_runner_24_04()
        done, _ = self.run_script("--step", STEP, *PACKAGES, FAKE_SUDO_FAIL="tee")
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertIn(f"::warning::{STEP}: could not switch the apt mirrors", done.stdout)
        self.assertEqual(self.sequence(), ["update", "install"])

    def test_the_switch_is_announced_in_the_log(self):
        self.hosted_runner_24_04()
        done, _ = self.run_script("--step", STEP, *PACKAGES)
        self.assertIn("azure.archive.ubuntu.com", done.stdout)
        self.assertNotIn("::warning::", done.stdout)


if __name__ == "__main__":
    unittest.main()
