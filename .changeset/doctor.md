---
default: minor
---

Add `lodger doctor`: it checks the libvirt socket, the lodger user in the libvirt group, the AppArmor rule for NIC hot-plug, the SELinux virt_use_nfs boolean, the libvirt connection, and snapshot revert support, and prints PASS, FAIL, or SKIP with a reason and the commands that fix each failure.
