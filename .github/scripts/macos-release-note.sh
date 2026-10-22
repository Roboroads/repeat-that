#!/usr/bin/env bash
# Adds the paragraph that says macOS is untested to the notes of a release, once. The paragraph starts with the marker
# below; notes that have it already are left alone, so a second run (a re-run of the release, or a late attach after a
# full run) changes nothing. The release-note job of .github/workflows/build-macos.yml runs this.
#
# Needs GH_TOKEN with contents: write (a draft is only visible with write access) and GITHUB_REPOSITORY.
#
# Usage: macos-release-note.sh TAG
set -euo pipefail

if [ $# -ne 1 ] || [ -z "$1" ]; then
  echo "usage: macos-release-note.sh TAG" >&2
  exit 2
fi
tag=$1
repo=${GITHUB_REPOSITORY:?GITHUB_REPOSITORY must name the repository, for example Roboroads/repeat-that}
server=${GITHUB_SERVER_URL:-https://github.com}
marker='<!-- macos-untested -->'

markers() { grep -cF -- "$marker" || true; }

if ! body=$(gh release view "$tag" --repo "$repo" --json body --jq .body); then
  echo "::error::There is no release (published or draft) for $tag."
  exit 1
fi
if [ "$(markers <<<"$body")" -gt 0 ]; then
  echo "The notes of $tag already say that macOS is untested; nothing to do."
  exit 0
fi

notes=$(mktemp)
trap 'rm -f "$notes"' EXIT
{
  if [ -n "$body" ]; then printf '%s\n\n' "$body"; fi
  cat <<EOF
$marker
### macOS

macOS support is new and hasn't been tested on a real Mac yet. The \`.dmg\` is for Macs with Apple Silicon on macOS 14.2 or later. If something doesn't work, please [report it]($server/$repo/issues/new?template=macos.yml).
EOF
} >"$notes"
gh release edit "$tag" --repo "$repo" --notes-file "$notes" >/dev/null

# Read back: the paragraph is there exactly once, after the notes as they were.
after=$(gh release view "$tag" --repo "$repo" --json body --jq .body)
if [ "$(markers <<<"$after")" -ne 1 ]; then
  echo "::error::The notes of $tag should have the macOS paragraph once now, but they don't:"
  echo "$after"
  exit 1
fi
echo "Added the macOS paragraph to the notes of $tag:"
echo
echo "$after"
