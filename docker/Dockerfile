# syntax=docker/dockerfile:1.7-labs

# If you want to include a file or directory in the Docker image, add it to .dockerignore.
#
# We use 2 stages:
# - release: builds release binaries
# - runtime: prepares the release image
#
# We first set default values for build arguments used across the stages.
# Each stage must define the build arguments (ARGs) it uses.

ARG RUST_VERSION=1.85.1

ARG UID=10002
ARG GID=${UID}
ARG USER="zallet"
ARG HOME="/home/${USER}"
ARG CARGO_HOME="${HOME}/.cargo"
ARG CARGO_TARGET_DIR="${HOME}/target"

# This stage prepares Zallet's build deps and captures build args as env vars.
FROM rust:${RUST_VERSION}-bookworm AS deps
SHELL ["/bin/bash", "-xo", "pipefail", "-c"]

# Install build deps (if any beyond Rust).
RUN apt-get -qq update && \
    apt-get -qq install -y --no-install-recommends \
    libclang-dev \
    && rm -rf /var/lib/apt/lists/* /tmp/*

# Build arguments and variables
ARG CARGO_INCREMENTAL
ENV CARGO_INCREMENTAL=${CARGO_INCREMENTAL:-0}

ARG CARGO_HOME
ENV CARGO_HOME=${CARGO_HOME}

ARG CARGO_TARGET_DIR
ENV CARGO_TARGET_DIR=${CARGO_TARGET_DIR}

# This stage builds the zallet release binary.
FROM deps AS release

# Set the working directory for the build.
ARG HOME
WORKDIR ${HOME}

ARG CARGO_HOME
ARG CARGO_TARGET_DIR

# Build the zallet binary
# Leverage a cache mount to ${CARGO_HOME} for downloaded dependencies,
# and a cache mount to ${CARGO_TARGET_DIR} for compiled dependencies.
RUN --mount=type=bind,source=zallet,target=zallet \
    --mount=type=bind,source=Cargo.toml,target=Cargo.toml \
    --mount=type=bind,source=Cargo.lock,target=Cargo.lock \
    --mount=type=cache,target=${CARGO_HOME} \
    --mount=type=cache,target=${CARGO_TARGET_DIR} \
    cargo build --locked --release --package zallet --bin zallet && \
    cp ${CARGO_TARGET_DIR}/release/zallet /usr/local/bin/

# This stage starts from scratch using Debian and copies the built zallet binary
# from the `release` stage along with other necessary files.
FROM debian:bookworm-slim AS runtime

ARG UID
ENV UID=${UID}
ARG GID
ENV GID=${GID}
ARG USER
ENV USER=${USER}
ARG HOME
ENV HOME=${HOME}

RUN addgroup --quiet --gid ${GID} ${USER} && \
    adduser --quiet --gid ${GID} --uid ${UID} --home ${HOME} ${USER} --disabled-password --gecos ""

WORKDIR ${HOME}
RUN chown -R ${UID}:${GID} ${HOME}

# Copy the gosu binary to be able to run the entrypoint as non-root user
COPY --link --from=tianon/gosu:bookworm /gosu /usr/local/bin/
COPY --link --from=release /usr/local/bin/zallet /usr/local/bin/
COPY docker/entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh

# COPY docker/default-zallet.toml /etc/zallet/zallet.toml

ENTRYPOINT ["entrypoint.sh"]
# Default command to run if no arguments are passed to `docker run`
CMD ["zallet"]
