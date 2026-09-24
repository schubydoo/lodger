# Repository rulesets

These JSON files mirror the rulesets that are live on GitHub. Ported from scratchsmith.
Apply a change with the GitHub API, for example:

```
gh api --method PUT repos/schubydoo/lodger/rulesets/<id> --input .github/rulesets/main.json
```

- `main.json` protects the default branch: no deletion or force-push, linear history,
  squash merges only, resolved review threads, 3 required checks
  (`ci required checks passed`, `security required checks passed`, and
  `conventional PR title`), and a CodeQL gate: a high or critical security alert, or
  an error alert, blocks the merge.
- `protect-version-tags.json` blocks deleting or force-pushing any `v*` tag.

Apply a new required check only after its workflow has run on `main`. A required check
that never runs blocks every merge.
