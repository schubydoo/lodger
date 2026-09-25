# Threat model

This document lists the threats to a Lodger install and the controls against each one.
It uses STRIDE, a set of 6 threat types: spoofing, tampering, repudiation, information
disclosure, denial of service, and elevation of privilege. The ASVS column names the
matching OWASP ASVS 5.0 requirements. The [ASVS Level 1 checklist](asvs-level-1.md)
records the review of every Level 1 requirement.

## What Lodger protects

Lodger controls libvirt on one Linux host. libvirt runs VMs as root, so a Lodger session
can start, stop, change, and remove every VM on the host. The web login is therefore the
real security boundary. An attacker with a session has the same power as a Lodger admin.

Lodger protects these assets:

- The VMs, their disks, the storage pools, and the virtual networks.
- The accounts: password hashes, session tokens, and the setup token.
- The audit log, which records who changed what.

## Trust boundaries

1. The browser and the Lodger server. Every request crosses this boundary. Lodger trusts
   nothing from it without a session, and it checks every input on the server.
2. A reverse proxy and the Lodger server. Lodger trusts `X-Real-IP` and
   `X-Forwarded-For` only from an address in `trusted_proxies`.
3. The Lodger server and libvirt. Lodger talks to libvirt through its Unix socket as the
   `lodger` user, which is in the `libvirt` group.
4. The guest and Lodger. VM names, guest agent data, and DHCP data come from outside
   Lodger, and Lodger shows them only as text.
5. The release and the host. `install.sh` checks the SHA-256 and the cosign signature of
   a release before it installs anything.

## Who the attacker is

- A device on the LAN that can reach the Lodger port, or a web page that a Lodger user
  opens in the same browser.
- A person who guesses passwords, or who reaches a new install before its owner.
- A guest OS that puts hostile text in its name, its guest agent answers, or its
  serial output.
- An unprivileged local user on the host.
- A person who changes a release archive on its way to the host.

Out of scope: an attacker with root on the host, an attacker in the `libvirt` group, and
a Lodger admin who misuses their own rights. The audit log records the actions of an
admin, but it cannot stop them.

## Threats and controls

The Status column says whether the control is in the code now, or in which version it
comes.

| STRIDE | Threat | Control | Status | ASVS |
| --- | --- | --- | --- | --- |
| Spoofing | A device on the LAN, or DNS rebinding, reaches the port | Lodger listens on `127.0.0.1` by default. A non-loopback address without built-in TLS logs a warning at every start. Lodger never compares `Origin` with the `Host` header. It trusts forwarded client addresses only from `trusted_proxies`. | In place | 3.5.1, 4.1.3 |
| Spoofing | A person claims a new install before its owner | The first account needs a 128-bit setup token from the journal. Lodger stores only its hash and compares it in constant time. The token works once, and then `/api/setup` answers 404. No default account exists. | In place | 6.3.2, 6.4.1 |
| Spoofing | Password guessing and credential stuffing | After 5 failures in 15 minutes, each attempt waits longer, up to 60 seconds. The limit applies per account and per client IP. No account locks, so an attacker cannot lock out the admin. Passwords need 15 characters, and the 3000 most common passwords fail. Hashes use argon2id. | In place | 6.2.1, 6.2.4, 6.3.1 |
| Spoofing | Password guessing and credential stuffing | TOTP as a second factor | Planned for v1.1 | 6.3.3 |
| Tampering | Cross-site request forgery, also from a sibling subdomain | Every state-changing request must send `Sec-Fetch-Site: same-origin`, or an `Origin` equal to `public_url`. It must also send the session's `X-CSRF-Token`, which Lodger compares in constant time. Lodger accepts only JSON bodies. The session cookie is `SameSite=Strict`. | In place | 3.5.1, 3.5.2, 3.5.3 |
| Spoofing, information disclosure | A foreign page opens the events WebSocket | The upgrade needs an exact `Origin` match, a valid session, and a single-use ticket that expires after 30 seconds. | In place | 4.4.2, 4.4.3, 4.4.4 |
| Spoofing, information disclosure | Console hijack | The console uses the same WebSocket rules. | Planned with the console | 4.4.2 to 4.4.4 |
| Tampering | Cross-site scripting through VM names, guest data, or serial output | The UI renders outside data only as text. A lint rule blocks raw HTML in Svelte. The Content Security Policy allows only Lodger's own scripts, by hash. | In place | 3.2.2, 3.4.3 |
| Tampering | XML or YAML injection into libvirt | Names match an allowlist. XML comes only from the builders in `lodger-core`, and input with `<!DOCTYPE` fails. Lodger runs no subprocess for libvirt. | In place | 1.2.1, 1.2.5, 1.5.1 |
| Information disclosure, elevation of privilege | Server-side request forgery through image import | HTTPS only, a check of every redirect, address filters, and a size cap | Planned for v1.0, with image import | 1.3.6, 15.3.2 |
| Spoofing, information disclosure | A stolen session | The cookie is `__Host-lodger_sid` with `Secure`, `HttpOnly`, and `SameSite=Strict`. Lodger stores only the SHA-256 hash of the token. A login makes a new token. Sessions end after 60 idle minutes or 24 hours. A password change ends every other session of the account. | In place | 3.3.1, 7.2.3, 7.2.4, 7.4.3 |
| Spoofing, information disclosure | A stolen session | A session list with a button that ends each session | Planned for v1.1 | 7.5.2 |
| Information disclosure | Eavesdropping on the LAN | Built-in TLS, or TLS from a reverse proxy. If TLS is set, Lodger never falls back to plain HTTP. HTTPS answers send `Strict-Transport-Security`. | In place, with one gap: Lodger still starts plain HTTP on a LAN address, with a warning. ASVS 12.2.1 is open. | 12.1.1, 12.2.1, 3.4.1 |
| Information disclosure | Data left in a shared browser after the session ends | If the user logs out or the session expires, the web app clears its cached data. | In place | 14.3.1 |
| Elevation of privilege, information disclosure | An unprivileged local user on the host | The systemd unit sets `UMask=0077`, `NoNewPrivileges`, `ProtectSystem=strict`, and an empty capability set. The state directory is mode 0700. New VMs get no VNC port. | In place. The VNC rule comes with VM creation in v1.0. | 13.2.2 |
| Tampering | A changed release archive | `install.sh` checks the SHA-256 against `checksums.txt`, and the cosign signature of `checksums.txt` against Lodger's release workflow. Releases carry SLSA provenance. | In place | 15.2.1 |
| Repudiation | A user denies an action | Every change to a VM, a storage object, a network, or an account writes an audit row and a journald copy. The row never holds a secret. | In place | 16.3.1, 16.4.2 |
| Denial of service | Floods of login attempts | The throttle keeps at most 10000 keys per map, so a flood cannot fill memory. A client IP with too many failures waits before its next attempt. | In place | 6.3.1 |

## When this document changes

A pull request that adds an endpoint, a new input from outside Lodger, or a new trust
boundary updates this document. Before v1.0, the new v1.0 features get a new ASVS Level 1
review.
