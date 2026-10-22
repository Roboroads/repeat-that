#!/usr/bin/env bash
# Installs apt packages in a CI step without letting a stuck mirror hold the job for half an hour. `apt-get update` and
# `apt-get install` each run under `timeout`, and when one fails or hangs the whole attempt is made again, up to three
# times. A normal run takes well under a minute; before this, three runs sat in their apt step until someone cancelled
# them (or, once, until it finished 34 minutes later), and a rerun passed in seconds.
#
#   .github/scripts/apt-install.sh [--quiet] [--step NAME] PACKAGE...
#
#   --quiet      -qq on both apt-get calls, for steps that only install one small tool
#   --step NAME  the step's name, put in the ::warning:: and ::error:: lines so the annotation says which step it is
#
# Installs with -y and --no-install-recommends, and runs apt-get through sudo unless it already is root (a container job
# is). Exits 0 on success, 1 when every attempt failed, 2 on a usage error. Set by environment (the defaults suit a
# hosted runner, the tests shrink them): APT_UPDATE_TIMEOUT (60 seconds for one `apt-get update`),
# APT_INSTALL_TIMEOUT (180 seconds for one `apt-get install`), APT_ATTEMPTS (3) and APT_RETRY_PAUSE (10 seconds).
#
# One hang that a second attempt gets past costs about 4 minutes, two about 8. When every attempt fails the worst case is
# about 15 minutes. Give the step `timeout-minutes: 25`, the last stop for anything this script can't see.
set -euo pipefail

usage() {
  echo "usage: apt-install.sh [--quiet] [--step NAME] PACKAGE..." >&2
  exit 2
}

quiet=()
step="apt install"
while (($# > 0)); do
  case $1 in
    --quiet) quiet=(-qq) ;;
    --step)
      (($# >= 2)) || usage
      step=$2
      shift
      ;;
    --) shift; break ;;
    -*) usage ;;
    *) break ;;
  esac
  shift
done
(($# > 0)) || usage

update_timeout=${APT_UPDATE_TIMEOUT:-60}
install_timeout=${APT_INSTALL_TIMEOUT:-180}
attempts=${APT_ATTEMPTS:-3}
pause=${APT_RETRY_PAUSE:-10}

# Network stalls give up after 30 seconds and a download is tried three times before apt fails; a held dpkg lock (the
# runner's own updater, a package that was killed half-way) is waited for instead of refused.
apt_options=(-o Acquire::Retries=3 -o Acquire::http::Timeout=30 -o Acquire::https::Timeout=30 -o DPkg::Lock::Timeout=120)

sudo=()
if [[ $(id -u) -ne 0 ]]; then sudo=(sudo); fi

# limited SECONDS COMMAND...: the command as root, stopped after SECONDS and killed for good 10 seconds later. `timeout`
# stops the command's whole process group, so a dpkg or a download helper under apt-get doesn't outlive it. (`env`,
# because sudo drops DEBIAN_FRONTEND.)
limited() {
  local seconds=$1
  shift
  "${sudo[@]}" env DEBIAN_FRONTEND=noninteractive timeout --kill-after=10 "$seconds" "$@"
}

# Why a command failed: `timeout` exits 124 when it stopped the command and 137 when it had to kill it.
describe() {
  local name=$1 status=$2 seconds=$3
  case $status in
    124 | 137) echo "$name timed out after ${seconds}s" ;;
    *) echo "$name exited with status $status" ;;
  esac
}

failure=""

# One attempt. On failure, `failure` says what went wrong.
attempt_install() {
  local status=0
  limited "$update_timeout" apt-get "${apt_options[@]}" "${quiet[@]}" update || status=$?
  if ((status != 0)); then
    failure=$(describe "apt-get update" "$status" "$update_timeout")
    return 1
  fi
  limited "$install_timeout" apt-get "${apt_options[@]}" "${quiet[@]}" install -y --no-install-recommends "$@" || status=$?
  if ((status != 0)); then
    failure=$(describe "apt-get install" "$status" "$install_timeout")
    return 1
  fi
}

for ((attempt = 1; attempt <= attempts; attempt++)); do
  if ((attempt > 1)); then
    echo "::warning::$step: $failure; trying again (attempt $attempt of $attempts)"
    sleep "$pause"
    # A dpkg that was stopped half-way leaves packages half-unpacked, and apt refuses to go on until this has finished
    # them. It does nothing on a clean system.
    limited 60 dpkg --configure -a || true
  fi
  if attempt_install "$@"; then
    exit 0
  fi
done

echo "::error::$step: $failure; gave up after $attempts attempts"
exit 1
