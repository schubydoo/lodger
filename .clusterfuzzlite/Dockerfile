# ClusterFuzzLite build image. The OSS-Fuzz Rust base ships a nightly
# toolchain, cargo-fuzz, and the sanitizer runtimes. This file adds only the
# source and the build script. The fuzz crate depends only on lodger-core,
# which has no system dependency, so the image needs no libvirt headers.
# https://google.github.io/clusterfuzzlite/build-integration/rust/
FROM gcr.io/oss-fuzz-base/base-builder-rust@sha256:2216f0ac0a289a0e8974917779c3f744c354067dd81e0c8f2fc53bc1a9a573db

COPY . $SRC/lodger
WORKDIR $SRC/lodger
COPY .clusterfuzzlite/build.sh $SRC/build.sh
