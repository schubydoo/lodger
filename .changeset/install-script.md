---
default: minor
---

Add `install.sh`: `curl -fsSL https://raw.githubusercontent.com/schubydoo/lodger/main/install.sh | sudo bash` downloads the latest release, verifies its SHA-256 and, if cosign is installed, the cosign signature of `checksums.txt`, and runs `lodger install`. A changed archive, or with cosign a bad or missing signature, stops it before anything is installed. Other arguments go to `lodger install`, for example `sudo bash -s -- --self-signed <ip>`.
