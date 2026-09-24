---
default: minor
---

Add the storage volumes API: `GET` and `POST /api/pools/{id}/volumes` list and create qcow2 and raw volumes in any running pool, and `DELETE /api/pools/{id}/volumes/{name}` deletes a volume unless a VM uses it, in which case the answer names the VMs. A duplicate name, a bad name, or a size outside 1 MiB to 1 PiB fails before any change.
