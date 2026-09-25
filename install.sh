#!/usr/bin/env bash
# Lodger installer. Downloads the signed release for Linux amd64, verifies it
# against the cosign-signed checksums.txt, and runs `lodger install`, which
# creates the lodger user and the systemd service.
#
#   curl -fsSL https://raw.githubusercontent.com/schubydoo/lodger/main/install.sh | sudo bash
#
# Other arguments go to `lodger install`, for example a self-signed
# certificate for the LAN:
#
#   curl -fsSL .../install.sh | sudo bash -s -- --self-signed 192.168.1.10
#
# Env overrides:
#   VERSION          release tag to install (default: the latest), e.g. VERSION=v0.1.0
#   LODGER_BASE_URL  where the release files are (default: the GitHub release of VERSION)
set -euo pipefail

REPO="schubydoo/lodger"
TOOL="lodger"

# --- output helpers ----------------------------------------------------------
if [ -t 1 ]; then
  B=$'\033[1m'; G=$'\033[32m'; Y=$'\033[33m'; R=$'\033[31m'; N=$'\033[0m'
else
  B=""; G=""; Y=""; R=""; N=""
fi
info() { printf '%s[info]%s %s\n' "$B" "$N" "$*"; }
ok()   { printf '%s[ ok ]%s %s\n' "$G" "$N" "$*"; }
warn() { printf '%s[warn]%s %s\n' "$Y" "$N" "$*" >&2; }
die()  { printf '%s[fail]%s %s\n' "$R" "$N" "$*" >&2; exit 1; }

have() { command -v "$1" >/dev/null 2>&1; }

usage() {
  cat <<EOF
Lodger installer.

  (no argument)   download, verify, and install the latest release
  <args>          pass <args> to 'lodger install', e.g. --self-signed 192.168.1.10
  --uninstall     stop and remove the service and the binary (keeps the data)
  --help          show this message

Env: VERSION=vX.Y.Z (tag to install), LODGER_BASE_URL=<url> (release files).

  install:   curl -fsSL https://raw.githubusercontent.com/${REPO}/main/install.sh | sudo bash
  uninstall: curl -fsSL https://raw.githubusercontent.com/${REPO}/main/install.sh | sudo bash -s -- --uninstall
EOF
}

case "${1:-}" in
  --help | -h | help) usage; exit 0 ;;
  --uninstall | uninstall)
    [ "$(id -u)" = "0" ] || die "run as root: ... | sudo bash -s -- --uninstall"
    have "$TOOL" || die "no ${TOOL} on PATH, so there is nothing to uninstall"
    exec "$TOOL" uninstall
    ;;
  # `bash -s -- <args>` hands the arguments over without the `--`, so every
  # other argument goes to `lodger install`. A leading `--` is dropped too.
  --) shift ;;
  *) : ;;
esac

# --- platform ----------------------------------------------------------------
[ "$(uname -s)" = "Linux" ] || die "Lodger runs only on Linux hosts with libvirt"
case "$(uname -m)" in
  x86_64 | amd64) arch="amd64" ;;
  *) die "unsupported architecture '$(uname -m)': Lodger ships linux amd64 only for now" ;;
esac
[ "$(id -u)" = "0" ] || die "run as root: curl -fsSL .../install.sh | sudo bash"

# --- prerequisites -----------------------------------------------------------
have tar || die "need 'tar' to extract the release"
have sha256sum || die "need 'sha256sum' to verify the download"
if have curl; then
  fetch() { curl -fsSL "$1"; }
  download() { curl -fsSL "$1" -o "$2"; }
elif have wget; then
  fetch() { wget -qO- "$1"; }
  download() { wget -qO "$2" "$1"; }
else
  die "need 'curl' or 'wget' to download the release"
fi

# --- resolve the version -----------------------------------------------------
tag="${VERSION:-}"
if [ -z "$tag" ]; then
  info "resolving the latest release…"
  # Read the whole answer first: `curl | grep -m1` ends curl with a broken
  # pipe, and under pipefail that would stop the script without a message.
  latest="$(fetch "https://api.github.com/repos/${REPO}/releases/latest")" \
    || die "could not ask GitHub for the latest release; set VERSION=vX.Y.Z"
  tag="$(printf '%s\n' "$latest" | sed -nE 's/.*"tag_name":[[:space:]]*"([^"]+)".*/\1/p' | head -1)"
  [ -n "$tag" ] || die "could not resolve the latest release tag; set VERSION=vX.Y.Z"
fi
case "$tag" in
  v[0-9]*.[0-9]*.[0-9]*) : ;;
  *) die "'$tag' is not a release tag such as v0.1.0" ;;
esac
base="${LODGER_BASE_URL:-https://github.com/${REPO}/releases/download/${tag}}"
pkg="lodger-${tag}-linux-${arch}"
tarball="${pkg}.tar.gz"

# --- download + verify -------------------------------------------------------
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

info "downloading ${tarball}…"
download "${base}/${tarball}" "${tmp}/${tarball}" || die "download failed: ${base}/${tarball}"
download "${base}/checksums.txt" "${tmp}/checksums.txt" || die "could not fetch checksums.txt"

info "verifying the SHA-256 against the release checksums…"
( cd "$tmp" && grep " ${tarball}\$" checksums.txt | sha256sum -c - ) \
  || die "checksum verification failed for ${tarball}; nothing was installed"
ok "checksum verified"

# The SHA-256 alone does not protect against a tampered mirror: whoever swaps
# the archive can swap checksums.txt to match. The cosign signature is the real
# check, and a failed check stops the install. Every release has a signature
# bundle, so a missing bundle stops the install too. Only a host without cosign
# falls back to the checksum, with a warning.
if ! have cosign; then
  warn "cosign is not installed, so only the SHA-256 was verified. Install cosign for the signature check."
elif ! download "${base}/checksums.txt.sigstore.json" "${tmp}/checksums.txt.sigstore.json" 2>/dev/null; then
  die "could not fetch the signature bundle (checksums.txt.sigstore.json), which every release has; refusing to install a possibly tampered release. Nothing was installed."
elif ( cd "$tmp" && cosign verify-blob checksums.txt \
        --bundle checksums.txt.sigstore.json \
        --certificate-identity-regexp "^https://github\.com/${REPO}/\.github/workflows/knope-release\.yml@" \
        --certificate-oidc-issuer https://token.actions.githubusercontent.com >/dev/null 2>&1 ); then
  ok "cosign signature verified"
else
  die "cosign signature verification FAILED for checksums.txt; refusing to install a possibly tampered release. Nothing was installed."
fi

# --- install -----------------------------------------------------------------
tar -xzf "${tmp}/${tarball}" -C "$tmp"
src="${tmp}/${pkg}/${TOOL}"
[ -f "$src" ] || die "unexpected archive layout: ${src} not found"
info "running '${TOOL} install${*:+ $*}'…"
# `lodger install` checks the host first and changes nothing if a check fails.
# It copies itself to /usr/local/bin, so the temporary folder can go.
"$src" install "$@"
ok "Lodger ${tag} is installed. Run 'sudo lodger doctor' to check the host."
