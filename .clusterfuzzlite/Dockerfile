# ClusterFuzzLite build image. The OSS-Fuzz Rust base ships a nightly
# toolchain, cargo-fuzz, and the sanitizer runtimes. This file adds only the
# source and the build script. The fuzz crate depends only on lodger-core,
# which has no system dependency, so the image needs no libvirt headers.
# https://google.github.io/clusterfuzzlite/build-integration/rust/
FROM gcr.io/oss-fuzz-base/base-builder-rust@sha256:73be59e48262b7243f815f1c981a6e753e95e4b13e4d085cc9fc06bd79d42daa

COPY . $SRC/lodger
WORKDIR $SRC/lodger
COPY .clusterfuzzlite/build.sh $SRC/build.sh
