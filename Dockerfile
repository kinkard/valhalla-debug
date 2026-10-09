ARG RUST_VERSION=1.98
FROM rust:${RUST_VERSION}-slim-trixie AS builder
ARG protobuf_version

# Rust 1.98 is built on LLVM 22: clang/lld must be the same major for `-Clinker-plugin-lto`, bump them together.
ARG LLVM_VERSION=22

# LLVM toolchain from apt.llvm.org, as Debian trixie ships only LLVM 19
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl gnupg \
    && curl -fsSL https://apt.llvm.org/llvm-snapshot.gpg.key | gpg --dearmor -o /usr/share/keyrings/llvm.gpg \
    && echo "deb [signed-by=/usr/share/keyrings/llvm.gpg] https://apt.llvm.org/trixie/ llvm-toolchain-trixie-${LLVM_VERSION} main" > /etc/apt/sources.list.d/llvm.list

# System dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    clang-${LLVM_VERSION} \
    llvm-${LLVM_VERSION} \
    lld-${LLVM_VERSION} \
    # Valhalla build dependencies
    build-essential \
    cmake \
    libboost-dev \
    liblz4-dev \
    libprotobuf-dev \
    protobuf-compiler \
    zlib1g-dev

# https://doc.rust-lang.org/beta/rustc/linker-plugin-lto.html
ENV CC=clang-${LLVM_VERSION} CXX=clang++-${LLVM_VERSION} AR=llvm-ar-${LLVM_VERSION} RANLIB=llvm-ranlib-${LLVM_VERSION}
ENV RUSTFLAGS="-Clinker-plugin-lto -Clinker=clang-${LLVM_VERSION} -Clink-arg=-fuse-ld=lld-${LLVM_VERSION}"

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
