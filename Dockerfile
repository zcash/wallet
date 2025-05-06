# --- Stage 1: Build with Rust --- (amd64)
FROM rustlang/rust:nightly-slim@sha256:e1fb83983ca45a2f2afe386077cdf6873a7c0a496a5ae6e469b25dc1a90b1561 AS builder

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
