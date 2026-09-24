# Lodger: agent guide

Lodger is a self-hosted web UI for the KVM/QEMU VMs on one Linux host. It talks to
libvirt, and it ships as one binary: a Rust server with an embedded SvelteKit app. The
license is Apache-2.0. This file holds the rules for every coding agent. `CLAUDE.md`
adds only what is specific to Claude Code.

## Critical commands

Rust (toolchain 1.98.1 from `rust-toolchain.toml`, MSRV 1.95):

- Run every local gate: `just check` (fmt, clippy with `-D warnings`, tests).
- Run the tests: `cargo nextest run --workspace`. CI uses `--profile ci`.
- Filter the unit tests of the server with `--bin lodger`. The `lodger` crate is a
  binary, so `--lib` selects no test at all and still passes.
- Lint: `cargo clippy --workspace --all-targets -- -D warnings`. Format: `cargo fmt --all`.
- Run the server on libvirt's built-in test driver, with no libvirtd:
  `cargo run -p lodger -- serve --uri test:///default --state-dir /tmp/lodger-dev`.
  The setup token is in stderr.
- The cargo steps need `libvirt-dev` (the libvirt client library and headers).

Web (run every command inside `web/`, with pnpm 12.5.1 from corepack):

- Install: `pnpm install --frozen-lockfile`.
- Gates: `pnpm run lint` (prettier and eslint), `pnpm run check` (svelte-check),
  `pnpm run test` (vitest), and `pnpm run build`.
- A failed `pnpm run build` leaves the old `web/build/` in place, and a debug build of
  the server serves `web/build/` from disk. Read the exit code of every build.

## Architecture map

- `crates/lodger-core`: pure logic with no I/O. The model types, input checks, and the
  password policy. Later tasks add the XML and cloud-init builders here.
- `crates/lodger-virt`: the libvirt adapter. It is the only crate that imports `virt`.
  `conn.rs` holds 2 connections: a read connection with the event callbacks, and a job
  connection for slow calls. `events/` turns libvirt callbacks into events, and
  `stats/` reads the live stats of the running domains. `events/ffi.rs` and
  `stats/ffi.rs` are the only modules with `unsafe` code. `cache.rs` keeps the
  inventory, `supervisor.rs` reconnects, and `power.rs` starts and stops domains.
- `crates/lodger`: the server binary. axum routes are in `server.rs`. Sessions, login,
  and the CSRF check are in `auth.rs` (`require_session`), and the throttle is in
  `throttle.rs`. The Origin rule and the security headers are in `security.rs`. The
  SQLite store is `db/`, and the audit log is `audit.rs`. Built-in TLS (rustls with
  the `ring` provider) is in `tls.rs`. `admin.rs` is the root-only recovery CLI.
- `web/`: a SvelteKit 2 single-page app with Svelte 5, shadcn-svelte, and TanStack
  Query. `src/lib/api.ts` is the API client, and `src/lib/events.ts` refreshes queries
  from the `/ws/events` socket.

## Hard rules

- Every change is a pull request to `main`. Never commit or push to `main`. The PR body
  uses the sections of `.github/PULL_REQUEST_TEMPLATE.md`. The PR title follows
  Conventional Commits, because a squash merge of one commit uses its subject.
- A user-facing change adds one `.changeset/<slug>.md` fragment with a one-line body.
  An internal change (CI, tests, refactor, docs for contributors) gets the
  `no-changelog` label instead. Never edit `CHANGELOG.md` by hand: knope generates it.
- `unsafe` code is allowed only in `crates/lodger-virt/src/events/ffi.rs` and
  `crates/lodger-virt/src/stats/ffi.rs`. Each block needs a `// SAFETY:` comment. A
  change to either file needs the AddressSanitizer stress tests in
  `.github/workflows/nightly.yml`. A CI guard searches the crates for the word, comments
  included. In a comment elsewhere, write "state-changing" or another word.
- Only `lodger-virt` imports `virt`. Lodger calls libvirt through the Rust bindings.
  Never add a subprocess call (`virsh`, `qemu-img`, or a shell). The one exception is
  `crates/lodger/src/install.rs`: `lodger install` and `uninstall` run `systemd-sysusers`,
  `userdel`, and `systemctl` with fixed argument lists, because Rust cannot create a
  system user.
- libvirt is the source of truth for VMs, pools, networks, and snapshots. SQLite holds
  only accounts, sessions, recovery codes, the audit log, and UI settings.
- Build XML and YAML with the builders, never by joining strings.
- A password, token, session cookie, CSRF token, or cloud-init user-data must never
  reach a log line or the audit log. A failed login for an unknown name stores no
  name, because the name can be a password typed into the wrong field.
- Every change to a VM or an account writes an audit row through `audit::log` (server)
  or `audit::record` (CLI). The detail holds only the allowlisted `Detail` fields, and a
  failure reason is a fixed code.
- API paths identify objects by UUID, and UI routes use names.
- Every state-changing request passes the Origin rule and the `X-CSRF-Token` check.
  A WebSocket needs a single-use ticket.
- Tests use libvirt's test driver, never a real VM. `test:///default` shares its state
  across one process, so each test defines domains with its own unique name. A
  `test://<file>` URI gives each connection its own state. Put a timeout on every wait
  for an event.
- Every new test must be able to fail. Break the code on purpose, run the test, and
  make sure that it fails, then restore the code.
- Add each new Rust source file to the component list in `codecov.yml`, because
  components do not match path globs.
- All GitHub Actions are pinned by commit SHA.

## Workflow preferences

- Keep changes small and surgical. Match the style, comment density, and naming of the
  surrounding code.
- Make sure of a fact before you report it. Read `git log`, the PR, or the code. A status
  line in a document is a cache, not the truth.
- Search the whole repository with `command grep`. A plain `grep` can skip gitignored
  paths without a warning.
- Write documentation, comments, and messages in plain English: short sentences, active
  voice, and simple tenses.
- Never write a bare `#<number>` in text that GitHub renders unless it means that issue
  or PR. GitHub turns it into a link.

## What not to put here

Versions of crates and packages live in `Cargo.toml` and `web/package.json`. Plans,
task status, and open-PR notes belong in the maintainer's notes, not in this file.
Host-specific paths and personal tools belong in a local, gitignored file.
