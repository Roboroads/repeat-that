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
open that bumps the version (`Cargo.toml` `workspace.package.version`) and updates `CHANGELOG.md`. Merging that PR tags
the release and creates the GitHub release. Before 1.0, `feat` bumps the patch version and breaking changes (`feat!:`,
`fix!:`) bump the minor version.

Release PRs are opened with the default `GITHUB_TOKEN`, so CI doesn't run on them by itself, and once the ruleset is
active they'd wait forever for the required checks. Before applying the ruleset, either give the release-please
workflow a fine-grained personal access token (`token: ${{ secrets.RELEASE_PLEASE_TOKEN }}`) or close and reopen the
release PR to trigger CI.

## Going public checklist

- [ ] Make the repository public
- [ ] Apply the branch ruleset (see above)
- [ ] Turn on private vulnerability reporting (Settings → Security) and add a `SECURITY.md`
- [ ] Turn on Dependabot alerts and security updates (version updates are already set up in `.github/dependabot.yml`)
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
- `APPLE_API_KEY_PATH`: the path to the `.p8` file, which Tauri reads from disk. A step before the tauri-action step
  writes the secret to a file and exports the path for the later steps:

```yaml
- name: Write the App Store Connect API key
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
