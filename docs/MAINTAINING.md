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
- [ ] Publish the GitHub Pages site (milestone 8 in the roadmap; on a free plan, Pages needs the public repo)
- [ ] Apply to [SignPath Foundation](https://signpath.org) for free Windows code signing. The Pages site has to be live
  first, because SignPath asks for a project website and a public repository
- [ ] Add topics and a social preview image so people can find the project
