# Upgrading

To move to a new release, run the install command again:

```sh
curl -fsSL https://raw.githubusercontent.com/schubydoo/lodger/main/install.sh | sudo bash
```

The script checks the new release the same way as a first install. Then
`lodger install` replaces the binary in `/usr/local/bin` and restarts the service. It
keeps `/etc/lodger/config.toml`, the database in `/var/lib/lodger`, and the `lodger`
user. If the configuration names a TLS certificate, the upgrade keeps it.

## Before you upgrade

- Read the release notes on the [release page](https://github.com/schubydoo/lodger/releases).
  Until v1.0, a release can change the configuration, the API, or the stored data.
- Copy the database, so that you can go back:

    ```sh
    sudo systemctl stop lodger
    sudo cp -a /var/lib/lodger /var/lib/lodger.bak
    ```

Lodger has no downgrade. To go back, install the older release with
`VERSION=vX.Y.Z`, and restore the copy of `/var/lib/lodger`.

## After you upgrade

Run `sudo lodger doctor`, and log in once. The journal shows the start:

```sh
sudo journalctl -u lodger -n 20
```

## Changes that need your action

| Release | Change | What to do |
| --- | --- | --- |
| v0.2 | Lodger refuses to start on an address that is not loopback without TLS. | Set `tls_cert` and `tls_key`, or list your reverse proxy in `trusted_proxies`, before the upgrade. The [installation page](installation.md#reach-lodger-from-the-lan) gives the choices. |
