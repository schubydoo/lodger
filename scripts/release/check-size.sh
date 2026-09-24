#!/usr/bin/env bash
# Release gate (PRD 5.1, TAD 8.1): fail if the Lodger binary, web assets
# included, reaches 30 MiB. It writes the size to the CI summary when GitHub
# gives one.
set -euo pipefail

bin=${1:?usage: check-size.sh <binary> [limit in bytes]}
limit=${2:-$((30 * 1024 * 1024))}

size=$(stat -c %s "$bin")
mib() { awk -v b="$1" 'BEGIN { printf "%.1f MiB", b / 1048576 }'; }
line="Binary size: $(mib "$size") ($size bytes). The limit is $(mib "$limit")."
echo "$line"
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  echo "$line" >>"$GITHUB_STEP_SUMMARY"
fi
if [ "$size" -ge "$limit" ]; then
  echo "::error::$bin is $(mib "$size"), which reaches the limit of $(mib "$limit")"
  exit 1
fi
