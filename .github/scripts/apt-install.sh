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
# is). On a hosted Ubuntu runner it first moves apt off azure.archive.ubuntu.com, the slow mirror behind those hangs
# (see use_other_mirrors below). Exits 0 on success, 1 when every attempt failed, 2 on a usage error. Set by environment
# (the defaults suit a hosted runner, the tests shrink them): APT_UPDATE_TIMEOUT (60 seconds for one `apt-get update`),
# APT_INSTALL_TIMEOUT (180 seconds for one `apt-get install`), APT_ATTEMPTS (3), APT_RETRY_PAUSE (10 seconds),
# APT_LOCK_POLL (5 seconds between two tries while another apt holds a lock) and APT_ETC_DIR (/etc/apt).
#
# The timeouts and retries are the backstop: with the mirrors switched, a hang should be rare. One hang that a second
# attempt gets past costs about 4 minutes, two about 8. When every attempt fails the worst case is about 17 minutes.
# Give the step `timeout-minutes: 25`, the last stop for anything this script can't see.
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
apt_options=(-o Acquire::Retries=3 -o Acquire::http::Timeout=30 -o Acquire::https::Timeout=30
  -o DPkg::Lock::Timeout=120)

# What apt prints (grep -E patterns): a lock another process holds, an error that asking again won't change, and an
# update that lost a package index.
lock_pattern='Could not get lock|Unable to acquire the dpkg frontend lock'
permanent_pattern="Unable to locate package|has no installation candidate|^E: Couldn.t find any package"
permanent_pattern+="|^E: Version .* was not found"
incomplete_pattern='^[WE]: (Failed to fetch|Some index files failed|Hash Sum mismatch)'

sudo=()
if [[ $(id -u) -ne 0 ]]; then sudo=(sudo); fi

# limited SECONDS COMMAND...: the command as root, stopped after SECONDS and killed for good $kill_after seconds later.
# `timeout` stops the command's whole process group, so a dpkg or a download helper under apt-get doesn't outlive it.
# (`env`, because sudo drops DEBIAN_FRONTEND.)
kill_after=${APT_KILL_AFTER:-10}
limited() {
  local seconds=$1
  shift
  "${sudo[@]}" env DEBIAN_FRONTEND=noninteractive timeout --kill-after="$kill_after" "$seconds" "$@"
}

failure=""
# True when the failure is one a new attempt cannot fix, see apt_call.
permanent=false
# True when this attempt's `apt-get update` lost an index (a mirror half-way through a sync, say).
update_incomplete=false
log=$(mktemp "${TMPDIR:-/tmp}/apt-install.XXXXXX")
trap 'rm -f "$log"' EXIT

# apt_call NAME SECONDS ARG...: `apt-get ARG...` with the shared options, for at most SECONDS in all. Another apt that
# holds a lock (the runner's own updater, say) makes apt-get refuse at once, and DPkg::Lock::Timeout covers only dpkg's
# lock, not the one `apt-get update` takes on the package lists. So a refusal because of a lock is not a failed attempt:
# the call is made again every few seconds until the lock is free or SECONDS are up. The output goes to the step's log
# and to $log. On failure, `failure` says what went wrong, with the last error line apt printed, and `permanent` is
# true for a package that doesn't exist or can't be installed: asking again gets the same answer, so there is no retry.
# Not after an update that lost an index, though: then the package may only be missing from the lists.
#
# `timeout` exits 124 when it stopped the command, and 137 when the command ignored that and had to be killed. But 137
# is also what a command killed by the system (the out-of-memory killer) leaves, and that one has not been running for
# the whole time it was given: a kill after a timeout comes later than that. So 137 counts as a timeout only then.
apt_call() {
  local name=$1 seconds=$2 deadline remaining started microseconds status detail
  shift 2
  deadline=$((SECONDS + seconds))
  while :; do
    remaining=$((deadline - SECONDS))
    if ((remaining < 1)); then
      failure="$name could not get the apt lock within ${seconds}s"
      return 1
    fi
    started=${EPOCHREALTIME/[.,]/}
    set +e
    limited "$remaining" apt-get "${apt_options[@]}" "${quiet[@]}" "$@" 2>&1 | tee "$log"
    status=${PIPESTATUS[0]}
    set -e
    microseconds=$((${EPOCHREALTIME/[.,]/} - started))
    if ((status == 0)); then
      return 0
    fi
    if ((status == 124 || (status == 137 && microseconds >= remaining * 1000000))); then
      failure="$name timed out after ${seconds}s"
      return 1
    fi
    if grep -qE "$lock_pattern" "$log"; then
      sleep "$lock_poll"
      continue
    fi
    if ((status == 137)); then
      failure="$name was killed (exit status 137, probably out of memory)"
    else
      failure="$name exited with status $status"
    fi
    detail=$(grep -E '^E: ' "$log" | tail -n 1 || true)
    if [[ -n $detail ]]; then failure="$failure ($detail)"; fi
    if [[ $update_incomplete == false ]] && grep -qE "$permanent_pattern" "$log"; then
      permanent=true
    fi
    return 1
  done
}

