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
milestone 6 release workflow does both through tauri-action, which reads the repository secrets below (Settings →
Secrets and variables → Actions). They are needed before the first macOS release, not before the repository goes
public. The names come from the Tauri [macOS code signing guide](https://v2.tauri.app/distribute/sign/macos/) and
[environment variable reference](https://v2.tauri.app/reference/environment-variables/).

Signing:
- `APPLE_CERTIFICATE`: the Developer ID Application certificate exported from Keychain Access as a `.p12`,
  base64-encoded (`base64 -i certificate.p12`).
- `APPLE_CERTIFICATE_PASSWORD`: the password set when exporting the `.p12`.
- `APPLE_SIGNING_IDENTITY`: the certificate name from `security find-identity -v -p codesigning`, for example
  `Developer ID Application: Your Name (TEAMID)`. Tauri can work it out from `APPLE_CERTIFICATE` when this is unset, but
  setting it removes the guesswork.

Notarization, with an App Store Connect API key (the recommended route, since it doesn't depend on an Apple ID password
or two-factor prompts). Create the key under Users and Access → Integrations in App Store Connect with Developer access.
Apple lets you download the `.p8` file once.
- `APPLE_API_ISSUER`: the Issuer ID shown above the keys table.
- `APPLE_API_KEY`: the Key ID of that key.
- `APPLE_API_KEY_PATH`: the path to the `.p8` file. A secret is a string, so store the file's contents as a secret
  (the workflow can call it `APPLE_API_KEY_CONTENT`; that name is ours, Tauri doesn't read it) and add a step that
  writes it to a file and sets this variable to the file's path.

Notarization with an Apple ID instead: `APPLE_ID` (the account email), `APPLE_PASSWORD` (an app-specific password, not
the account password) and `APPLE_TEAM_ID`. Tauri requires all three together. Use one notarization route, not both.

A Developer ID certificate is valid for five years. Renew it before then and update the three signing secrets.
