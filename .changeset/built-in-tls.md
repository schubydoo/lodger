---
default: minor
---

Serve HTTPS without a reverse proxy. Set `tls_cert` and `tls_key` in `config.toml`, and Lodger serves HTTPS on its `listen` address, so a browser on the LAN can log in. A certificate or key that cannot be used stops the start, and Lodger never falls back to plain HTTP. `lodger doctor` checks the pair and fails 30 days before the certificate expires.
