# Reverse proxy

A reverse proxy is a web server that takes the browser's HTTPS connection and passes
each request on to Lodger. If a proxy already serves your other sites, use it for Lodger
too. If not, Lodger's built-in TLS is simpler: run `sudo lodger install --self-signed <ip-or-name>`,
or set `tls_cert` and `tls_key` in `/etc/lodger/config.toml`.

This page covers nginx, Caddy, and Nginx Proxy Manager. The nginx and Caddy files in
[`docs/proxy/`](https://github.com/schubydoo/lodger/tree/main/docs/proxy) are the ones that CI tests: `scripts/proxy-smoke.sh` logs in
through each proxy, opens the live updates socket, and checks the audit log.

## What every proxy must do

- Serve HTTPS to the browser. The session cookie is `Secure`, so a browser does not
  keep it over plain HTTP, and no login works.
- Set `X-Real-IP` to the client's address, and replace any `X-Real-IP` that the
  client sent. Lodger uses this address for the login throttle and the audit log.
- Pass the WebSocket upgrade. The live updates and the console use WebSockets.
- Keep an idle connection open for at least an hour. The live updates socket stays
  open while a page is open.
- Pass the `Host` header unchanged.

## Lodger's configuration

Set these keys in `/etc/lodger/config.toml`, then run `sudo systemctl restart lodger`.

| Key | Value |
| --- | --- |
| `public_url` | The address that browsers open, such as `https://lodger.example.lan`. A current browser marks each request from a Lodger page as same-origin, and Lodger accepts that at any address. When a browser sends no `Sec-Fetch-Site` header, Lodger accepts a change only from a page at this address. |
| `trusted_proxies` | The proxy's address, as seen by Lodger. Lodger reads `X-Real-IP` only from these addresses, and ignores it from anyone else. |
| `listen` | Where Lodger listens. It depends on where the proxy runs, as the next section shows. |

## Where the proxy runs

If the proxy runs on the host itself, or in Docker with `network_mode: host`, keep
Lodger on its default loopback address. Only programs on the host can then reach
Lodger.

```toml
listen = "127.0.0.1:8460"
public_url = "https://lodger.example.lan"
trusted_proxies = ["127.0.0.1/32"]
```

If the proxy runs in Docker on a bridge network, it cannot reach the host's
`127.0.0.1`. Let Lodger listen on the bridge's gateway address, and give the proxy
container a fixed address. Docker gives fixed addresses only on a network that you
create, so create one first:

```sh
docker network create --subnet 172.30.0.0/24 --gateway 172.30.0.1 proxy
```

Then run the proxy on it with `--network proxy --ip 172.30.0.2`, or with
`ipv4_address: 172.30.0.2` in Docker Compose, and set:

```toml
listen = "172.30.0.1:8460"
public_url = "https://lodger.example.lan"
trusted_proxies = ["172.30.0.2/32"]
```

The gateway address exists only after Docker starts and creates the network. Tell
systemd to start Lodger after Docker, or Lodger can fail to bind at boot and give
up. Run `sudo systemctl edit lodger`, and add:

```ini
[Unit]
After=docker.service
Wants=docker.service

[Service]
RestartSec=5
```

`RestartSec` covers the moment between Docker's start and the new address.

Trust only the proxy's own address, never the whole bridge network. Every container
on the bridge can reach this address, and a trusted container can write any
client address into the audit log.

Lodger refuses to start on an address that is not loopback without TLS, unless
`trusted_proxies` is set. With a proxy on the bridge, Lodger logs a warning at each
start: only the proxies in `trusted_proxies` must reach that address.

## nginx

Copy [`docs/proxy/nginx.conf`](proxy/nginx.conf) to `/etc/nginx/conf.d/lodger.conf`.
Change the server name, the certificate paths, and the `proxy_pass` address to
yours. Then run `sudo nginx -t` and `sudo systemctl reload nginx`.

## Caddy

Copy the site block from [`docs/proxy/Caddyfile`](proxy/Caddyfile) into your
Caddyfile. Change the site name and the `reverse_proxy` address to yours. For a
public name, remove the `tls internal` line, and Caddy gets a certificate itself.

Keep the `header_up X-Real-IP {remote_host}` line. Caddy does not set `X-Real-IP`,
and it passes on the one that a client sends. Without the line, a client can choose
the address that Lodger logs and throttles. The CI test fails without it.

## Nginx Proxy Manager

Nginx Proxy Manager runs nginx inside, and its default proxy configuration already sets
`X-Real-IP` to the client's address. Add a proxy host as follows:

1. On the Details tab, enter your domain name, choose the scheme `http`, and enter
   the forward host and port. With `network_mode: host`, the host is `127.0.0.1`.
   On a bridge network, the host is the gateway address that Lodger listens on.
2. Turn on Websockets Support. Without it, the live updates do not work.
3. On the SSL tab, choose a certificate, and turn on Force SSL.
4. On the Advanced tab, enter `proxy_read_timeout 1h;`.
5. Set `public_url` and `trusted_proxies` in Lodger's configuration, as shown above.

## Check the setup

Log in through the proxy, then run:

```sh
sudo journalctl -u lodger | grep login.succeeded | tail -1
```

The `client_ip` in that line must be your computer's address. If it is the proxy's
address, the proxy is not in `trusted_proxies`, or it does not send `X-Real-IP`.
