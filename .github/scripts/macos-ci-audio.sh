#!/usr/bin/env bash
# Sets up a GitHub macOS runner for the capture test (crates/rt-capture/tests/macos_audio.rs), and refuses to run
# anywhere else, because it changes the system's audio devices and permissions:
#   * installs the BlackHole 2ch virtual device (GPL-3.0, on the runner only, never shipped) and makes it the
#     output and the input, so what the test plays can be recorded back from its input;
#   * tries to give the runner the audio capture (kTCCServiceAudioCapture) and microphone permissions by adding them to
#     its TCC databases, for the process macOS asks about (the one responsible for the probe) and a few likely others.
# Writes `tcc=granted` or `tcc=missing` to $GITHUB_OUTPUT, and what it found to the step summary.
#
# Usage: macos-ci-audio.sh <probe> [<test binary>]
set -euo pipefail

if [ "${GITHUB_ACTIONS:-}" != "true" ] || [ "${RUNNER_OS:-}" != "macOS" ]; then
  echo "This changes the system's audio devices and permissions, so it only runs on a GitHub macOS runner." >&2
  exit 1
fi
probe=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
test_binary=${2:-}
summary=${GITHUB_STEP_SUMMARY:-/dev/null}

echo "::group::Install BlackHole 2ch"
brew install --cask blackhole-2ch
brew install switchaudio-osx
# Core Audio loads the new driver when it starts again; launchd restarts it.
sudo killall coreaudiod || true
found=false
for _ in $(seq 1 30); do
  if SwitchAudioSource -a -t output 2>/dev/null | grep -q "BlackHole 2ch"; then
    found=true
    break
  fi
  sleep 1
done
if [ "$found" != true ]; then
  echo "::error::BlackHole 2ch didn't show up as an output device"
  SwitchAudioSource -a || true
  exit 1
fi
SwitchAudioSource -t output -s "BlackHole 2ch"
SwitchAudioSource -t input -s "BlackHole 2ch"
SwitchAudioSource -t system -s "BlackHole 2ch"
SwitchAudioSource -a
echo "::endgroup::"

echo "::group::Permissions before"
before=$("$probe" permission 2>&1 || true)
echo "$before"
responsible=$(sed -n 's/^responsible: [0-9]* //p' <<<"$before")
echo "::endgroup::"

# The process macOS asks about is the one responsible for the probe; the others are where GitHub's runner images
# grant their own permissions, in case the responsible one can't be found.
clients=()
[ -n "$responsible" ] && clients+=("$responsible")
clients+=(
  /opt/hca/hosted-compute-agent
  /usr/local/opt/runner/provisioner/provisioner
  /bin/bash
  /bin/sh
  "$probe"
)
[ -n "$test_binary" ] && clients+=("$(cd "$(dirname "$test_binary")" && pwd)/$(basename "$test_binary")")

# The code requirement of the binary at $1 in the binary form TCC stores, as hex; empty if it has none. tccd checks a
# client against it, and may refuse a row without one.
csreq_hex() {
  local requirement blob
  requirement=$(codesign -d -r- "$1" 2>&1 | sed -n 's/^designated => //p' || true)
  [ -n "$requirement" ] || return 0
  blob="${RUNNER_TEMP:-/tmp}/csreq-$$.bin"
  if echo "$requirement" | csreq -r- -b "$blob" 2>/dev/null; then
    xxd -p "$blob" | tr -d '\n'
  fi
  rm -f "$blob"
}

echo "::group::Grant audio capture and microphone"
user_db="$HOME/Library/Application Support/com.apple.TCC/TCC.db"
system_db="/Library/Application Support/com.apple.TCC/TCC.db"
for db in "$user_db" "$system_db"; do
  if ! sudo test -f "$db"; then
    echo "no $db"
    continue
  fi
  for service in kTCCServiceAudioCapture kTCCServiceMicrophone; do
    for client in "${clients[@]}"; do
      csreq=$(csreq_hex "$client")
      csreq_sql=NULL
      [ -n "$csreq" ] && csreq_sql="X'$csreq'"
      # Named columns, so it works with the schema of any recent macOS. client_type 1: a path; auth_value 2: allowed;
      # auth_reason 4: set by the system.
      if ! sudo sqlite3 "$db" "INSERT OR REPLACE INTO access
          (service, client, client_type, auth_value, auth_reason, auth_version, csreq, indirect_object_identifier,
           flags, last_modified)
          VALUES ('$service', '$client', 1, 2, 4, 1, $csreq_sql, 'UNUSED', 0, CAST(strftime('%s','now') AS INTEGER));"
      then
        echo "could not add $service for $client to $db"
        sudo sqlite3 "$db" ".schema access" || true
      fi
    done
  done
  echo "$db:"
  sudo sqlite3 "$db" "SELECT service, client, auth_value, auth_reason, length(csreq) FROM access
    WHERE service IN ('kTCCServiceAudioCapture', 'kTCCServiceMicrophone');" || true
done
# tccd reads the databases again when it starts; launchd restarts it. Right after that it can answer with an error for
# a few seconds (CI saw "Checking for Service Override", which reads as denied), so it gets some time to settle.
sudo killall tccd || true
killall tccd || true
echo "::endgroup::"

echo "::group::Permissions after"
for attempt in $(seq 1 30); do
  sleep 1
  after=$("$probe" permission 2>&1 || true)
  status=$(sed -n 's/^status: //p' <<<"$after")
  [ "$status" = "Allowed" ] && break
done
echo "$after"
echo "after $attempt s"
echo "::endgroup::"

if [ "$status" != "Allowed" ]; then
  # What tccd made of it, for the next attempt.
  echo "::group::tccd's log"
  log show --last 30s --style compact --predicate 'subsystem == "com.apple.TCC"' 2>/dev/null \
    | grep -i -E "AudioCapture|Microphone|hosted-compute|probe|csreq|requirement|refus|denied|auth" | tail -60 || true
  echo "::endgroup::"
fi

if [ "$status" = "Allowed" ]; then
  tcc=granted
else
  tcc=missing
fi
echo "tcc=$tcc" >>"${GITHUB_OUTPUT:-/dev/null}"
{
  echo "### Audio setup"
  echo
  echo "- Output and input: BlackHole 2ch"
  echo "- Responsible process: \`${responsible:-unknown}\`"
  echo "- Audio capture permission before: \`$(sed -n 's/^status: //p' <<<"$before")\`, after the grant: \`$status\`"
  if [ "$tcc" = granted ]; then
    echo "- The grant worked: every capture test runs, with \`RT_REQUIRE_TCC=1\`."
  else
    echo "- **The grant didn't work**: only the test without the permission runs; the others are skipped."
  fi
  echo
} >>"$summary"
echo "tcc=$tcc"
