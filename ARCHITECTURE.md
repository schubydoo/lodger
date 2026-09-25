# Architecture

Lodger is one binary: a Rust server with an embedded web app. It manages the
KVM/QEMU VMs of the host it runs on, through libvirt. This file tells you where each
part lives.

## The main rules

- libvirt is the source of truth for VMs, pools, volumes, networks, and snapshots.
  Lodger reads them from libvirt and keeps a cache, never a copy in its database.
- SQLite holds only what libvirt does not know: accounts, sessions, recovery codes,
  the audit log, and UI configuration.
- Remove the binary, the service, and the state folder, and every VM keeps working
  with `virsh`.

## The workspace

The Cargo workspace has 3 crates, and `web/` holds the web app.

```text
crates/lodger-core   pure logic, no I/O
crates/lodger-virt   the libvirt adapter
crates/lodger        the server binary and its CLI
web/                 the SvelteKit app that the binary embeds
```

`lodger-core` depends on no system library, so its tests need only Rust. Only
`lodger-virt` links libvirt.

## lodger-core

| Path | Purpose |
| --- | --- |
| `model/` | The types for the host, VMs, pools, volumes, networks, snapshots, and stats |
| `validate.rs` | The input checks: names, paths, hosts, bridges, subnets |
| `xml/` | Builders and parsers for pool, volume, and network XML. Every parse refuses `<!DOCTYPE`. |
| `password/` | The password policy and the list of 3000 common passwords |

## lodger-virt

| Path | Purpose |
| --- | --- |
| `conn.rs` | The 2 libvirt connections: a read connection with the event callbacks, and a job connection for slow calls. Every call runs on a blocking thread. |
| `events/` | Turns libvirt callbacks into events. `events/ffi.rs` holds the `unsafe` glue. |
| `stats/` | Reads the live stats of running VMs. `stats/ffi.rs` holds the `unsafe` call. |
| `cache.rs` | The inventory cache, which the events keep current |
| `supervisor.rs` | Reconnects after libvirt restarts |
| `power.rs`, `delete.rs` | Start, shut down, force off, and delete VMs |
| `pool.rs`, `volume.rs`, `network.rs` | Storage pools, volumes, and virtual networks |
| `console.rs` | The file descriptor for a VM's VNC console |
| `errors.rs` | Maps known libvirt errors to a cause and a fix |

`events/ffi.rs` and `stats/ffi.rs` are the only files with `unsafe` code. A nightly
job runs their stress tests under AddressSanitizer.

## lodger

| Path | Purpose |
| --- | --- |
| `main.rs`, `cli.rs` | The command line: `serve`, `admin`, `doctor`, `install`, `uninstall`, and `version` |
| `server.rs` | The axum routes, and the start of `lodger serve` |
| `config.rs` | `/etc/lodger/config.toml` and the command line options |
| `tls.rs` | Built-in TLS with rustls |
| `setup.rs` | First-run setup with the one-time token |
| `auth.rs`, `throttle.rs` | Login, sessions, logout, the CSRF check, and the login throttle |
| `security.rs` | The Origin rule, the Content Security Policy, and the other headers |
| `client_ip.rs` | The client address, which Lodger reads from a trusted proxy only |
| `tickets.rs`, `ws.rs` | The single-use WebSocket tickets and the events socket |
| `console.rs` | The VNC console relay |
| `api.rs` | The host and VM reads, which come from the inventory cache |
| `accounts.rs`, `actions.rs`, `pools.rs`, `volumes.rs`, `networks.rs`, `stats.rs` | The other API handlers |
| `passwords.rs` | argon2id password hashes, with a limit on how many run at once |
| `db/` | SQLite, its migrations, and its queries |
| `audit.rs` | The audit log, with a copy in the journal |
| `install.rs`, `doctor.rs`, `admin.rs` | `lodger install`, `lodger doctor`, and the root-only recovery commands |
| `assets.rs` | Serves the embedded web app |

`install.rs` is the only file that runs other programs: `systemd-sysusers`,
`userdel`, and `systemctl`, with fixed arguments.

## web

A SvelteKit single-page app with Svelte 5, shadcn-svelte, and TanStack Query.

| Path | Purpose |
| --- | --- |
| `src/lib/api.ts` | The API client |
| `src/lib/events.ts` | Refreshes the cached queries from the events socket |
| `src/lib/session.ts` | The session and the WebSocket tickets |
| `src/lib/components/` | The pages' parts, such as the VM actions and the pool volumes |
| `src/routes/` | The pages: overview, VMs, storage, networks, account, login, setup |
| `tests/` | The Playwright flows with axe accessibility checks |

## A request, end to end

1. The browser sends a request with the session cookie and, for a change, the CSRF
   token.
2. `security.rs` checks the Origin rule. `auth.rs` checks the session and the token.
3. The handler checks its input with `lodger-core`, then calls `lodger-virt`.
4. `lodger-virt` runs the libvirt call on a blocking thread.
5. The handler writes an audit row for a change, and answers.
6. libvirt sends an event. The cache updates, and the events socket tells each open
   page to refresh.

The plans and the full design live outside the repository. [docs/security/](docs/security/)
holds the threat model and the ASVS checklist.
