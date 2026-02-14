FROM rust:slim-trixie AS builder
ARG protobuf_version

# System dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    # LLVM toolchain for proper LTO support between Rust and C/C++
    clang \
    llvm \
    lld \
    # Valhalla build dependencies
    build-essential \
    cmake \
    libboost-dev \
    liblz4-dev \
    libprotobuf-dev \
    protobuf-compiler \
    zlib1g-dev

# https://doc.rust-lang.org/beta/rustc/linker-plugin-lto.html
ENV CC=clang CXX=clang++ AR=llvm-ar RANLIB=llvm-ranlib
ENV RUSTFLAGS="-Clink-arg=-fuse-ld=lld"

WORKDIR /usr/src/app

# First build a dummy target to cache dependencies in a separate Docker layer
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() { println!("Dummy image called!"); }' > src/main.rs
RUN cargo build --release

# Now build the real target
COPY src ./src
# Update modified attribute as otherwise cargo won't rebuild it
RUN touch -a -m ./src/main.rs
RUN cargo build --release

FROM debian:trixie-slim AS runner
# Web page with map
WORKDIR /usr
COPY web ./web
# Runtime dependency for valhalla
RUN apt-get update && apt-get install -y --no-install-recommends libprotobuf-lite32
COPY --from=builder /usr/src/app/target/release/valhalla-debug /usr/local/bin/valhalla-debug
ENTRYPOINT ["valhalla-debug"]
