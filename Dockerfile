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

COPY . .

RUN cargo build --release && strip target/release/zallet

# --- Stage 2: Minimal runtime with distroless ---
FROM gcr.io/distroless/cc

COPY --from=builder /app/target/release/zallet /usr/local/bin/zallet

USER nonroot
ENTRYPOINT ["/usr/local/bin/zallet"]
