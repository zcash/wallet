# syntax=docker/dockerfile:1
# --- Stage 1: Build with Rust --- (amd64) (RUST version: 1.86.0)
FROM rust:1.86.0-slim@sha256:57d415bbd61ce11e2d5f73de068103c7bd9f3188dc132c97cef4a8f62989e944 AS builder

WORKDIR /app

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        clang \
        libclang-dev \
        pkg-config \
        git && \
    rm -rf /var/lib/apt/lists/*

COPY --link . .

# Build the zallet binary
# Leverage a cache mount to ${CARGO_HOME} for downloaded dependencies,
# and a cache mount to ${CARGO_TARGET_DIR} for compiled dependencies.
RUN --mount=type=bind,source=zallet,target=zallet \
    --mount=type=bind,source=Cargo.toml,target=Cargo.toml \
    --mount=type=bind,source=Cargo.lock,target=Cargo.lock \
    --mount=type=cache,target=/app/.cargo \
    --mount=type=cache,target=/app/target/ \
    cargo build --locked --release --features rpc-cli,zcashd-import --package zallet --bin zallet && \
    cp /app/target/release/zallet /usr/local/bin/


# --- Stage 2: Minimal runtime with distroless ---
FROM gcr.io/distroless/cc AS runtime

COPY --link --from=builder /usr/local/bin/zallet /usr/local/bin/

# USER nonroot (UID 65532) — for K8s, use runAsUser: 65532
USER nonroot

WORKDIR /var/lib/zallet

ENTRYPOINT ["zallet"]
