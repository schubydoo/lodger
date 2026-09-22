---
default: minor
---

Add `GET /api/host`, `GET /api/vms`, `GET /api/vms/{id}`, and the `/ws/events` WebSocket, which read a live copy of libvirt's inventory, and add `lodger serve --uri` to choose the libvirt connection (default `qemu:///system`).
