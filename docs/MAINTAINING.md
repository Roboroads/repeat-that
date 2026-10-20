# Maintaining

Notes for maintainers. Contributors don't need any of this.

## Branch protection

`main` is protected by the ruleset in [`.github/rulesets/main.json`](../.github/rulesets/main.json):

- no direct pushes, force pushes or deleting the branch
- changes land through a pull request, squash-merged (so history stays linear)
- CI must pass on Linux, Windows and macOS, and all review threads must be resolved
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

## Going public checklist

- [ ] Make the repository public
- [ ] Apply the branch ruleset (see above)
- [ ] Turn on private vulnerability reporting (Settings → Security) and add a `SECURITY.md`
- [ ] Turn on Dependabot alerts and security updates (version updates are already set up in `.github/dependabot.yml`)
- [ ] Apply to [SignPath Foundation](https://signpath.org) for free Windows code signing
- [ ] Add topics and a social preview image so people can find the project
