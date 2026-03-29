# syntax=docker/dockerfile:1.22@sha256:4a43a54dd1fedceb30ba47e76cfcf2b47304f4161c0caeac2db1c61804ea3c91

FROM rust:bookworm

COPY rust-toolchain.toml /tmp/rust-toolchain.toml

RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt,sharing=locked \
    apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates curl zsh \
    && rm -rf /var/lib/apt/lists/*

RUN sh -c "$(curl --location https://taskfile.dev/install.sh)" -- -d -b /usr/local/bin

RUN rustup toolchain install nightly --profile default --component clippy --component rustfmt

ENV RUSTUP_TOOLCHAIN=nightly

WORKDIR /workspace
