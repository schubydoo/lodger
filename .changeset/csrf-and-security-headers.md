---
default: minor
---

Protect state-changing requests from other sites: each needs Sec-Fetch-Site: same-origin or an Origin equal to public_url, and a logged-in request also needs its session's X-CSRF-Token. Every response now carries a Content Security Policy, Referrer-Policy: no-referrer, and X-Content-Type-Options: nosniff.
