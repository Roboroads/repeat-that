#!/usr/bin/env bash
# Checks the Linux packages the way people get them: it unpacks the .deb, the .rpm and the AppImage and checks what is
# inside, so what is checked is what ships. .github/workflows/build-linux.yml runs it on every build, in the
# ubuntu:24.04 container the packages are built in; smoke-linux-package.sh then installs them on each supported
# distribution.
#
#   * DIR holds exactly one Repeat-That_<v>_amd64.deb, one Repeat-That-<v>-1.x86_64.rpm and one
#     Repeat-That_<v>_amd64.AppImage, all of the same version (--version's, when given), and nothing else;
#   * the .deb and the .rpm are called repeat-that, are for x86-64 and depend on WebKitGTK, GTK, the AppIndicator
#     library (loaded at run time, so the bundler lists it), PipeWire's client library and ALSA's, glibc 2.38 and GCC
#     13's libstdc++, and recommend the PipeWire daemon and WirePlumber;
#   * both install the same files: the binary, the desktop entry under the app ID (me.roboroads.repeat-that.desktop,
#     the name the portal and the desktop match the app by) and its icons, plus the bundler's own "Repeat That.desktop",
#     which must be hidden. Both desktop files pass desktop-file-validate;
#   * the binary links libpipewire-0.3 and needs no newer glibc and libstdc++ than the packages ask for (glibc 2.38,
#     GLIBCXX_3.4.31). Weak references don't count: the binary starts without them (Rust's standard library looks
#     up pidfd_spawnp that way);
#   * the AppImage is a type 2 AppImage whose root desktop entry is the real one (visible, valid, its icon at the root),
#     with no libpipewire-0.3 or libspa-0.2 inside: a bundled PipeWire client would load the host's PipeWire plugins,
#     which belong to another version. The tray library and WebKit's helper processes must be inside.
#
# Prints every check as "ok: ..." or "FAIL: ..." and exits 1 when one failed. Needs dpkg-deb, rpm, rpm2cpio, cpio,
# desktop-file-validate and readelf (binutils). Unpacks into a folder under TMPDIR.
#
# Usage: check-linux-packages.sh [--version X.Y.Z] DIR
set -euo pipefail

