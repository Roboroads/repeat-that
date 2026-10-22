#!/usr/bin/env bash
# Installs the Linux packages on the system it runs on, the way a user would, and checks that the app starts there.
# .github/workflows/build-linux.yml runs it as root in a fresh container of each supported distribution (Debian 13,
# Ubuntu 24.04, Fedora 40), on the files check-linux-packages.sh has checked.
#
#   * the .deb (with apt) or the .rpm (with dnf) installs with its required dependencies alone, no recommended ones;
#   * the installed binary has every library it links (ldd) and its self-check (`--rt-self-check`) reports Silero with
#     the packages' version, which also proves the binary loads at all on this glibc;
#   * the installed desktop entries pass this distribution's desktop-file-validate, and the icons are there;
#   * the AppImage's self-check passes too, with the libraries an AppImage takes from the system (the ones the
#     package's dependencies brought in, PipeWire's and ALSA's among them);
#   * the package uninstalls and takes its files with it.
#
# With --refused it checks the opposite, on a system that is too old (Debian 12): the package manager must refuse the
# package, naming the glibc or libstdc++ it needs, rather than install a binary that can't start.
#
# The self-check plays and captures nothing and needs no display. Prints every check as "ok: ..." or "FAIL: ..." and
# exits 1 when one failed. Needs linux-packages-lib.sh next to it.
#
# Usage: smoke-linux-package.sh [--refused] DIR
set -euo pipefail

refused=false
if [[ "${1:-}" == --refused ]]; then
  refused=true
  shift
fi
if [[ $# -ne 1 || ! -d "$1" ]]; then
  echo "usage: smoke-linux-package.sh [--refused] DIR" >&2
  exit 2
fi
dir=$(cd "$1" && pwd)
app_id=me.roboroads.repeat-that

# ok, fail, indent and find_packages.
# shellcheck source=linux-packages-lib.sh
source "$(dirname "$0")/linux-packages-lib.sh"

# Sets deb, rpm, appimage and version.
if ! find_packages "$dir"; then
  ls -l "$dir"
  exit 1
fi
expected="{\"vad\":\"silero\",\"arch\":\"x86_64\",\"os\":\"linux\",\"version\":\"$version\"}"

echo "System: $(sed -n 's/^PRETTY_NAME=//p' /etc/os-release | tr -d '"')"
if command -v apt-get >/dev/null; then
  kind=deb
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -qq
  install_packages() { apt-get install -y -qq --no-install-recommends "$@"; }
  # Without -qq, so a refusal names the dependency it can't meet.
  try_install() { apt-get install -y --no-install-recommends "$@"; }
  uninstall() { apt-get remove -y -qq repeat-that; }
elif command -v dnf >/dev/null; then
  kind=rpm
  install_packages() { dnf install -y -q --setopt=install_weak_deps=False "$@"; }
  try_install() { dnf install -y --setopt=install_weak_deps=False "$@"; }
  uninstall() { dnf remove -y -q repeat-that; }
else
  echo "FAIL: neither apt-get nor dnf on this system" >&2
  exit 1
fi
if [[ $kind == deb ]]; then package=$deb; else package=$rpm; fi

# Checks one self-check run: its output must be exactly the expected line, and its exit code 0.
check_self_check() {
  local label=$1 out code=0 err
  shift
  # The report is a line of its own on stdout. Whatever else is printed (a library's warning on stderr, say) is shown
  # but doesn't decide.
  err=$(mktemp)
  out=$("$@" 2>"$err") || code=$?
  if [[ $code -eq 0 ]] && grep -qxF -- "$expected" <<<"$out"; then
    ok "$label: the self-check reports $expected"
  else
    fail "$label: the self-check exited $code, expected $expected on stdout"
    echo "    stdout:"
    indent <<<"$out"
  fi
  if [[ -s "$err" ]]; then
    echo "    stderr:"
    indent <"$err"
  fi
  rm -f "$err"
}

echo
echo "Installing $(basename "$package")"
if [[ $refused == true ]]; then
  if out=$(try_install "$package" 2>&1); then
    fail "$(basename "$package") installs, but this system is too old to run it"
  elif grep -qE 'libc6|libstdc\+\+6|GLIBC_|GLIBCXX_' <<<"$out"; then
    ok "$(basename "$package") is refused for the glibc or libstdc++ it needs"
  else
    fail "$(basename "$package") is refused, but not for glibc or libstdc++"
  fi
  indent <<<"$out" | tail -n 15
  if [[ $failures -gt 0 ]]; then exit 1; fi
  exit 0
fi
if install_packages "$package" 2>&1 | tail -n 5; then
  ok "$(basename "$package") installs"
else
  fail "$(basename "$package") does not install"
  exit 1
fi

if [[ "$(command -v repeat-that)" == /usr/bin/repeat-that ]]; then
  ok "repeat-that is /usr/bin/repeat-that"
else
  fail "repeat-that is not on the PATH as /usr/bin/repeat-that"
fi
missing=$(ldd /usr/bin/repeat-that | grep 'not found' || true)
if [[ -z "$missing" ]]; then
  ok "ldd finds every library"
else
  fail "libraries are missing:"
  indent <<<"$missing"
fi
check_self_check "the package" repeat-that --rt-self-check

# The validator comes from the distribution, so it is installed only now: it must not bring in what the package lacks.
install_packages desktop-file-utils >/dev/null 2>&1
for entry in "$app_id.desktop" "Repeat That.desktop"; do
  if out=$(desktop-file-validate "/usr/share/applications/$entry" 2>&1); then
    ok "/usr/share/applications/$entry passes desktop-file-validate"
  else
    fail "/usr/share/applications/$entry fails desktop-file-validate"
  fi
  if [[ -n "$out" ]]; then indent <<<"$out"; fi
done
for icon in /usr/share/icons/hicolor/scalable/apps/$app_id.svg /usr/share/icons/hicolor/{32x32,128x128,256x256,512x512}/apps/$app_id.png; do
  if [[ -s "$icon" ]]; then ok "$icon"; else fail "no $icon"; fi
done

# The AppImage on the same system. A container has no FUSE, so it extracts itself into TMPDIR first. Artifacts lose the
# executable bit, and downloads from a release don't have it either.
chmod +x "$appimage"
check_self_check "the AppImage" env APPIMAGE_EXTRACT_AND_RUN=1 "$appimage" --rt-self-check

echo
if uninstall >/dev/null 2>&1 && [[ ! -e /usr/bin/repeat-that && ! -e /usr/share/applications/$app_id.desktop ]]; then
  ok "the package uninstalls and takes its files with it"
else
  fail "the package does not uninstall cleanly"
fi

echo
if [[ $failures -gt 0 ]]; then
  echo "$failures check(s) failed"
  exit 1
fi
echo "all checks passed"
