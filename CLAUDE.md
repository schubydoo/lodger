# CLAUDE.md

The shared agent instructions for this repo live in [AGENTS.md](AGENTS.md). The line
below imports that file, so it loads together with this one. You do not need to open it
separately. This file adds only what is specific to Claude Code.

@AGENTS.md

The `@AGENTS.md` line does real work. Do not change it into a plain link. Claude Code
reads `CLAUDE.md`, not `AGENTS.md`. A markdown link is only a suggestion, and an agent
can ignore it. The `@` form is an import that Claude Code expands into context at
launch. `@` does nothing inside backticks or code fences, so `` `@AGENTS.md` `` in prose
does not import.

---

## Local files

`CLAUDE.local.md` is gitignored, and it loads after this file. Host-specific paths and
personal tool notes belong there, not here. If a note is true on one machine only, put
it in `CLAUDE.local.md`.

`.claude/` is gitignored too. Its subagents, skills, hooks, and settings are local, and
they differ per contributor. Do not assume that any of them exist, and do not name them
in a committed file. For a contributor without them, the name points to nothing.

## Claude Review

The repo owner starts a review with a PR comment that contains `@claude review`. The
workflow runs only for the owner, and it never runs on its own. Other text in the same
comment is allowed. Use it to say why a review runs again.

- The review rules live in `.github/claude-review-instructions.md`. The workflow reads
  them from the base branch, so a PR cannot change the rules for its own review.
- Do not add a `prompt:` input to `.github/workflows/claude-review.yml`. With a prompt,
  the action runs in automation mode and posts no review comments.
- The review takes about 4 minutes, and it does not show in the PR's check list. Read
  the submitted review and its inline comments through the GitHub API.
- The review is advisory. It posts a comment review, never "request changes".
