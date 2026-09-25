#!/usr/bin/env bash
# Tests the reverse proxy configurations in docs/proxy/ (Task 3.10).
#
# Usage: scripts/proxy-smoke.sh <nginx|caddy> <host|bridge>
#
#   host    The proxy runs on the host network, and Lodger listens on
#           127.0.0.1, as with Nginx Proxy Manager and `network_mode: host`.
#   bridge  The proxy runs on a Docker bridge, and Lodger listens on the
#           bridge's gateway address, with the proxy in trusted_proxies.
#
# A client container with its own address logs in through the proxy with
# forged X-Real-IP and X-Forwarded-For headers, and opens the events socket.
# The audit log must show the client's address and never a forged one. In the
# bridge layout, the client also sends the forged headers to Lodger directly,
# which must ignore them. Every container is unprivileged.
#
# Needs docker, openssl, python3, and a built server in LODGER_BIN
# (default: target/debug/lodger).
set -euo pipefail

# renovate: the image pins of this test.
NGINX_IMAGE=nginx:1.30.5@sha256:b972f831f200b19ef0767938224f9711e74cd783718738cd7405d5cabf75c442
CADDY_IMAGE=caddy:2.11.4@sha256:0c994536bddb66445885237f1a5dcc1916bccea922661c76b4e9fc24061f9b52
CURL_IMAGE=curlimages/curl:8.22.0@sha256:58adaa4e8dca9c988bae2aba4ab3434a0bb2da16bbe3f92dec39ec7785166777

proxy=${1:?usage: proxy-smoke.sh <nginx|caddy> <host|bridge>}
layout=${2:?usage: proxy-smoke.sh <nginx|caddy> <host|bridge>}
case "$proxy/$layout" in
  nginx/host | nginx/bridge | caddy/host | caddy/bridge) ;;
  *) echo "usage: proxy-smoke.sh <nginx|caddy> <host|bridge>" >&2; exit 2 ;;
esac

repo=$(cd "$(dirname "$0")/.." && pwd)
bin=${LODGER_BIN:-$repo/target/debug/lodger}
[ -x "$bin" ] || { echo "no server at $bin: run cargo build -p lodger" >&2; exit 1; }

NET=lodger-proxy-smoke
GATEWAY=172.31.99.1
PROXY_IP=172.31.99.2
CLIENT_IP=172.31.99.10
LODGER_PORT=18460
PROXY_PORT=18443
NAME=lodger.example.lan
ORIGIN=https://$NAME:$PROXY_PORT
FORGED=(-H "X-Real-IP: 203.0.113.9" -H "X-Forwarded-For: 203.0.113.8")

work=$(mktemp -d)
lodger_pid=
cleanup() {
  docker rm -f "$NET-proxy" >/dev/null 2>&1 || true
  docker network rm "$NET" >/dev/null 2>&1 || true
  if [ -n "$lodger_pid" ]; then kill -TERM "$lodger_pid" 2>/dev/null || true; fi
  rm -rf "$work"
}
trap cleanup EXIT
fail() { echo "FAIL: $*" >&2; echo "--- Lodger log:" >&2; cat "$work/lodger.log" >&2; exit 1; }

docker network create --subnet 172.31.99.0/24 --gateway "$GATEWAY" "$NET" >/dev/null

if [ "$layout" = host ]; then
  listen=127.0.0.1:$LODGER_PORT
  trusted=127.0.0.1/32
  # A host-network proxy listens on every host address, the gateway included.
  proxy_addr=$GATEWAY
else
  listen=$GATEWAY:$LODGER_PORT
  trusted=$PROXY_IP/32
  proxy_addr=$PROXY_IP
fi

# The documented file, with only the port and the upstream address changed.
if [ "$proxy" = nginx ]; then
  conf=$work/lodger.conf
  sed -e "s/listen 443 ssl;/listen $PROXY_PORT ssl;/" \
    -e "s#proxy_pass http://127.0.0.1:8460;#proxy_pass http://$listen;#" \
    "$repo/docs/proxy/nginx.conf" >"$conf"
  if ! grep -q "listen $PROXY_PORT ssl;" "$conf" || ! grep -q "proxy_pass http://$listen;" "$conf"; then
    fail "the nginx.conf edits did not apply"
  fi
  mkdir -p "$work/certs"
  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 1 \
    -subj "/CN=$NAME" -addext "subjectAltName=DNS:$NAME" \
    -keyout "$work/certs/lodger.key" -out "$work/certs/lodger.crt" 2>/dev/null
  chmod 644 "$work/certs/lodger.key"
  mounts=(-v "$conf:/etc/nginx/conf.d/default.conf:ro" -v "$work/certs:/etc/nginx/certs:ro")
  image=$NGINX_IMAGE
else
  conf=$work/Caddyfile
  sed -e "s/^$NAME {/$NAME:$PROXY_PORT {/" \
    -e "s/reverse_proxy 127.0.0.1:8460 {/reverse_proxy $listen {/" \
    "$repo/docs/proxy/Caddyfile" >"$conf"
  if ! grep -q "^$NAME:$PROXY_PORT {" "$conf" || ! grep -q "reverse_proxy $listen {" "$conf"; then
    fail "the Caddyfile edits did not apply"
  fi
  mounts=(-v "$conf:/etc/caddy/Caddyfile:ro")
  image=$CADDY_IMAGE
