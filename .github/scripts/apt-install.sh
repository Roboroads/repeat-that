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
# APT_INSTALL_TIMEOUT (180 seconds for one `apt-get install`), APT_ATTEMPTS (3), APT_RETRY_PAUSE (10 seconds) and
# APT_LOCK_POLL (5 seconds between two tries while another apt holds a lock).
#
# One hang that a second attempt gets past costs about 4 minutes, two about 8. When every attempt fails the worst case is
# about 17 minutes. Give the step `timeout-minutes: 25`, the last stop for anything this script can't see.
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
lock_poll=${APT_LOCK_POLL:-5}

# Network stalls give up after 30 seconds and a download is tried three times before apt fails; a held dpkg lock is
# waited for instead of refused (the lock on the package lists is handled in apt_call).
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
log=$(mktemp "${TMPDIR:-/tmp}/apt-install.XXXXXX")
trap 'rm -f "$log"' EXIT

# apt_call NAME SECONDS ARG...: `apt-get ARG...` with the shared options, for at most SECONDS in all. Another apt that
# holds a lock (the runner's own updater, say) makes apt-get refuse at once, and DPkg::Lock::Timeout covers only dpkg's
# lock, not the one `apt-get update` takes on the package lists. So a refusal because of a lock is not a failed attempt:
# the call is made again every few seconds until the lock is free or SECONDS are up. The output goes to the step's log
# and to $log. On failure, `failure` says what went wrong.
apt_call() {
  local name=$1 seconds=$2 deadline remaining status
  shift 2
  deadline=$((SECONDS + seconds))
  while :; do
    remaining=$((deadline - SECONDS))
    if ((remaining < 1)); then
      failure="$name could not get the apt lock within ${seconds}s"
      return 1
    fi
    set +e
    limited "$remaining" apt-get "${apt_options[@]}" "${quiet[@]}" "$@" 2>&1 | tee "$log"
    status=${PIPESTATUS[0]}
    set -e
    if ((status == 0)); then
      return 0
    fi
    if ((status != 124 && status != 137)) && grep -qE 'Could not get lock|Unable to acquire the dpkg frontend lock' "$log"; then
      sleep "$lock_poll"
      continue
    fi
    failure=$(describe "$name" "$status" "$seconds")
    return 1
  done
}

installing=false

# One attempt. On failure, `failure` says what went wrong, and `installing` whether it got as far as the install.
attempt_install() {
  installing=false
  apt_call "apt-get update" "$update_timeout" update || return 1
  installing=true
  apt_call "apt-get install" "$install_timeout" install -y --no-install-recommends "$@"
}

# What an install that failed or was stopped half-way can leave behind: a package dpkg had unpacked but not configured,
# and dependencies apt had not installed yet. apt refuses to start again until both are dealt with. Neither fixes
# everything, so a failure of either is a warning in the log and the retry goes on.
repair() {
  local status=0 failed=$failure
  limited 60 dpkg --configure -a || status=$?
  if ((status != 0)); then
    echo "::warning::$step: dpkg --configure -a exited with status $status; going on"
  fi
  apt_call "apt-get -f install" 60 -f install -y || echo "::warning::$step: $failure; going on"
  failure=$failed
}

for ((attempt = 1; attempt <= attempts; attempt++)); do
  if ((attempt > 1)); then
    echo "::warning::$step: $failure; trying again (attempt $attempt of $attempts)"
    sleep "$pause"
    if [[ $installing == true ]]; then repair; fi
  fi
  if attempt_install "$@"; then
    exit 0
  fi
done

echo "::error::$step: $failure; gave up after $attempts attempts"
exit 1