usage="usage: check-linux-packages.sh [--version X.Y.Z] DIR"
expected_version=""
if [[ "${1:-}" == --version ]]; then
  expected_version=${2:-}
  if [[ ! "$expected_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "$usage" >&2
    exit 2
  fi
  shift 2
fi
if [[ $# -ne 1 || ! -d "$1" ]]; then
  echo "$usage" >&2
  exit 2
fi
dir=$(cd "$1" && pwd)

app_id=me.roboroads.repeat-that
hidden_entry="Repeat That.desktop"
# The newest glibc and libstdc++ symbol versions the binary may need: what the packages depend on (libc6 (>= 2.38) and
# libstdc++6 (>= 13), or the same as rpm sonames). The statically linked ONNX Runtime is what needs them.
glibc_max=2.38
glibcxx_max=3.4.31

failures=0
ok() { echo "ok: $*"; }
fail() {
  echo "FAIL: $*"
  failures=$((failures + 1))
}
# Prints command output indented under the check it belongs to.
indent() { sed 's/^/    /'; }

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
# A command that fails outside a check stops the script; say which, so the log isn't silent about it.
trap 'echo "FAIL: stopped at line $LINENO: $BASH_COMMAND (exit $?)"' ERR

# The value of KEY in the [Desktop Entry] group of a desktop file.
entry_key() {
  sed -n '/^\[Desktop Entry\]/,/^\[/p' "$1" | sed -n "s/^$2=//p" | head -n 1
}

# One dependency list ("a, b | c, d") as one item per line.
split_list() { echo "${1//, /$'\n'}"; }

# Checks that every wanted item is in the list. $1 names the list, $2 is the list, one item per line.
has_all() {
  local what=$1 list=$2 wanted
  shift 2
  for wanted in "$@"; do
    if grep -qxF -- "$wanted" <<<"$list"; then
      ok "$what has $wanted"
    else
      fail "$what lacks $wanted"
      indent <<<"$list"
    fi
  done
}

# --- the files -------------------------------------------------------------------------------------------------------

shopt -s nullglob
debs=("$dir"/*.deb)
rpms=("$dir"/*.rpm)
appimages=("$dir"/*.AppImage)
shopt -u nullglob

for kind in deb rpm AppImage; do
  case $kind in
    deb) count=${#debs[@]} ;;
    rpm) count=${#rpms[@]} ;;
    AppImage) count=${#appimages[@]} ;;
  esac
  if [[ $count -eq 1 ]]; then
    ok "one .$kind"
  else
    fail "expected one .$kind in $dir, found $count"
  fi
done
others=$(find "$dir" -mindepth 1 -maxdepth 1 -not -name '*.deb' -not -name '*.rpm' -not -name '*.AppImage')
if [[ -z "$others" ]]; then
  ok "no other files"
else
  fail "files that are not packages:"
  indent <<<"$others"
fi
if [[ ${#debs[@]} -ne 1 || ${#rpms[@]} -ne 1 || ${#appimages[@]} -ne 1 ]]; then
  echo "$failures check(s) failed"
  exit 1
fi
deb=${debs[0]}
rpm=${rpms[0]}
appimage=${appimages[0]}

# The names people download. GitHub turns spaces into dots, so there must be none.
versions=()
for file in "$deb" "$rpm" "$appimage"; do
  name=$(basename "$file")
  if [[ "$name" =~ ^Repeat-That_([0-9]+\.[0-9]+\.[0-9]+)_amd64\.(deb|AppImage)$ ||
    "$name" =~ ^Repeat-That-([0-9]+\.[0-9]+\.[0-9]+)-1\.x86_64\.rpm$ ]]; then
    ok "$name is named as expected"
    versions+=("${BASH_REMATCH[1]}")
  else
    fail "$name is not named Repeat-That_<v>_amd64.deb, Repeat-That-<v>-1.x86_64.rpm or Repeat-That_<v>_amd64.AppImage"
  fi
done
version=${expected_version:-${versions[0]:-}}
for found in "${versions[@]}"; do
  if [[ "$found" != "$version" ]]; then
    fail "the files are not all version $version (one is $found)"
  fi
done
echo "Version: ${version:-unknown}"

# --- what the .deb and the .rpm install ------------------------------------------------------------------------------

# Checks an unpacked package (or AppDir) $2 that installs into usr/: the binary and how it was linked.
check_binary() {
  local label=$1 root=$2 binary="$2/usr/bin/repeat-that" lib max newest
  if [[ -x "$binary" ]]; then
    ok "$label: /usr/bin/repeat-that"
  else
    fail "$label: no executable /usr/bin/repeat-that"
    return
  fi
  if readelf -d "$binary" | grep -qF '[libpipewire-0.3.so.0]'; then
    ok "$label: the binary links libpipewire-0.3.so.0"
  else
    fail "$label: the binary doesn't link libpipewire-0.3.so.0"
  fi
  # The version needs (readelf -V) that are not weak: "Name: GLIBC_2.38  Flags: none".
  for lib in GLIBC GLIBCXX; do
    if [[ $lib == GLIBC ]]; then max=$glibc_max; else max=$glibcxx_max; fi
    newest=$(readelf -V --wide "$binary" | sed -n "s/.*Name: ${lib}_\([0-9][0-9.]*\)  Flags: none.*/\1/p" |
      sort -uV | tail -n 1 || true)
    if [[ -n "$newest" && "$(printf '%s\n%s\n' "$newest" "$max" | sort -V | tail -n 1)" == "$max" ]]; then
      ok "$label: the binary needs ${lib}_$newest, no newer than ${lib}_$max"
    else
      fail "$label: the binary needs ${lib}_${newest:-?}, newer than ${lib}_$max that the packages ask for"
    fi
  done
}

# Checks the desktop entry the app is known by: valid, shown, starting the binary, with the app ID's icon.
check_entry() {
  local label=$1 file=$2 out
  if out=$(desktop-file-validate "$file" 2>&1); then
    ok "$label passes desktop-file-validate"
  else
    fail "$label fails desktop-file-validate"
  fi
  # Hints (more than one main category) are allowed; errors and warnings are not.
  if [[ -n "$out" ]]; then indent <<<"$out"; fi
  if [[ "$(entry_key "$file" Exec)" == repeat-that && "$(entry_key "$file" Icon)" == "$app_id" &&
    "$(entry_key "$file" StartupWMClass)" == repeat-that && -n "$(entry_key "$file" Categories)" ]]; then
    ok "$label starts repeat-that, has the icon $app_id, StartupWMClass repeat-that and categories"
  else
    fail "$label: Exec, Icon, StartupWMClass or Categories is not what it should be"
    indent <"$file"
  fi
  if [[ -z "$(entry_key "$file" NoDisplay)$(entry_key "$file" Hidden)" ]]; then
    ok "$label is shown in the menu"
  else
    fail "$label is hidden (NoDisplay or Hidden)"
  fi
}

check_installed() {
  local label=$1 root=$2 apps="$2/usr/share/applications" entries icon
  check_binary "$label" "$root"
  if [[ -f "$apps/$app_id.desktop" ]]; then
    check_entry "$label: $app_id.desktop" "$apps/$app_id.desktop"
  else
    fail "$label: no /usr/share/applications/$app_id.desktop"
  fi
  # The bundler's own entry, named after the product, can't be left out. It must be hidden, and valid like any other.
  if [[ -f "$apps/$hidden_entry" ]]; then
    if [[ "$(entry_key "$apps/$hidden_entry" NoDisplay)" == true ]] && desktop-file-validate "$apps/$hidden_entry"; then
      ok "$label: the bundler's $hidden_entry is hidden and valid"
    else
      fail "$label: the bundler's $hidden_entry is not hidden, or not valid"
      indent <"$apps/$hidden_entry"
    fi
  else
    fail "$label: no /usr/share/applications/$hidden_entry (the bundler's own entry; did the bundler change?)"
  fi
  entries=$(cd "$apps" 2>/dev/null && find . -name '*.desktop' | sed 's|^\./||' | sort || true)
  if [[ "$entries" == "$(printf '%s\n' "$hidden_entry" "$app_id.desktop" | sort)" ]]; then
    ok "$label: no other desktop files"
  else
    fail "$label: unexpected desktop files"
    indent <<<"$entries"
  fi
  for icon in hicolor/scalable/apps/$app_id.svg hicolor/{32x32,128x128,256x256,512x512}/apps/$app_id.png; do
    if [[ -s "$root/usr/share/icons/$icon" ]]; then
      ok "$label: /usr/share/icons/$icon"
    else
      fail "$label: no /usr/share/icons/$icon"
    fi
  done
  if [[ -s "$root/usr/share/pixmaps/$app_id.png" ]]; then
    ok "$label: /usr/share/pixmaps/$app_id.png"
  else
    fail "$label: no /usr/share/pixmaps/$app_id.png"
  fi
}

echo
echo "Debian package: $(basename "$deb")"
deb_field() { dpkg-deb -f "$deb" "$1"; }
for pair in Package=repeat-that "Version=$version" Architecture=amd64; do
  if [[ "$(deb_field "${pair%%=*}")" == "${pair#*=}" ]]; then
    ok ".deb ${pair%%=*} is ${pair#*=}"
  else
    fail ".deb ${pair%%=*} is $(deb_field "${pair%%=*}"), not ${pair#*=}"
  fi
done
has_all ".deb Depends" "$(split_list "$(deb_field Depends)")" libwebkit2gtk-4.1-0 libgtk-3-0 \
  libayatana-appindicator3-1 "libpipewire-0.3-0t64 | libpipewire-0.3-0" "libasound2t64 | libasound2" \
  "libc6 (>= $glibc_max)" "libstdc++6 (>= 13)"
has_all ".deb Recommends" "$(split_list "$(deb_field Recommends)")" pipewire wireplumber
mkdir "$work/deb"
dpkg-deb -x "$deb" "$work/deb"
check_installed ".deb" "$work/deb"

echo
echo "RPM package: $(basename "$rpm")"
rpm_query() { rpm -qp --nosignature "$@" "$rpm" 2>/dev/null; }
for pair in "%{NAME}=repeat-that" "%{VERSION}=$version" "%{RELEASE}=1" "%{ARCH}=x86_64"; do
  if [[ "$(rpm_query --qf "${pair%%=*}")" == "${pair#*=}" ]]; then
    ok ".rpm ${pair%%=*} is ${pair#*=}"
  else
    fail ".rpm ${pair%%=*} is $(rpm_query --qf "${pair%%=*}"), not ${pair#*=}"
  fi
done
has_all ".rpm Requires" "$(rpm_query --requires)" "libwebkit2gtk-4.1.so.0()(64bit)" "libgtk-3.so.0()(64bit)" \
  "libayatana-appindicator3.so.1()(64bit)" "libpipewire-0.3.so.0()(64bit)" "libasound.so.2()(64bit)" \
  "libc.so.6(GLIBC_$glibc_max)(64bit)" "libstdc++.so.6(GLIBCXX_$glibcxx_max)(64bit)"
has_all ".rpm Recommends" "$(rpm_query --recommends)" pipewire wireplumber
mkdir "$work/rpm"
# In two steps, so a failure says which tool failed and how.
code=0
rpm2cpio "$rpm" >"$work/rpm.cpio" 2>"$work/rpm2cpio.err" || code=$?
if [[ $code -ne 0 ]]; then
  echo "note: rpm2cpio exited $code ($(wc -c <"$work/rpm.cpio") bytes of payload)"
  indent <"$work/rpm2cpio.err"
fi
if (cd "$work/rpm" && cpio -idm --quiet <"$work/rpm.cpio") 2>&1 | indent; then
  ok ".rpm unpacks"
else
  fail ".rpm does not unpack with rpm2cpio and cpio"
fi
check_installed ".rpm" "$work/rpm"

deb_files=$(cd "$work/deb" && find . -type f | sort)
rpm_files=$(cd "$work/rpm" && find . -type f | sort)
if [[ "$deb_files" == "$rpm_files" ]]; then
  ok "the .deb and the .rpm install the same files"
else
  fail "the .deb and the .rpm install different files (< .deb, > .rpm)"
  diff <(echo "$deb_files") <(echo "$rpm_files") | indent || true
fi

# --- the AppImage ----------------------------------------------------------------------------------------------------

echo
echo "AppImage: $(basename "$appimage")"
# An ELF file with the AppImage type 2 magic ("AI" and 2) at offset 8.
if [[ "$(od -An -c -N4 "$appimage" | tr -d ' ')" == '177ELF' && "$(od -An -tx1 -j8 -N3 "$appimage" | tr -d ' ')" == 414902 ]]; then
  ok "it is a type 2 AppImage"
else
  fail "it is not a type 2 AppImage (no ELF header or no AI\\x02 magic)"
fi
chmod +x "$appimage"
(cd "$work" && "$appimage" --appimage-extract >/dev/null)
appdir="$work/squashfs-root"
if [[ -d "$appdir" ]]; then
  ok "it extracts"
  roots=$(cd "$appdir" && find . -maxdepth 1 -name '*.desktop' | sed 's|^\./||')
  if [[ $(grep -c . <<<"$roots") -eq 1 ]]; then
    check_entry "the AppImage's $roots" "$appdir/$roots"
    icon=$(entry_key "$appdir/$roots" Icon)
    if [[ -e "$appdir/$icon.svg" || -e "$appdir/$icon.png" ]]; then
      ok "the AppImage has its icon $icon at the root"
    else
      fail "the AppImage has no $icon.svg or $icon.png at the root"
    fi
  else
    fail "the AppImage's root should hold one desktop file, it holds: ${roots:-none}"
  fi
  if [[ -e "$appdir/.DirIcon" && -x "$appdir/AppRun" ]]; then
    ok "the AppImage has .DirIcon and AppRun"
  else
    fail "the AppImage lacks .DirIcon or an executable AppRun"
  fi
  check_binary "the AppImage" "$appdir"
  bundled=$(cd "$appdir" && find . \( -name 'libpipewire-0.3.so*' -o -name 'libspa-*' -o -path '*/spa-0.2/*' \) | sort)
  if [[ -z "$bundled" ]]; then
    ok "the AppImage holds no libpipewire-0.3 or libspa-0.2"
  else
    fail "the AppImage holds PipeWire libraries, which must come from the system:"
    indent <<<"$bundled"
  fi
  for inside in usr/lib/libayatana-appindicator3.so.1 usr/lib/x86_64-linux-gnu/webkit2gtk-4.1/WebKitWebProcess \
    usr/lib/x86_64-linux-gnu/webkit2gtk-4.1/WebKitNetworkProcess; do
    if [[ -e "$appdir/$inside" ]]; then
      ok "the AppImage holds $inside"
    else
      fail "the AppImage lacks $inside"
    fi
  done
else
  fail "it does not extract with --appimage-extract"
fi

echo
if [[ $failures -gt 0 ]]; then
  echo "$failures check(s) failed"
  exit 1
fi
echo "all checks passed"
