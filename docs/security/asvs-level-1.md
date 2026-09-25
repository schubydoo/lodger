# ASVS 5.0 Level 1 checklist

This checklist records the review of Lodger against every Level 1 requirement of
OWASP ASVS 5.0.0. The review ran on 2026-09-24, before v0.2. The
[threat model](threat-model.md) explains the threats behind these controls.

Each requirement has one verdict:

- Pass: the code meets the requirement. The evidence names the code and the test that
  proves it.
- N/A: Lodger has no such feature. The evidence says why.

The totals: 56 pass and 14 N/A.

A pull request that adds an endpoint or a new input updates this checklist. Before
v1.0, the new v1.0 features get a new Level 1 review.

## V1 Encoding and sanitization

| ID | Verdict | Evidence |
| --- | --- | --- |
| 1.2.1 | Pass | Svelte renders text interpolation as text, and `web/src` has no `{@html}` or `innerHTML`. XML comes only from the builders in `crates/lodger-core/src/xml/`. Header values go through `HeaderValue`, which refuses control characters. |
| 1.2.2 | Pass | Route links encode each VM, pool, and network name with `encodeURIComponent`, because libvirt allows `#`, `?`, and `%` in a name. API paths use UUIDs, and the volume, ticket, and console URLs encode their parts. Every link is a relative path. Test: `encodes a name that holds URL characters` in `web/src/routes/vms/page.svelte.spec.ts`. |
| 1.2.3 | Pass | The server writes JSON only through serde, and the client only through `JSON.stringify`. Lodger builds no JavaScript at run time. |
| 1.2.4 | Pass | Every SQL value in `crates/lodger/src/db/` is a bound parameter. The `format!` calls in SQL insert only 3 time constants. |
| 1.2.5 | Pass | Lodger reaches libvirt through the `virt` bindings, with no subprocess. `lodger install` runs `systemd-sysusers`, `userdel`, and `systemctl` with fixed argument lists and no shell (`crates/lodger/src/install.rs`). |
| 1.3.1 | N/A | Lodger takes no HTML input. |
| 1.3.2 | Pass | `web/src` has no `eval` or `new Function`, and the Content Security Policy has no `'unsafe-eval'`. Test: `the_csp_allows_the_inline_script_by_its_hash_only`. |
| 1.5.1 | Pass | Every XML parse goes through `xml::parse`, which refuses `<!DOCTYPE` before the parser runs. Test: `a_doctype_is_rejected_before_parsing`. |

## V2 Validation and business logic

