---
default: minor
---

Add Start, Shut down, and Force off to the VM list: shut down sends an ACPI request, force off asks for the VM's name first, the list shows the new state when libvirt reports it, and every action writes a `vm.lifecycle` audit row.
