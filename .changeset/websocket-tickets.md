---
default: minor
---

Protect the WebSockets: each upgrade needs a live session and a single-use ticket from POST /api/ws-tickets that works for 30 seconds and only from the page origin that asked for it, and logging out closes the session's sockets within 5 seconds.
