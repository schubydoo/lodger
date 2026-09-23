---
default: minor
---

Add the audit log: setup, logins, and account changes write a row with the account, client IP, target, and result, each row has a copy in journald, and a daily task deletes rows older than 365 days.
