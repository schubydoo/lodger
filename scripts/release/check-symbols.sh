#!/usr/bin/env bash
# Release gate (TAD 6.3): fail if a Lodger binary needs a newer glibc or libvirt
# than the floors. The highest GLIBC_ symbol version must be 2.34 or lower
# (Rocky 9), and the highest LIBVIRT_ version must be 9.0.0 or lower (Debian 12).
# A binary built on Debian 13 needs GLIBC_2.39, so it fails here.
set -euo pipefail

bin=${1:?usage: check-symbols.sh <binary>}
GLIBC_MAX=2.34
LIBVIRT_MAX=9.0.0

# The highest version that the binary needs for a prefix, from `readelf -V`.
# The digit after the underscore leaves out GLIBC_PRIVATE and LIBVIRT_PRIVATE_*.
highest() {
  readelf -V --wide "$bin" | grep -oE "\b$1_[0-9]+(\.[0-9]+)*\b" | sed "s/^$1_//" | sort -uV | tail -1
}

check() {
  local prefix=$1 max=$2 got newest
  got=$(highest "$prefix" || true)
  if [ -z "$got" ]; then
    echo "::error::$bin needs no ${prefix}_ symbol version, so the check cannot work"
    exit 1
  fi
  newest=$(printf '%s\n%s\n' "$got" "$max" | sort -V | tail -1)
  if [ "$newest" != "$max" ]; then
    echo "::error::$bin needs ${prefix}_$got, but the limit is ${prefix}_$max"
    exit 1
  fi
  echo "${prefix}: the highest version is $got, and the limit is $max"
}

check GLIBC "$GLIBC_MAX"
check LIBVIRT "$LIBVIRT_MAX"
