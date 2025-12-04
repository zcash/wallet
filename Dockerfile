# syntax=docker/dockerfile:1

FROM stagex/pallet-rust@sha256:9c38bf1066dd9ad1b6a6b584974dd798c2bf798985bf82e58024fbe0515592ca AS pallet-rust
FROM stagex/user-protobuf@sha256:5e67b3d3a7e7e9db9aa8ab516ffa13e54acde5f0b3d4e8638f79880ab16da72c AS protobuf 
FROM stagex/user-abseil-cpp@sha256:3dca99adfda0cb631bd3a948a99c2d5f89fab517bda034ce417f222721115aa2 AS abseil-cpp
FROM stagex/core-user-runtime@sha256:055ae534e1e01259449fb4e0226f035a7474674c7371a136298e8bdac65d90bb AS user-runtime

# --- Stage 1: Build with Rust --- (amd64)
FROM pallet-rust AS builder
COPY --from=protobuf . /
COPY --from=abseil-cpp . /

ENV SOURCE_DATE_EPOCH=1
ENV CXXFLAGS="-include cstdint"
ENV ROCKSDB_USE_PKG_CONFIG=0
ENV CARGO_HOME=/usr/local/cargo

# Make a fake Rust app to keep a cached layer of compiled crates
WORKDIR /usr/src/app
COPY Cargo.toml Cargo.lock ./
COPY zallet/Cargo.toml ./zallet/
# Needs at least a main.rs file with a main function
RUN mkdir -p zallet/src/bin/zallet && echo "fn main(){}" > zallet/src/bin/zallet/main.rs
RUN mkdir zallet/tests && touch zallet/tests/cli_tests.rs

ENV RUST_BACKTRACE=1
ENV RUSTFLAGS="-C codegen-units=1"
ENV RUSTFLAGS="${RUSTFLAGS} -C target-feature=+crt-static"
ENV RUSTFLAGS="${RUSTFLAGS} -C link-arg=-Wl,--build-id=none"
ENV CFLAGS="-D__GNUC_PREREQ(maj,min)=1"
ENV TARGET_ARCH="x86_64-unknown-linux-musl"

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    cargo fetch --locked --target $TARGET_ARCH

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    cargo metadata --locked --format-version=1 > /dev/null 2>&1

# Will build all dependent crates in release mode
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/usr/src/app/target \
    --network=none \
    cargo build --release --frozen \
      --target ${TARGET_ARCH} \
      --features rpc-cli,zcashd-import

# Copy the rest
COPY . .
# Build the zallet binary
# Compile & install offline
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/usr/src/app/target \
    --network=none \
    cargo build --release --frozen \
      --bin zallet \
      --target ${TARGET_ARCH} \
      --features rpc-cli,zcashd-import \
    && OUT="/usr/src/app/target/${TARGET_ARCH}/release" \
    \
    # Install main binary
    && install -D -m 0755 \
         "${OUT}/zallet" \
         /usr/local/bin/zallet \
    \
    # Copy whole trees for completions, manpages and metadata
    && install -d /usr/local/share/zallet \
    && cp -a "${OUT}/completions" /usr/local/share/zallet/completions \
    && cp -a "${OUT}/manpages"  /usr/local/share/zallet/manpages \
    && install -D -m 0644 \
         "${OUT}/debian-copyright" \
         /usr/local/share/zallet/debian-copyright

# --- Stage 2: layer for local binary extraction ---
FROM scratch AS export

# Binary at the root for easy extraction
COPY --from=builder /usr/local/bin/zallet /zallet

# Export the whole zallet share tree (completions, manpages, metadata, etc.)
COPY --from=builder /usr/local/share/zallet /usr/local/share/zallet

# --- Stage 3: Minimal runtime with stagex ---
# `stagex/core-user-runtime` sets the user to non-root by default
FROM user-runtime AS runtime
COPY --from=export /zallet /usr/local/bin/zallet

WORKDIR /var/lib/zallet

ENTRYPOINT ["zallet"]
