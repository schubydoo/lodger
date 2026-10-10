# ClusterFuzzLite build image. The OSS-Fuzz Rust base ships a nightly
# toolchain, cargo-fuzz, and the sanitizer runtimes. This file adds only the
# source and the build script. The fuzz crate depends only on lodger-core,
# which has no system dependency, so the image needs no libvirt headers.
# https://google.github.io/clusterfuzzlite/build-integration/rust/
FROM gcr.io/oss-fuzz-base/base-builder-rust@sha256:9178c8abf83e7a82d02c1a9bf7b2f564320f669f7a1cec512ae9a8c4ea078427

COPY . $SRC/lodger
WORKDIR $SRC/lodger
COPY .clusterfuzzlite/build.sh $SRC/build.sh
