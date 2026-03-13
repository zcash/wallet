# syntax=docker/dockerfile:1

FROM stagex/pallet-rust:1.91.1@sha256:4062550919db682ebaeea07661551b5b89b3921e3f3a2b0bc665ddea7f6af1ca AS pallet-rust
FROM stagex/user-protobuf:26.1@sha256:b399bb058216a55130d83abcba4e5271d8630fff55abbb02ed40818b0d96ced1 AS protobuf 
FROM stagex/user-abseil-cpp:20240116.2@sha256:183e8aff7b3e8b37ab8e89a20a364a21d99ce506ae624028b92d3bed747d2c06 AS abseil-cpp

FROM pallet-rust AS builder
COPY --from=protobuf . /
COPY --from=abseil-cpp . /

ENV TARGET_ARCH=x86_64-unknown-linux-musl
ENV CXXFLAGS=-stdlib=libc++
ENV CFLAGS=-target\ x86_64-unknown-linux-musl
ENV CARGO_HOME=/usr/local/cargo
ENV CARGO_TARGET_DIR="/usr/src/zallet/target"
ENV CARGO_INCREMENTAL=0
ENV RUST_BACKTRACE=1
ENV RUSTFLAGS="\
-C codegen-units=1 \
-C target-feature=+crt-static \
-C linker=clang \
-C link-arg=-fuse-ld=lld \
-C link-arg=-Wl,--allow-multiple-definition \
-C link-arg=-Wl,--whole-archive \
-C link-arg=/usr/lib/libc++.a \
-C link-arg=/usr/lib/libc++abi.a \
-C link-arg=/usr/lib/libzstd.a \
-C link-arg=/usr/lib/libz.a \
-C link-arg=-Wl,--no-whole-archive \
-C link-arg=-ldl \
-C link-arg=-lm \
-C link-arg=-Wl,--build-id=none"
ENV SOURCE_DATE_EPOCH=1
WORKDIR /usr/src/zallet
COPY . .

# Fetch dependencies
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    cargo fetch \
        --locked \
        --target ${TARGET_ARCH}

# Build Zallet
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/usr/src/zallet/target \
    cargo install \
	--frozen \
        --path zallet \
        --bin zallet \
        --target ${TARGET_ARCH} \
        --features rpc-cli,zcashd-import

FROM scratch AS export
COPY --from=builder /usr/local/cargo/bin/zallet /zallet

FROM scratch AS runtime
USER 1000:1000
COPY --from=export /zallet /usr/local/bin/zallet
WORKDIR /var/lib/zallet
ENTRYPOINT ["zallet"]
