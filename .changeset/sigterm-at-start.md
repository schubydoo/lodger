---
default: patch
---

Fix a SIGTERM that arrives right after the start, for example `systemctl stop` during startup: Lodger now shuts down cleanly instead of being killed by the signal.
