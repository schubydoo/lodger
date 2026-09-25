# Installation

Lodger runs on the host that runs your VMs. It needs:

- Debian 13 or Ubuntu 24.04 on x86_64. Fedora, Arch, Rocky Linux 9 and 10, and
  Debian 12 work on a best-effort basis.
- libvirt 9.0 or later, with its socket running. Snapshot revert needs libvirt 9.9.
- systemd, because Lodger installs as a systemd service.

## Install with one command

```sh
curl -fsSL https://raw.githubusercontent.com/schubydoo/lodger/main/install.sh | sudo bash
```

The script does these steps:

1. It downloads the newest release for Linux x86_64.
2. It checks the archive's SHA-256 against the release's `checksums.txt`.
3. If cosign is installed, it checks the cosign signature of `checksums.txt`. A bad
   or missing signature stops the install. Without cosign, the script warns and
   keeps only the SHA-256 check.
4. It runs `lodger install`, which checks the host, creates the `lodger` user in the
   `libvirt` group, writes `/etc/lodger/config.toml`, and starts the service.

The service listens on `127.0.0.1:8460`. The script accepts these options:

| Option | Effect |
| --- | --- |
| `VERSION=vX.Y.Z` | Installs that release instead of the newest one. Put it before `bash`: `sudo VERSION=v0.2.0 bash`. |
| `-s -- --self-signed <ip-or-name>` | Makes a self-signed certificate for that address, and turns on HTTPS. |
| `-s -- --uninstall` | Stops and removes the service and the binary. The configuration and the data stay. |

## Reach Lodger from the LAN

The session cookie is `Secure`, so a browser logs in only over HTTPS or on the host
itself. Choose one of these ways:

- Built-in TLS with a self-signed certificate. Pass the host's LAN address:

    ```sh
    curl -fsSL https://raw.githubusercontent.com/schubydoo/lodger/main/install.sh | sudo bash -s -- --self-signed 192.168.1.10
    ```

    The command prints the certificate's SHA-256 fingerprint. The browser warns about
    the certificate once. If the browser shows the same fingerprint, accept it. If not,
    do not.
    Then set `listen = "192.168.1.10:8460"` in `/etc/lodger/config.toml`, and run
    `sudo systemctl restart lodger`.

- Built-in TLS with your own certificate. Set `tls_cert` and `tls_key` in
  `/etc/lodger/config.toml`. The `lodger` user must be able to read both files. Set
  `listen` to the host's LAN address too, as above, and run
  `sudo systemctl restart lodger`.
- A [reverse proxy](reverse-proxy.md) that already serves your other sites.

Without TLS, Lodger refuses to start on an address that is not loopback, unless
`trusted_proxies` or `allow_plain_http = true` is set. Then it logs a warning at each
start, and a password crosses the network in clear text to any host that reaches it.

## Install by hand

Download the archive, `checksums.txt`, and `checksums.txt.sigstore.json` from the
[release page](https://github.com/schubydoo/lodger/releases). Then check them:

```sh
sha256sum --ignore-missing -c checksums.txt
cosign verify-blob checksums.txt --bundle checksums.txt.sigstore.json \
  --certificate-identity-regexp '^https://github\.com/schubydoo/lodger/\.github/workflows/knope-release\.yml@' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
gh attestation verify lodger-vX.Y.Z-linux-amd64.tar.gz --repo schubydoo/lodger
```

The last command checks the build provenance. It needs the GitHub CLI. Then unpack
the archive, and run the install:

```sh
tar -xzf lodger-vX.Y.Z-linux-amd64.tar.gz
sudo ./lodger-vX.Y.Z-linux-amd64/lodger install
```

The archive holds the binary, `LICENSE`, and `third-party-notices.txt`. `.deb` and
`.rpm` packages come with v1.0.

## Log in the first time

Lodger has no default account. At its first start, it writes a one-time setup token
to the journal:

```sh
sudo journalctl -u lodger | grep 'setup token'
```

Open Lodger in the browser, enter the token, and create the first account. The token
works once and for 60 minutes. A restart writes a new token until an account exists.

## Recover access

If you forget your password, or you lose every account, use the recovery commands on
the host. They need root, and they work without the web UI.

```sh
sudo lodger admin reset-password <username>
sudo lodger admin create <username>
```

`reset-password` sets a new password and ends every session of the account. `create`
adds an account with full rights. In a terminal, each command asks for the new
password. If standard input is not a terminal, the command reads one line from it. The
password follows the same rules as in the web UI, and each command writes an audit row.

## Check the host

```sh
sudo lodger doctor
```

`lodger doctor` checks the libvirt socket and connection, the `libvirt` group, and the
TLS certificate. It also checks snapshot revert, the AppArmor rule for NIC hot-plug, and
the SELinux boolean for NFS pools. It prints a fix for each problem and changes nothing.

## Uninstall

```sh
sudo lodger uninstall
```

This stops and removes the service and the binary. The configuration, the database,
and the `lodger` user stay, unless you add `--purge`. libvirt keeps every VM, pool,
and network.
