#!/usr/bin/env bash
# Installs the Linux packages on the system it runs on, the way a user would, and checks that the app starts there.
# .github/workflows/build-linux.yml runs it as root in a fresh container of each supported distribution (Debian 13,
# Ubuntu 24.04, Fedora 40 and 44), on the files check-linux-packages.sh has checked.
#
#   * the .deb (with apt) or the .rpm (with dnf) installs with its required dependencies alone, no recommended ones;
#   * the installed binary has every library it links (ldd) and its self-check (`--rt-self-check`) reports Silero with
#     the packages' version, which also proves the binary loads at all on this glibc;
#   * the installed desktop entries pass this distribution's desktop-file-validate, and the icons are there;
#   * the AppImage's self-check passes too, with the libraries an AppImage takes from the system (the ones the
#     package's dependencies brought in, PipeWire's and ALSA's among them);
#   * the environment the AppImage gives the host programs it starts (`--rt-host-env`: a browser through xdg-open, a
#     settings page) points nowhere into the AppImage, and this system's `gio` and `dbus-uuidgen` run with it;
#   * the package uninstalls and takes its files with it.
#
# With --refused it checks the opposite, on a system that is too old (Debian 12): the package manager must refuse the
# package, naming the glibc or libstdc++ it needs, rather than install a binary that can't start.
#
# With --archive, on Fedora, dnf takes the release's packages from archives.fedoraproject.org, where a release goes
# once it is end of life (Fedora 40 since May 2025) and the mirrors drop it. A Fedora whose mirrors don't answer falls
# back to the archive too, so a release that has just gone end of life doesn't block anything.
#
# The self-check plays and captures nothing and needs no display. Prints every check as "ok: ..." or "FAIL: ..." and
# exits 1 when one failed. Needs linux-packages-lib.sh next to it.
#
# Usage: smoke-linux-package.sh [--refused] [--archive] DIR
set -euo pipefail

usage="usage: smoke-linux-package.sh [--refused] [--archive] DIR"
refused=false
archive=false
while [[ "${1:-}" == --* ]]; do
  case $1 in
    --refused) refused=true ;;
    --archive) archive=true ;;
    *)
      echo "$usage" >&2
      exit 2
      ;;
  esac
  shift