fi

cat >"$work/config.toml" <<EOF
public_url = "$ORIGIN"
trusted_proxies = ["$trusted"]
EOF
"$bin" serve --listen "$listen" --uri test:///default --state-dir "$work/state" \
  --config "$work/config.toml" >"$work/lodger.out" 2>"$work/lodger.log" &
lodger_pid=$!
for _ in $(seq 100); do
  grep -q "^lodger listening on" "$work/lodger.out" && break
  kill -0 "$lodger_pid" 2>/dev/null || fail "Lodger exited"
  sleep 0.1
done
token=$(sed -n 's/^lodger: setup token: \([0-9a-f]*\).*/\1/p' "$work/lodger.log")
[ -n "$token" ] || fail "no setup token in the log"

if [ "$layout" = host ]; then
  docker run -d --name "$NET-proxy" --network host "${mounts[@]}" "$image" >/dev/null
else
  docker run -d --name "$NET-proxy" --network "$NET" --ip "$PROXY_IP" "${mounts[@]}" "$image" >/dev/null
fi

# One request from the client container, which has its own address.
client() {
  docker run --rm --network "$NET" --ip "$CLIENT_IP" "$CURL_IMAGE" \
    -sk --resolve "$NAME:$PROXY_PORT:$proxy_addr" "$@"
}

# The proxy needs a moment to start (Caddy makes its certificate first).
code=$(client -o /dev/null -w '%{http_code}' --retry 30 --retry-all-errors --retry-delay 1 \
  "$ORIGIN/api/health") || true
[ "$code" = 200 ] || fail "GET /api/health through the proxy answered $code"

json='{"token":"'$token'","username":"admin","password":"correct horse battery staple"}'
code=$(client -o /dev/null -w '%{http_code}' -X POST -H "Content-Type: application/json" \
  -H "Origin: $ORIGIN" "${FORGED[@]}" -d "$json" "$ORIGIN/api/setup")
[ "$code" = 201 ] || fail "setup through the proxy answered $code"

json='{"username":"admin","password":"correct horse battery staple"}'
out=$(client -si -X POST -H "Content-Type: application/json" -H "Origin: $ORIGIN" \
  "${FORGED[@]}" -d "$json" "$ORIGIN/api/session")
cookie=$(printf '%s\n' "$out" | sed -n 's/^[Ss]et-[Cc]ookie: \(__Host-lodger_sid=[0-9a-f]*\).*/\1/p')
csrf=$(printf '%s\n' "$out" | sed -n 's/.*"csrf_token":"\([0-9a-f]*\)".*/\1/p')
if [ -z "$cookie" ] || [ -z "$csrf" ]; then fail "no session from the login: $out"; fi

out=$(client -s -X POST -H "Cookie: $cookie" -H "X-CSRF-Token: $csrf" -H "Origin: $ORIGIN" \
  "$ORIGIN/api/ws-tickets")
ticket=$(printf '%s\n' "$out" | sed -n 's/.*"ticket":"\([^"]*\)".*/\1/p')
[ -n "$ticket" ] || fail "no WebSocket ticket: $out"
# A WebSocket key is 16 random bytes in base64. curl stops at its time
# limit, because the socket stays open.
key=$(head -c 16 /dev/urandom | base64)
out=$(client -si --http1.1 --max-time 3 -H "Connection: Upgrade" -H "Upgrade: websocket" \
  -H "Sec-WebSocket-Version: 13" -H "Sec-WebSocket-Key: $key" \
  -H "Origin: $ORIGIN" -H "Cookie: $cookie" "$ORIGIN/ws/events?ticket=$ticket") || true
printf '%s\n' "$out" | head -1 | grep -q "^HTTP/1.1 101" \
  || fail "the events socket did not upgrade through the proxy: $(printf '%s\n' "$out" | head -1)"

events=(setup.completed login.succeeded)
if [ "$layout" = bridge ]; then
  # Straight to Lodger, from a peer that is not a trusted proxy.
  code=$(docker run --rm --network "$NET" --ip "$CLIENT_IP" "$CURL_IMAGE" -s -o /dev/null \
    -w '%{http_code}' -X POST -H "Content-Type: application/json" \
    -H "Sec-Fetch-Site: same-origin" "${FORGED[@]}" \
    -d '{"username":"admin","password":"not the password"}' "http://$listen/api/session")
  [ "$code" = 401 ] || fail "the direct login answered $code"
  events+=(login.failed)
fi

python3 - "$work/lodger.log" "$CLIENT_IP" "${events[@]}" <<'PY' || fail "the audit log is wrong"
import json, sys
log, client, events = sys.argv[1], sys.argv[2], sys.argv[3:]
rows = [json.loads(l.split("lodger: audit: ", 1)[1]) for l in open(log) if l.startswith("lodger: audit: ")]
ok = True
for event in events:
    ips = [r.get("client_ip") for r in rows if r["event"] == event]
    if ips != [client]:
        print(f"{event}: client_ip {ips}, want [{client!r}]", file=sys.stderr)
        ok = False
if any("203.0.113." in json.dumps(r) for r in rows):
    print("a forged address reached the audit log", file=sys.stderr)
    ok = False
sys.exit(0 if ok else 1)
PY

echo "PASS: $proxy/$layout: login and events socket through the proxy; the audit log shows $CLIENT_IP for: ${events[*]}"
