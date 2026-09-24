---
default: minor
---

Add virtual networks. The Networks page lists every network and creates a NAT, isolated, or host bridge network, which starts at once with autostart on. A subnet that overlaps another network is rejected with that network's name, and bridge mode explains that a host bridge must exist first, because Lodger never changes the host's network. Each network's page starts and stops it, switches autostart, and deletes it after the typed name, listing the VMs on it first.