done
if [[ $# -ne 1 || ! -d "$1" ]]; then
  echo "$usage" >&2
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
# No update key is compiled into these builds (yet), so the self-check says there is no updater.
expected="{\"vad\":\"silero\",\"arch\":\"x86_64\",\"os\":\"linux\",\"version\":\"$version\",\"updater\":false}"

echo "System: $(sed -n 's/^PRETTY_NAME=//p' /etc/os-release | tr -d '"')"

# Replaces dnf's repositories by the release's archive: the release itself and its final updates. The archive doesn't
# change any more, and its packages are signed with the same keys as before.
use_fedora_archive() {
  local release base=https://archives.fedoraproject.org/pub/archive/fedora/linux
  release=$(sed -n 's/^VERSION_ID=//p' /etc/os-release | tr -d '"')
  mkdir -p /etc/yum.repos.d/mirrors
  mv /etc/yum.repos.d/*.repo /etc/yum.repos.d/mirrors/
  cat >/etc/yum.repos.d/fedora-archive.repo <<EOF
[fedora-archive]
name=Fedora $release (archive)
baseurl=$base/releases/$release/Everything/\$basearch/os/
gpgcheck=1
gpgkey=file:///etc/pki/rpm-gpg/RPM-GPG-KEY-fedora-$release-\$basearch

[updates-archive]
name=Fedora $release updates (archive)
baseurl=$base/updates/$release/Everything/\$basearch/
gpgcheck=1
gpgkey=file:///etc/pki/rpm-gpg/RPM-GPG-KEY-fedora-$release-\$basearch
EOF
  echo "note: dnf takes Fedora $release from $base"
}
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
  if [[ $archive == true ]] || ! dnf -q makecache >/dev/null 2>&1; then
    use_fedora_archive
  fi
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
  else
    # What the package manager couldn't satisfy, one per line: apt's "Depends: libc6 (>= 2.38) but ..." and dnf's
    # "nothing provides libc.so.6(GLIBC_2.38)(64bit) needed by ...". Every one must be glibc or libstdc++, else the
    # refusal is for another reason.
    unmet=$(sed -nE 's/.*(Pre)?Depends: ([^ ]+).*/\2/p; s/.*nothing provides ([^ ]+) needed by.*/\1/p' <<<"$out" | sort -u)
    others=$(grep -vxE 'libc6|libstdc\+\+6|libc\.so\.6\(GLIBC_[0-9.]+\)\(64bit\)|libstdc\+\+\.so\.6\(GLIBCXX_[0-9.]+\)\(64bit\)' \
      <<<"$unmet" || true)
    if [[ -n "$unmet" && -z "$others" ]]; then
      ok "$(basename "$package") is refused for the glibc or libstdc++ it needs: $(paste -sd " " <<<"$unmet")"
    elif [[ -n "$unmet" ]]; then
      fail "$(basename "$package") is refused, but also for $(paste -sd " " <<<"$others")"
    else
      fail "$(basename "$package") is refused, but the package manager names no unmet dependency"
    fi
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

# The same file, not the same name: since Fedora 42 /usr/sbin is a link to /usr/bin, and it comes first on the PATH.
found=$(command -v repeat-that || true)
if [[ -x /usr/bin/repeat-that && -n "$found" && "$found" -ef /usr/bin/repeat-that ]]; then
  ok "repeat-that on the PATH is /usr/bin/repeat-that"
else
  fail "repeat-that on the PATH (${found:-none}) is not /usr/bin/repeat-that"
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
if out=$(install_packages desktop-file-utils 2>&1); then
  for entry in "$app_id.desktop" "Repeat That.desktop"; do
    if out=$(desktop-file-validate "/usr/share/applications/$entry" 2>&1); then
      ok "/usr/share/applications/$entry passes desktop-file-validate"
    else
      fail "/usr/share/applications/$entry fails desktop-file-validate"
    fi
    if [[ -n "$out" ]]; then indent <<<"$out"; fi
  done
else
  fail "desktop-file-utils does not install, so the desktop entries can't be validated"
  indent <<<"$out" | tail -n 15
fi
for icon in /usr/share/icons/hicolor/scalable/apps/$app_id.svg /usr/share/icons/hicolor/{32x32,128x128,256x256,512x512}/apps/$app_id.png; do
  if [[ -s "$icon" ]]; then ok "$icon"; else fail "no $icon"; fi
done

# The AppImage on the same system. A container has no FUSE, so it extracts itself into TMPDIR first. Artifacts lose the
# executable bit, and downloads from a release don't have it either.
chmod +x "$appimage"
check_self_check "the AppImage" env APPIMAGE_EXTRACT_AND_RUN=1 "$appimage" --rt-self-check

# What a host program started from the AppImage gets. With the AppImage's own environment, a newer system's gio and
# the D-Bus tools load the AppImage's GLib and D-Bus libraries and fail (Debian 13: an undefined symbol, a missing
# symbol version).
check_host_env() {
  local out env_file leaks command
  local -a tools host
  case $kind in
    deb) tools=(libglib2.0-bin dbus-bin) ;;
    rpm) tools=(glib2 dbus-tools) ;;
  esac
  if ! out=$(install_packages "${tools[@]}" 2>&1); then
    fail "${tools[*]} do not install, so the host environment can't be tried"
    indent <<<"$out" | tail -n 15
    return
  fi
  env_file=$(mktemp)
  if ! env APPIMAGE_EXTRACT_AND_RUN=1 "$appimage" --rt-host-env >"$env_file" 2>/dev/null; then
    fail "the AppImage does not print the environment of host programs (--rt-host-env)"
    return
  fi
  mapfile -d '' -t host <"$env_file"
  rm -f "$env_file"
  # Where the AppImage is while it runs: extracted (as here) or mounted.
  leaks=$(printf '%s\n' "${host[@]}" | grep -E '/appimage_extracted_|/\.mount_' || true)
  if [[ ${#host[@]} -gt 0 && -z "$leaks" ]]; then
    ok "the AppImage gives host programs ${#host[@]} variables, none pointing into it"
  else
    fail "the environment of host programs points into the AppImage"
    indent <<<"$leaks"
  fi
  for command in "gio info /" dbus-uuidgen; do
    # shellcheck disable=SC2086 # the command and its arguments
    if out=$(env -i "${host[@]}" $command 2>&1); then
      ok "$command runs with the environment the AppImage gives host programs"
    else
      fail "$command fails with the environment the AppImage gives host programs"
      indent <<<"$out" | tail -n 5
    fi
  done
}
check_host_env

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
