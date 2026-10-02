# ClusterFuzzLite build image. The OSS-Fuzz Rust base ships a nightly
# toolchain, cargo-fuzz, and the sanitizer runtimes. This file adds only the
# source and the build script. The fuzz crate depends only on lodger-core,
# which has no system dependency, so the image needs no libvirt headers.
# https://google.github.io/clusterfuzzlite/build-integration/rust/
FROM gcr.io/oss-fuzz-base/base-builder-rust@sha256:9643b583ef5aa3d956b3c170e93ef8283878c70c6df8c99d8a70d73c5019677f

COPY . $SRC/lodger
WORKDIR $SRC/lodger
COPY .clusterfuzzlite/build.sh $SRC/build.sh
