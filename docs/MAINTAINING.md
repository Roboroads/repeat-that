# Maintaining

Notes for maintainers. Contributors don't need any of this.

## Branch protection

`main` is protected by the ruleset in [`.github/rulesets/main.json`](../.github/rulesets/main.json):

- no direct pushes, force pushes or deleting the branch
- changes land through a pull request, squash-merged (so history stays linear)
- CI must pass on Linux, Windows and macOS, the PR title must be a Conventional Commit, and all review threads must be
  resolved
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
Windows installers, attaches them and then publishes the release, which is also when its tag appears (see
[Windows installers](#windows-installers)). Before 1.0, `feat` bumps the patch version and breaking changes (`feat!:`,
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

## Windows installers

[`build-windows.yml`](../.github/workflows/build-windows.yml) builds the Windows x64 installers (an NSIS `.exe` that
installs per user, and an MSI that installs for all users) plus a `SHA256SUMS` file. Install instructions for users are
in [`install-windows.md`](install-windows.md).

- **Pull requests** that touch `app/`, `crates/`, the Cargo files or the release config run the workflow. The two
  installers and `SHA256SUMS` are uploaded as workflow artifacts (kept for 30 days), so a change can be tried on a real
  Windows PC before it merges.
- **Releases.** release-please creates its GitHub release as a draft. `release-please.yml` calls `build-windows.yml`
  itself when its run created a release, rather than relying on an `on: release` trigger: that would fire only once
  the release is public, and a release that a workflow publishes with `GITHUB_TOKEN` doesn't start workflows at all.
  The `publish` job attaches the installers and `SHA256SUMS` to the draft.
- **Draft first.** release-please creates the release as a draft (`"draft": true` in its config), and `publish` makes it
  public only after the files are attached, so there is never a public release without installers. A draft has no
  tag yet (publishing creates it at the release commit), which is why the call passes the commit as `sha`. If the build
  fails, the draft stays: re-run the failed jobs of that `release-please` run. The release PR is already labelled as
  released, so merging another PR doesn't retry it.
- **Running it by hand** (Actions → Build Windows installers → Run workflow, once the workflow is on `main`): leave
  `tag` empty for a dry run that only produces artifacts (the job summary shows the checksums). Give a `tag` to build
  and attach the installers to that release. For a draft, also give the release commit as `sha`, because the tag doesn't
  exist yet; the draft is published when the files are attached.
- **No silent replacement.** The `Check release` job runs first and fails within seconds if the release doesn't exist,
  or if it is public and already has installers: replacing those changes files people may have downloaded and
  verified against `SHA256SUMS`. A draft has no public files, so its assets are overwritten freely. To replace the
  installers of a published release on purpose, run the workflow by hand with `replace` ticked.
- Asset names use hyphens (`Repeat-That_0.1.0_x64-setup.exe`) instead of Tauri's `Repeat That_...`: GitHub rewrites
  spaces in asset names, which would leave `SHA256SUMS` listing names that don't exist.

### First release

`release-please-config.json` has `"release-as": "0.1.0"`, so the first release is 0.1.0 whatever the commits suggest.
Once 0.1.0 is out, **remove `release-as`**; otherwise the next release PR proposes 0.1.0 again and collides with the
tag. The plain config check can't tell a pin whose release PR is still open (that PR bumps the manifest to the
pinned version) from one whose release shipped, so it only warns about a pin below the manifest version. The
`release-please` workflow therefore also runs it with `--tags` before it acts: once `v0.1.0` exists and the pin is still
there, that run fails and no further release PRs are opened until the pin is gone. Hold the 0.1.0 release PR until the
app captures and replays audio, because merging it publishes the installers.

### Version numbers

MSI versions must be numeric, so the installers can't carry a pre-release suffix like `0.2.0-rc.1`. Keep versions and
tags plain `X.Y.Z` / `vX.Y.Z`; the config check rejects any other `release-as`, and the build workflow rejects other
tags.

### Signing

The installers aren't code-signed, so SmartScreen warns on first run (the install guide explains the click-through).
Signing through SignPath is on the checklist below. There's no auto-updater, so there is no update signing key to look
after.

## Going public checklist

- [x] Make the repository public (2026-10-20)
- [ ] Apply the branch ruleset (see above), but only once the `Frontend` CI job (added by #9) is on `main`. The
  release-please token (`RELEASE_PLEASE_TOKEN`) is done, see [Release token](#release-token). Applied earlier, PRs wait
  forever for a check that never runs.
- [x] Turn on private vulnerability reporting (Settings → Security) and add a `SECURITY.md`
- [x] Turn on Dependabot alerts and security updates (version updates are already set up in `.github/dependabot.yml`)
- [ ] Apply to [SignPath Foundation](https://signpath.org) for free Windows code signing
- [ ] Add topics and a social preview image so people can find the project

## macOS signing and notarization

macOS releases are signed with a Developer ID Application certificate and notarized with Apple's `notarytool`. The
milestone 6 release workflow does both through tauri-action, which reads the variables below. They are needed before the
first macOS release, not before the repository goes public. The names come from the Tauri
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

Repository secrets (Settings → Secrets and variables → Actions):
- `APPLE_CERTIFICATE`: the Developer ID Application certificate exported from Keychain Access as a `.p12`,
  base64-encoded as in the Tauri reference: `openssl base64 -A -in MyCertificate.p12 -out MyCertificate-base64.txt`.
- `APPLE_CERTIFICATE_PASSWORD`: the password set when exporting the `.p12`.
- `APPLE_SIGNING_IDENTITY`: the certificate name from `security find-identity -v -p codesigning`, for example
  `Developer ID Application: Your Name (TEAMID)`.
- `APPLE_API_ISSUER`: the Issuer ID shown above the keys table.
- `APPLE_API_KEY`: the Key ID of the key.
- `APPLE_API_KEY_CONTENT`: the full text of the `.p8` file. The name is ours, Tauri doesn't read it.

Set in the workflow, not a secret:
- `APPLE_API_KEY_PATH`: the path to the `.p8` file, which Tauri reads from disk. In the macOS build workflow (for
  example `build-macos.yml`), a step before the tauri-action step writes the secret to a file and exports the path for
  the later steps:

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
`APPLE_API_ISSUER` and `APPLE_API_KEY` from the secrets through its `env:` block.

`APPLE_SIGNING_IDENTITY` is listed as required on purpose. The Tauri reference says it is inferred from
`APPLE_CERTIFICATE` when neither the variable nor `bundle.macOS.signingIdentity` is set, but the Tauri CI example passes
it, so we do too and the identity is never a guess.

Notarization with an Apple ID instead: `APPLE_ID` (the account email), `APPLE_PASSWORD` (an app-specific password, not
the account password) and `APPLE_TEAM_ID`. Tauri requires all three together. Use one notarization route, not both.

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
