#!/usr/bin/env bash
# Writes what the Flatpak build needs besides the repository, next to the manifest:
#
#   cargo-sources.json   every Rust crate of Cargo.lock, as sources for flatpak-builder (flatpak-cargo-generator)
#   node-sources.json    every npm package of app/package-lock.json (flatpak-node-generator)
#   shared-modules/      Flathub's shared-modules at the commit pinned below (the tray library's modules)
#
# None of them is committed: they follow the lock files, so a dependency update never leaves them stale. CI runs this
# before flatpak-builder (.github/workflows/build-flatpak.yml); run it yourself before building by hand:
#
#   packaging/flatpak/generate-sources.sh
#   flatpak-builder --user --install-deps-from=flathub --force-clean --repo=repo build-dir packaging/flatpak/me.roboroads.RepeatThat.yml
#
# With --flathub TAG COMMIT it also writes packaging/flatpak/flathub/, the files for a pull request to Flathub's
# me.roboroads.RepeatThat repository: the manifest building the release TAG (at COMMIT) from GitHub instead of from this
# folder, the sources and flathub.json, and a link to the shared-modules checkout. See docs/MAINTAINING.md.
#
# Needs git, network access and either uv or python3 with venv. The generators run in a virtual environment under
# target/flatpak (or $FLATPAK_WORK), so nothing is installed on the system.
set -euo pipefail

# The tools are pinned by commit, so the sources only change when this file does.
tools_repo=https://github.com/flatpak/flatpak-builder-tools.git
tools_commit=74697c75b630d7330e77250fc13cb5ea688d9479 # 2026-09-30
shared_repo=https://github.com/flathub/shared-modules.git
shared_commit=d1ab132e6480acefc5c510ee13d8d586d36cab19 # 2026-10-21
source_repo=https://github.com/Roboroads/repeat-that.git

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
work=${FLATPAK_WORK:-$root/target/flatpak}

flathub_tag=
flathub_commit=
while [[ $# -gt 0 ]]; do
  case $1 in
    --flathub)
      if [[ $# -lt 3 ]]; then
        echo "usage: generate-sources.sh [--flathub TAG COMMIT]" >&2
        exit 2
      fi
      flathub_tag=$2
      flathub_commit=$3
      shift 3
      ;;
    *)
      echo "usage: generate-sources.sh [--flathub TAG COMMIT]" >&2
      exit 2
      ;;
  esac
done
if [[ -n "$flathub_tag" ]]; then
  if [[ ! "$flathub_tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "error: the tag must look like v0.1.0, got '$flathub_tag'" >&2
    exit 2
  fi
  if [[ ! "$flathub_commit" =~ ^[0-9a-f]{40}$ ]]; then
    echo "error: the commit must be a full 40-character hash, got '$flathub_commit'" >&2
    exit 2
  fi
fi

mkdir -p "$work"

# A clone of $1 at commit $2 in $3, which is reused when it is at that commit already.
checkout() {
  local repo=$1 commit=$2 dir=$3
  if [[ -d "$dir/.git" ]] && [[ $(git -C "$dir" rev-parse HEAD 2>/dev/null) == "$commit" ]]; then
    return
  fi
  rm -rf "$dir"
  git init -q "$dir"
  git -C "$dir" remote add origin "$repo"
  git -C "$dir" fetch -q --depth 1 origin "$commit"
  git -C "$dir" -c advice.detachedHead=false checkout -q FETCH_HEAD
}

echo "Setting up the generators"
checkout "$tools_repo" "$tools_commit" "$work/tools"
if [[ ! -e "$work/venv/.ready" ]]; then
  packages=('aiohttp>=3.9,<4' 'PyYAML>=6,<7' 'tomlkit>=0.13,<1')
  rm -rf "$work/venv"
  if command -v uv >/dev/null; then
    uv venv -q "$work/venv"
    uv pip install -q --python "$work/venv/bin/python" "${packages[@]}"
  else
    python3 -m venv "$work/venv"
    "$work/venv/bin/python" -m pip install -q --disable-pip-version-check "${packages[@]}"
  fi
  touch "$work/venv/.ready"
fi
python="$work/venv/bin/python"

echo "Writing cargo-sources.json"
"$python" "$work/tools/cargo/flatpak-cargo-generator.py" "$root/Cargo.lock" -o "$here/cargo-sources.json"

# npm only reads package-lock.json, so the generator can run with node_modules present or not; it never touches them.
echo "Writing node-sources.json"
(cd "$work/tools/node" && PYTHONPATH="$work/tools/node" "$python" -m flatpak_node_generator \
  npm "$root/app/package-lock.json" -o "$here/node-sources.json")

echo "Checking out shared-modules"
checkout "$shared_repo" "$shared_commit" "$here/shared-modules"

if [[ -z "$flathub_tag" ]]; then
  exit 0
fi

# The files of the Flathub repository: the manifest builds the tag from GitHub. Only the source of the repeat-that module
# differs, so the manifest is copied with the lines between the flathub:begin and flathub:end markers replaced.
out="$here/flathub"
echo "Writing $out for $flathub_tag"
rm -rf "$out"
mkdir -p "$out"
cp "$here/cargo-sources.json" "$here/node-sources.json" "$here/flathub.json" "$out/"
# A link here so the manifest can be checked in place. In the Flathub repository it is a git submodule at the commit
# pinned above: `git submodule add https://github.com/flathub/shared-modules.git shared-modules` and check that commit out.
ln -s ../shared-modules "$out/shared-modules"
awk -v repo="$source_repo" -v tag="$flathub_tag" -v commit="$flathub_commit" '
  /# flathub:begin/ { skipping = 1
    print "      - type: git"
    print "        url: " repo
    print "        tag: " tag
    print "        commit: " commit
    next }
  /# flathub:end/ { skipping = 0; next }
  !skipping { print }
' "$here/me.roboroads.RepeatThat.yml" > "$out/me.roboroads.RepeatThat.yml"
if grep -q 'type: dir' "$out/me.roboroads.RepeatThat.yml"; then
  echo "error: the Flathub manifest still builds from a folder" >&2
  exit 1
fi
echo "Flathub files are in $out; shared-modules goes in as a submodule at $shared_commit"
