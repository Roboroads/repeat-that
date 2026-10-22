#!/usr/bin/env bash
# Signs the update files of a release and writes the updater manifests, in the folder that holds the release's files.
# The publish and late-attach jobs of .github/workflows/publish-release.yml run it after they have downloaded the files
# the build jobs made and before they write SHA256SUMS, so the signatures and manifests are release files like the rest.
#
# For each platform that has an update payload in DIR (Windows: the NSIS `*-setup.exe`, macOS: the `*.app.tar.gz`,
# Linux: the `*.AppImage`; .github/scripts/updater_manifest.py has the table):
#   1. `tauri signer sign` writes `<payload>.sig` next to it, with the version in the signature, so a manifest that
#      announces another version than the one signed is refused by the app's updater;
#   2. `minisign -V` checks that signature against the public key the builds have compiled in (--pubkey), so a private
#      key that is not the one of that public key stops the release here, before anything is uploaded;
#   3. updater_manifest.py writes `latest-<target>-<arch>.json`, with the notes of the release (--notes).
# Then release_assets.py check-manifests reads every manifest again against the files in DIR.
#
# The private key is only read from the environment: TAURI_SIGNING_PRIVATE_KEY (the key itself, not a path) and
# TAURI_SIGNING_PRIVATE_KEY_PASSWORD (empty or unset for a key without one). Dry runs pass a throwaway key made in
# the job.
#
# Needs the Tauri CLI that `npm ci` installs in app/ (or TAURI_CLI), minisign, python3 and base64. The folder may hold
# other release files; only the payloads are touched. Prints what it did. Exits 1, with ::error:: lines, when
# something is wrong.
#
# Usage: sign-updates.sh --dir DIR --repo OWNER/NAME --notes FILE --pub-date TIMESTAMP --pubkey FILE [--tag TAG]
#   --tag is the release tag; without one (a dry run) it is made from the version in the payloads' names.
#   --pubkey is a file holding the public key as one line of base64, as the `.pub` file of `tauri signer generate`.
#   --pub-date is an RFC 3339 timestamp for the manifests.
set -euo pipefail

usage="usage: sign-updates.sh --dir DIR --repo OWNER/NAME --notes FILE --pub-date TIMESTAMP --pubkey FILE [--tag TAG]"
dir="" repo="" notes="" pub_date="" pubkey="" tag=""
while [[ $# -gt 0 ]]; do
  case $1 in
    --dir | --repo | --notes | --pub-date | --pubkey | --tag)
      if [[ $# -lt 2 ]]; then
        echo "$usage" >&2
        exit 2
      fi
      case $1 in
        --dir) dir=$2 ;;
        --repo) repo=$2 ;;
        --notes) notes=$2 ;;
        --pub-date) pub_date=$2 ;;
        --pubkey) pubkey=$2 ;;
        --tag) tag=$2 ;;
      esac
      shift 2
      ;;
    *)
      echo "$usage" >&2
      exit 2
      ;;
  esac
done
if [[ -z "$dir" || -z "$repo" || -z "$notes" || -z "$pub_date" || -z "$pubkey" ]]; then
  echo "$usage" >&2
  exit 2
fi
for path in "$dir" "$notes" "$pubkey"; do
  if [[ ! -e "$path" ]]; then
    echo "::error::$path does not exist"
    exit 1
  fi
done

# Prints command output indented under the step it belongs to.
indent() { sed 's/^/    /'; }

here=$(cd "$(dirname "$0")" && pwd)
dir=$(cd "$dir" && pwd)
tauri=${TAURI_CLI:-$here/../../app/node_modules/.bin/tauri}
if [[ ! -x "$tauri" ]]; then
  echo "::error::The Tauri CLI is not at $tauri; run npm ci in app/ first (or set TAURI_CLI)"
  exit 1
fi
if ! command -v minisign >/dev/null; then
  echo "::error::minisign is not installed (apt-get install minisign)"
  exit 1
fi
if [[ -z "${TAURI_SIGNING_PRIVATE_KEY:-}" ]]; then
  echo "::error::TAURI_SIGNING_PRIVATE_KEY is empty, so there is nothing to sign with"
  exit 1
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
if ! base64 -d <"$pubkey" >"$work/pub" 2>/dev/null || ! grep -q '^untrusted comment:' "$work/pub"; then
  echo "::error::$pubkey is not a public key as the .pub file of tauri signer generate has it (one line of base64)"
  exit 1
fi

shopt -s nullglob
# Platform, then the pattern of its payload; the same as TARGETS of updater_manifest.py, which refuses a file that
# doesn't match its platform.
payloads=()
platforms=()
for entry in "windows:*-setup.exe" "macos:*.app.tar.gz" "linux:*.AppImage"; do
  platform=${entry%%:*}
  # shellcheck disable=SC2206 # the pattern is meant to expand
  found=("$dir"/${entry#*:})
  if [[ ${#found[@]} -gt 1 ]]; then
    echo "::error::expected one update file of $platform in $dir, found ${#found[@]}: ${found[*]##*/}"
    exit 1
  fi
  if [[ ${#found[@]} -eq 1 ]]; then
    platforms+=("$platform")
    payloads+=("${found[0]}")
  fi
done
if [[ ${#payloads[@]} -eq 0 ]]; then
  echo "::error::no update file (*-setup.exe, *.app.tar.gz or *.AppImage) in $dir"
  exit 1
fi

# A dry run has no tag: the version is the one in the file names, Repeat-That_<version>_<arch>.<extension>.
if [[ -z "$tag" ]]; then
  version=$(basename "${payloads[0]}" | cut -d_ -f2)
  tag=v$version
  echo "No tag given: using $tag, the version in the name of ${payloads[0]##*/}"
fi
if [[ ! "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "::error::the tag must look like v0.1.0, got '$tag'"
  exit 1
fi

for index in "${!payloads[@]}"; do
  platform=${platforms[$index]}
  payload=${payloads[$index]}
  name=${payload##*/}
  echo "== $platform: $name"
  rm -f "$payload.sig"
  if ! out=$("$tauri" signer sign --app-version "${tag#v}" "$payload" 2>&1); then
    indent <<<"$out"
    echo "::error::Signing $name failed. Does TAURI_SIGNING_PRIVATE_KEY_PASSWORD belong to TAURI_SIGNING_PRIVATE_KEY?"
    exit 1
  fi
  if [[ ! -s "$payload.sig" ]]; then
    echo "::error::The Tauri CLI did not write $name.sig"
    exit 1
  fi
  base64 -d <"$payload.sig" >"$work/signature"
  if out=$(minisign -V -p "$work/pub" -x "$work/signature" -m "$payload" 2>&1); then
    echo "    signed, and the signature holds for TAURI_UPDATER_PUBKEY"
  else
    indent <<<"$out"
    echo "::error::The signature of $name does not hold for the public key the builds have (TAURI_UPDATER_PUBKEY): that is not the key that goes with TAURI_SIGNING_PRIVATE_KEY"
    exit 1
  fi
  python3 "$here/updater_manifest.py" --tag "$tag" --platform "$platform" --payload "$payload" --sig "$payload.sig" \
    --notes "$notes" --repo "$repo" --pub-date "$pub_date" --output-dir "$dir" | indent
done

# Once more, over what is in the folder.
python3 "$here/release_assets.py" check-manifests --tag "$tag" --repo "$repo" "$dir"/*
echo "Update files for $tag: ${platforms[*]}"
