# Shared by check-linux-packages.sh and smoke-linux-package.sh, which source it: how they report, and how they find the
# packages and their version, so the two always agree on both.
#
#   ok MESSAGE, fail MESSAGE   print "ok: MESSAGE" or "FAIL: MESSAGE"; fail counts the failures in $failures
#   indent                     indents its input under the check it belongs to
#   find_packages DIR          sets $deb, $rpm and $appimage to the one file of each kind in DIR and $version to the
#                              version their names carry. Prints an ok or FAIL line per check and returns 1 when a kind
#                              is missing or doubled, a name is not the release name, or the versions differ.
# shellcheck shell=bash disable=SC2034

failures=0
ok() { echo "ok: $*"; }
fail() {
  echo "FAIL: $*"
  failures=$((failures + 1))
}
indent() { sed 's/^/    /'; }

# The names people download: Repeat-That_<v>_amd64.deb, Repeat-That-<v>-1.x86_64.rpm and Repeat-That_<v>_amd64.AppImage.
# GitHub turns spaces into dots, so there must be none.
package_name_version() {
  local name=$1
  if [[ "$name" =~ ^Repeat-That_([0-9]+\.[0-9]+\.[0-9]+)_amd64\.(deb|AppImage)$ ||
    "$name" =~ ^Repeat-That-([0-9]+\.[0-9]+\.[0-9]+)-1\.x86_64\.rpm$ ]]; then
    echo "${BASH_REMATCH[1]}"
  else
    return 1
  fi
}

find_packages() {
  local dir=$1 kind file name found missing=false
  local -a files
  local -a versions=()
  deb="" rpm="" appimage="" version=""
  for kind in deb rpm AppImage; do
    shopt -s nullglob
    files=("$dir"/*."$kind")
    shopt -u nullglob
    if [[ ${#files[@]} -ne 1 ]]; then
      fail "expected one .$kind in $dir, found ${#files[@]}"
      missing=true
      continue
    fi
    ok "one .$kind"
    case $kind in
      deb) deb=${files[0]} ;;
      rpm) rpm=${files[0]} ;;
      AppImage) appimage=${files[0]} ;;
    esac
    name=$(basename "${files[0]}")
    if found=$(package_name_version "$name"); then
      ok "$name is named as expected"
      versions+=("$found")
    else
      fail "$name is not named Repeat-That_<v>_amd64.deb, Repeat-That-<v>-1.x86_64.rpm or Repeat-That_<v>_amd64.AppImage"
      missing=true
    fi
  done
  if [[ $missing == true ]]; then
    return 1
  fi
  version=${versions[0]}
  for found in "${versions[@]}"; do
    if [[ "$found" != "$version" ]]; then
      fail "the packages are not all of one version (${versions[*]})"
      return 1
    fi
  done
  echo "Version: $version"
}
