#!/usr/bin/env bash
# Checks a macOS .dmg the way people will get it: it mounts the image read-only and checks the app inside, so what is
# checked is what ships. .github/workflows/build-macos.yml runs it on every build.
#
# Every build, the ad hoc signed builds of pull requests too:
#   * the image holds exactly one .app;
#   * its Info.plist has NSAudioCaptureUsageDescription, LSUIElement true and LSMinimumSystemVersion 14.2;
#   * the binary is arm64 only (Apple Silicon), and built for no newer macOS than LSMinimumSystemVersion;
#   * `codesign --verify --deep --strict` passes, the hardened runtime is on, and the entitlements are exactly those in
#     app/src-tauri/Entitlements.plist;
#   * the app's self-check (`--rt-self-check`) reports Silero on aarch64, with the bundle's version.
# With --signed (signed dry runs and releases) also:
#   * the app is signed with IDENTITY (the Developer ID Application certificate), not ad hoc, and so is the image;
#   * Gatekeeper accepts the image and the app as notarized (spctl), and both have their ticket stapled.
#
# Prints every check as "ok: ..." or "FAIL: ..." and exits 1 when one failed. Needs macOS (hdiutil, codesign, plutil,
# lipo, otool, spctl, xcrun) and python3. Written for bash 3.2, the one macOS has.
#
# Usage: check-macos-bundle.sh [--signed IDENTITY] DMG
set -euo pipefail

usage="usage: check-macos-bundle.sh [--signed IDENTITY] DMG"
identity=""
if [ "${1:-}" = "--signed" ]; then
  identity=${2:-}
  if [ -z "$identity" ]; then
    echo "$usage" >&2
    exit 2
  fi
  shift 2
