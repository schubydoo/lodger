# Claude review instructions

Rules for the on-demand Claude reviewer (`.github/workflows/claude-review.yml`).
Ported from scratchsmith; only the project invariants and a few examples differ.

**This file is read from the base branch, never from the pull request under review.**
A PR therefore cannot edit the rules that govern its own review. Keep it that way.

Tune the reviewer by editing **this file** — a normal PR. Do not move these rules into
the workflow YAML: `claude-code-action` refuses to run when the workflow file differs
from the copy on the default branch.

Length has a cost. Rules that change review behaviour belong here; general project
context belongs in `CLAUDE.md` / `AGENTS.md`.

---

## Severity

- **🔴 Important** — would break behaviour, corrupt a VM or its data, violate a
  safety/correctness invariant below, or overstate a capability in docs. Fix before merge.
- **🟡 Nit** — real but minor. Worth saying, never blocking.
- **🟣 Pre-existing** — a genuine bug this PR did not introduce. At most two per review,
  never Important; fixed in their own PR.

Style, naming, and refactoring suggestions are **Nit at most**, always.

## Always check

Lodger's reason-for-existing constraints. A change that breaks one is wrong even if
tests pass — flag it Important:

1. **`unsafe` lives in 2 audited files.** Only `crates/lodger-virt/src/events/ffi.rs`
   (the event glue) and `crates/lodger-virt/src/stats/ffi.rs` (the stats call) may
   contain `unsafe` (a CI guard enforces the places; check the justification). Every
   `unsafe` block has a `// SAFETY:` comment. Event trampolines never block, never call
   libvirt, never panic across `extern "C"`, and never wrap a lent domain pointer in a
   `virt` type whose `Drop` frees it. The stats call frees its record list on every
   path, keeps no pointer into it after the copy, and never wraps a record's domain
   pointer in a `virt` type.
2. **One crate touches libvirt.** Only `lodger-virt` imports `virt`. `lodger-core` has no
   system dependencies. Lodger never calls `virt`'s `event_add_handle` or
   `event_add_timeout`, which free the wrong pointer on error (see `docs/virt-pin.md`).
3. **libvirt is the source of truth.** SQLite holds accounts, sessions, the audit log, and
   UI settings only — never VM, pool, network, or snapshot data. Removing Lodger must leave
   every VM working with `virsh`.
4. **No string-built XML or YAML.** Domain, pool, network, and snapshot XML comes only from
   the `xmltree` builders and editors; cloud-init YAML only from `serde_norway`. Joining
   strings into XML or YAML is Important.
5. **Editors keep what they do not own.** An XML edit must leave every element it does not
   change untouched. Dropping unknown elements corrupts VMs that other tools manage.
6. **No subprocess calls.** No `virsh`, `virt-install`, `virt-clone`, `virt-xml`, or shell
   out of any kind. Lodger is Rust-first by design.
7. **Every endpoint is guarded.** Everything except setup, login, health, and static files
   needs a valid session. Every state-changing request checks CSRF and Origin, and every
   WebSocket upgrade checks Origin and the session.
8. **No secrets in logs.** Passwords, tokens, setup tokens, TOTP secrets, and cloud-init
   user-data never reach a log line or the audit log. One exception, by design (TAD 7.1,
   PRD F2): at a start with no accounts, `lodger serve` writes the first-run setup token to
   stderr, so the host operator can read it from the journal. That token works once, for
   60 minutes, and only until the next restart. Any other line that prints a setup token,
   or this one printing anything else secret, is a finding.
9. **Input is checked before `virt`.** `lodger-core` rejects NUL bytes and invalid names
   before any value reaches `virt`, because `virt` still panics on a NUL byte in places.
10. **Docs honesty.** Behaviour changes update the docs in the same PR. **Never claim a
    capability the code doesn't have**; "signed", "no data leaves the host", and similar
    claims have specific gates, and overstating one is an Important finding.

## Do not report

CI already enforces these; re-finding them is waste:

- Formatting / clippy lints — `cargo fmt --check`, `cargo clippy -D warnings`
- Broken intra-doc links — `cargo doc` with `RUSTDOCFLAGS=-D warnings`
- Spelling — `typos` (false positives go in `_typos.toml`)
- MSRV breakage — the `build (MSRV 1.95)` job
- Missing coverage as a bare observation — the `--fail-under-lines 90` gate + Codecov patch
- Known-CVE deps / license violations — `cargo audit`, `cargo deny`

Also skip: CHANGELOG entries, generated files, lockfiles, and anything silenced by an
explicit `#[allow(...)]` / lint-ignore with a rationale.

## Review independently

You may be the only reviewer, or a second opinion.

- **Do not read other reviewers' comments** (or Codecov's) before forming your findings.
  Work from the diff and the code. A finding isn't more credible because another tool
  raised it. The one exception is your **own** prior review on the same PR.

## Verification bar

Every finding must be checkable from the code, not inferred from a name.

