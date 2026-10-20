# Maintaining

Notes for maintainers. Contributors don't need any of this.

## Branch protection

`main` is protected by the ruleset in [`.github/rulesets/main.json`](../.github/rulesets/main.json):

- no direct pushes, force pushes or deleting the branch
- changes land through a pull request, squash-merged (so history stays linear)
- CI must pass on Linux, Windows and macOS, the PR title must be a Conventional Commit, and all review threads must be
  resolved
- 0 required approvals, so a solo maintainer can merge their own PRs

GitHub only enforces rulesets on private repositories with a paid plan, so this is applied when the repo goes public.
To apply it, or to update it after editing the JSON:

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
open that bumps the version and updates `CHANGELOG.md`. Merging that PR tags the release, creates the GitHub release and
builds the Windows installers (see [Windows installers](#windows-installers)). Before 1.0, `feat` bumps the patch
version and breaking changes (`feat!:`, `fix!:`) bump the minor version.

The version lives in `Cargo.toml` (`workspace.package.version`, which the app crate and `tauri.conf.json` inherit) and
`app/package.json`. Both are listed under `extra-files` in `release-please-config.json`. It does not touch
`Cargo.lock` or `app/package-lock.json`; they pick up the new version the next time cargo or npm runs.
`.github/scripts/check-release-config.py` checks that the files and the manifest agree. It runs as the `Release config`
job of the `Build Windows installers` workflow whenever a PR touches the release config or the app.

Release PRs are opened with the default `GITHUB_TOKEN`, so CI doesn't run on them by itself, and once the ruleset is
active they'd wait forever for the required checks. Before applying the ruleset, either give the release-please
workflow a fine-grained personal access token (`token: ${{ secrets.RELEASE_PLEASE_TOKEN }}`) or close and reopen the
release PR to trigger CI.

## Windows installers

[`build-windows.yml`](../.github/workflows/build-windows.yml) builds the Windows x64 installers (an NSIS `.exe` that
installs per user, and an MSI that installs for all users) plus a `SHA256SUMS` file. Install instructions for users are
in [`install-windows.md`](install-windows.md).

- **Pull requests** that touch `app/`, `crates/`, the Cargo files or the release config run the workflow. The two
  installers and `SHA256SUMS` are uploaded as workflow artifacts (kept for 30 days), so a change can be tried on a real
  Windows PC before it merges.
- **Releases.** release-please creates its GitHub release with `GITHUB_TOKEN`, and events caused by that token don't
  start other workflows, so an `on: release` workflow would never run. `release-please.yml` therefore calls
  `build-windows.yml` itself when its run created a release, and the `publish` job attaches the installers and
  `SHA256SUMS` to that release.
- **Running it by hand** (Actions → Build Windows installers → Run workflow, once the workflow is on `main`): leave
  `tag` empty for a dry run that only produces artifacts (the job summary shows the checksums), or give an existing tag
  such as `v0.1.0` to rebuild that tag and replace its release assets. Use the second form when a release's build failed.
- Asset names use hyphens (`Repeat-that_0.1.0_x64-setup.exe`) instead of Tauri's `Repeat that_...`: GitHub rewrites
  spaces in asset names, which would leave `SHA256SUMS` listing names that don't exist.

### First release

`release-please-config.json` has `"release-as": "0.1.0"`, so the first release is 0.1.0 whatever the commits suggest.
Once 0.1.0 is out, **remove `release-as`**; otherwise the next release PR proposes 0.1.0 again (the config check warns
when the pin is stale). Hold the 0.1.0 release PR until the app captures and replays audio, because merging it publishes
the installers.

### Version numbers

MSI versions must be numeric, so the installers can't carry a pre-release suffix like `0.2.0-rc.1`. Keep versions and
tags plain `X.Y.Z` / `vX.Y.Z`; the config check rejects any other `release-as`, and the build workflow rejects other
tags.

### Signing

The installers aren't code-signed, so SmartScreen warns on first run (the install guide explains the click-through).
Signing through SignPath is on the checklist below. There's no auto-updater, so there is no update signing key to look
after.

## Going public checklist

- [ ] Make the repository public
- [ ] Apply the branch ruleset (see above)
- [ ] Turn on private vulnerability reporting (Settings → Security) and add a `SECURITY.md`
- [ ] Turn on Dependabot alerts and security updates (version updates are already set up in `.github/dependabot.yml`)
- [ ] Apply to [SignPath Foundation](https://signpath.org) for free Windows code signing
- [ ] Add topics and a social preview image so people can find the project
