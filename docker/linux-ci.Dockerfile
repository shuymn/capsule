# syntax=docker/dockerfile:1.28@sha256:bb22d9815c728170f72750f4e5b0d672e06176142e1d602c7e66c050100b7e5b

FROM rust:bookworm@sha256:13c186980fa33cc12759b429662a1322939dbe697484b7c33b47dd2698d28460

COPY rust-toolchain.toml /tmp/rust-toolchain.toml

RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt,sharing=locked \
    apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates curl git jq procps zsh \
    && rm -rf /var/lib/apt/lists/*

RUN rustup toolchain install stable --profile minimal --component clippy --component rustfmt

ENV RUSTUP_TOOLCHAIN=stable

# Containers run as the caller's UID: tools from mise.toml are preinstalled read-only,
# mutable mise and uv state lives in /tmp, and only the mounted repository and the
# baked config are trusted.
ENV MISE_DATA_DIR=/opt/mise MISE_CACHE_DIR=/tmp/mise-cache MISE_STATE_DIR=/tmp/mise-state \
    MISE_TRUSTED_CONFIG_PATHS=/workspace:/opt/mise-config PATH=/opt/mise/shims:$PATH \
    UV_CACHE_DIR=/tmp/uv-cache UV_PYTHON_INSTALL_DIR=/tmp/uv-python

COPY --from=ghcr.io/jdx/mise:2026.10.17@sha256:a80f941369a7cc3af35d6eca41c63756f6a243367b52bdeda481954cd6fca0e2 /usr/local/bin/mise /usr/local/bin/mise
COPY mise.toml /opt/mise-config/
RUN cd /opt/mise-config && mise install && chmod -R a+rX /opt/mise \
    && rm -rf /tmp/mise-cache /tmp/mise-state

WORKDIR /workspace
