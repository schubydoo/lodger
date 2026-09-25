# Lodger

You already have a Linux server. Keep it. Lodger gives you a modern web UI for the
KVM/QEMU VMs that already run on it, without the reinstall that Proxmox VE needs.
VM management that does not move in.

> [!WARNING]
> Lodger is pre-release software. Do not use it on a host with VMs that you care about.
> v0.2 is a quiet pre-release for the author's own host. Until v1.0, any release can
> break the configuration, the API, or the stored data.

Lodger is one binary: a Rust server with an embedded web app. It talks to libvirt, and
libvirt stays the source of truth for your VMs. `virsh`, `virt-manager`, and Cockpit
keep working next to it. If you remove Lodger, every VM, pool, and network keeps
working.

## What it does today

- Shows the host and every VM that libvirt knows, with live CPU and memory use.
- Starts, shuts down, reboots, pauses, resumes, forces off, and deletes VMs.
- Opens a VM's graphical console in the browser, when the VM has a VNC display.
- Creates and manages storage pools in a folder or on an NFS share, and the volumes
  in them.
- Creates NAT, isolated, and bridged virtual networks.
- Keeps accounts with a login throttle, and writes every change to an audit log.
- Serves HTTPS itself, with your certificate or a self-signed one, or runs behind a
  [reverse proxy](docs/reverse-proxy.md).
- Checks the host with `lodger doctor`, and prints the fix for each problem.

## What comes next

| Release | Features |
| --- | --- |
| v1.0 | Snapshots with a tree view, full clones, templates, VMs from a cloud image with cloud-init, and hardware edits of stopped VMs |
| v1.1 | A serial console, VM IP addresses, volume resize, image import from a URL, linked clones, and VMs from an ISO. Hot-plug of disks and NICs, an audit log page, TOTP, aarch64 builds, and a container image. |

## What Lodger does not do

| Item | Status |
| --- | --- |
| GPU and PCI passthrough | Planned for V2 |
| More than one host | Planned for V2 |
| Roles, per-VM rights, and single sign-on | Planned for V2. In V1, every account is an admin. |
| Metrics history and charts | Planned for V2. V1 shows live values only. |
| Backups and schedules | Planned for V2 |
| Creating host bridges | V2 at the earliest. A wrong bridge can cut a headless host off the network. |
| A SPICE console | Not planned. The browser console speaks VNC. |
| LXC containers | Not planned. Lodger manages QEMU/KVM VMs only. |
| Clustering, high availability, and live migration | Never |
| Telemetry | Never |

## Requirements

- Debian 13 or Ubuntu 24.04 on x86_64. Fedora, Arch, Rocky Linux 9 and 10, and
  Debian 12 work on a best-effort basis.
- libvirt 9.0 or later, with the local `qemu:///system` connection. Snapshot revert
  needs libvirt 9.9.
- glibc. musl systems such as Alpine are out of scope.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/schubydoo/lodger/main/install.sh | sudo bash
```

The script checks the release's SHA-256. If cosign is installed, it also checks the
release's signature. Then it creates the `lodger` user and starts the service on
`127.0.0.1:8460`. The setup token for the first account is in the journal:
`sudo journalctl -u lodger`.

To reach Lodger from your LAN with a self-signed certificate, pass the host's address:

```sh
curl -fsSL https://raw.githubusercontent.com/schubydoo/lodger/main/install.sh | sudo bash -s -- --self-signed 192.168.1.10
```

## Contributing

Lodger does not accept pull requests yet. Contributions open after most of the planned
features exist. [CONTRIBUTING.md](CONTRIBUTING.md) explains how to build and run it,
and [ARCHITECTURE.md](ARCHITECTURE.md) explains where each part lives. Everyone
follows the [Code of Conduct](CODE_OF_CONDUCT.md).

## Security

To report a vulnerability, read [SECURITY.md](SECURITY.md). Do not open a public issue.
The [threat model](docs/security/threat-model.md) and the
[ASVS Level 1 checklist](docs/security/asvs-level-1.md) describe how Lodger protects
the host.

## License

Lodger is licensed under the [Apache License 2.0](LICENSE).
