#!/usr/bin/env bash
# Sets up a Lodger development machine on Debian 13 or Ubuntu 24.04 and builds
# the project (PRD F14). Run it from the repository, as a user with sudo, or as
# root in a container:
#
#   scripts/bootstrap.sh
#
# It installs the system packages, the Rust toolchain of rust-toolchain.toml,
# Node from .node-version into ~/.local, and pnpm through corepack. Then it
# builds the web app and the debug server. It never starts or changes a VM, and
# it installs no libvirt daemon: the dev server uses libvirt's test driver.
set -euo pipefail

repo=$(cd "$(dirname "$0")/.." && pwd)
cd "$repo"

info() { printf '[info] %s\n' "$*"; }
die() { printf '[fail] %s\n' "$*" >&2; exit 1; }

sudo=
if [ "$(id -u)" != 0 ]; then
  command -v sudo >/dev/null || die "run as root, or install sudo"
  sudo=sudo
fi
command -v apt-get >/dev/null \
  || die "this script supports Debian 13 and Ubuntu 24.04. On another system, install the packages that CONTRIBUTING.md lists."

info "installing the system packages…"
$sudo apt-get update -qq
# env passes the variable through sudo, which drops it otherwise.
$sudo env DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends \
  build-essential pkg-config libvirt-dev rustup just git curl ca-certificates xz-utils >/dev/null

info "installing the Rust toolchain of rust-toolchain.toml…"
# The packaged rustup of Ubuntu 24.04 needs the toolchain name, and it cannot
# add a component later, so read both from the file and install them together.
channel=$(sed -n 's/^channel = "\(.*\)"$/\1/p' rust-toolchain.toml)
[ -n "$channel" ] || die "no channel in rust-toolchain.toml"
components=()
for c in $(sed -n 's/^components = \[\(.*\)\]$/\1/p' rust-toolchain.toml | tr -d '" ' | tr ',' ' '); do
  components+=(--component "$c")
done
# Ubuntu's rustup installs the toolchain, then fails to set itself up in
# ~/.cargo, which it does not use. So check the result, not the exit code.
rustup toolchain install "$channel" --profile minimal "${components[@]}" \
  || info "rustup reported an error after the install, so the next step checks the toolchain"
rustc_version=$(rustc --version)
case "$rustc_version" in
  "rustc $channel "*) info "$rustc_version" ;;
  *) die "rustc is '$rustc_version', not $channel" ;;
esac
if ! cargo clippy --version >/dev/null || ! cargo fmt --version >/dev/null; then
  die "rustup did not install the components of rust-toolchain.toml"
fi

node_version=$(tr -d '[:space:]' <.node-version)
case "$(uname -m)" in
  x86_64) node_arch=x64 ;;
  aarch64) node_arch=arm64 ;;
  *) die "no Node build for $(uname -m)" ;;
esac
node_dir=$HOME/.local/lib/node-v$node_version-linux-$node_arch
mkdir -p "$HOME/.local/bin" "$HOME/.local/lib"
if [ ! -x "$node_dir/bin/node" ]; then
  info "installing Node $node_version into $node_dir…"
  tarball=node-v$node_version-linux-$node_arch.tar.xz
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT
  curl -fsSL "https://nodejs.org/dist/v$node_version/$tarball" -o "$tmp/$tarball"
  curl -fsSL "https://nodejs.org/dist/v$node_version/SHASUMS256.txt" -o "$tmp/SHASUMS256.txt"
  (cd "$tmp" && grep " $tarball\$" SHASUMS256.txt | sha256sum -c --quiet -) \
    || die "the Node download does not match its SHA-256"
  tar -xJf "$tmp/$tarball" -C "$HOME/.local/lib"
fi
export PATH=$HOME/.local/bin:$node_dir/bin:$PATH
ln -sf "$node_dir/bin/node" "$node_dir/bin/npm" "$node_dir/bin/corepack" "$HOME/.local/bin/"
node --version

info "enabling pnpm through corepack…"
export COREPACK_ENABLE_DOWNLOAD_PROMPT=0
corepack enable --install-directory "$HOME/.local/bin" pnpm
(cd web && pnpm --version)

info "building the web app…"
(cd web && pnpm install --frozen-lockfile && pnpm run build)

info "building the debug server…"
cargo build -p lodger

cat <<EOF

Lodger is ready. Start it on libvirt's test driver, which needs no libvirtd:

  cargo run -p lodger -- serve --uri test:///default --state-dir /tmp/lodger-dev

Then open http://127.0.0.1:8460 and enter the setup token from the output.
Add ~/.local/bin to your PATH for node and pnpm. Run \`just check\` before a commit.
EOF