fi
if [ $# -ne 1 ] || [ ! -f "$1" ]; then
  echo "$usage" >&2
  exit 2
fi
dmg=$1

here=$(cd "$(dirname "$0")" && pwd)
entitlements_file="$here/../../app/src-tauri/Entitlements.plist"
minimum_macos=14.2

failures=0
ok() { echo "ok: $*"; }
fail() {
  echo "FAIL: $*"
  failures=$((failures + 1))
}
# Prints command output indented under the check it belongs to.
indent() { sed 's/^/    /'; }

# True when dotted version $1 is not newer than $2 (11.0 <= 14.2, 14.2 <= 14.2, 15.0 > 14.2).
version_at_most() {
  python3 -c '
import sys
def parts(v): return [int(p) for p in v.split(".")] + [0, 0]
sys.exit(0 if parts(sys.argv[1])[:3] <= parts(sys.argv[2])[:3] else 1)' "$1" "$2"
}

work=$(mktemp -d)
mnt="$work/image"
mounted=false
cleanup() {
  if [ "$mounted" = true ]; then
    hdiutil detach "$mnt" -quiet || hdiutil detach "$mnt" -force -quiet || true
  fi
  rm -rf "$work"
}
trap cleanup EXIT

echo "Image: $(basename "$dmg")"

# The image itself: only a signed build signs and notarizes it. Tauri doesn't sign an ad hoc build's image.
if [ -n "$identity" ]; then
  if out=$(codesign --verify --strict --verbose=2 "$dmg" 2>&1); then
    ok "the image's signature is valid"
  else
    fail "the image's signature is not valid"
    echo "$out" | indent
  fi
  authority=$(codesign -dvv "$dmg" 2>&1 | sed -n 's/^Authority=//p' | head -n 1 || true)
  if [ "$authority" = "$identity" ]; then
    ok "the image is signed by $identity"
  else
    fail "the image is signed by '${authority:-nobody}', not by $identity"
  fi
  if out=$(spctl -a -t open --context context:primary-signature -v "$dmg" 2>&1) &&
    echo "$out" | grep -q 'source=Notarized Developer ID'; then
    ok "Gatekeeper accepts the image as notarized"
  else
    fail "Gatekeeper doesn't accept the image as notarized"
  fi
  echo "$out" | indent
  if out=$(xcrun stapler validate "$dmg" 2>&1); then
    ok "the image has its notarization ticket stapled"
  else
    fail "the image has no valid stapled ticket"
  fi
  echo "$out" | indent
fi

mkdir "$mnt"
if ! out=$(hdiutil attach -readonly -nobrowse -noautoopen -mountpoint "$mnt" "$dmg" 2>&1); then
  fail "the image doesn't mount"
  echo "$out" | indent
  echo "1 check failed"
  exit 1
fi
mounted=true

apps=$(find "$mnt" -mindepth 1 -maxdepth 1 -name '*.app' -type d)
count=$(printf '%s' "$apps" | grep -c . || true)
if [ "$count" -ne 1 ]; then
  fail "the image holds $count apps, expected 1"
  find "$mnt" -mindepth 1 -maxdepth 1 | indent
  echo "$failures check(s) failed"
  exit 1
fi
app=$apps
ok "the image holds one app: $(basename "$app")"

plist="$app/Contents/Info.plist"
# One key's value; empty when the key is missing.
plist_value() { plutil -extract "$1" raw -o - "$plist" 2>/dev/null || true; }

identifier=$(plist_value CFBundleIdentifier)
version=$(plist_value CFBundleShortVersionString)
echo "Bundle: ${identifier:-?} ${version:-?}"

if [ -n "$(plist_value NSAudioCaptureUsageDescription)" ]; then
  ok "Info.plist has NSAudioCaptureUsageDescription"
else
  fail "Info.plist has no NSAudioCaptureUsageDescription, so macOS can't ask for the audio capture permission"
fi
ui_element=$(plist_value LSUIElement)
if [ "$ui_element" = "true" ]; then
  ok "Info.plist has LSUIElement true"
else
  fail "Info.plist has LSUIElement '${ui_element}', expected true (no Dock icon)"
fi
minimum=$(plist_value LSMinimumSystemVersion)
if [ "$minimum" = "$minimum_macos" ]; then
  ok "Info.plist has LSMinimumSystemVersion $minimum_macos"
else
  fail "Info.plist has LSMinimumSystemVersion '${minimum}', expected $minimum_macos"
fi

binary="$app/Contents/MacOS/$(plist_value CFBundleExecutable)"
if [ ! -f "$binary" ] || [ ! -x "$binary" ]; then
  fail "the app has no executable at Contents/MacOS/$(plist_value CFBundleExecutable)"
  echo "$failures check(s) failed"
  exit 1
fi

archs=$(lipo -archs "$binary" 2>&1 || true)
if [ "$archs" = "arm64" ]; then
  ok "the binary is arm64 only"
else
  fail "the binary is for '${archs}', expected arm64 only"
fi
# LC_BUILD_VERSION's minos: the oldest macOS the binary loads on.
built_for=$(otool -l "$binary" | awk '$1 == "cmd" { build = ($2 == "LC_BUILD_VERSION") } build && $1 == "minos" { print $2; exit }' || true)
if [ -n "$built_for" ] && [ -n "$minimum" ] && version_at_most "$built_for" "$minimum"; then
  ok "the binary is built for macOS $built_for, not newer than LSMinimumSystemVersion"
else
  fail "the binary is built for macOS '${built_for}', newer than LSMinimumSystemVersion '${minimum}' (or unknown)"
fi

if out=$(codesign --verify --deep --strict --verbose=2 "$app" 2>&1); then
  ok "codesign --verify --deep --strict passes"
else
  fail "codesign --verify --deep --strict fails"
fi
echo "$out" | indent

signature=$(codesign -dvv "$app" 2>&1 || true)
echo "$signature" | grep -E '^(Identifier|Format|CodeDirectory|Signature|Authority|TeamIdentifier|Timestamp)' | indent
if echo "$signature" | grep -Eq '^CodeDirectory .*flags=0x[0-9a-f]+\([^)]*runtime'; then
  ok "the hardened runtime is on"
else
  fail "the hardened runtime is off"
fi
if [ -n "$identity" ]; then
  authority=$(echo "$signature" | sed -n 's/^Authority=//p' | head -n 1 || true)
  team=$(echo "$signature" | sed -n 's/^TeamIdentifier=//p')
  if [ "$authority" = "$identity" ] && [ -n "$team" ] && [ "$team" != "not set" ]; then
    ok "the app is signed by $identity (team $team)"
  else
    fail "the app is signed by '${authority:-nobody}' (team '${team}'), not by $identity"
  fi
elif echo "$signature" | grep -q '^Signature=adhoc'; then
  echo "info: the app is signed ad hoc, as builds without the signing secrets are"
fi

# `--xml` prints the entitlements as an XML plist; some versions print a line before it, so read from `<?xml` on.
codesign -d --entitlements - --xml "$app" >"$work/entitlements.out" 2>/dev/null || true
if out=$(python3 - "$work/entitlements.out" "$entitlements_file" <<'EOF'
import plistlib, sys
raw = open(sys.argv[1], "rb").read()
start = raw.find(b"<?xml")
if start < 0:
    start = raw.find(b"<plist")
signed = plistlib.loads(raw[start:]) if start >= 0 else {}
expected = plistlib.load(open(sys.argv[2], "rb"))
print("signed with: " + (", ".join(f"{key}={value}" for key, value in sorted(signed.items())) or "none"))
if signed != expected:
    print("expected:    " + ", ".join(f"{key}={value}" for key, value in sorted(expected.items())))
    sys.exit(1)
EOF
); then
  ok "the entitlements are exactly those in Entitlements.plist"
else
  fail "the entitlements differ from Entitlements.plist"
fi
echo "$out" | indent

# The self-check plays and captures nothing; perl's alarm stops it if it hangs (macOS has no `timeout`).
if report=$(perl -e 'alarm shift; exec @ARGV' 120 "$binary" --rt-self-check 2>"$work/self-check.err"); then
  status=0
else
  status=$?
fi
echo "self-check (exit $status): $report"
if [ -s "$work/self-check.err" ]; then indent <"$work/self-check.err"; fi
if out=$(python3 - "$report" "$version" <<'EOF'
import json, sys
try:
    report = json.loads(sys.argv[1])
except ValueError:
    report = None
if not isinstance(report, dict):
    print("its output is not one line of JSON")
    sys.exit(1)
expected = {"vad": "silero", "arch": "aarch64", "os": "macos", "version": sys.argv[2]}
wrong = [f"{key} is {report.get(key)!r}, expected {value!r}" for key, value in expected.items() if report.get(key) != value]
if wrong:
    print("; ".join(wrong))
    sys.exit(1)
EOF
) && [ "$status" -eq 0 ]; then
  ok "the self-check reports Silero on aarch64, version $version"
else
  fail "the self-check failed (exit $status): $out"
fi

if [ -n "$identity" ]; then
  if out=$(xcrun stapler validate "$app" 2>&1); then
    ok "the app has its notarization ticket stapled"
  else
    fail "the app has no valid stapled ticket"
  fi
  echo "$out" | indent
  if out=$(spctl -a -t exec -vv "$app" 2>&1) && echo "$out" | grep -q 'source=Notarized Developer ID'; then
    ok "Gatekeeper accepts the app as notarized"
  else
    fail "Gatekeeper doesn't accept the app as notarized"
  fi
  echo "$out" | indent
fi

if [ "$failures" -gt 0 ]; then
  echo "$failures check(s) failed"
  exit 1
fi
echo "all checks passed"
