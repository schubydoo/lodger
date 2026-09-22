# Repository rulesets

These JSON files mirror the rulesets that are live on GitHub. Ported from scratchsmith.
Apply a change with the GitHub API, for example:

```
gh api --method PUT repos/schubydoo/lodger/rulesets/<id> --input .github/rulesets/main.json
```

- `main.json` protects the default branch: no deletion or force-push, linear history,
  squash merges only, resolved review threads, and 2 required checks:
  `ci required checks passed` and `conventional PR title`.
- `protect-version-tags.json` blocks deleting or force-pushing any `v*` tag.

Scratchsmith's `main` ruleset also requires `security required checks passed` and a
CodeQL gate. Lodger adds both when its security and CodeQL workflows exist. A required
check that never runs would block every merge.
