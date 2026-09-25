#!/bin/bash -eu
# Builds every cargo-fuzz target in fuzz/ and copies the libFuzzer binaries to
# $OUT for ClusterFuzzLite. Runs inside the base-builder-rust image, which
# sets the sanitizer flags through $RUSTFLAGS and $SANITIZER.

cd "$SRC/lodger"

# OSS-Fuzz pins an older nightly than the workspace's rust-version, and
# cargo-fuzz cannot pass --ignore-rust-version. lodger-core builds on that
# nightly, so this copy of the source declares the image's version instead.
# The MSRV job in ci.yml still checks the real minimum.
image_rust=$(rustc --version | awk '{print $2}' | cut -d- -f1)
sed -i "s/^rust-version = .*/rust-version = \"${image_rust}\"/" Cargo.toml
grep -q "^rust-version = \"${image_rust}\"" Cargo.toml

# `cargo fuzz build` follows OSS-Fuzz's sanitizer settings; -O builds release.
cargo fuzz build -O

# Every target that fuzz/Cargo.toml declares, so a new one needs no edit here.
out_dir="fuzz/target/x86_64-unknown-linux-gnu/release"
targets=$(cargo fuzz list)
[ -n "$targets" ] || { echo "cargo fuzz list found no target" >&2; exit 1; }
for target in $targets; do
    cp "${out_dir}/${target}" "${OUT}/"
done
