# Standard, multi-arch Zallet image — the "build it yourself" path.
#
# This is the DEFAULT Dockerfile: a plain, widely-understood multi-stage build
# on official images that works out of the box on linux/amd64 AND linux/arm64
# (incl. Apple Silicon) via `docker build` / `docker buildx`. It uses Docker's
# native $TARGETARCH so a single `docker buildx build --platform
# linux/amd64,linux/arm64 .` produces both.
#
#   docker build -t zallet .
#   docker buildx build --platform linux/amd64,linux/arm64 -t zallet .
#
# It does NOT attempt to reproduce the published artifact bit-for-bit. For the
# high-assurance, reproducible release artifacts we publish, see:
#   * Dockerfile.stagex — full-source-bootstrapped amd64 image (StageX), and
#   * flake.nix         — bit-for-bit reproducible static-musl build for both
#                         arches (`nix build .#zallet`).
# Both are documented in book/src/slsa/slsa.md. This file is for convenience and
# accessibility; the release pipeline does not use it.

# --- Stage 1: build ---------------------------------------------------------
# Pin a specific Rust to match Cargo's expectations; bookworm matches the
# runtime base below so the dynamically-linked glibc binary just works.
FROM rust:1.91.1-slim-bookworm AS builder

# Build deps: protobuf (tonic/PROTOC), clang+llvm (bindgen / *-sys C/C++ deps),
# pkg-config, and git (zaino-state's build.rs embeds the commit).
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential clang llvm-dev libclang-dev \
        protobuf-compiler pkg-config git ca-certificates \
    && rm -rf /var/lib/apt/lists/*

ENV PROTOC=/usr/bin/protoc
WORKDIR /usr/src/zallet
COPY . .

# Build for the image's target architecture. buildx sets TARGETARCH
# (amd64|arm64) automatically per --platform; default to amd64 for a plain
# `docker build` on an amd64 host.
ARG TARGETARCH=amd64
RUN set -eux; \
    case "$TARGETARCH" in \
      amd64) RUST_TARGET=x86_64-unknown-linux-gnu  ;; \
      arm64) RUST_TARGET=aarch64-unknown-linux-gnu ;; \
      *) echo "unsupported TARGETARCH: $TARGETARCH" >&2; exit 1 ;; \
    esac; \
    rustup target add "$RUST_TARGET"; \
    cargo build --release --locked \
      --bin zallet --features rpc-cli,zcashd-import \
      --target "$RUST_TARGET"; \
    install -D -m0755 "target/${RUST_TARGET}/release/zallet" /out/zallet; \
    # Collect the build.rs-generated share tree (completions, manpages,
    # debian-copyright), matching the StageX export layout where present.
    REL="target/${RUST_TARGET}/release"; \
    mkdir -p /out/usr/local/share/zallet; \
    for d in completions manpages; do \
      [ -d "$REL/$d" ] && cp -a "$REL/$d" /out/usr/local/share/zallet/ || true; \
    done; \
    [ -f "$REL/debian-copyright" ] \
      && install -D -m0644 "$REL/debian-copyright" /out/usr/local/share/zallet/debian-copyright || true

# --- Stage 2: export (parity with Dockerfile.stagex) ------------------------
# Binary at the root for easy extraction; full share tree alongside it.
FROM scratch AS export
COPY --from=builder /out/zallet /zallet
COPY --from=builder /out/usr/local/share/zallet /usr/local/share/zallet

# --- Stage 3: minimal runtime -----------------------------------------------
# debian-slim (not scratch): this is a dynamically-linked glibc binary, so it
# needs a libc + CA certs at runtime. The reproducible StageX/Nix paths produce
# a static-musl binary that can live on scratch; this convenience image trades
# that for a standard, easy-to-extend base.
FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 1000 --user-group --no-create-home --shell /usr/sbin/nologin zallet
COPY --from=builder /out/zallet /usr/local/bin/zallet
COPY --from=builder /out/usr/local/share/zallet /usr/local/share/zallet
USER 1000:1000
WORKDIR /var/lib/zallet
ENTRYPOINT ["zallet"]
