#!/bin/sh
# Starts the debug server for the end-to-end tests on libvirt's test driver,
# with a fresh state folder, so the first page is the setup page. The tests
# read the setup token from the log. Build the server and the web app first:
# `cargo build -p lodger` and `pnpm run build`.
set -eu

port=${1:?usage: serve.sh <port>}
here=$(cd "$(dirname "$0")" && pwd)
state="$here/.state"
rm -rf "${state:?}"
mkdir -p "$state"
exec "$here/../../target/debug/lodger" serve \
	--listen "127.0.0.1:$port" --uri test:///default --state-dir "$state/db" \
	2>"$state/server.log"
