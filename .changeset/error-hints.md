---
default: minor
---

Explain known libvirt errors: a failed action whose error Lodger knows now also shows the cause, the fix, and the commands to run on the host, starting with the AppArmor rule that a NIC hot-plug needs on Debian 13 and the rule that blocks deleting the current external snapshot; any other error shows libvirt's text unchanged.
