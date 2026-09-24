---
default: minor
---

Add `sudo lodger install --self-signed <ip-or-name>`, which makes a self-signed TLS certificate for that address or host name, keeps its key readable only by root and the lodger user, turns on HTTPS in `config.toml`, and prints the SHA-256 fingerprint to compare with the one that the browser shows. It never replaces a certificate that the configuration already names.
