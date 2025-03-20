FROM rust:slim-bookworm AS builder

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
    libprotobuf-dev \
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
# Web page with map
WORKDIR /usr
COPY web ./web
# Runtime dependency for tokio and reqwest
RUN apt-get update && apt-get install -y --no-install-recommends libssl3
COPY --from=builder /usr/src/app/target/release/valhalla-debug /usr/local/bin/valhalla-debug
ENTRYPOINT ["valhalla-debug"]
