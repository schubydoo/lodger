---
default: minor
---

Add `sudo lodger install` and `sudo lodger uninstall`: install checks the host, copies the binary to /usr/local/bin, creates the lodger user in the libvirt group, writes /etc/lodger/config.toml and a hardened systemd unit, and starts the service; uninstall keeps the configuration, the database, and the user unless `--purge` is given.
