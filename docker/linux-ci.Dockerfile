# syntax=docker/dockerfile:1.27@sha256:bde3983e9c939224420ddaf6b784cc30e09b035a4dea01f581230c50809f372e

FROM rust:bookworm@sha256:13c186980fa33cc12759b429662a1322939dbe697484b7c33b47dd2698d28460

COPY rust-toolchain.toml /tmp/rust-toolchain.toml

RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt,sharing=locked \
    apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates curl jq zsh \
    && rm -rf /var/lib/apt/lists/*

RUN sh -c "$(curl --location https://taskfile.dev/install.sh)" -- -d -b /usr/local/bin

RUN rustup toolchain install nightly --profile default --component clippy --component rustfmt

ENV RUSTUP_TOOLCHAIN=nightly

WORKDIR /workspace
