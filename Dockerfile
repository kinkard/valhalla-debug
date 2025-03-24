# Valhalla relies on protobuf dynamic library that should match in both build and runtime environments.
# It would probably be easy just to use `libprotobuf-dev` in both places, but `libprotobuf-lite32` is much smaller.
ARG protobuf_version=3.21.12-3

FROM rust:slim-bookworm AS builder
ARG protobuf_version

# System dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    # Required for tokio and reqwest via `openssl-sys`
    libssl-dev \
    pkg-config \
    # For some reason GCC fails to compile valhalla so we use clang instead
    clang \
    # Valhalla build dependencies
    build-essential \
    cmake \
    libboost-dev \
    liblz4-dev \
    libprotobuf-dev=$protobuf_version \
    protobuf-compiler \
    zlib1g-dev

ENV CC=clang CXX=clang++

WORKDIR /usr/src/app

# One day it would be a nice stand-alone crate, but for now we just copy the source
COPY libvalhalla ./libvalhalla

# First build a dummy target to cache dependencies in a separate Docker layer
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() { println!("Dummy image called!"); }' > src/main.rs
RUN cargo build --release

# Now build the real target
COPY src ./src
# Update modified attribute as otherwise cargo won't rebuild it
RUN touch -a -m ./src/main.rs
RUN cargo build --release

FROM debian:bookworm-slim AS runner
ARG protobuf_version
# Web page with map
WORKDIR /usr
COPY web ./web
# Runtime dependency for tokio/reqwest and valhalla
RUN apt-get update && apt-get install -y --no-install-recommends libssl3 libprotobuf-lite32=$protobuf_version
COPY --from=builder /usr/src/app/target/release/valhalla-debug /usr/local/bin/valhalla-debug
ENTRYPOINT ["valhalla-debug"]
