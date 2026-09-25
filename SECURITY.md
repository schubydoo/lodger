# Security policy

## Supported versions

Lodger is pre-release software. Only the newest release gets security fixes. There are
no backports to earlier releases.

## Reporting a vulnerability

Do not open a public issue for a vulnerability.

Report it privately through GitHub's private vulnerability reporting: open the
**Security** tab of this repository, then select **Report a vulnerability**, or go
directly to <https://github.com/schubydoo/lodger/security/advisories/new>.

Include these facts:

- The type of vulnerability and its impact.
- The affected file paths and a commit, tag, or branch.
- The steps to reproduce it, and any proof of concept that you have.

## What to expect

Lodger is a side project with one maintainer, so the times are best effort:

- An answer within a few days.
- Updates on the progress of the fix, and a message after the fix ships.
- Credit in the advisory, on request.

## Scope

Lodger controls libvirt, so a Lodger account can start, stop, and change the VMs on the
host. Report any way to get that control without a valid session. Report these problems
too:

- A login or session bypass.
- A state-changing request that passes without the Origin or CSRF check.
- A WebSocket that opens without a valid ticket.
- A secret in a log line or in the audit log.
- A release archive that does not match the signed `checksums.txt` or its provenance.

## Dependencies

CI runs `cargo audit`, `cargo deny`, and `pnpm audit` on every pull request, and
GitHub reports new advisories through Dependabot alerts. Renovate opens the update
pull requests. These time frames start when a fixed version of the dependency
exists:

| Advisory | Time frame |
| --- | --- |
| Critical or high, in code that the release binary runs | A fixed release within 7 days |
| Medium, in code that the release binary runs | A fixed release within 30 days |
| Low, or any advisory in a build-time or test-only dependency | The next release, and within 90 days |

If no fixed version exists, the maintainer removes the dependency or works around the
problem within the same time frame. Routine updates without an advisory merge at
least once a month.

## Security documents

- [Threat model](docs/security/threat-model.md)
- [ASVS 5.0 Level 1 checklist](docs/security/asvs-level-1.md)