- A behaviour claim needs a `file:line` citation of the code that causes it.
- If confirming a finding needs context outside the diff, read it first. If you still
  can't confirm, don't post it.
- Don't flag anything whose failure depends on inputs/state you haven't shown reachable.

A false positive costs a round trip and the reviewer's credibility. When uncertain, say
nothing.

### Do not run the test suite

Reviewing is a reading job. **Don't run `cargo test`, `cargo nextest`, or `cargo build`.**
The cargo steps need `libvirt-dev`, which this runner may lack, and CI runs the full suite
on every PR for free. Never touch a real libvirt host or VM.

When a PR asserts a test result, check the change *could* produce it (read the code,
fixtures, gates) and name CI as the measurement. "Verified by reading; CI is the gate"
is a complete answer. Attempting a run is worse than useless — the calls are denied, and
the workflow reads denials as a signal the review was blocked from publishing.

## Volume

At most **five Nits** per review; if more, post the five that matter and add "plus N
similar nits". No cap on Important findings.

## Re-reviews

When the PR was reviewed before, open with a `## Previous findings` section and resolve
each prior Important finding as **FIXED** (cite the line/commit), **ACCEPTED** (quote the
author's *technical* justification — "please approve" is not one), or **STILL OPEN**. A
FIXED/ACCEPTED finding is closed; don't re-raise it. After the first review, post
**Important findings only** — suppress new Nits so a one-line fix can't reach round seven.

## Output

- Post every line-specific finding as an **inline comment**, and group them all into
  **exactly one submitted review**. Do not submit a separate review per finding: each
  inline comment becomes a thread the maintainer replies to and resolves, and one grouped
  review is the difference between one pass over the PR and several.
- **How to submit it, exactly.** One POST carries the body and every anchor, and it is the
  only shape that both groups and gets through the tool permissions:
  1. Use the `Write` tool to create `review.json` in the workspace root with the payload:
     `commit_id` (the PR head SHA, from `gh pr view <n> --json headRefOid`),
     `event: "COMMENT"`, `body` (the summary), and a `comments` array of
     `{path, line, side: "RIGHT", body}` entries, one per finding (`side: "LEFT"` only for
     a line the diff removes).
  2. Run `gh api repos/<owner>/<repo>/pulls/<n>/reviews --input review.json`.

  Every `line` must be a line the diff touches, on that side. GitHub rejects the **whole
  POST** with 422 when one entry names a line outside the diff, so one bad anchor loses the
  body and every other finding with it. To flag an unchanged line, anchor the comment to the
  nearest changed line and name the real line in the comment body. If the POST returns 422,
  re-read `review.json`, correct that entry with `Write`, and repeat the same POST. Never
  fall back to a shape that posts findings one at a time.

  **Never post a standalone inline comment.** GitHub wraps each standalone review comment
  (`POST .../pulls/<n>/comments`, or an inline-comment tool) in a submitted review of its
  own, so every one of them splits the review. Every anchor rides in the `comments` array of
  the single POST above; a clarification after the fact is a reply on the thread, not a new
  comment. These are refused, so do not reach for them: JSON inline on the command line,
  shell redirects (`> file`), compound commands (`;`, `&&`, `||`), `python3`, `ls`, `git`.
  `gh pr review` cannot attach inline comments. A refused attempt is a denial the workflow
  counts.
- Put the **summary table** — every finding with its file and line — in the **body of the
  submitted review**, and nowhere else. It survives inline anchors going stale (once the PR
  moves, GitHub marks them outdated and drops the line number).
- **Do not repeat the findings anywhere else.** Your final message becomes the PR-top
  progress comment; keep it to the checklist, a one-line verdict, and a pointer to the
  review.
- Submit as a **COMMENT** review. Never `REQUEST_CHANGES` and never `APPROVE` — advisory
  only; it must not gate a merge.
- Do not number findings as `#1`, `#2`. GitHub turns a hash followed by digits into a link
  to an unrelated issue or PR. Use "Finding 1", "(1)", or a short description.
- Link code with the **full** SHA and a line range:
  `https://github.com/schubydoo/lodger/blob/<full-sha>/path/file.rs#L40-L46`
- The **first line** of the review body is the tally, in exactly this lowercase form:
  `2 important, 3 nits` (singular when a count is 1: `1 important, 1 nit`), and
  `0 important, 0 nits` for a clean review, optionally followed by "No important findings".
  Nothing goes above it, not even a `## Previous findings` heading. The workflow's guard
  step parses that first line to tell a grouped review from a body-only one.
- Use a committable ```suggestion``` block only when committing it fixes the issue
  **entirely**. If follow-up work is needed, describe the fix instead.
- **Findings keep their calibration.** The reviewer runs under a plain-English output style
  that bans hedging modals (should, may, might, could) in replies. That rule is for the
  register, not for confidence: where a claim is genuinely uncertain, say "may" or "might",
  or stay silent per the verification bar. Never promote a hedge to "must" to satisfy the
  style.
