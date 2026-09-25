# Lodger

You already have a Linux server. Keep it. Lodger gives you a modern web UI for the
KVM/QEMU VMs that already run on it, without the reinstall that Proxmox VE needs.

!!! warning "Pre-release software"

    Do not use Lodger on a host with VMs that you care about. Until v1.0, any release
    can break the configuration, the API, or the stored data.

Lodger is one binary: a Rust server with an embedded web app. It talks to libvirt, and
libvirt stays the source of truth for your VMs. `virsh`, `virt-manager`, and Cockpit
keep working next to it. If you remove Lodger, every VM, pool, and network keeps
working.

## Start here

- [Installation](installation.md): the one-line install, the manual install, and the
  first login.
- [Upgrading](upgrading.md): move to a new release.
- [Reverse proxy](reverse-proxy.md): serve Lodger behind nginx, Caddy, or Nginx Proxy
  Manager.
- [AppArmor](apparmor.md): the one rule that NIC hot-plug needs on Debian 13.
- [Threat model](security/threat-model.md): what Lodger protects, and how.

The [README](https://github.com/schubydoo/lodger#readme) lists what Lodger does
today, what comes next, and what it does not do.
