# Changelog

All notable changes to Lodger are documented here. This file is generated from
`.changeset/*.md` fragments by [knope](https://knope.tech). Do not hand-edit it.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
## 0.1.0 (2026-09-24)

### Features

- Add `sudo lodger admin reset-password` and `sudo lodger admin create` for recovery without the web UI: the reset sets a new password and ends every session of the account, both commands need root and write an audit row. ([#35](https://github.com/schubydoo/lodger/pull/35))
- Add the audit log: setup, logins, and account changes write a row with the account, client IP, target, and result, each row has a copy in journald, and a daily task deletes rows older than 365 days. ([#37](https://github.com/schubydoo/lodger/pull/37))
- Add the setup, login, and account pages: the web UI now sends visitors to setup or login, and the account page adds and deletes accounts and changes the own password, which needs the current one and ends every other session. ([#29](https://github.com/schubydoo/lodger/pull/29))
- Protect state-changing requests from other sites: each needs Sec-Fetch-Site: same-origin or an Origin equal to public_url, and a logged-in request also needs its session's X-CSRF-Token. Every response now carries a Content Security Policy, Referrer-Policy: no-referrer, and X-Content-Type-Options: nosniff. ([#27](https://github.com/schubydoo/lodger/pull/27))
- Add `lodger doctor`: it checks the libvirt socket, the lodger user in the libvirt group, the AppArmor rule for NIC hot-plug, the SELinux virt_use_nfs boolean, the libvirt connection, and snapshot revert support, and prints PASS, FAIL, or SKIP with a reason and the commands that fix each failure. ([#46](https://github.com/schubydoo/lodger/pull/46))
- Explain known libvirt errors. A failed action with a known error also shows the cause, the fix, and the host commands, for example the AppArmor rule that a NIC hot-plug needs on Debian 13. Other errors show libvirt's text unchanged. ([#52](https://github.com/schubydoo/lodger/pull/52))
- Add first-run setup: at a start with no accounts, Lodger writes a one-time setup token to the log, and POST /api/setup uses it to create the first account, with a password of at least 15 characters that is not among the 3000 most common ones. ([#25](https://github.com/schubydoo/lodger/pull/25))
- Add `GET /api/host`, `GET /api/vms`, `GET /api/vms/{id}`, and the `/ws/events` WebSocket, which read a live copy of libvirt's inventory, and add `lodger serve --uri` to choose the libvirt connection (default `qemu:///system`). These endpoints have no login yet, so keep `--listen` on 127.0.0.1. ([#19](https://github.com/schubydoo/lodger/pull/19))
- Add the host overview and virtual machine list pages, which update live from libvirt events and show a banner when libvirt or the Lodger server is unreachable. ([#20](https://github.com/schubydoo/lodger/pull/20))
- Add `sudo lodger install` and `sudo lodger uninstall`: install checks the host, copies the binary to /usr/local/bin, creates the lodger user in the libvirt group, writes /etc/lodger/config.toml and a hardened systemd unit, and starts the service; uninstall keeps the configuration, the database, and the user unless `--purge` is given. ([#43](https://github.com/schubydoo/lodger/pull/43))
- Add a page for each VM with live CPU, memory, disk, and network use that refreshes every 5 seconds: Lodger reads the stats from libvirt only while a VM page is open. ([#40](https://github.com/schubydoo/lodger/pull/40))
- Add login and logout (POST, GET, and DELETE /api/session) with a __Host- session cookie that ends after 60 idle minutes or 24 hours, require a session on every API endpoint except health, setup, and login, and slow down repeated failed logins per account and per client IP. ([#26](https://github.com/schubydoo/lodger/pull/26))
- Add `lodger serve`, which serves the web UI embedded in the binary on 127.0.0.1:8460 by default, and `lodger version`, which prints the Lodger and libvirt client versions. ([#9](https://github.com/schubydoo/lodger/pull/9))
- Add the configuration file /etc/lodger/config.toml (listen, uri, state_dir, public_url, trusted_proxies), the `--config` and `--state-dir` flags, the SQLite database in the state directory with mode 0600, and GET /api/health. ([#24](https://github.com/schubydoo/lodger/pull/24))
- Add storage pools. The Storage page lists every pool and creates a folder pool or an NFS pool, which starts at once with autostart on, and a failed NFS mount leaves no pool behind. Each pool's page starts and stops the pool, switches its autostart, and removes it after the typed name. It lists the VMs that use the pool first, and deleting its volumes is a separate choice. ([#54](https://github.com/schubydoo/lodger/pull/54))
- Add virtual networks. The Networks page lists every network and creates a NAT, isolated, or host bridge network, which starts at once with autostart on. A subnet that overlaps another network is rejected with that network's name, and bridge mode explains that a host bridge must exist first, because Lodger never changes the host's network. Each network's page starts and stops it, switches autostart, and deletes it after the typed name, listing the VMs on it first. ([#55](https://github.com/schubydoo/lodger/pull/55))
- Add reboot, pause, resume, autostart, and delete for VMs: delete needs the typed VM name and a shut-off VM, can also delete the VM's volumes that no other VM uses, and lists every kept disk with the reason; after a shutdown request that the guest ignores for 120 seconds, the VM row offers Force off. ([#49](https://github.com/schubydoo/lodger/pull/49))
- Add Start, Shut down, and Force off to the VM list: shut down sends an ACPI request, force off asks for the VM's name first, the list shows the new state when libvirt reports it, and every action writes a `vm.lifecycle` audit row. ([#38](https://github.com/schubydoo/lodger/pull/38))
- Add the VNC console: open a running VM's screen in the browser through noVNC. Lodger relays it over a socket pair from libvirt, so Lodger needs no VNC port on the host. ([#21](https://github.com/schubydoo/lodger/pull/21))
- Protect the WebSockets: each upgrade needs a live session and a single-use ticket from POST /api/ws-tickets that works for 30 seconds and only from the page origin that asked for it, and logging out closes the session's sockets within 5 seconds. ([#28](https://github.com/schubydoo/lodger/pull/28))

### Fixes

- Fix a SIGTERM that arrives right after the start, for example `systemctl stop` during startup: Lodger now shuts down cleanly instead of being killed by the signal. ([#41](https://github.com/schubydoo/lodger/pull/41))
- Ship complete third-party notices: /third-party-notices.txt now lists every Rust crate in the binary and every web package in the UI, generated at build time, and the build fails on a license outside the allowed list. ([#23](https://github.com/schubydoo/lodger/pull/23))
