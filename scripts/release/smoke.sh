#!/usr/bin/env bash
# Release gate (TAD 6.3): start a Lodger binary on this distribution. It prints
# the version, which loads the libvirt client library, then serves on libvirt's
# test driver until it writes its address line.
set -euo pipefail

bin=${1:?usage: smoke.sh <binary>}
"$bin" version

state=$(mktemp -d)
# The address line goes to stdout, and the other lines go to stderr.
log="$state/serve.log"
"$bin" serve --listen 127.0.0.1:18460 --uri test:///default --state-dir "$state" >"$log" 2>&1 &
pid=$!
for _ in $(seq 1 100); do
  grep -q '^lodger listening on ' "$log" && break
  kill -0 "$pid" 2>/dev/null || break
  sleep 0.1
done
started=no
grep -q '^lodger listening on ' "$log" && started=yes
kill -TERM "$pid" 2>/dev/null || true
wait "$pid" || true
# The log holds a setup token for this throwaway state. Leave it out.
grep -v 'setup token' "$log" || true
if [ "$started" != yes ]; then
  echo "::error::lodger serve did not start"
  exit 1
fi
echo "lodger serve started"
