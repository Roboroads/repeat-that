#!/usr/bin/env bash
# Installs the Flatpak bundle and checks it the way a user's system would see it. .github/workflows/build-flatpak.yml runs
# it on the bundle it has just built, in the container it built it in, which has Flatpak and the GNOME runtime. A failure
# fails the `flatpak` job of a release and holds the release back.
#
#   * the file is named Repeat-That_<version>_x86_64.flatpak, as people download it (no spaces);
#   * the bundle installs, as a user installation;
#   * the permissions are exactly the ones the manifest asks for: the display, the GPU, the PipeWire socket and the D-Bus
#     names of the tray and of KDE's shortcut daemon, and nothing more (no network, no PulseAudio, no home folder, no
#     other bus names);
#   * everything the desktop needs is exported: the desktop entry, the metainfo and the icons, under the Flatpak's ID;
#   * the binary has every library it links (ldd) and so has the tray library the app opens at run time;
#   * the self-check (`--rt-self-check`) reports Silero with the version of the bundle and no updater, which also proves
#     that the ONNX Runtime linked in works inside the sandbox;
#   * the exported desktop entry and metainfo pass desktop-file-validate and appstreamcli;
#   * the app starts in the sandbox under a virtual display, stays up, and writes its tray icon to the folder that the
#     sandbox shares with the desktop ($XDG_RUNTIME_DIR/app/<id>), which is where the desktop's tray reads it.
#
# It plays and captures nothing: there is no PipeWire here. Prints every check as "ok: ..." or "FAIL: ..." and exits 1
# when one failed. Needs linux-packages-lib.sh next to it, and flatpak, desktop-file-validate, appstreamcli, xvfb-run.
#
# Usage: check-flatpak.sh --version X.Y.Z BUNDLE
set -euo pipefail

usage="usage: check-flatpak.sh --version X.Y.Z BUNDLE"
if [[ $# -ne 3 || "$1" != --version ]]; then
  echo "$usage" >&2
  exit 2
fi
version=$2
bundle=$3
if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ || ! -f "$bundle" ]]; then
  echo "$usage" >&2
  exit 2
fi
app=me.roboroads.RepeatThat

# ok, fail and indent.
# shellcheck source=linux-packages-lib.sh
source "$(dirname "$0")/linux-packages-lib.sh"

# Runs a command in the sandbox of the app. No GPU: the machines that run this have none, and a virtual display has no
# DRI3 either.
in_sandbox() { flatpak run --nodevice=dri --command="$1" "$app" "${@:2}"; }

# The scratch files and folders of the checks below, removed when the script ends.
err=
work=
runtime=
trap 'rm -rf "$err" "$work" "$runtime"' EXIT

name=$(basename "$bundle")
if [[ "$name" == "Repeat-That_${version}_x86_64.flatpak" ]]; then
  ok "$name is named as expected"
else
  fail "$name is not named Repeat-That_${version}_x86_64.flatpak"
fi

echo
echo "Installing $name"
if out=$(flatpak install --user -y --noninteractive --bundle "$bundle" 2>&1); then
  ok "$name installs"
else
  fail "$name does not install"
  indent <<<"$out" | tail -n 15
  exit 1
fi
if [[ "$(flatpak info --user "$app" | sed -n 's/^ *Version: *//p')" == "$version" ]]; then
  ok "the installed version is $version"
else
  fail "the installed version is not $version"
fi

# What the manifest asks for, and nothing else. The metadata lists `x11` next to `fallback-x11` for the sake of older
# versions of Flatpak, so the sockets are read as a set that may or may not have it.
echo
echo "Permissions:"
permissions=$(flatpak info --user --show-permissions "$app")
indent <<<"$permissions"
if out=$(python3 -I - "$permissions" <<'PY' 2>&1
import sys

# section -> key -> values, from `flatpak info --show-permissions`.
found, section = {}, None
for line in sys.argv[1].splitlines():
    line = line.strip()
    if not line:
        continue
    if line.startswith("["):
        section = line[1:-1]
        found[section] = {}
    else:
        key, _, value = line.partition("=")
        found[section][key] = {part for part in value.split(";") if part}

wanted = {
    "Context": {
        "devices": {"dri"},
        "filesystems": {"xdg-run/pipewire-0"},
        "shared": {"ipc"},
        "sockets": {"wayland", "fallback-x11"},  # and `x11`, which an older Flatpak reads instead of fallback-x11
    },
    "Session Bus Policy": {"org.kde.StatusNotifierWatcher": {"talk"}, "org.kde.kglobalaccel": {"talk"}},
}
problems = []
for name in found.keys() | wanted.keys():
    got, want = found.get(name, {}), wanted.get(name, {})
    for key in got.keys() | want.keys():
        have, need = got.get(key, set()), want.get(key, set())
        if key == "sockets":
            have -= {"x11"}
        if have != need:
            problems.append(f"[{name}] {key}: found {sorted(have) or 'nothing'}, the manifest asks for {sorted(need) or 'nothing'}")
print("\n".join(problems))
sys.exit(1 if problems else 0)
PY
); then
  ok "the permissions are the display, the GPU, the PipeWire socket and the tray and shortcut daemon bus names, and nothing else"
else
  fail "the permissions are not what the manifest asks for:"
  indent <<<"$out"
fi

