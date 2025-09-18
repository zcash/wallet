# syntax=docker/dockerfile:1
# --- Stage 1: Build with Rust --- (amd64)
FROM rust:bookworm AS builder

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        libclang-dev

# Make a fake Rust app to keep a cached layer of compiled crates
WORKDIR /usr/src/app
COPY Cargo.toml Cargo.lock ./
COPY zallet/Cargo.toml ./zallet/
# Needs at least a main.rs file with a main function
RUN mkdir -p zallet/src/bin/zallet && echo "fn main(){}" > zallet/src/bin/zallet/main.rs
RUN mkdir zallet/tests && touch zallet/tests/cli_tests.rs
# Will build all dependent crates in release mode
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/src/app/target \
    cargo build --release --features rpc-cli,zcashd-import

# Copy the rest
COPY . .
# Build the zallet binary
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/src/app/target \
    cargo install --locked --features rpc-cli,zcashd-import --path ./zallet --bins


# --- Stage 2: Minimal runtime with distroless ---
FROM gcr.io/distroless/cc-debian12 AS runtime

COPY --link --from=builder /usr/local/cargo/bin/zallet /usr/local/bin/

# USER nonroot (UID 65532) — for K8s, use runAsUser: 65532
USER nonroot

WORKDIR /var/lib/zallet

ENTRYPOINT ["zallet"]
