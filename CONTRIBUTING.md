# Contributing to Lodger

Lodger does not accept pull requests yet. Contributions open after most of the
planned features exist. Until then, this file explains how to build and run Lodger,
and which rules the code follows.

To report a vulnerability, read [SECURITY.md](SECURITY.md). Do not open a public
issue.

## Set up a development machine

On Debian 13 or Ubuntu 24.04, run the bootstrap script from the repository:

```sh
scripts/bootstrap.sh
```

It installs the system packages, the Rust toolchain of `rust-toolchain.toml`, Node
from `.node-version`, and pnpm. Then it builds the web app and the debug server. A
weekly CI job runs it in a clean Ubuntu 24.04 container, in under 15 minutes. The
repository also has a devcontainer that runs the same script.

On another system, install these by hand:

- The Rust toolchain that `rust-toolchain.toml` names, with `rustfmt` and `clippy`.
- The libvirt client library and headers (`libvirt-dev` or `libvirt-devel`), and
  `pkg-config`.
- Node at the version in `.node-version`, and pnpm through corepack.
- `just`, for the recipes in the `justfile`.

## Run Lodger

Start the debug server on libvirt's test driver. It needs no libvirtd and no real VM:

```sh
cargo run -p lodger -- serve --uri test:///default --state-dir /tmp/lodger-dev
```

Open http://127.0.0.1:8460, and enter the setup token from the server's output. A
debug build serves the web app from `web/build/`. After a change in `web/`, run
`pnpm run build` in `web/` again.

## Check a change

Run these before each commit. CI runs the same checks.

```sh
just check
cd web && pnpm run lint && pnpm run check && pnpm run test && pnpm run build
```

The browser tests need a built web app and a debug server. The details are in
[web/README.md](web/README.md).

## Rules for the code

- libvirt is the source of truth for VMs, pools, networks, and snapshots. SQLite
  holds only accounts, sessions, the audit log, and UI configuration.
- Only the `lodger-virt` crate imports `virt`. Lodger calls libvirt through the
  Rust bindings, never through `virsh`, `qemu-img`, or a shell.
- `unsafe` code lives only in `crates/lodger-virt/src/events/ffi.rs` and
  `crates/lodger-virt/src/stats/ffi.rs`. Each block needs a `// SAFETY:` comment.
- XML and YAML come from the builders in `lodger-core`, never from joined strings.
- A password, a token, a cookie, or cloud-init user data never reaches a log line.
- Every change to a VM or an account writes an audit row.
- Every state-changing request passes the Origin rule and the CSRF check.
- Tests use libvirt's test driver, never a real VM. Every new test must be able to
  fail: break the code on purpose, and make sure that the test fails.

[ARCHITECTURE.md](ARCHITECTURE.md) explains where each part lives.

## Commits and pull requests

- Each change goes through a pull request to `main`. Nobody pushes to `main`.
- The pull request title follows [Conventional Commits](https://www.conventionalcommits.org/),
  because a squash merge uses it as the commit subject.
- A user-facing change adds one `.changeset/<slug>.md` file with a one-line body.
  Knope writes `CHANGELOG.md` from these files at each release.
- Write docs, comments, and messages in plain English: short sentences, active
  voice, and simple tenses.

## License

By contributing, you agree that your contribution is licensed under the
[Apache License 2.0](LICENSE).
