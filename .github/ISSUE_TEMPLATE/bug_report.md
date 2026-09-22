---
name: Bug Report
about: Lodger shows the wrong state, an action fails, or the UI breaks
title: '[BUG] '
labels: bug
assignees: ''
---

## What happened

A clear description of the bug, and what you expected instead.

## Steps to reproduce

1. Open '...' in Lodger
2. Click '...'
3. See '...'

## Does `virsh` agree?

Run the same action with `virsh` (for example `virsh start <vm>`). Does it work there?
This tells a Lodger bug apart from a libvirt or host problem.

## Environment

- Lodger version: [the output of `lodger --version`]
- Install method: [install.sh / .deb / .rpm / tarball / from source]
- OS & arch: [for example: Debian 13 amd64]
- libvirt version: [the output of `virsh --version`]
- `lodger doctor` output, once that command exists
- Browser: [for example: Firefox 131]

## Logs

The output of `journalctl -u lodger` around the time of the bug. Redact host names,
IP addresses, and anything private.

```
paste output here
```

## Additional context

Anything else that helps us fix the bug.
