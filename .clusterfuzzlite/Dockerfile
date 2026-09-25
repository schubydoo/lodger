# ClusterFuzzLite build image. The OSS-Fuzz Rust base ships a nightly
# toolchain, cargo-fuzz, and the sanitizer runtimes. This file adds only the
# source and the build script. The fuzz crate depends only on lodger-core,
# which has no system dependency, so the image needs no libvirt headers.
# https://google.github.io/clusterfuzzlite/build-integration/rust/
FROM gcr.io/oss-fuzz-base/base-builder-rust@sha256:1fea2c5bb1f452563710429f195f1d9661032866489dbcd9767aa8a1d1238758

COPY . $SRC/lodger
WORKDIR $SRC/lodger
COPY .clusterfuzzlite/build.sh $SRC/build.sh
