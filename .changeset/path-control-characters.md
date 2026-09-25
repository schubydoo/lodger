---
default: patch
---

A pool path or an NFS export with a control character now fails with a clear message, instead of an error from libvirt, which cannot parse the XML that such a path makes.