| ID | Verdict | Evidence |
| --- | --- | --- |
| 2.1.1 | Pass | The section [Input rules](#input-rules) below. |
| 2.2.1 | Pass | `crates/lodger-core/src/validate.rs` checks names, paths, hosts, bridges, and subnets against allowlists and limits. Formats and modes are enums. The create and change bodies of pools, networks, volumes, and VMs refuse unknown fields. |
| 2.2.2 | Pass | Each handler checks its input on the server before it acts. The web app checks only for empty fields. |
| 2.3.1 | Pass | Setup runs in a fixed order. Setup must be open, the token must be live and correct, and the name and password must be valid. Then one transaction adds the account. If any account exists, the transaction fails. Tests: `parallel_claims_create_exactly_one_account`, `closed_setup_answers_404_whatever_the_body`. |

## V3 Web frontend security

| ID | Verdict | Evidence |
| --- | --- | --- |
| 3.2.1 | Pass | Every response sends `X-Content-Type-Options: nosniff` and a CSP with `default-src 'self'` and `frame-ancestors 'none'`. The API answers only JSON, and an unknown `/api` path answers 404, never the HTML page. Test: `every_response_carries_the_security_headers`. |
| 3.2.2 | Pass | The UI shows all outside data, such as VM names, through Svelte text interpolation. |
| 3.3.1 | Pass | The cookie is `__Host-lodger_sid` with `Secure`, `HttpOnly`, `SameSite=Strict`, and `Path=/`. Test: `the_cookie_has_every_protection`. |
| 3.4.1 | Pass | With built-in TLS, every response sends `Strict-Transport-Security: max-age=31536000`. Behind a reverse proxy, the proxy must send it. Tests: `serve_answers_https_with_the_configured_pair` and the plain HTTP health test in `crates/lodger/tests/binary.rs`. |
| 3.4.2 | Pass | Lodger sends no CORS header at all. |
| 3.5.1 | Pass | A state-changing request needs `Sec-Fetch-Site: same-origin` or an `Origin` equal to `public_url`, and the session's `X-CSRF-Token`. Tests: `a_change_needs_the_csrf_token_and_the_lodger_origin`, `an_unsafe_request_from_another_origin_fails_with_403`. |
| 3.5.2 | Pass | Lodger does not rely on CORS preflight. The Origin rule applies to every method except GET and HEAD, whatever the `Content-Type`. |
| 3.5.3 | Pass | Every change uses POST, PATCH, or DELETE. A WebSocket upgrade needs a ticket from a CSRF-checked POST. |

## V4 API and web service

| ID | Verdict | Evidence |
| --- | --- | --- |
| 4.1.1 | Pass | Text assets send `charset=utf-8`, and API answers send `application/json`, which is always UTF-8. Test: `text_types_name_utf_8_and_binary_types_do_not`. |
| 4.4.1 | Pass | The web app uses `wss:` whenever the page uses HTTPS. The socket needs the `Secure` session cookie, which a browser sends over plain HTTP only on loopback. |

## V5 File handling

| ID | Verdict | Evidence |
| --- | --- | --- |
| 5.2.1 | N/A | Lodger has no file upload. Request bodies stay under axum's default limit of 2 MB. |
| 5.2.2 | N/A | Lodger accepts no files. |
| 5.3.1 | N/A | Lodger stores no uploads. It serves only the web app that the build embeds. |
| 5.3.2 | Pass | Lodger builds no file path from a volume name: libvirt looks the name up inside the pool. Pool paths must be absolute, with no `.` or `..`, and outside system folders. Static file paths with `..` answer 404. Test: `traversal_is_rejected_before_lookup`. |

## V6 Authentication

| ID | Verdict | Evidence |
| --- | --- | --- |
| 6.1.1 | Pass | The section [Login throttle](#login-throttle) below. |
| 6.2.1 | Pass | A password needs 15 characters. Tests: `short_and_long_passwords_fail`, `a_common_or_short_password_fails_with_a_clear_message`. |
| 6.2.2 | Pass | `POST /api/account/password` and the account page. |
| 6.2.3 | Pass | A change needs the current and the new password. Test: `a_password_change_needs_the_current_password_and_ends_other_sessions`. |
| 6.2.4 | Pass | Lodger refuses the 3000 most common passwords that are 15 characters or longer. Test: `the_list_has_3000_entries_that_meet_the_length_rule`. |
| 6.2.5 | Pass | Lodger has no composition rules, only the length and the common list. |
| 6.2.6 | Pass | Every password field uses `type="password"`. |
| 6.2.7 | Pass | No field blocks paste, and the fields carry `autocomplete="current-password"` or `"new-password"`. |
| 6.2.8 | Pass | Lodger verifies the password exactly as sent. It refuses a password over 1024 characters and never cuts one. |
| 6.3.1 | Pass | The login throttle. Tests: `the_sixth_failed_login_waits`, `one_ip_guessing_many_accounts_waits_too`, `an_ipv6_client_is_counted_by_its_64`. |
| 6.3.2 | Pass | No default account exists. The first account needs the one-time setup token, or `sudo lodger admin create`. |
| 6.4.1 | Pass | The setup token has 128 random bits, works once, and ends after 60 minutes. The first account sets its own password. Tests: `a_token_is_128_random_bits_in_hex`, `a_token_lives_60_minutes`, `an_expired_token_fails`. |
| 6.4.2 | Pass | Lodger has no password hints and no secret questions. |

## V7 Session management

| ID | Verdict | Evidence |
| --- | --- | --- |
| 7.2.1 | Pass | Every request looks up the SHA-256 of the cookie token in SQLite. Test: `protected_endpoints_answer_401_without_a_session`. |
| 7.2.2 | Pass | Sessions use random reference tokens. Lodger has no static API keys. |
| 7.2.3 | Pass | The token has 256 random bits, and the database stores only its SHA-256. Test: `a_token_is_256_random_bits_in_hex`. |
| 7.2.4 | Pass | A login makes a new token and ends the session of the cookie that the browser sent. A password change replaces the caller's session too. Tests: `a_login_ends_the_session_that_the_browser_had`, `a_password_change_needs_the_current_password_and_ends_other_sessions`. |
| 7.4.1 | Pass | Logout deletes the session. A session ends after 60 idle minutes or 24 hours. Open sockets close within 5 seconds. Tests: `login_then_logout_ends_the_session_everywhere`, `logging_out_closes_the_session_sockets_within_5_seconds`. |
| 7.4.2 | Pass | Deleting an account deletes its sessions. Test: `deleting_the_last_account_fails_and_a_delete_ends_its_sessions`. |

## V8 Authorization

| ID | Verdict | Evidence |
| --- | --- | --- |
| 8.1.1 | Pass | The section [Authorization](#authorization) below. |
| 8.2.1 | Pass | Every function except health, setup, and login needs a session. Test: `protected_endpoints_answer_401_without_a_session`. |
| 8.2.2 | Pass | Lodger has one tenant, and every account has full rights by design. The only per-user object, the caller's own password, comes from the session and not from the request. |
| 8.3.1 | Pass | The session, CSRF, and Origin checks are server middleware. The web app's redirects only change what it shows. |

## V9 Self-contained tokens and V10 OAuth

| ID | Verdict | Evidence |
| --- | --- | --- |
| 9.1.1 | N/A | Lodger uses no self-contained tokens, such as JWT. |
| 9.1.2 | N/A | Lodger uses no self-contained tokens. |
| 9.1.3 | N/A | Lodger uses no self-contained tokens. |
| 9.2.1 | N/A | Lodger uses no self-contained tokens. |
| 10.4.1 | N/A | Lodger is not an OAuth authorization server. |
| 10.4.2 | N/A | Lodger is not an OAuth authorization server. |
| 10.4.3 | N/A | Lodger is not an OAuth authorization server. |
| 10.4.4 | N/A | Lodger is not an OAuth authorization server. |
| 10.4.5 | N/A | Lodger is not an OAuth authorization server. |

## V11 Cryptography

| ID | Verdict | Evidence |
| --- | --- | --- |
| 11.3.1 | Pass | Lodger has no encryption code of its own. TLS uses rustls, which has no ECB or PKCS#1 v1.5 suites. |
| 11.3.2 | Pass | The rustls `ring` provider offers only AES-GCM and ChaCha20-Poly1305. |
| 11.4.1 | Pass | SHA-256 hashes the tokens, and argon2id hashes the passwords. SHA-1 appears only inside the WebSocket handshake, which RFC 6455 defines. |

## V12 Secure communication

| ID | Verdict | Evidence |
| --- | --- | --- |
| 12.1.1 | Pass | rustls allows only TLS 1.2 and TLS 1.3, and it prefers TLS 1.3. |
| 12.2.1 | Pass | If TLS is set, Lodger serves only HTTPS and never falls back. Without TLS, Lodger refuses to start on an address that is not loopback, unless `trusted_proxies` names a reverse proxy with TLS. `allow_plain_http = true` allows a TLS proxy that is not in `trusted_proxies`. If no TLS proxy sits in front, that host does not meet 12.2.1. Both cases log a warning at every start, and `lodger install` refuses the configuration without them. Tests: `serve_refuses_plain_http_on_the_network_without_the_opt_in`, `plain_http_on_the_network_gets_a_warning_that_fits_its_reason`, `plain_http_on_the_network_stops_the_install_unless_it_sets_tls`. |
| 12.2.2 | N/A | Lodger is a LAN admin UI, not an external service. If you expose Lodger to the internet, use a publicly trusted certificate. |

## V13 Configuration

| ID | Verdict | Evidence |
| --- | --- | --- |
| 13.4.1 | Pass | The binary embeds only `web/build/`, which holds no `.git` folder. Nothing serves the repository. |

## V14 Data protection

| ID | Verdict | Evidence |
| --- | --- | --- |
| 14.2.1 | Pass | The session token travels in the cookie, the CSRF token in a header, and passwords in JSON bodies. The WebSocket ticket is the only secret in a URL, because a browser cannot add a header to a WebSocket upgrade. The ticket works once, ends after 30 seconds, and is bound to its session and Origin. |
| 14.3.1 | Pass | When the session ends, by logout or by expiry, the web app clears its cached data. The web app uses no browser storage. Test: `drops the cached answers when the session ends without a logout`. |

## V15 Secure coding and architecture

| ID | Verdict | Evidence |
| --- | --- | --- |
| 15.1.1 | Pass | [SECURITY.md](../../SECURITY.md) gives the time frames for dependency fixes and updates. |
| 15.2.1 | Pass | On 2026-09-24, `cargo audit` and `cargo deny` report no advisory. `pnpm audit` reports 1 low advisory in `cookie`, a build-time dependency of SvelteKit, which is inside its time frame. |
| 15.3.1 | Pass | No answer holds a password hash or a session token. Account answers hold only the ID and the name. No endpoint lists sessions. |

## Input rules

This section covers ASVS 2.1.1. Lodger checks every input on the server with these rules.

- Names of VMs, pools, volumes, networks, and accounts that Lodger creates: 1 to 64
  ASCII characters from `A-Z`, `a-z`, `0-9`, `.`, `_`, and `-`. The first character
  is a letter or a digit. Account names are unique without regard to case.
- Names of objects that other tools created keep their names. API paths use UUIDs, so
  such a name needs no check, except that NUL is refused.
- Passwords: 15 to 1024 characters, counted as Unicode characters, and not on the
  common list. Lodger has no rule about character types.
- Setup token: 32 hex characters.
- Pool path: absolute, at most 4096 bytes, with no `.` or `..` segment, and not a
  system folder. The NFS host has at most 253 characters from `A-Z`, `a-z`, `0-9`,
  `.`, `:`, and `-`. Two pools cannot share a path or an NFS export.
- Network subnet: canonical IPv4 CIDR inside `10.0.0.0/8`, `172.16.0.0/12`, or
  `192.168.0.0/16`, with a prefix of /30 or shorter and no host bits. It must not
  overlap another network. A bridge name has 1 to 15 characters and must exist on the
  host.
- Volume: format `qcow2` or `raw`, and a size from 1 MiB to 1 PiB. The name must be
  new in the pool.
- VM actions: a fixed list. Force off and delete need the VM name typed again.
- Bodies: JSON only, with a limit of 2 MB, which is axum's default. The create and
  change bodies of pools, networks, volumes, and VMs refuse unknown fields.
- Headers: Lodger stores at most 256 characters of `User-Agent`. It reads `X-Real-IP`
  and `X-Forwarded-For` only from an address in `trusted_proxies`.

## Login throttle

This section covers ASVS 6.1.1. The throttle slows password guessing and credential
stuffing, and it never locks an account.

- The throttle covers login and the current-password check of a password change. The
  setup token has 128 random bits, so setup needs no throttle.
- Lodger counts failures per account and per client IP. An IPv6 client counts by its
  /64. The larger count applies.
- The first 5 failures in 15 minutes cost nothing. After that, each attempt waits
  1 second, then 2, 4, and so on, up to 60 seconds.
- Attempts from one IP run one after another. If an attempt must wait more than
  60 seconds, it gets `429 Too Many Requests` with `Retry-After`.
- A correct password clears the count of the account. The IP keeps its other failures,
  so one good account does not reset guesses at other accounts.
- No account locks, so an attacker cannot lock out the admin. At most 10000 keys per
  map stay in memory. A restart of Lodger resets the counts.
- An unknown account costs the same argon2 work as a known one, and it gets the same
  message.
- The numbers are fixed in the code. The only related setting is `trusted_proxies`,
  which decides the client IP behind a reverse proxy.

## Authorization

This section covers ASVS 8.1.1. Lodger has one role in v1: every account is an admin
with full rights.

- Without a session: `GET /api/health`, setup while no account exists, login, and the
  static web app.
- With a session: everything else under `/api`. Without a session, the answer is 401.
  A state-changing request also needs the session's `X-CSRF-Token`, or it gets 403.
- WebSockets need a live session, a single-use ticket from that session, and the same
  `Origin` that asked for the ticket.
- Every account can manage all VMs, pools, networks, volumes, and accounts. An account
  cannot delete the last account. A password change applies only to the caller's own
  account.
- `lodger admin reset-password` and `lodger admin create` need root on the host.
