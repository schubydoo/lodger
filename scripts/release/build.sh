#!/usr/bin/env bash
# Build the Lodger release binary inside rockylinux/rockylinux:9 (TAD 6.3). Its
# glibc 2.34 sets the floor, so one binary starts on every tested distribution.
# web/build must exist first: the release binary embeds it. Run from the
# repository root, for example:
#   docker run --rm -v "$PWD:/src" -w /src <rocky 9 image> scripts/release/build.sh
# The binary lands in target/release/lodger, and the symbol gate runs on it.
set -euo pipefail

# rustup-init, pinned by version and SHA-256. The toolchain itself comes from
# rust-toolchain.toml.
RUSTUP_VERSION=1.29.1
RUSTUP_SHA256=dda7234360b7f578ca8b0ddcb80145646fa61a67c1720a5abc7051b35c9fcb71

[ -f web/build/200.html ] || { echo "::error::web/build is missing: build the web UI first"; exit 1; }

dnf -q -y install dnf-plugins-core
# libvirt-devel is in the CRB repository.
dnf config-manager --set-enabled crb
dnf -q -y install gcc libvirt-devel pkgconf-pkg-config binutils

curl --proto '=https' --tlsv1.2 -sSfL -o /tmp/rustup-init \
  "https://static.rust-lang.org/rustup/archive/${RUSTUP_VERSION}/x86_64-unknown-linux-gnu/rustup-init"
echo "${RUSTUP_SHA256}  /tmp/rustup-init" | sha256sum -c -
chmod +x /tmp/rustup-init
/tmp/rustup-init -y -q --profile minimal --default-toolchain none
# shellcheck source=/dev/null
. "$HOME/.cargo/env"

cargo build --release --locked -p lodger
# The container runs as root. Give the build back to the caller's user.
if [ -n "${HOST_UID:-}" ]; then chown -R "$HOST_UID" target; fi
scripts/release/check-symbols.sh target/release/lodger
