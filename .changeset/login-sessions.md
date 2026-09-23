---
default: minor
---

Add login and logout (POST, GET, and DELETE /api/session) with a __Host- session cookie that ends after 60 idle minutes or 24 hours, require a session on every API endpoint except health, setup, and login, and slow down repeated failed logins per account and per client IP.
