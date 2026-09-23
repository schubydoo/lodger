---
default: minor
---

Add first-run setup: at a start with no accounts, Lodger writes a one-time setup token to the log, and POST /api/setup uses it to create the first account, with a password of at least 15 characters that is not among the 3000 most common ones.
