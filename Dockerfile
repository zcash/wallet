# syntax=docker/dockerfile:1

# Rust 1.91.1
FROM stagex/pallet-rust@sha256:4062550919db682ebaeea07661551b5b89b3921e3f3a2b0bc665ddea7f6af1ca AS pallet-rust
FROM stagex/user-protobuf@sha256:b399bb058216a55130d83abcba4e5271d8630fff55abbb02ed40818b0d96ced1 AS protobuf 
FROM stagex/user-abseil-cpp@sha256:183e8aff7b3e8b37ab8e89a20a364a21d99ce506ae624028b92d3bed747d2c06 AS abseil-cpp
FROM stagex/core-filesystem@sha256:cd3a66471ce1f630fa77d5c9bd9829f9f9fab6302a1aaa64d67b74f1f069b750 AS filesystem

# --- Stage 1: Build with Rust --- (amd64)
FROM pallet-rust AS builder
COPY --from=stagex/pallet-clang . /
COPY --from=protobuf . /
COPY --from=abseil-cpp . /

ENV SOURCE_DATE_EPOCH=1
ENV CXXFLAGS="-include cstdint"
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
ENV RUSTFLAGS="${RUSTFLAGS} -C linker=clang -C link-arg=-fuse-ld=lld -C link-arg=-lc++ -C link-arg=-lc++abi"
ENV RUSTFLAGS="${RUSTFLAGS} -C link-arg=-Wl,--build-id=none"
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
      && install -D -m 0755 /usr/src/app/target/${TARGET_ARCH}/release/zallet /usr/local/bin/zallet


# --- Stage 2: layer for local binary extraction ---
FROM scratch AS export

COPY --from=builder /usr/local/bin/zallet /zallet

# --- Stage 3: Minimal runtime with stagex ---
# `stagex/core-filesystem` with a basic filesystem
FROM filesystem AS runtime
USER 1000:1000
COPY --from=export /zallet /usr/local/bin/zallet

WORKDIR /var/lib/zallet

ENTRYPOINT ["zallet"]
