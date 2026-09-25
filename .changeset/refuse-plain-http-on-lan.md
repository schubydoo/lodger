---
default: minor
---

`lodger serve` now refuses to start on an address that is not loopback without TLS, because passwords would cross the network in clear text: set `tls_cert` and `tls_key`, list a reverse proxy with TLS in `trusted_proxies`, or set `allow_plain_http = true` in `config.toml` to accept the risk.
