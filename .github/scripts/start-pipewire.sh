#!/usr/bin/env bash
# Starts a private, headless PipeWire for the Linux capture tests: a D-Bus session bus, PipeWire and WirePlumber in a
# runtime directory of their own, plus a null sink that players play into. Nothing here touches the audio session of
# the user who runs it: the sockets, WirePlumber's remembered volumes and the config all live in that directory, and the
# hardware monitors (ALSA, Bluetooth, cameras) are off.
#
#   eval "$(.github/scripts/start-pipewire.sh)"        # start, and point this shell at it
#   cargo test -p rt-capture --test linux_audio -- --ignored --nocapture
#   .github/scripts/start-pipewire.sh --stop          # stop the one this shell points at ($RT_PIPEWIRE_DIR)
#
# It prints the `export` lines for the variables clients need (XDG_RUNTIME_DIR, DBUS_SESSION_BUS_ADDRESS) and writes
# them to $GITHUB_ENV too when that is set, so later CI steps use it. The directory to use can be given as the first
# argument; by default it is a new one in $TMPDIR. Logs are in its `log` folder.
#
# Needs pipewire, wireplumber, pipewire-bin (pw-cli, pw-metadata) and dbus-daemon; with pipewire-pulse installed it
# also serves the PulseAudio API, for apps (and `pacat`) that play through it.
set -euo pipefail

stop() {
  local dir=$1
  [ -f "$dir/pids" ] || { echo "no PipeWire started in $dir" >&2; return 1; }
  # Newest first: WirePlumber, then PipeWire, then the bus.
  tac "$dir/pids" | while read -r pid; do kill "$pid" 2>/dev/null || true; done
  rm -f "$dir/pids"
}

if [ "${1:-}" = "--stop" ]; then
  stop "${2:-${RT_PIPEWIRE_DIR:?pass the directory or set RT_PIPEWIRE_DIR}}"
  exit
fi

dir=${1:-$(mktemp -d "${TMPDIR:-/tmp}/rt-pipewire.XXXXXX")}
mkdir -p "$dir"/{run,config,state,log}
chmod 700 "$dir/run"
dir=$(cd "$dir" && pwd)
# Unix socket paths are limited to 108 bytes.
if [ ${#dir} -gt 80 ]; then
  echo "$dir is too long a path for the sockets in it; pass a shorter directory" >&2
  exit 1
fi
[ ! -e "$dir/pids" ] || stop "$dir" 2>/dev/null || true

# The null sink, as a fixed object of the daemon, so it lives as long as PipeWire does.
mkdir -p "$dir/config/pipewire/pipewire.conf.d"
cat > "$dir/config/pipewire/pipewire.conf.d/90-repeat-that-tests.conf" <<'EOF'
context.objects = [
  { factory = adapter
    args = {
      factory.name     = support.null-audio-sink
      node.name        = rt-null-sink
      node.description = "Repeat That test sink"
      media.class      = Audio/Sink
      audio.position   = [ FL FR ]
      monitor.channel-volumes = true
      monitor.passthrough = true
    }
  }
]
EOF

# No hardware: WirePlumber 0.5 reads profiles from wireplumber.conf.d, 0.4 (Ubuntu 24.04) from Lua files.
mkdir -p "$dir/config/wireplumber/wireplumber.conf.d"
cat > "$dir/config/wireplumber/wireplumber.conf.d/90-repeat-that-tests.conf" <<'EOF'
wireplumber.profiles = {
  main = {
    monitor.alsa = disabled
    monitor.alsa-midi = disabled
    monitor.bluez = disabled
    monitor.bluez-midi = disabled
    monitor.v4l2 = disabled
    monitor.libcamera = disabled
    support.logind = disabled
    support.reserve-device = disabled
  }
}
EOF
if wireplumber --version | grep -q 'libwireplumber 0\.4\.'; then
  mkdir -p "$dir/config/wireplumber/main.lua.d" "$dir/config/wireplumber/bluetooth.lua.d"
  cat > "$dir/config/wireplumber/main.lua.d/51-repeat-that-tests.lua" <<'EOF'
alsa_monitor.enabled = false
v4l2_monitor.enabled = false
libcamera_monitor.enabled = false
EOF
  echo 'bluez_monitor.enabled = false' > "$dir/config/wireplumber/bluetooth.lua.d/51-repeat-that-tests.lua"
fi

run="$dir/run"
export XDG_RUNTIME_DIR=$run
export XDG_CONFIG_HOME=$dir/config
export XDG_STATE_HOME=$dir/state
export DBUS_SESSION_BUS_ADDRESS=unix:path=$run/bus
unset PIPEWIRE_REMOTE PIPEWIRE_RUNTIME_DIR PIPEWIRE_CONFIG_DIR

# Each in a session of its own, so it outlives this script (and a CI step) and a Ctrl+C in the caller's shell.
start() {
  local name=$1
  shift
  setsid "$@" > "$dir/log/$name.log" 2>&1 < /dev/null &
  echo $! >> "$dir/pids"
}

wait_until() {
  local what=$1
  shift
  for _ in $(seq 100); do
    if "$@" > /dev/null 2>&1; then return 0; fi
    sleep 0.1
  done
  echo "$what did not come up; logs in $dir/log:" >&2
  tail -n 20 "$dir"/log/*.log >&2 || true
  stop "$dir" || true
  return 1
}

start dbus dbus-daemon --session --nofork --nopidfile --address="$DBUS_SESSION_BUS_ADDRESS"
wait_until "dbus-daemon" test -S "$run/bus"
start pipewire pipewire
wait_until "PipeWire" pw-cli info 0
start wireplumber wireplumber
# WirePlumber is ready once it has picked the null sink as the default output.
wait_until "WirePlumber" sh -c 'pw-metadata 0 default.audio.sink | grep -q rt-null-sink'
# Most apps play through the PulseAudio API, which pipewire-pulse serves, when it is installed.
if command -v pipewire-pulse > /dev/null; then
  start pipewire-pulse pipewire-pulse
  wait_until "pipewire-pulse" test -S "$run/pulse/native"
fi

vars=(
  "RT_PIPEWIRE_DIR=$dir"
  "XDG_RUNTIME_DIR=$run"
  "DBUS_SESSION_BUS_ADDRESS=$DBUS_SESSION_BUS_ADDRESS"
)
if [ -n "${GITHUB_ENV:-}" ]; then
  printf '%s\n' "${vars[@]}" >> "$GITHUB_ENV"
fi
echo "PipeWire is running in $dir" >&2
echo "unset PIPEWIRE_REMOTE PIPEWIRE_RUNTIME_DIR"
for var in "${vars[@]}"; do
  printf 'export %s=%q\n' "${var%%=*}" "${var#*=}"
done
