# Claude review instructions

Project rules for the on-demand Claude reviewer (`.github/workflows/claude-review.yml`,
which calls the central job in `schubydoo/shared-workflows`).
Ported from scratchsmith; only the project invariants and a few examples differ.

**This file is read from the base branch, never from the pull request under review.**
A PR therefore cannot edit the rules that govern its own review. Keep it that way.

Tune what the reviewer looks for by editing **this file** in a normal pull request. How the
review is posted is central: the shared workflow carries that recipe, and it wins on
posting.

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
   out of any kind. Lodger is Rust-first by design. The one exception is
   `crates/lodger/src/install.rs`, which runs `systemd-sysusers`, `userdel`, and `systemctl`
   with fixed argument lists and no shell. A subprocess anywhere else is Important.
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

When the PR was reviewed before, add a `## Previous findings` section below the tally
line, and resolve
each prior Important finding as **FIXED** (cite the line/commit), **ACCEPTED** (quote the
author's *technical* justification — "please approve" is not one), or **STILL OPEN**. A
FIXED/ACCEPTED finding is closed; don't re-raise it. After the first review, post
**Important findings only** — suppress new Nits so a one-line fix can't reach round seven.

## Output

The central workflow writes a posting recipe next to this file, and it governs how the review
is posted: the grouped review, `review.json`, thread replies, the tally line and the allowed
commands. Do not add posting rules here.

Link code in this repo with the full SHA and a line range:
`https://github.com/schubydoo/lodger/blob/<full-sha>/path/file.rs#L40-L46`