# GitHub-hosted Ubuntu runners get their packages from azure.archive.ubuntu.com, which has been painfully slow: the
# updates and installs that sat for half an hour were all waiting on it. Their sources name a list of mirrors
# (mirror+file:/etc/apt/apt-mirrors.txt, azure first, or on older images the azure address itself), and apt only tries
# the next mirror when one fails, not when one is slow. So before the first `apt-get update` the sources are pointed
# at a list of our own: archive.ubuntu.com first, then two others, which apt moves on to when one of them fails.
# Sources that don't name the azure mirror (the ubuntu:24.04 container of build-linux.yml, other distributions) are
# left as they are. Does nothing when run a second time.
mirrors=(
  "http://archive.ubuntu.com/ubuntu/"
  "https://mirrors.edge.kernel.org/ubuntu/"
  "https://mirror.us.leaseweb.net/ubuntu/"
)
etc=${APT_ETC_DIR:-/etc/apt}
mirror_list=$etc/ci-apt-mirrors.txt

use_other_mirrors() {
  local azure='azure\.archive\.ubuntu\.com' image_list='mirror\+file:[^[:space:]]*/apt-mirrors\.txt'
  local listed=false file files=() n
  if grep -qs "$azure" "$etc/apt-mirrors.txt"; then listed=true; fi
  for file in "$etc/sources.list" "$etc"/sources.list.d/*.list "$etc"/sources.list.d/*.sources; do
    [[ -f $file ]] || continue
    if grep -qsE "$azure" "$file" || { [[ $listed == true ]] && grep -qsE "$image_list" "$file"; }; then
      files+=("$file")
    fi
  done
  if ((${#files[@]} == 0)); then
    return 0
  fi
  echo "Replacing azure.archive.ubuntu.com, the slow default mirror of the hosted runners, in ${files[*]}"
  for n in "${!mirrors[@]}"; do
    printf '%s\tpriority:%d\n' "${mirrors[n]}" $((n + 1))
  done | "${sudo[@]}" tee "$mirror_list" > /dev/null || return 1
  "${sudo[@]}" sed -i -E "s#(https?://$azure/ubuntu/?|$image_list)#mirror+file:$mirror_list#g" "${files[@]}" || return 1
}

installing=false

# One attempt. On failure, `failure` says what went wrong, and `installing` whether it got as far as the install.
attempt_install() {
  installing=false
  permanent=false
  update_incomplete=false
  apt_call "apt-get update" "$update_timeout" update || return 1
  if grep -qE "$incomplete_pattern" "$log"; then update_incomplete=true; fi
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

if ! use_other_mirrors; then
  echo "::warning::$step: could not switch the apt mirrors; going on with the ones the system has"
fi

for ((attempt = 1; attempt <= attempts; attempt++)); do
  if ((attempt > 1)); then
    echo "::warning::$step: $failure; trying again (attempt $attempt of $attempts)"
    sleep "$pause"
    if [[ $installing == true ]]; then repair; fi
  fi
  if attempt_install "$@"; then
    exit 0
  fi
  if [[ $permanent == true ]]; then
    echo "::error::$step: $failure; not retried, because it would fail the same way"
    exit 1
  fi
done

echo "::error::$step: $failure; gave up after $attempts attempts"
exit 1
