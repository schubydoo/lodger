#!/usr/bin/env bash
# Start a Lodger binary in a container of one distribution (TAD 6.3). It
# installs the libvirt client library, then runs smoke.sh. Run from the
# repository root:
#   scripts/release/smoke-in.sh <image> <binary>
# The container is a plain `docker run` with only this directory mounted.
set -euo pipefail

image=${1:?usage: smoke-in.sh <image> <binary>}
bin=${2:?usage: smoke-in.sh <image> <binary>}

case "$image" in
  debian*|ubuntu*)
    install='apt-get -qq update && apt-get -qq install -y --no-install-recommends libvirt0 >/dev/null' ;;
  fedora*|rockylinux/*)
    install='dnf -q -y install libvirt-libs' ;;
  archlinux*)
    install='pacman -Sy --noconfirm --needed libvirt >/dev/null' ;;
  *)
    echo "::error::no install command for $image"; exit 1 ;;
esac

docker run --rm -v "$PWD:/src:ro" -w /src "$image" \
  bash -c "set -euo pipefail; $install; scripts/release/smoke.sh '$bin'"
