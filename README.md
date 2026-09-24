# Lodger

A self-hosted web UI for the KVM/QEMU VMs on your existing Linux host. VM management
that does not move in.

> [!WARNING]
> Lodger is pre-release software. Do not use it on a host with VMs that you care about.
> v0.1 runs only on the author's own host. v0.2 will be the first public alpha. Until
> v1.0, any release can break the configuration, the API, or the stored data.

Lodger is one binary: a Rust server with an embedded web app. It talks to libvirt, and
libvirt stays the source of truth for your VMs.

## Contributing

Lodger does not accept pull requests yet. Contributions open after most of the planned
features exist.

## Security

To report a vulnerability, read [SECURITY.md](SECURITY.md). Do not open a public issue.

## License

Lodger is licensed under the [Apache License 2.0](LICENSE).
