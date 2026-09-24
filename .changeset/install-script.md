---
default: minor
---

Add `install.sh`: `curl -fsSL https://raw.githubusercontent.com/schubydoo/lodger/main/install.sh | sudo bash` downloads the latest release, verifies its SHA-256 and the cosign signature of `checksums.txt`, and runs `lodger install`. A tampered archive or signature stops it before anything is installed, and arguments after `--` go to `lodger install`, for example `--self-signed <ip>`.
