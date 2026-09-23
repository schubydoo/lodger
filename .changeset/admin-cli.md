---
default: minor
---

Add `sudo lodger admin reset-password` and `sudo lodger admin create` for recovery without the web UI: the reset sets a new password and ends every session of the account, both commands need root and write an audit row.
