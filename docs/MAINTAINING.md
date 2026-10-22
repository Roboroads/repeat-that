# Maintaining

Notes for maintainers. Contributors don't need any of this.

## Branch protection

`main` is protected by the ruleset in [`.github/rulesets/main.json`](../.github/rulesets/main.json):

- no direct pushes, force pushes or deleting the branch
- changes land through a pull request, squash-merged (so history stays linear)
- CI must pass on Linux, Windows and macOS (plus the end-to-end test of the app on Linux), the website checks
  (`Site plan` and `Site build`, see [Website](#when-it-runs)), the PR title must be a Conventional Commit, and all
  review threads must be resolved
- 0 required approvals, so a solo maintainer can merge their own PRs

The repository is public, so GitHub enforces rulesets on it. It is not applied yet, see the
[going public checklist](#going-public-checklist) for when to apply it. To apply it, or to update it after editing the
JSON:

```sh
# first time
gh api -X POST repos/Roboroads/repeat-that/rulesets --input .github/rulesets/main.json

# later updates (look up the id with: gh api repos/Roboroads/repeat-that/rulesets)
gh api -X PUT repos/Roboroads/repeat-that/rulesets/<id> --input .github/rulesets/main.json
```

If you rename a CI job, update `required_status_checks` too, or PRs will wait forever for a check that never runs.

## Merging and releases

PRs are squash-merged only. The repo is set up so the squash commit is just the PR title with no body, so every commit
on `main` is one Conventional Commits line. The `PR title` workflow rejects titles that aren't Conventional Commits.

[release-please](https://github.com/googleapis/release-please) reads those commits on `main` and keeps a release PR
open that bumps the version and updates `CHANGELOG.md`. Merging that PR creates a draft GitHub release, builds the
installers, attaches them and then publishes the release, which is also when its tag appears (see
[Release assets](#release-assets)). Before 1.0, `feat` bumps the patch version and breaking changes (`feat!:`,
`fix!:`) bump the minor version.

The version lives in `Cargo.toml` (`workspace.package.version`, which the app crate and `tauri.conf.json` inherit) and
`app/package.json`, and their lockfiles carry it too. All of them are listed under `extra-files` in
`release-please-config.json`, so the release PR bumps them together: `Cargo.lock` through a filter that picks the
workspace crates (the entries without a `source`), and `app/package-lock.json` at the top and under `packages[""]`.
`.github/scripts/check-release-config.py` checks that the files and the manifest agree. It runs as the `Release config`
job of the `Build Windows installers` workflow whenever a PR touches the release config or the app.

Release PRs are opened with a personal access token, so CI runs on them like on any other PR (see
[Release token](#release-token)).

### Release token

PRs opened with the default `GITHUB_TOKEN` don't start other workflows, so CI would never run on a release PR, and once
the ruleset is active the PR would wait forever for the required checks. The `release-please` workflow therefore passes
a fine-grained personal access token, stored as the `RELEASE_PLEASE_TOKEN` repository secret (Settings → Secrets and
variables → Actions), to the release-please step. Nothing else uses it: the other steps and jobs keep `GITHUB_TOKEN`
with the least permissions they need. If the secret is missing or empty, the workflow fails on its first step with an
error pointing here, instead of falling back to `GITHUB_TOKEN`.

CI therefore runs in full on a release PR, including the Windows installer build: a dry run of the release commit before
it is cut. A newer push to the release PR cancels the build of the older one.

The token that exists now:

- Repository access: only `Roboroads/repeat-that`.
- Permissions: Contents, Pull requests and Issues with read and write, and Metadata with read.
- Created 2026-10-20 and valid for a year, so it **expires around 2027-10-20**. Release PRs are opened as the account
  that owns the token.

From 30 days before the expiry, the `Check the release token` step of every `release-please` run adds a warning to the
run summary (it reads the date from the `github-authentication-token-expiration` header that the API sends with a
token that expires). A warning on a green run is easy to miss, so put the date in a calendar too.

When it expires (or leaks), the `release-please` run on `main` fails in the release-please step with an authentication
error, and no release PR is opened or updated until it is renewed. To renew it:

1. Create a new fine-grained token (Settings → Developer settings → Personal access tokens → Fine-grained tokens) with
   the same repository access and permissions as above, and a new expiry.
2. Replace the secret: `gh secret set RELEASE_PLEASE_TOKEN --repo Roboroads/repeat-that`, then paste the token.
3. Re-run the failed `release-please` run (Actions → release-please) to check that it works, then delete the old token.

## Release assets

[`publish-release.yml`](../.github/workflows/publish-release.yml) owns every release: it builds each platform, checks
the builds, writes one `SHA256SUMS` over all assets, attaches them to the GitHub release and publishes it. The build
workflows, such as [`build-windows.yml`](../.github/workflows/build-windows.yml), only build and upload their files as
`release-<platform>-<file>` artifacts.
[`.github/scripts/release_assets.py`](../.github/scripts/release_assets.py) is the one place that knows the platforms
and makes the decisions. Its unit tests (`test_release_assets.py`) run in every full run of the workflow, and by hand
with `python3 -m unittest discover -s .github/scripts`. The `Workflow scripts` job of `ci.yml` runs every test in that
folder on every pull request and every push to `main`, `test_workflow_secrets.py` among them.

| Platform | Required | Files | Update files (only with an [update key](#update-signing-key)) | Build job in `publish-release.yml` |
|---|---|---|---|---|
| `windows` | yes | `*-setup.exe` | `*-setup.exe.sig`, `latest-windows-x86_64.json` | `windows`, calls `build-windows.yml` |
| `linux` | yes | `*.deb`, `*.rpm`, `*.AppImage` | `*.AppImage.sig`, `latest-linux-x86_64.json` | `linux`, calls `build-linux.yml` |
| `flatpak` | yes | `*.flatpak` | none | `flatpak`, calls `build-flatpak.yml` |
| `macos` | **no** | `*.dmg` | `*.app.tar.gz`, `*.app.tar.gz.sig`, `latest-darwin-aarch64.json` | `macos`, calls `build-macos.yml` |

The update files are what the app's updater reads. A platform doesn't have to produce them to count as built: they exist
only when an update key is set, so a release without a key has none and is still complete. With a key, the check that
a platform is complete also wants its update files, see [Update signing key](#update-signing-key). A platform never
has some of them: it has all of its update files or none.

A required platform must build before a release is published: if one fails, nothing is published and the draft stays.
An optional platform (macOS) that fails or is skipped only warns, in the job summary and as a `::warning::`; the
release goes out without it and the [late attach](#late-macos-attach) adds it later. Jobs that call a reusable
workflow can't use `continue-on-error`, which is why the first step of the `Publish` job reads the result of every
build job and decides itself. To add a platform: add a job named like the platform that `uses:` its build workflow
(with `secrets: inherit` if that workflow reads a secret), add it to the `needs` of `Publish`, and set `build=True` in
`release_assets.py` (it fails the run when the three disagree).

- **Pull requests.** Each build workflow runs its own dry run when a PR touches the app, the crates or the Cargo files.
  A PR that touches the publishing itself (`publish-release.yml`, a build workflow, or `release_assets.py`,
  `sign-updates.sh`, `updater_manifest.py`, `site_release.py` and their tests) also runs `publish-release.yml` as a dry
  run: it builds every platform, runs the script's unit tests and the same checks as a release, and uploads the
  combined `SHA256SUMS` as a workflow artifact. No release is touched.
- **Releases.** release-please creates its GitHub release as a draft. `release-please.yml` calls
  `publish-release.yml` itself when its run created a release, rather than relying on an `on: release` trigger: that
  would fire only once the release is public, and a release that a workflow publishes with `GITHUB_TOKEN` doesn't
  start workflows at all.
- **Draft first.** release-please creates the release as a draft (`"draft": true` in its config), and the `Publish`
  job makes it public only after the files are attached, so there is never a public release without its installers.
  A draft has no tag yet (publishing creates it at the release commit), which is why the call passes the commit as
  `sha`. If a build fails, the draft stays: re-run the failed jobs of that `release-please` run. The release PR is
  already labelled as released, so merging another PR doesn't retry it. A re-run rebuilds every platform.
- **Running it by hand** (Actions → Publish release → Run workflow): leave `tag` empty for a dry run that builds every
  platform and uploads the combined `SHA256SUMS` (the job summary shows it). Give a `tag` to build and attach to that
  release. For a draft, also give the release commit as `sha`, because the tag doesn't exist yet; the draft is
  published when the files are attached.
- **No silent replacement.** The `Check release` job runs first and fails within seconds if the release doesn't exist,
  or if it is public and already has installers: replacing them changes files people may have downloaded and verified
  against `SHA256SUMS`. A draft has no public files, so its assets are overwritten freely. To replace the files of a
  published release on purpose, run the workflow by hand with `replace` ticked.
- **No stale assets.** Every file already on the release must be one this run produces, so that `SHA256SUMS` lists it.
  Anything else fails the run (delete it with `gh release delete-asset <tag> <name>`). The one exception is an
  optional platform's file that this run did not rebuild, such as a `.dmg` attached late earlier: it is kept. Its line
  is carried over from the release's current `SHA256SUMS`, and the downloaded file is checked against that line
  before anything is uploaded.
- **Only successful platforms count.** The files of a platform whose job did not succeed are left out, even if its
  build uploaded them before a later step failed. For an optional platform that means a warning and a release without
  it.
- **Update files.** With an update key, the `Publish` job signs the update file of each platform that built (the
  Windows installer, the AppImage, and for macOS a `.app.tar.gz` of the stapled app that `build-macos.yml` makes, not
  the `.dmg`), checks every signature against the public key, and writes one `latest-<target>-<arch>.json` per platform
  from the release notes. Each of them is written once and never changes, so `SHA256SUMS` only gains lines. The late attach
  does the same for macOS, which adds its own `.app.tar.gz`, `.sig` and manifest. What the job checks, and what a
  release does with and without the key, is under [Update signing key](#update-signing-key).
- **Checked before upload.** `SHA256SUMS` is written over the produced and kept files and verified with
  `sha256sum --check`. The files are uploaded first and `SHA256SUMS` last, in calls of their own, so an upload that
  stops half way never leaves a `SHA256SUMS` that lists files that aren't there. After the upload the release must
  hold exactly those files plus `SHA256SUMS`, and only then is the draft published. If the upload stops, run the
  failed job again (`gh run rerun <run id> --failed`, the log prints the command): it uploads everything again. On a
  published release, run the workflow again with `replace` ticked instead.
- Asset names use hyphens (`Repeat-That_0.1.0_x64-setup.exe`) instead of Tauri's `Repeat That_...`: GitHub rewrites
  spaces in asset names, which would leave `SHA256SUMS` listing names that don't exist.
- **Releases from before a required platform** can't be run again in full mode. `v0.1.0` predates the Linux packages:
  the `linux` job builds the tag's commit, which has neither the Linux bundle settings nor the check scripts, so it
  fails, and with it `Publish`, as Linux is required. A full run always publishes every required platform, so a fix to
  such a release goes out as a new release instead.

### Windows installer

`build-windows.yml` builds the Windows x64 installer: an NSIS `.exe` that installs per user, without administrator
rights. It is the only Windows installer: the bundle targets for Windows are set to `nsis` in
[`tauri.windows.conf.json`](../app/src-tauri/tauri.windows.conf.json), which Tauri merges over `tauri.conf.json` on
Windows, and the build fails if another installer shows up. Install instructions for users are in
[`install-windows.md`](install-windows.md).

On a pull request, the installer is a workflow artifact (kept for 30 days) named `release-windows-<file>`, so a change
can be tried on a real Windows PC before it merges. Artifacts are downloaded as a zip holding the installer. The job
summary shows its checksum. The workflow checks that the app version matches the tag when it is given one.

### Linux packages

[`build-linux.yml`](../.github/workflows/build-linux.yml) builds the x64 `.deb`, `.rpm` and AppImage with tauri-action
in an `ubuntu:24.04` container. Its glibc (2.39) is the oldest of the supported systems (Fedora 40 has the same,
Debian 13 a newer one), so the binary needs nothing newer, and the AppImage carries Ubuntu 24.04's libraries. The
settings are in
[`tauri.linux.conf.json`](../app/src-tauri/tauri.linux.conf.json): the three targets, the dependencies, the desktop
entry and the icons. The files are staged as `Repeat-That_<version>_amd64.deb`, `Repeat-That-<version>-1.x86_64.rpm` and
`Repeat-That_<version>_amd64.AppImage` and uploaded as one `release-linux-<file>` artifact each. Install instructions
for users are in [`install-linux.md`](install-linux.md), and how the packages are put together is under Releases in
[`ARCHITECTURE.md`](ARCHITECTURE.md#releases).

Two jobs check them, and a failure in either fails the `linux` job of a release, which holds the release back:

- `Packages (Linux x64)` runs [`check-linux-packages.sh`](../.github/scripts/check-linux-packages.sh) on the staged
  files: names and versions, the dependencies, the files both packages install, the desktop entries
  (`desktop-file-validate`, the hidden one included), the binary's glibc and libstdc++ needs against what the
  packages ask for (`glibc_max` and `glibcxx_max`), and that the AppImage has
  the real desktop entry at its root and no `libpipewire-0.3` or `libspa-0.2` inside. The job summary shows the result
  and the checksums.
- `Install on <system>` runs [`smoke-linux-package.sh`](../.github/scripts/smoke-linux-package.sh) as root in a fresh
  `debian:13`, `ubuntu:24.04`, `fedora:40` and `fedora:44` container: it installs the `.deb` or the `.rpm` without
  recommended packages, checks `ldd`, runs `repeat-that --rt-self-check` (which loads the binary and Silero and plays
  nothing), validates the installed desktop entries, runs the AppImage's self-check, and uninstalls. It also takes
  the environment the AppImage gives host programs (the hidden `--rt-host-env`), checks that nothing in it points into
  the AppImage, and runs the system's `gio` and `dbus-uuidgen` with it. In `debian:12` it checks that apt refuses the
  `.deb` for its glibc or libstdc++ and nothing else.

On a pull request the packages are workflow artifacts (kept for 30 days), so they can be tried on a real desktop
before the change merges. Artifacts lose the executable bit, so `chmod +x` the AppImage after unzipping it.

**Dependencies.** Tauri adds WebKitGTK, GTK and the AppIndicator library to both packages itself. The config adds what
the binary links on top of that: PipeWire's and ALSA's client libraries. Debian 13 and Ubuntu 24.04 renamed both for the
64-bit `time_t` transition, so the `.deb` names each as an alternative (`libpipewire-0.3-0t64 | libpipewire-0.3-0`);
the `.rpm` names sonames (`libpipewire-0.3.so.0()(64bit)`), like Tauri does for its own. When the binary starts linking
another system library, `ldd` in the install jobs fails on the system that lacks it: add the library to both lists.

**glibc and libstdc++.** The prebuilt ONNX Runtime that `ort` links in statically is built against glibc 2.38 and GCC
13's libstdc++ (it needs `__isoc23_strtol` and GLIBCXX_3.4.31), so both packages ask for those (`libc6 (>= 2.38)` and
`libstdc++6 (>= 13)` on the `.deb`, the matching versioned sonames on the `.rpm`) and Debian 12 can't run the app.
When an `ort` bump needs newer ones, `check-linux-packages.sh` fails with the version the binary needs: raise the
dependencies in `tauri.linux.conf.json`, `glibc_max`/`glibcxx_max` in the script and the test in
`linux_package_tests.rs`, and check that every supported system still has them. Getting Debian 12 back would mean an
ONNX Runtime built for its glibc: Microsoft's own shared library (built on an older glibc), shipped with the packages
and loaded at run time, or one built from source in a `debian:12` container.

**Fedora releases go end of life** about 13 months after they come out, and the mirrors drop them a little later.
Fedora 40, the oldest supported one, has been end of life since May 2025, so its install job (`archive: true` in the
matrix) takes its packages from `archives.fedoraproject.org`, which doesn't change any more. A Fedora job whose
mirrors stop answering falls back to the archive on its own, so a release that goes end of life doesn't block
publishing. When Fedora 44 does, replace it in the matrix by the newest Fedora. Raising the Fedora minimum is a
decision for the README, `install-linux.md` and the matrix together.

**Raising the minimum** (for example to Ubuntu 26.04): change the build container, the install matrix and the systems
named in `install-linux.md`, the README and `ARCHITECTURE.md`. To test another distribution, add it to the matrix of
the `install` job; the script needs `apt-get` or `dnf`.

### Flatpak

[`build-flatpak.yml`](../.github/workflows/build-flatpak.yml) builds the x64 Flatpak, app ID `me.roboroads.RepeatThat`,
from [`packaging/flatpak`](../packaging/flatpak), in Flathub's own build container for the GNOME 51 runtime
(`ghcr.io/flathub-infra/flatpak-github-actions:gnome-51`), and uploads `Repeat-That_<version>_x86_64.flatpak` as the
artifact `release-flatpak-<file>`. Flatpak is a required platform of `publish-release.yml`: a failing step holds the
release back. Install instructions for users are in [`install-linux.md`](install-linux.md#install-the-flatpak).

What is in `packaging/flatpak`:

| File | What it is |
|---|---|
| `me.roboroads.RepeatThat.yml` | The manifest. Runtime `org.gnome.Platform` 51, SDK extensions `rust-stable`, `node22` and `llvm22`. Its `repeat-that` module builds the settings window with Vite and the app with `cargo build --release --offline`, with the identifier of [`tauri.flatpak.conf.json`](../app/src-tauri/tauri.flatpak.conf.json) (`me.roboroads.RepeatThat`, which is also the GTK application ID and the app's single-instance D-Bus name). |
| `me.roboroads.RepeatThat.desktop`, `me.roboroads.RepeatThat.metainfo.xml` | The menu entry and the AppStream data (GPL-3.0-or-later, metadata CC0-1.0, OARS, screenshot). The metainfo's `<releases>` is filled in during the build by `metainfo_releases.py` from `CHANGELOG.md`, for the version in `Cargo.toml`, so release-please needs no change; the build fails when the changelog has no section for that version. |
| `generate-sources.sh` | Writes `cargo-sources.json` (from `Cargo.lock`) and `node-sources.json` (from `app/package-lock.json`) with `flatpak-cargo-generator` and `flatpak-node-generator`, and checks out Flathub's `shared-modules`, all pinned by commit. It also copies the repository's files (what git knows, not what it ignores, so no `target/`, `node_modules` or tool folders) to `source/`, which the manifest builds. None of it is committed, so a Dependabot PR never leaves them stale. `--flathub TAG COMMIT` also writes the files for Flathub, see [Flathub](#flathub). |
| `check_ort_pin.py`, `metainfo_releases.py`, `filter_lint.py` and their `test_*.py` | Small scripts of the build, with unit tests that the workflow runs first (`python3 -m unittest discover -s packaging/flatpak -p 'test_*.py'`). |
| `flathub.json` | Tells Flathub to build for x86_64 only. |

**Offline.** `flatpak-builder` downloads every source first and builds without network. The one prebuilt file is the
static ONNX Runtime that `ort-sys` would download itself (`build/download/dist.tsv` in the crate, for x86_64 Linux),
pinned by URL and sha256 in the manifest and linked through `ORT_LIB_LOCATION`. `check_ort_pin.py` compares the pin with
the `ort-sys` in `Cargo.lock`, so an `ort` update that needs another ONNX Runtime fails the Flatpak build and prints the
URL and hash to put in the manifest. If Flathub refuses a prebuilt binary, the fallback is a module that builds ONNX
Runtime from source (CMake, the version `ort` wants) and points `ORT_LIB_LOCATION` at its output; that is untested.

**Rust.** The workspace names the newest stable (`rust-version`), and Flathub's `rust-stable` extension is usually a
release behind it (1.98 when the workspace asked for 1.99), so the manifest builds with `--ignore-rust-version`. If the
code starts to use something newer than the extension has, the Flatpak build fails to compile: wait for the extension,
or hold that change back.

**Checks.** After the build, [`check-flatpak.sh`](../.github/scripts/check-flatpak.sh) installs the bundle as a user
installation and checks, in the same container:

- the file name and version, and that the permissions are exactly the manifest's (the display, the GPU, the PipeWire
  socket and the bus names of the tray and of KDE's shortcut daemon: no network, no PulseAudio, no home folder);
- the exported desktop entry, metainfo and icons (named after the app ID), `ldd` on the binary and on the tray library,
  and `desktop-file-validate` and `appstreamcli validate` on the exports;
- the self-check (`--rt-self-check`), which proves that the ONNX Runtime linked in works inside the sandbox;
- that the app starts under a virtual display, is still running ten seconds later, and wrote its tray icon to
  `$XDG_RUNTIME_DIR/app/me.roboroads.RepeatThat`, the folder the sandbox shares with the desktop's tray.

Then `flatpak-builder-lint`, Flathub's own linter, runs on the manifest, on the manifest that goes to Flathub and on the
built repository, through `filter_lint.py`, and on the metainfo once its screenshot is on `main` (the linter looks it up
on the network). Three errors are let through until the app is on Flathub: `appid-url-not-reachable`
(`https://roboroads.me` has to answer, see [Flathub](#flathub)), `appstream-screenshots-not-mirrored-in-ostree` and
`appstream-external-screenshot-url` (Flathub fetches the screenshot, which is a link to the main branch, and keeps its
own copy on dl.flathub.org). A fourth,
`appstream-missing-screenshots`, is let through only while the screenshot isn't on `main` yet, on the change that adds it.
Anything else the linter reports fails the build, except in a release run (one with a tag): the linter comes with a
floating container image and looks things up on the network, so a new rule or a network failure must not hold back
Windows and Linux too. There its errors are annotations on the run, and the step doesn't fail the job.

**Not covered by CI.** There is no PipeWire in that container, so capture, replay and ducking inside the sandbox are
tried by hand on a desktop: pick an app, replay it with the hotkey and with `flatpak run me.roboroads.RepeatThat
--replay`, check that the app's volume comes back, turn on Start at login, and `kill -9` the app during a replay. So are
the tray on a real desktop and the Wayland hotkey portal.

To try it without a desktop, start a headless PipeWire with [`start-pipewire.sh`](../.github/scripts/start-pipewire.sh)
in a short folder under `$HOME` (a socket path is limited to 108 bytes), play something into it with `pw-cat`, and run
the app under `xvfb-run` with `XDG_RUNTIME_DIR` set as the script prints, and with `flatpak run
--filesystem=<folder>/run --env=PIPEWIRE_RUNTIME_DIR=<folder>/run --nodevice=dri me.roboroads.RepeatThat`. That is how
the Flatpak was shown to capture, replay and duck an app. `--nodevice=dri` because the DRM device of a virtual machine
can make WebKit hang.

**Building it yourself:**

```sh
packaging/flatpak/generate-sources.sh
flatpak-builder --user --install-deps-from=flathub --force-clean --repo=repo build-dir packaging/flatpak/me.roboroads.RepeatThat.yml
flatpak build-bundle repo Repeat-That.flatpak me.roboroads.RepeatThat --runtime-repo=https://flathub.org/repo/flathub.flatpakrepo
```

The first build takes about 25 minutes (add `--jobs=3` on a small machine: the Rust build is what needs the memory).

**A new GNOME runtime.** Change `runtime-version` in the manifest, the `gnome-NN` tag of the container in
`build-flatpak.yml`, and the branch of the SDK extensions if the freedesktop base changed (GNOME 51 uses 26.08, which
`flatpak install` picks by itself). Flathub drops old runtimes about a year after the release.

### macOS installer

`build-macos.yml` builds the Apple Silicon `.dmg` on `macos-latest` with tauri-action and `--target
aarch64-apple-darwin`. Tauri takes its macOS settings from
[`tauri.macos.conf.json`](../app/src-tauri/tauri.macos.conf.json) (the `dmg` target only, macOS 14.2 as the minimum, the
hardened runtime and [`Entitlements.plist`](../app/src-tauri/Entitlements.plist)) and merges
[`Info.plist`](../app/src-tauri/Info.plist) into the app's. The file is staged as `Repeat-That_<version>_aarch64.dmg`
(hyphens, for the reason given under [Release assets](#release-assets)) and uploaded as `release-macos-<file>`. There is
no Intel or universal build: see [ONNX Runtime on macOS](#onnx-runtime-on-macos). The user-facing steps are in
[`install-macos.md`](install-macos.md), and how the build is put together is under Releases in
[`ARCHITECTURE.md`](ARCHITECTURE.md#releases).

The `Mode` job runs first and picks one of three modes, which the run's summary names:

| Mode | How to start it | What you get |
|---|---|---|
| Dry run | a pull request, or Actions → Build macOS installers → Run workflow on a branch other than `main`, no tag | an ad hoc signed `.dmg` that isn't notarized, from a job with no secrets. Open it with right-click → Open. |
| Signed dry run | Actions → Build macOS installers → Run workflow on `main`, no tag | a signed, notarized `.dmg` as a workflow artifact, to check that the secrets and Apple's service work |
| Release | called by `publish-release.yml`, or Run workflow on `main` with the release `tag` | the same, plus the version check and the "untested" paragraph in the release notes |

Only the signed modes use the `apple-signing` Environment, so only they see the secrets, and only runs on `main` get
them. A signed build also refuses a commit that isn't on `main`, since it runs that commit's build scripts with the
secrets in reach. A run with a tag on any other branch fails at once.

Every run ends with `check-macos-bundle.sh` on the finished `.dmg` and prints its `ok:` and `FAIL:` lines in the job
summary; the list of checks is under Releases in [`ARCHITECTURE.md`](ARCHITECTURE.md#releases). It needs a Mac to run,
so it can't be tried locally without one. The `Notarize and staple the .dmg` step prints Apple's answer; when the status
isn't `Accepted` it also prints `notarytool log`, which says what Apple objected to. If the secrets are missing, the
`Check the signing secrets` step names the ones that are empty. If it names all six while the Environment has them, the
build was called by another workflow that didn't pass them on. A called workflow only sees the secrets its caller
passes, Environment secrets included, so every job that calls `build-macos.yml` (and the job in `release-please.yml`
that calls `publish-release.yml`) has `secrets: inherit`. `.github/scripts/test_workflow_secrets.py` checks every call,
in the `Workflow scripts` job of `ci.yml`.

While the repository variable `TAURI_UPDATER_PUBKEY` is set, and in every dry run, the build also packs the `.app` of the
`.dmg` as `Repeat-That_<version>_aarch64.app.tar.gz` (the update bundle), checks it with `check-macos-bundle.sh
--update-bundle` and uploads it as `release-macos-<file>`. It doesn't sign it: `publish-release.yml` does, see
[Update signing key](#update-signing-key). The check of the `.dmg` expects `"updater":true` in the self-check exactly
when the variable is set.

**The release note.** In release mode a `Release note` job runs after a successful build and adds a paragraph to the
release notes: macOS hasn't been tested on a real Mac, plus a link to the macOS issue form. It starts with the marker
`<!-- macos-untested -->` and is added once, so running it again, or the late attach after a full run, changes nothing.
It never runs for a build that failed, so a release without a `.dmg` has no macOS paragraph.

**Removing the "untested" notices** is a follow-up that waits until someone has confirmed on a Mac that it works. They
are in:

- `README.md` (the status block, Install (macOS), the platform table and the macOS note), `docs/macos.md`,
  `docs/install-macos.md`, `docs/ARCHITECTURE.md` (the app section and the build modes), `CONTRIBUTING.md` (Testing on
  real hardware) and `ROADMAP.md` (the milestone 6 heading and intro);
- the first lines of `.github/ISSUE_TEMPLATE/macos.yml`;
- the settings window: `app/src/lib/components/MacosNotice.svelte`, its test `MacosNotice.test.ts`, its use in
  `GeneralSection.svelte`, and the entry for it in the links test in `app/src-tauri/src/lib.rs`;
- the release notes: the `Release note` job of `build-macos.yml` and `macos-release-note.sh`;
- the website: the notice in `site/src/components/DownloadCard.astro`, the note under the hero button in
  `site/src/pages/index.astro`, the browser test of that notice in `site/tests/e2e/all.spec.ts`, and the quotes of the
  README in `site/tests/fixtures/readme-requirements.md` and `site/tests/unit/requirements.test.ts`. The site also
  renders `docs/guide.md`, which has the notice too.

`grep -rniE "untested|on a real Mac"` finds them (the comment in `Entitlements.plist` matches too; it is not a notice).

**When the macOS job fails in a release.** The required platforms still publish, without a `.dmg` and without its line
in `SHA256SUMS`. Fix the cause (usually the Apple side: a lapsed membership, a revoked key, or the notary service being
down), then run Build macOS installers on `main` with the tag, and attach the result with the
[late attach](#late-macos-attach) below.

### ONNX Runtime on macOS

On macOS the speech detector's ONNX Runtime is linked in, as on Windows and Linux: `ort` (pinned with `=` in
`crates/rt-core/Cargo.toml`) downloads its prebuilt static library for `aarch64-apple-darwin` at build time. There is
no fetch script, no library of ours to pin or checksum, and nothing to bundle, sign or list under licenses.

When you bump `ort`, watch the macOS jobs. The `Rust (macos-latest)` check fails to link if the new `ort` has no
`aarch64-apple-darwin` prebuilt. The macOS build's `--rt-self-check` step fails unless Silero loads, in the ad hoc mode
too, so it also catches a runtime that links but doesn't run.

Intel Macs are out because of the table behind this: `ort` has no prebuilt for `x86_64-apple-darwin`, and Microsoft
publishes no x86_64 or universal macOS ONNX Runtime after 1.23.2 (`ort` 2.0.0-rc.15 needs 1.31). If either changes, an
Intel or universal build needs a second target in `build-macos.yml`, `lipo` checks in `check-macos-bundle.sh`, and a
new file name, which `release_assets.py` would have to know.

### Late macOS attach

macOS is optional, so a macOS build or notarization that fails doesn't hold back the required platforms. The late attach
adds the `.dmg` to a release that is already out (or still a draft), and extends its `SHA256SUMS` by one line. With an
[update key](#update-signing-key) it adds the macOS update files too (the `.app.tar.gz`, its `.sig` and
`latest-darwin-aarch64.json`), and `SHA256SUMS` gains a line for each. It is the `late-attach` mode of
`publish-release.yml`, and it takes the files from a macOS build run, in two steps:

1. Actions → Build macOS installers → Run workflow on `main`, with the release `tag` (for a draft, also its commit as
   `sha`). It builds, signs and notarizes the `.dmg`, uploads `release-macos-*` and adds the "untested" paragraph to
   the release notes. While the repository variable `TAURI_UPDATER_PUBKEY` is set it also packs the update bundle, the
   `.app.tar.gz`, which is an artifact of its own.
2. Once that run has succeeded: Actions → Publish release → Run workflow on `main`, with `mode` `late-attach`, the
   `tag`, `platform` `macos` and `artifacts_run_id` set to the number in the URL of the macOS build run
   (`.../actions/runs/<number>`). The build run's summary prints the matching `gh workflow run` command. With the
   variable set the job uses the `updater-signing` Environment, which only `main` may use.

The build can't call `publish-release.yml` itself: that one calls the build for a release, and GitHub refuses workflows
that call each other in a loop. Artifacts are kept for 30 days, so after that the build has to run again. The build
run is trusted with the release's files, so it is checked first: it must be a run of `build-macos.yml` that was started
by hand (a pull request build is not notarized), that succeeded, on the default branch of this repository (not a fork),
and its commit must be the tag's commit or one of its ancestors (for a draft, the commit the draft targets).

What it does, in order, stopping at the first problem:

1. The release must exist (published or draft) and `platform` must be an optional platform. With `artifacts_run_id`,
   the earlier run is checked as described above.
2. With the variable set, it signs the `.app.tar.gz` and writes `latest-darwin-aarch64.json`, the way `Publish` does
   (see [Update signing key](#update-signing-key)). If the build run made no `.app.tar.gz`, because its `.dmg` was built
   before the variable was set, the job stops here with "no update file ... build the platform again". Run Build macOS
   installers again on `main` and use the number of that run. Without the variable the late attach adds the `.dmg`
   only, and says so in a notice.
3. The downloaded files must all belong to that platform, and their names must carry the tag's version
   (`Repeat-That_0.2.0_aarch64.dmg` for `v0.2.0`). A file of another platform or another version fails. With the
   variable set, the platform's update files must all be there.
4. Every file on the release must be listed in its current `SHA256SUMS`, and `SHA256SUMS` must list nothing that is
   missing from the release. Without `replace`, the release must not have a file for that platform yet.
5. All files on the release are downloaded. The new `SHA256SUMS` is the old one plus the new lines, and every line
   must hold for the file next to it, so it extends a consistent base. Lines already in it never change; the job fails
   otherwise.
6. With `replace`, every file of the platform on the release is replaced or, when its name differs from the new
   file's, deleted, so the release never carries two `.dmg` files. Only their lines change or go.
7. The new files are uploaded first (the `.dmg` and the update files), then `SHA256SUMS` (replacing the old one), then
   the files that leave the release are deleted, so `SHA256SUMS` never lists a file that isn't there yet. `SHA256SUMS`
   is the only file the late attach may replace on a published release. Right before replacing it, the job checks that
   nobody changed it since it was read; if so, it stops instead of dropping that change.

**If the job stops half way.** The most likely case is a `.dmg` that is uploaded while `SHA256SUMS` is not. Run the
failed job again (`gh run rerun <run id> --failed`; the log prints the command and the new `SHA256SUMS`). A `.dmg`
that is on the release but not in `SHA256SUMS`, and identical to the one just built, is taken as the leftover of that
stop: it isn't uploaded again, and the job finishes by replacing `SHA256SUMS`. A `.dmg` that differs has to be deleted
first (`gh release delete-asset <tag> <name>`) or replaced by running again with `replace` ticked. The same rerun
finishes a `replace` that stopped before the old file was deleted.

The update files are the exception. The `.app.tar.gz` is the same file in a rerun, but a rerun signs it again, and a
signature carries a timestamp, so the new `.sig` and `latest-darwin-aarch64.json` differ from the ones the stopped
attach left on the release. The rerun refuses them. Delete those two with `gh release delete-asset`, or run again with
`replace` ticked.

A late attach runs in a concurrency group of its own, so it is never queued behind another run or dropped when
another one is waiting (GitHub keeps only one waiting run per group). That also means it doesn't wait for a full run
of the same tag: don't start one while the other is still going. If the release changes in between, the checks
before and after the upload fail the job instead of leaving wrong files.

The late attach never changes whether the release is a draft or published: on a draft, a full run does the publishing.
Running it twice fails because the `.dmg` already exists, unless `replace` is ticked. A full run after a late attach
(for example a re-run of the release) keeps the `.dmg` and its line when macOS isn't rebuilt.
[`verify-downloads.md`](verify-downloads.md) tells users to download `SHA256SUMS` again when theirs has no `.dmg` line.

### macOS capture test

[`macos-capture.yml`](../.github/workflows/macos-capture.yml) runs `crates/rt-capture/tests/macos_audio.rs` on a
`macos-latest` runner: `afplay` plays synthetic tones and noise into the BlackHole 2ch virtual device (installed on the
runner only, GPL-3.0, never shipped), and the tests list, capture, duck and mute that player and record what the device
plays from its input. All audio stays in memory. The summary shows the captured level, the click check figures and the
time it took to get the app back after a `kill -9`.

Nobody can click the permission dialog on a runner. `macos-ci-audio.sh` therefore first runs the test without the
permission, while nothing has been asked yet, and then writes the grant into the runner's TCC databases. That worked on
the first runs: the status went from `NotAsked` to `Allowed`. The test without the permission passed before the grant
(it skips itself once capture is allowed), and the other six ran after it with `RT_REQUIRE_TCC=1` and passed. If a
future runner image stops accepting the grant, the script reports `tcc=missing`, only the test without the permission
runs, and the summary says that the others were skipped. That is a warning and not a failure, so read the summary now
and then.

The test binary is unsigned and runs outside the hardened runtime, so this doesn't cover the real permission dialog, the
System Settings pane, or the notarized app. The
[checklist for people with a Mac](install-macos.md#testing-checklist-if-you-have-a-mac) does.

The check isn't required yet: it isn't in `required_status_checks` in
[`.github/rulesets/main.json`](../.github/rulesets/main.json), because it has to prove it runs reliably first. To
require it later, add its name, `macOS capture`, to that list and apply the ruleset again (see
[Branch protection](#branch-protection)). The workflow only runs for changes to the capture and core crates, the Cargo
files and its own files, so first take the `paths` filter off its `pull_request` trigger: a required check that doesn't
run leaves every other PR waiting for it.

### Version numbers

release-please picks each version from the commits since the last release (the rules are under
[Merging and releases](#merging-and-releases)). The config has no `release-as` pin: the one for 0.1.0 was removed
after that release. Add a pin only to force a version the commits wouldn't give, and remove it as soon as that release
exists. The `release-please` workflow runs the config check with `--tags` before it acts (the script explains why).
While the pinned version already has a tag, that run fails and no release PRs are opened.

Keep versions and tags plain `X.Y.Z` / `vX.Y.Z`, without a pre-release suffix like `0.2.0-rc.1`: the config check
rejects any other `release-as`, and the release workflows reject other tags.

### Signing

The Windows installer isn't code-signed, so SmartScreen warns on first run (the install guide explains the
click-through). Signing through SignPath is on the checklist below. The macOS `.dmg` is signed and notarized, see
[macOS signing and notarization](#macos-signing-and-notarization). The files the app's updater downloads are signed
with a key of their own, the update signing key. It has nothing to do with the Apple certificate or with SignPath: see
[Update signing key](#update-signing-key).

## Website

The site at <https://repeat-that.roboroads.me> is built from [`site/`](../site) with Astro and deployed to GitHub Pages
by [`site.yml`](../.github/workflows/site.yml). [`site/README.md`](../site/README.md) has the commands and says what a
build reads. The site renders the Markdown in `docs/` and in the repository root, and writes the download links of the
latest release into the pages when it is built, so a visitor's browser contacts no other site.

### When it runs

`site.yml` runs on:

- a push to `main` that touches `site/`, `docs/`, a Markdown file in the root, `app/src-tauri/icons/svg/app.svg`, the
  release data script (`site_release.py`, its tests and `release_assets.py`) or the workflow itself
- every finished run of `release-please` or `Publish release` on `main`. A release can't start a workflow
  (`publish-release.yml` publishes it with `GITHUB_TOKEN`), so this is how a new release gets onto the site, and how the
  `.dmg` of a [late attach](#late-macos-attach) does. `release-please` runs on every push to `main`, so most of these
  runs find nothing new and stop early, see below
- a schedule, every day at 04:17 UTC, which also stops early when there is nothing new
- `gh workflow run site.yml`, for example after uploading the introduction video

Every pull request gets the `Site plan` job, which looks at what the PR changes. When it touches those paths, `Site build`
runs the build and the tests, and nothing is deployed. When it doesn't, the build is skipped, and a skipped job counts as
passed. Both are required checks in [`.github/rulesets/main.json`](../.github/rulesets/main.json). `ci.yml` counts
`site/` as documentation, so nothing else looks at a PR that only changes the site, and the workflow has no `paths`
filter on pull requests for the same reason `ci.yml` has none: a required check that doesn't run leaves the PR waiting.
Like the rest of the ruleset, the two checks count once it is applied (see [Branch protection](#branch-protection)).

A run reads the latest release and its `SHA256SUMS` and turns them into the download data with `site_release.py`
(the `Site plan` job). The update files of a release (the `.sig` files, the `latest-*.json` manifests and the macOS
`.app.tar.gz`) have lines in `SHA256SUMS` like the installers, but the script leaves them out of the download data
without a warning: they are for the app's updater, not for people.

The run then fetches the introduction video if there is one, and runs the type check, the unit tests, the build,
`npm run check:dist` and the browser tests. It uploads `site/dist`, checks that the upload kept the hidden files
(`.nojekyll` and `.well-known/security.txt`), deploys, and asks the live site for its front page, `security.txt` and
`build-info.txt`.

**Stopping early.** The build puts `build-info.txt` in the root of the site: a hash of the files the site is built from,
a hash of the download data, a hash of the files on the `site-media` release, and the build date. A run started by
`release-please`, `Publish release` or the schedule fetches the live file first. When it says the same as this run would
build and is less than 60 days old, the run ends after the plan job with a note in its summary, and nothing is built or
deployed. So a new release (or a `.dmg` from a late attach), a changed introduction video and a change under the site
paths get built, and the rest of the pushes to `main` and most of the daily runs don't create a Pages deployment. A push
that touches the site paths and `gh workflow run site.yml` always build. While the site isn't live yet, every run
builds.

When the release data doesn't add up (an asset without a line in `SHA256SUMS`, a line for a file that isn't on the
release, a malformed line), the build fails with an error that names the problem and the previous deploy stays up. The
next trigger tries again. Deploys run one at a time, a deploy in progress is never cancelled, and of the runs waiting
behind it only the newest is kept. Dependabot opens one npm PR a week for `site/`.

### Turning on GitHub Pages and the domain

Until this is done the Deploy job ends with the warning "GitHub Pages is off" and everything else passes. These steps
are the maintainer's:

1. At the provider of `roboroads.me`, add a `CNAME` record for `repeat-that` that points to `roboroads.github.io.`.
2. Recommended: verify `roboroads.me` under the GitHub account's Settings → Pages → Add a domain. GitHub asks for a TXT
   record named `_github-pages-challenge-Roboroads.roboroads.me`. With it, nobody else can claim the subdomain if the
   `CNAME` record is ever left pointing at nothing.
3. In the repository, Settings → Pages: Source is GitHub Actions, the custom domain is `repeat-that.roboroads.me`, and
   Enforce HTTPS can be ticked once the certificate has been issued. From the command line:

   ```sh
   gh api -X POST repos/Roboroads/repeat-that/pages -f build_type=workflow
   gh api -X PUT repos/Roboroads/repeat-that/pages -f cname=repeat-that.roboroads.me
   gh workflow run site.yml
   ```

`site/public/CNAME` holds the domain as well. GitHub ignores that file when Actions does the deploy, so it only records
where the site lives, and `check:dist` fails the build if it goes missing. If Pages is on but its source is a branch,
the Deploy job fails and says so.

The last step of a deploy, "Check the live site", needs the custom domain. The site is built for the base path `/`,
which doesn't work at `roboroads.github.io/repeat-that/`, so on that address the step warns and stops.

### Topics and the social preview

```sh
gh repo edit Roboroads/repeat-that --homepage https://repeat-that.roboroads.me \
  --add-topic accessibility,adhd,audio,voice-chat,discord,teams,tauri,rust
```

The social preview has no API. Upload `site/public/og.png` (1280 by 640) under Settings → General → Social preview.
`site/README.md` says how that picture is made.

### Introduction video

The site shows this video under the heading "Watch the introduction". The files keep the names `demo.mp4`,
`demo-poster.jpg` and `demo.vtt`. The video is played from the site's own domain. Its files sit on a prerelease called
`site-media`, and the deploy copies them into the site. They can't be on a version release, because
`publish-release.yml` refuses files it didn't build. release-please ignores the `site-media` tag: it only reads tags
that look like `X.Y.Z`, and `check-release-config.py --tags` only looks up `v<release-as>`. A prerelease is never the
"latest" release either, so the download data doesn't change.

```sh
gh release create site-media --prerelease --title "Website media (not a release)" \
  --notes "Files for the website, not an app release." demo.mp4 demo-poster.jpg demo.vtt

# later, to replace a file
gh release upload site-media demo.mp4 --clobber

gh workflow run site.yml
```

`demo.mp4` is the video (H.264 and AAC, ideally under 20 MB). `demo-poster.jpg` and the captions in `demo.vtt` are
optional. Without `demo.mp4` the site has no video section. The player has `preload="none"`, so nothing is fetched until
someone presses play. The video never contains audio captured from someone else's call, see [ROADMAP.md](../ROADMAP.md).

### The well-known folder and security.txt

Everything in `site/public/.well-known/` is published as it is, under `https://repeat-that.roboroads.me/.well-known/`.
Jekyll doesn't run, because the site goes up as an artifact, and both `check:dist` and the check after the upload fail
the build if a hidden file is missing.

The build writes `.well-known/security.txt` (RFC 9116) itself. `Contact` is private vulnerability reporting, `Policy` is
the security policy, `Canonical` is the file's own address and `Expires` is the build date plus 180 days. The daily run
builds again once the live site is 60 days old, which keeps it current. GitHub switches scheduled workflows off after 60 days without activity in the repository, and then the
file runs out 180 days after the last build. `gh workflow enable site.yml` and `gh workflow run site.yml` fix that.

### Flathub and this site

Flathub verifies the Flatpak `me.roboroads.RepeatThat` at `https://roboroads.me/.well-known/org.flathub.VerifiedApps.txt`.
The domain comes from the app ID, so it is the bare `roboroads.me` and not this site. A token placed in
`site/public/.well-known/` would be served at `repeat-that.roboroads.me`, where it verifies nothing; host it wherever
`roboroads.me` itself is served. The Tauri identifier stays `me.roboroads.repeat-that`, and only the Flatpak uses
`me.roboroads.RepeatThat`.

## SignPath Foundation application

Apply once the site is live: the application asks for a project website, and the policy page it points at is
[`code-signing.md`](code-signing.md), served at `https://repeat-that.roboroads.me/code-signing/`. SignPath's
[terms](https://signpath.org/terms) are the authority, so read them again before applying. They ask for these, and this
is where Repeat That stands:

- an OSI-approved license: GPL-3.0-or-later
- a project that has a release and is maintained: `v0.1.0` is out
- two-factor authentication on every account that touches the project: turn it on for the GitHub account, and for the
  SignPath account once there is one
- a code signing policy and a privacy statement on the website: the policy page, and its Privacy section
- builds from the repository's source on a CI service: GitHub Actions on GitHub-hosted runners, from release tags

Draft of the project description for the form (change it to fit the fields):

> Repeat That is a desktop app that keeps the last seconds of speech from one application in memory and replays them on
> a hotkey, for people who lose the thread during calls. Repository: https://github.com/Roboroads/repeat-that
> (GPL-3.0-or-later). Website: https://repeat-that.roboroads.me. Code signing policy:
> https://repeat-that.roboroads.me/code-signing/. The first release, v0.1.0, is on GitHub. We want to sign the Windows
> installer (`Repeat-That_<version>_x64-setup.exe`) and `repeat-that.exe`. GitHub Actions builds both on GitHub-hosted
> runners from a release tag (`publish-release.yml`). The project has one maintainer, who is its only committer,
> reviewer and approver. The app sends nothing over the network unless the user asks it to, and captured audio stays in
> memory.

When SignPath accepts the project, a follow-up adds the signing step to the Windows build and changes the code signing
page and the SmartScreen notes (`grep -rniE "smartscreen|code-signed"` finds them).

## Going public checklist

- [x] Make the repository public (2026-10-20)
- [ ] Apply the branch ruleset (see above), but only once the `Frontend` CI job (added by #9) is on `main`. The
  release-please token (`RELEASE_PLEASE_TOKEN`) is done, see [Release token](#release-token). Applied earlier, PRs wait
  forever for a check that never runs.
- [x] Turn on private vulnerability reporting (Settings → Security) and add a `SECURITY.md`
- [x] Turn on Dependabot alerts and security updates (version updates are already set up in `.github/dependabot.yml`)
- [ ] Publish the GitHub Pages site (milestone 8 in the roadmap; on a free plan, Pages needs the public repo). The site
  and its workflow are on `main`; what is left is the DNS record, the Pages source and the custom domain, see
  [Website](#website). The topics and the social preview below are under it too
- [ ] Apply to [SignPath Foundation](https://signpath.org) for free Windows code signing. The Pages site has to be live
  first, because SignPath asks for a project website and a public repository. The prerequisites and a draft of the
  application are under [SignPath Foundation application](#signpath-foundation-application)
- [ ] Add topics and a social preview image so people can find the project
- [ ] Submit the Flatpak to Flathub and host the verification token (see [Flathub](#flathub))
- [ ] When the app is on Flathub, change `FLATPAK_REASON` in `app/src-tauri/src/updater/mode.rs` and the test beside it.
  Until then it sends Flatpak users to the GitHub releases page, because the Flatpak only comes from there

## Flathub

The Flatpak on Flathub is a repository of its own, `flathub/me.roboroads.RepeatThat`, whose manifest builds a release tag
of this repository. The build and the files for it are in this repository (see [Flatpak](#flatpak)); sending them to
Flathub and hosting the verification token are for the maintainer to do. Flathub's own pages are the authority, and they
change: [submission](https://docs.flathub.org/docs/for-app-authors/submission) and
[verification](https://docs.flathub.org/docs/for-app-authors/verification).

### Before submitting

- A release exists (a tag `vX.Y.Z`), Flathub builds from a tag, and `CHANGELOG.md` has its section (the metainfo's
  release notes come from it).
- `https://roboroads.me` answers over HTTPS. The app ID `me.roboroads.RepeatThat` is the reverse of the domain
  `roboroads.me`, which Flathub checks (`appid-url-not-reachable` in the linter) and uses to verify that the app is yours.
- The screenshot is on `main`: the metainfo links to `packaging/flatpak/screenshots/settings.png` there, and Flathub
  fetches it when it builds. Retake it (a Flatpak under a virtual display shows the window) when the window changes.
- The Flatpak works on a real desktop: pick an app and replay it, the hotkeys on your session, the tray, Start at login,
  and the volume coming back after `kill -9` during a replay, see [Not covered by CI](#flatpak).

### Submit

1. Make the files, from a checkout of the release tag:

   ```sh
   packaging/flatpak/generate-sources.sh --flathub vX.Y.Z "$(git rev-parse vX.Y.Z^{commit})"
   ```

   `packaging/flatpak/flathub/` then has the manifest (building that tag and commit from GitHub), `cargo-sources.json`,
   `node-sources.json` and `flathub.json`. The script prints the commit of Flathub's `shared-modules` that the manifest
   was made with.
2. Fork [`flathub/flathub`](https://github.com/flathub/flathub) (all branches, not only `master`), and clone the
   `new-pr` branch of your fork. Start a branch from it: `git checkout -b add-repeat-that new-pr`.
3. Copy the four files into the root of that branch and add the shared modules as a submodule at the commit the script
   printed:

   ```sh
   git submodule add https://github.com/flathub/shared-modules.git shared-modules
   git -C shared-modules checkout <commit>
   ```

   Commit, push, and open a pull request against the **`new-pr`** branch of `flathub/flathub`, titled
   `Add me.roboroads.RepeatThat`. Fill in its checklist.
4. A Flathub bot builds it when a reviewer (or you) comments `bot, build`. Answer the reviewer's questions with the notes
   below, and push changes to the same branch. When it is merged, Flathub creates `flathub/me.roboroads.RepeatThat`
   and invites you to it; accept the invitation soon, it expires.

**Notes for the reviewer** (the same as the comments in the manifest):

- *What it is.* Repeat That keeps the last seconds of speech from one app the user picks in memory and replays it on a
  hotkey. GPL-3.0-or-later, built from the release tag. The Rust and npm sources are generated from the lock files with
  `flatpak-cargo-generator` and `flatpak-node-generator`; the build has no network.
- *`--filesystem=xdg-run/pipewire-0`.* The app needs PipeWire's socket to list the apps that play sound, link the picked
  app's output ports to its capture stream, lower that app's volume while a replay plays (and give it back), and play the
  replay. It connects as a manager (`media.category = Manager`), which WirePlumber's default access rules give the right
  to link streams and set volumes, as for the other PipeWire graph tools on Flathub (Helvum, qpwgraph). No portal
  captures one app's audio or sets another app's volume. The app never links a microphone or any other source (a test
  checks it). No PulseAudio socket is asked for.
- *`--talk-name=org.kde.StatusNotifierWatcher`.* For the tray icon. The icon file goes to
  `$XDG_RUNTIME_DIR/app/me.roboroads.RepeatThat`, which the tray can read.
- *`--talk-name=org.kde.kglobalaccel`.* KDE's shortcut daemon, for a cosmetic fix. Plasma 6.3's GlobalShortcuts portal
  (Debian 13 has 6.3.5) lists the app's shortcuts in System Settings under the Flatpak ID instead of "Repeat That", and
  the app gives the component its name through kglobalaccel (`allActionsForComponent` and `doRegister`, for its own
  component only). The sandbox hides the name without the permission (`NameHasOwner` answers false, a call fails with
  `ServiceUnknown`), so there is no other way. Plasma 6.4 and later read the name from the desktop file, and the
  permission can be dropped when 6.3 is out of use. Other Flathub apps ask for it for their global shortcuts (Strawberry,
  and KDE's own apps).
- *No network and no filesystem access.* The app has no network code; audio never leaves memory. Settings and the duck
  journal (the volumes to give back after a crash) are in the app's own folders.
- *Start at login* goes through the Background portal (`RequestBackground`), and so does the permission to run in the
  background, which the app asks for at start because it lives in the tray without a window. The Wayland hotkeys go
  through the GlobalShortcuts portal.
- *The prebuilt ONNX Runtime.* The speech detector (Silero VAD) runs on ONNX Runtime, linked statically from the file that
  the `ort` crate downloads for x86_64 Linux. The manifest pins its URL and sha256, and CI checks the pin against
  `Cargo.lock`. If Flathub wants it built from source, see the fallback in [Flatpak](#flatpak).
- *`--ignore-rust-version`.* The `rust-stable` extension is a release behind the Rust the workspace names; the code
  builds with it.
- *The linter's `appid-url-not-reachable`* is the domain (above), until it answers.

### Verify the app

When the app is on Flathub, the developer page on flathub.org (log in with GitHub) shows a token for verifying it as
`me.roboroads.RepeatThat`. Serve it as plain text at `https://roboroads.me/.well-known/org.flathub.VerifiedApps.txt`, and
check it on that page. Verification shows the maintainer as the developer on the app's page.

### Updating on each release

After a release is published:

1. In a checkout of the new tag, run `packaging/flatpak/generate-sources.sh --flathub vX.Y.Z "$(git rev-parse vX.Y.Z^{commit})"`.
2. In your clone of `flathub/me.roboroads.RepeatThat`, start a branch, replace the manifest, `cargo-sources.json` and
   `node-sources.json` with the new ones (and move the `shared-modules` submodule to the commit the script printed, if it
   changed), commit and open a pull request. The bot builds it; merge it when it is green, and Flathub publishes the
   new version.
3. If the window changed, retake the screenshot first (it is in this repository, so it ships with the tag).

Nothing here is automatic on purpose: a new release changes the lock files, so the generated sources change with it. The
CI build of this repository already builds the same manifest from the same files on every pull request, so a release
that built here builds on Flathub too, apart from the changes to Flathub itself.

## macOS signing and notarization

macOS releases are signed with a Developer ID Application certificate and notarized with Apple's `notarytool`.
[`build-macos.yml`](../.github/workflows/build-macos.yml) does both: tauri-action signs the app and notarizes it, which
reads the variables below, and a later step notarizes the `.dmg` with the same API key. They are needed before the first
macOS release, not before the repository goes public. The names come from the Tauri
[macOS code signing guide](https://v2.tauri.app/distribute/sign/macos/) and
[environment variable reference](https://v2.tauri.app/reference/environment-variables/).

Both need a paid Apple Developer Program membership, renewed every year. If it lapses, the team loses access to the
Apple notary service for Mac software, to Certificates, Identifiers & Profiles and to the App Store Connect API
([Apple: resolving access issues](https://developer.apple.com/help/account/access/resolving-access-issues/)), so
releases can't be notarized until the Account Holder renews it. Apps that are already out keep running
([Apple: Developer ID certificates](https://developer.apple.com/support/developer-id/)).

When you create the certificate and Apple asks for the intermediary, pick G2 Sub-CA. Certificates from the original
Developer ID Sub-CA stop working on 1 February 2027
([Apple news](https://developer.apple.com/news/?id=w4atic4c)).

Notarization uses an App Store Connect API key. That is the recommended route, since it doesn't depend on an Apple ID
password or two-factor prompts. Create the key under Users and Access → Integrations in App Store Connect with Developer
access. Apple lets you download the `.p8` file once.

Secrets of the `apple-signing` Environment, so pull request builds never get them. To create it: Settings → Environments
→ New environment → `apple-signing`, then under Deployment branches and tags choose Selected branches and tags and add
`main`. Add the six values below as Environment secrets, not as repository secrets. Only the signed modes of the build
name the Environment, and `main` is the only branch that may use it:
- `APPLE_CERTIFICATE`: the Developer ID Application certificate exported from Keychain Access as a `.p12`,
  base64-encoded as in the Tauri reference: `openssl base64 -A -in MyCertificate.p12 -out MyCertificate-base64.txt`.
- `APPLE_CERTIFICATE_PASSWORD`: the password set when exporting the `.p12`.
- `APPLE_SIGNING_IDENTITY`: the certificate name from `security find-identity -v -p codesigning`, for example
  `Developer ID Application: Your Name (TEAMID)`.
- `APPLE_API_ISSUER`: the Issuer ID shown above the keys table.
- `APPLE_API_KEY`: the Key ID of the key.
- `APPLE_API_KEY_CONTENT`: the full text of the `.p8` file. The name is ours, Tauri doesn't read it.

Set in the workflow, not a secret:
- `APPLE_API_KEY_PATH`: the path to the `.p8` file, which Tauri reads from disk. In `build-macos.yml` a step before the
  tauri-action step writes the secret to a file and exports the path for the later steps (the `.dmg` notarization uses
  it too):

```yaml
- name: Write the App Store Connect API key
  shell: bash
  env:
    KEY_ID: ${{ secrets.APPLE_API_KEY }}
    KEY_CONTENT: ${{ secrets.APPLE_API_KEY_CONTENT }}
  run: |
    key_file="$RUNNER_TEMP/AuthKey_$KEY_ID.p8"
    printf '%s\n' "$KEY_CONTENT" > "$key_file"
    echo "APPLE_API_KEY_PATH=$key_file" >> "$GITHUB_ENV"
```

The tauri-action step then gets `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`,
`APPLE_API_ISSUER` and `APPLE_API_KEY` from the secrets through its `env:` block. The ad hoc build gets none of them,
not even empty: Tauri takes an empty `APPLE_CERTIFICATE` as a certificate to import.

`APPLE_SIGNING_IDENTITY` is listed as required on purpose. The Tauri reference says it is inferred from
`APPLE_CERTIFICATE` when neither the variable nor `bundle.macOS.signingIdentity` is set, but the Tauri CI example passes
it, so we do too and the identity is never a guess.

Notarization with an Apple ID instead: `APPLE_ID` (the account email), `APPLE_PASSWORD` (an app-specific password, not
the account password) and `APPLE_TEAM_ID`. Tauri requires all three together. Use one notarization route, not both.

### Making the certificate without a Mac

Keychain Access on a Mac is the usual way to get the `.p12`, but you can do it with OpenSSL on Linux. Only the Account
Holder can create a Developer ID certificate.

1. Make a key and a certificate signing request. Keep `developer-id.key` private: whoever has it and the certificate
   can sign software as the project.

   ```sh
   openssl genrsa -out developer-id.key 2048
   openssl req -new -key developer-id.key -out developer-id.csr \
     -subj "/emailAddress=<Apple ID email>/CN=<your name>/C=<country code>"
   ```

2. At developer.apple.com, go to Certificates, Identifiers & Profiles → Certificates → add one, choose **Developer ID
   Application** and the **G2 Sub-CA**, and upload `developer-id.csr`. Download the certificate, which is
   `developerID_application.cer`.
3. Convert it, add Apple's intermediate certificate and build the `.p12`. The `.cer` files Apple hands out are in DER
   form, and OpenSSL wants PEM. The intermediate for the G2 Sub-CA is "Developer ID - G2" on Apple's
   [certificate authority page](https://www.apple.com/certificateauthority/) (`DeveloperIDG2CA.cer`).

   ```sh
   openssl x509 -inform DER -in developerID_application.cer -out developerID_application.pem
   openssl x509 -inform DER -in DeveloperIDG2CA.cer -out DeveloperIDG2CA.pem
   openssl pkcs12 -export -legacy -inkey developer-id.key -in developerID_application.pem \
     -certfile DeveloperIDG2CA.pem -out cert.p12
   ```

   OpenSSL 3 writes a `.p12` that macOS's `security import` can't read (it fails with a MAC verification error) unless
   you ask for the older algorithms with `-legacy`; with OpenSSL 1.1 leave the flag out. The command asks for the
   password that becomes `APPLE_CERTIFICATE_PASSWORD`.
4. `openssl base64 -A -in cert.p12 -out cert-base64.txt` gives the text for `APPLE_CERTIFICATE`, as above.
5. `openssl x509 -in developerID_application.pem -noout -subject` prints the certificate's name in `CN`, for example
   `Developer ID Application: Your Name (TEAMID)`. That is `APPLE_SIGNING_IDENTITY`, used as it is.

Delete `cert.p12`, the base64 text and, unless you want to renew with the same key, `developer-id.key` once the secrets
are saved. A signed dry run on `main` (see [macOS installer](#macos-installer)) shows whether they work.

### Renewals and rotation

- Membership: renews every year, and Apple says the Account Holder has to do it. Keep the renewal date in a calendar.
- Certificate: don't rely on a fixed lifetime. Read the expiry date in Certificates, Identifiers & Profiles (or in
  Keychain Access) and set a reminder ahead of it. After it expires, released apps keep running, but signing updates
  needs a new certificate, which needs an active membership. To renew, create a new Developer ID Application certificate
  (G2 Sub-CA), export it as a `.p12`, and replace `APPLE_CERTIFICATE` and `APPLE_CERTIFICATE_PASSWORD`. Run
  `security find-identity -v -p codesigning` with the new certificate and update `APPLE_SIGNING_IDENTITY` if the name
  changed. The API key secrets stay as they are. Don't revoke the old certificate: Apple says apps signed with a revoked
  one can no longer be installed or launched.
- API key: to rotate it, or if the `.p8` leaks, create a new key with Developer access and replace `APPLE_API_KEY` and
  `APPLE_API_KEY_CONTENT`. Check that `APPLE_API_ISSUER` still matches the Issuer ID above the keys table. Run a
  notarization, then revoke the old key under Users and Access → Integrations. Revoke last, since notarization fails
  while the secrets point at a revoked key.

## Update signing key

The app can update itself, and [PRIVACY.md](../PRIVACY.md#update-checks) says what that sends. It installs a downloaded
file only if the file is signed with the update signing key, so whoever holds the private half of that key decides what
every installed copy accepts. The key is a minisign key pair made by `tauri signer generate`. It is separate from the
Apple certificate and from any Windows code signing: those make the operating system trust a file, this one makes the
app trust an update.

The key is in use: the Environment `updater-signing` holds both secrets, and the repository variable
`TAURI_UPDATER_PUBKEY` has been set since 2026-10-22 (step 4 below), so every build since then has the updater.

### Where the parts live

- The private key and its password are the secrets `TAURI_SIGNING_PRIVATE_KEY` and
  `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` of the Environment `updater-signing`, which only `main` may use. Only the
  private key is required. GitHub can't store an empty secret, so a key made without a password has no password secret.
  The jobs of `publish-release.yml` that run for a tag (`Check release`, `Publish` and the late attach) use the
  Environment once the variable below is set, and not before, so its rule that only `main` may use it can't hold up a
  release that has no update key. `Check release` looks at the secrets, and `Publish` and the late attach sign with
  them. A dry run doesn't use the Environment. For a release, `release-please.yml` calls `publish-release.yml` with
  `secrets: inherit`; without it these jobs would get empty secrets, Environment or not. GitHub doesn't show a secret
  again after it is saved, so the secrets are not a backup: keep the key file and the password in a password manager.
- The public key is the repository variable `TAURI_UPDATER_PUBKEY`, set to the contents of the `.pub` file. It isn't
  secret. The three build workflows pass it to the build as `REPEAT_THAT_UPDATER_PUBKEY`, and
  [`updater.rs`](../app/src-tauri/src/updater.rs) compiles it into the app.
- Without the variable, a build has no updater: the plugin isn't registered, the window asks nothing, the tray has no
  update item, nothing contacts the network, and `--rt-self-check` prints `"updater":false`. Builds from source are
  like that. With the variable it prints `"updater":true`, which the Linux and macOS builds check.

### What a release does with it

| `TAURI_UPDATER_PUBKEY` | `TAURI_SIGNING_PRIVATE_KEY` | What happens |
|---|---|---|
| set | set | The update files are signed and attached. |
| set | missing | `Check release` fails the release before any build, and the error points here. |
| not set | any | No update files, and a notice. This is the state today. |

`Check release` also fails the release when the variable or the private key isn't what `tauri signer generate` wrote:
one line of base64, the contents of the file and not its path. A variable of blanks counts as set (the workflows test it
with `!= ''`), so it fails there as "not a key". The password secret is never required. While the variable isn't set,
`Check release` doesn't use the Environment and can't see its secrets, so it only prints the notice. Set the variable
last, once the secrets are in.

With the key, `Publish` (and the late attach, for macOS) does this after it downloads the builds. Steps 2 to 5 are
[`sign-updates.sh`](../.github/scripts/sign-updates.sh):

1. It installs the Tauri CLI at the version locked in `app/package-lock.json` with `npm ci --ignore-scripts` (this job
   may hold the private key, so no install script runs), and `minisign`.
2. It signs each update file with `tauri signer sign`: the `*-setup.exe`, the `*.AppImage` and the `*.app.tar.gz`.
3. It checks every `.sig` with `minisign -V` against the public key. A private key that doesn't match the variable
   stops the job here, before anything is uploaded. A `.sig` that isn't base64 stops it too, and the error names the
   file.
4. It writes one `latest-<target>-<arch>.json` per platform with
   [`updater_manifest.py`](../.github/scripts/updater_manifest.py), taking the notes from the release body.
5. It runs `release_assets.py check-manifests` over the folder, which fails when a manifest's `version` isn't the tag,
   when a `url` in it isn't a file of this release among the files, or when its `signature` isn't the content of that
   file's `.sig`.
6. Before it writes `SHA256SUMS`, `release_assets.py check-assets --updater` fails if a platform that was built lacks
   one of its update files.

A dry run, which is a run without a tag (and so every pull request that touches the publishing), makes a throwaway key
with `tauri signer generate --ci` and goes through the same steps, so this path runs before a release does. Its
signatures are checked against the public key of the throwaway key, not against the variable. Nothing is attached, and
the only file the run keeps is `SHA256SUMS`, as a workflow artifact. The job summary shows the manifests and lists the
files.

The app reads `https://github.com/Roboroads/repeat-that/releases/latest/download/latest-<target>-<arch>.json`, which
follows whichever release GitHub calls the latest. There is one manifest per platform (`latest-windows-x86_64.json`,
`latest-linux-x86_64.json`, `latest-darwin-aarch64.json`) and no single `latest.json`: each is written once and never
changes, and the macOS late attach adds its own without touching the others. A 404 counts as "up to date", which is
what an app sees until the macOS manifest is there. A manifest is in Tauri's static format: `version` (the tag without
the `v`), `notes`, `pub_date`, and `platforms`, whose entries have the `url` of the update file on the release and its
`signature`. The app shows `notes` as plain text, with HTML comments taken out and cut at 4000 characters.

The app only offers a version higher than its own, and it never goes back. To undo a bad release, publish a higher one.

### Setting it up

Only the maintainer does these, in this order. Steps 1 to 3 are done.

1. Make the key on your own machine, not in CI. Pick a strong password when it asks. Run this in the `app` folder of a
   checkout, after `npm ci`:

   ```sh
   npx tauri signer generate -w ~/.tauri/repeat-that-updater.key
   ```

   It writes `~/.tauri/repeat-that-updater.key` (the private key) and `~/.tauri/repeat-that-updater.key.pub`. Put both,
   and the password, in a password manager. If they are lost, see [below](#rotating-or-losing-the-key).
2. Make the Environment, limited to `main`:

   ```sh
   gh api -X PUT repos/Roboroads/repeat-that/environments/updater-signing \
     -F 'deployment_branch_policy[protected_branches]=false' -F 'deployment_branch_policy[custom_branch_policies]=true'
   gh api -X POST repos/Roboroads/repeat-that/environments/updater-signing/deployment-branch-policies \
     -f name=main -f type=branch
   ```

3. Store the secrets. The second command asks for the password. It is only for a key that has one, which this key does:
   a key made without a password needs the first secret only.

   ```sh
   gh secret set TAURI_SIGNING_PRIVATE_KEY --env updater-signing --repo Roboroads/repeat-that < ~/.tauri/repeat-that-updater.key
   gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD --env updater-signing --repo Roboroads/repeat-that
   ```

4. Switch the updater on. Do this last, because from then on every build has the updater compiled in:

   ```sh
   gh variable set TAURI_UPDATER_PUBKEY --repo Roboroads/repeat-that < ~/.tauri/repeat-that-updater.key.pub
   ```

5. Check it. Run Actions → Publish release → Run workflow on `main` without a tag. The builds now have the public key
   compiled in, and the Linux and macOS self-checks must say `"updater":true`. The job summary lists the `.sig` and
   `latest-*.json` files, signed with the throwaway key: a dry run never uses the real one or the Environment. The real
   key is used for the first time by the first release. `Check release` then fails the run before any build if the
   variable or the private key isn't a key file, and `Publish` fails before it uploads anything, leaving the draft, if
   the password is wrong or the key doesn't match the variable. Fix the secret and run the failed job again.

The first release made after step 4 carries the updater. The first update anyone receives is the release after that,
because v0.1.0 has no updater.

### Trying the updater locally

This runs the whole path on one Linux machine, with a throwaway key and a server on `127.0.0.1`, without touching a
release. It needs what a [build from source](install-linux.md#build-from-source) needs. The endpoint can be changed at
build time only (`REPEAT_THAT_UPDATER_ENDPOINT`), and only to `https` or to `http` on `127.0.0.1` or `localhost`.

1. Make the throwaway key and build two AppImages, 0.0.1 and 0.0.2, that trust it and look at the local server. Run
   this in the `app` folder:

   ```sh
   npx tauri signer generate --ci -w ~/.tauri/trial.key
   export REPEAT_THAT_UPDATER_PUBKEY="$(cat ~/.tauri/trial.key.pub)"
   export REPEAT_THAT_UPDATER_ENDPOINT=http://127.0.0.1:8765/latest-linux-x86_64.json
   mkdir -p ~/updater-trial/old ~/updater-trial/serve
   npm run tauri build -- --bundles appimage --config '{"version":"0.0.1"}'
   cp "../target/release/bundle/appimage/Repeat That_0.0.1_amd64.AppImage" ~/updater-trial/old/Repeat-That_0.0.1_amd64.AppImage
   npm run tauri build -- --bundles appimage --config '{"version":"0.0.2"}'
   cp "../target/release/bundle/appimage/Repeat That_0.0.2_amd64.AppImage" ~/updater-trial/serve/Repeat-That_0.0.2_amd64.AppImage
   ```

2. Sign 0.0.2 and write its manifest by hand. `updater_manifest.py` would point the `url` at GitHub, and here it has to
   point at the local server:

   ```sh
   npx tauri signer sign -f ~/.tauri/trial.key ~/updater-trial/serve/Repeat-That_0.0.2_amd64.AppImage
   cat > ~/updater-trial/serve/latest-linux-x86_64.json <<MANIFEST
   {
     "version": "0.0.2",
     "notes": "Local trial.",
     "pub_date": "2026-10-22T12:00:00Z",
     "platforms": {
       "linux-x86_64-appimage": {
         "url": "http://127.0.0.1:8765/Repeat-That_0.0.2_amd64.AppImage",
         "signature": "$(cat ~/updater-trial/serve/Repeat-That_0.0.2_amd64.AppImage.sig)"
       }
     }
   }
   MANIFEST
   ```

3. Keep a copy of 0.0.1, because the update replaces the file, and serve the folder:

   ```sh
   cp ~/updater-trial/old/Repeat-That_0.0.1_amd64.AppImage ~/updater-trial/Repeat-That_0.0.1_amd64.AppImage.orig
   cd ~/updater-trial/serve && python3 -m http.server 8765 --bind 127.0.0.1
   ```

4. In another terminal, run 0.0.1 (`chmod +x` it first). In the settings window, under General, click **Check now**.
   The banner says version 0.0.2 is available. Click **Install and restart**. The app comes back as 0.0.2: General
   shows the version, and there is exactly one tray icon.
5. Put the copy of 0.0.1 back, change one byte of the served 0.0.2 file, and repeat step 4:

   ```sh
   cp ~/updater-trial/Repeat-That_0.0.1_amd64.AppImage.orig ~/updater-trial/old/Repeat-That_0.0.1_amd64.AppImage
   printf '\0' | dd of=~/updater-trial/serve/Repeat-That_0.0.2_amd64.AppImage bs=1 seek=100000 conv=notrunc
   ```

   The window now says "Couldn't install version 0.0.2" and links to the release page, and the app stays on 0.0.1.

A debug build has a shorter way that needs no key, build or server: start it with `REPEAT_THAT_MOCK_UPDATE` set to
`available`, `none`, `fail-check` or `fail-download`, and the window gets that answer as if it came from GitHub. The
version on offer is 99.0.0, and nothing is installed.

### Rotating or losing the key

An installed app trusts the public key it was built with and no other. A new key therefore reaches an app only through
a build that has it, and an old app can't be told to switch. The pipeline has no way to bridge that, with a version
signed by the old key that carries the new public key: the build and the signing take the same key pair, and the
`minisign -V` check insists on it. Whatever the reason for the change, the steps are the same:

1. Make a new key pair (step 1 above, with another file name), replace the secrets (step 3) and set the variable to
   the new `.pub` file (step 4). Do all three before the next release. If the new key has no password, delete the old
   password secret. `Check release` fails a release that has the variable but no private key, or a value that isn't a
   key file, and the `minisign -V` check fails one where the two keys don't match.
2. Make a release. It is signed with the new key and has the new public key inside.
3. An app from before the change finds the new version, downloads it and refuses it, because the signature isn't from
   the key it has. It says "Couldn't install" and links to the release page, and the user installs the new version by
   hand. From then on the app has the new key.

If the private key or its password is lost, nothing leaks and nothing needs revoking, but no update can be signed:
`Publish` fails at the signing step and the draft stays. Follow the steps above. To ship a release in the meantime,
delete the variable (see below). That release has no updater, and its users update by hand until a release with a key
reaches them.

If the private key leaks, follow the same steps at once and say so in the release notes. The old key can't be taken out
of the apps that have it. A thief would also need a way to hand a file to those apps, and they read their manifest from
this repository's releases over HTTPS, so the key alone isn't enough: a break into the GitHub account or the release
would be. [SECURITY.md](../SECURITY.md) says the same to people who report a problem.

The key has no expiry, so there is no date to put in a calendar.

### Turning the updater off

Delete the variable:

```sh
gh variable delete TAURI_UPDATER_PUBKEY --repo Roboroads/repeat-that
```

Builds after that have no updater, and releases carry no update files (the summary says so). Apps that already have the
updater and whose user agreed keep checking, find no manifest on the latest release, and take that as up to date. The
only way to stop them is a release without the updater reaching them by hand.