echo
echo "Files:"
for file in \
  "/app/bin/repeat-that" \
  "/app/share/applications/$app.desktop" \
  "/app/share/metainfo/$app.metainfo.xml" \
  "/app/share/icons/hicolor/scalable/apps/$app.svg" \
  "/app/share/icons/hicolor/32x32/apps/$app.png" \
  "/app/share/icons/hicolor/128x128/apps/$app.png" \
  "/app/share/icons/hicolor/256x256/apps/$app.png" \
  "/app/share/icons/hicolor/512x512/apps/$app.png" \
  "/app/lib/libayatana-appindicator3.so.1"; do
  if in_sandbox test -s "$file"; then ok "$file"; else fail "no $file"; fi
done
# Only Flatpak's exports are visible on the host, so nothing else may be in them.
exported=$(in_sandbox find /app/share/applications /app/share/metainfo -type f | sort)
if [[ "$exported" == "$(printf '%s\n' "/app/share/applications/$app.desktop" "/app/share/metainfo/$app.metainfo.xml" | sort)" ]]; then
  ok "the exports are the desktop entry and the metainfo, named after the app ID"
else
  fail "unexpected exports:"
  indent <<<"$exported"
fi

echo
echo "Libraries:"
for binary in /app/bin/repeat-that /app/lib/libayatana-appindicator3.so.1; do
  missing=$(in_sandbox ldd "$binary" | grep 'not found' || true)
  if [[ -z "$missing" ]]; then
    ok "ldd finds every library of $binary"
  else
    fail "libraries of $binary are missing:"
    indent <<<"$missing"
  fi
done

# The Flatpak is built without an update key, as Flathub builds it too, since Flathub does the updating. The
# self-check's updater field is about the key, so it says there is no updater.
expected="{\"vad\":\"silero\",\"arch\":\"x86_64\",\"os\":\"linux\",\"version\":\"$version\",\"updater\":false}"
err=$(mktemp)
code=0
out=$(in_sandbox repeat-that --rt-self-check 2>"$err") || code=$?
if [[ $code -eq 0 ]] && grep -qxF -- "$expected" <<<"$out"; then
  ok "the self-check reports $expected"
else
  fail "the self-check exited $code, expected $expected on stdout"
  echo "    stdout:"
  indent <<<"$out" | indent
fi
if [[ -s "$err" ]]; then
  echo "    stderr:"
  indent <"$err" | indent
fi

echo
echo "Desktop files:"
work=$(mktemp -d)
in_sandbox cat "/app/share/applications/$app.desktop" >"$work/$app.desktop"
in_sandbox cat "/app/share/metainfo/$app.metainfo.xml" >"$work/$app.metainfo.xml"
if out=$(desktop-file-validate "$work/$app.desktop" 2>&1); then
  ok "the desktop entry passes desktop-file-validate"
else
  fail "the desktop entry fails desktop-file-validate"
fi
if [[ -n "$out" ]]; then indent <<<"$out"; fi
# No network: the screenshot URL points at the repository's main branch, which is for Flathub to fetch.
if out=$(appstreamcli validate --no-net "$work/$app.metainfo.xml" 2>&1); then
  ok "the metainfo passes appstreamcli validate"
else
  fail "the metainfo fails appstreamcli validate"
fi
indent <<<"$out"
if grep -q "<release version=\"$version\"" "$work/$app.metainfo.xml"; then
  ok "the metainfo has a release for $version"
else
  fail "the metainfo has no release for $version"
fi

# The app in its sandbox under a virtual display. Its runtime folder is a new one, so the tray icon file it writes for
# the desktop's tray can be looked for there: that is the host's view of $XDG_RUNTIME_DIR/app/<id>. A bus of its own
# stands in for the desktop's: it has no tray and no portals, which the app is built for (it shows its window then).
echo
echo "Starting the app:"
runtime=$(mktemp -d)
chmod 700 "$runtime"
log=$work/start.log
(
  export XDG_RUNTIME_DIR=$runtime
  exec dbus-run-session -- xvfb-run -a -s "-screen 0 800x800x24 -ac" flatpak run --nodevice=dri "$app"
) >"$log" 2>&1 &
launcher=$!
alive=false
for _ in $(seq 40); do
  sleep 0.5
  if XDG_RUNTIME_DIR=$runtime flatpak ps --columns=application 2>/dev/null | grep -qx "$app"; then
    alive=true
    break
  fi
done
if [[ $alive == true ]]; then
  # Long enough for the window, the tray and the engine to come up, and for anything fatal among them to show.
  sleep 10
fi
if [[ $alive == true ]] && XDG_RUNTIME_DIR=$runtime flatpak ps --columns=application 2>/dev/null | grep -qx "$app"; then
  ok "the app starts in the sandbox and is still running 10 seconds later"
else
  fail "the app did not start, or stopped soon after"
fi
shopt -s nullglob
icons=("$runtime/app/$app"/tray-icon-*.png)
shopt -u nullglob
if [[ ${#icons[@]} -gt 0 ]]; then
  ok "the tray icon is written to ${icons[0]%/*}, where the desktop's tray can read it"
else
  fail "no tray icon in $runtime/app/$app"
  find "$runtime" -maxdepth 3 | sort | indent
fi
if grep -qi 'panicked\|failed to build' "$log"; then
  fail "the app reported a failure:"
  indent <"$log" | tail -n 20
fi
XDG_RUNTIME_DIR=$runtime flatpak kill "$app" 2>/dev/null || true
wait "$launcher" 2>/dev/null || true
if [[ -s "$log" ]]; then
  echo "    output of the app:"
  indent <"$log" | indent | tail -n 15
fi

echo
if [[ $failures -gt 0 ]]; then
  echo "$failures check(s) failed"
  exit 1
fi
echo "all checks passed"
