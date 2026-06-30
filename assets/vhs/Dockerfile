# syntax=docker/dockerfile:1.25@sha256:0adf442eae370b6087e08edc7c50b552d80ddf261576f4ebd6421006b2461f12

FROM rust:bookworm@sha256:13c186980fa33cc12759b429662a1322939dbe697484b7c33b47dd2698d28460

COPY rust-toolchain.toml /tmp/rust-toolchain.toml

RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt,sharing=locked \
    apt-get update \
    && apt-get install --yes --no-install-recommends git zsh ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN rustup toolchain install nightly --profile default

ENV RUSTUP_TOOLCHAIN=nightly

WORKDIR /src

COPY . /src

RUN cargo build --workspace --locked

RUN useradd --create-home --shell /usr/bin/zsh demo

RUN mkdir -p /home/demo/.config/capsule /home/demo/capsule \
    && cat >/home/demo/.config/capsule/config.toml <<'EOF'
[time]
disabled = false

[cmd_duration]
threshold_ms = 1000

[[module]]
name = "rust"
format = "v{version}"
icon = "󱘗"
connector = "via"

[module.when]
files = ["Cargo.toml"]

[[module.source]]
name = "version"
command = ["/bin/sh", "-lc", "sleep 0.8; /usr/local/cargo/bin/rustc --version"]
regex = 'rustc\s+(\S+)-nightly'

[module.style]
fg = "red"
EOF

RUN cat >/home/demo/.zshrc <<'EOF'
export PATH="/src/target/debug:$PATH"
export XDG_CONFIG_HOME="$HOME/.config"
export GIT_CONFIG_GLOBAL=/dev/null
export GIT_CONFIG_SYSTEM=/dev/null
clear
eval "$(/src/target/debug/capsule init zsh)"
EOF

RUN cat >/home/demo/capsule/Cargo.toml <<'EOF'
[package]
name = "capsule-demo"
version = "0.1.0"
edition = "2024"
EOF

RUN cat >/home/demo/capsule/README.md <<'EOF'
# capsule demo
EOF

RUN chown -R demo:demo /home/demo

USER demo
WORKDIR /home/demo

RUN git init --quiet --initial-branch=render-cache /home/demo/capsule \
    && cd /home/demo/capsule \
    && printf 'dirty\n' >>README.md \
    && : >scratch.txt

CMD ["zsh", "-i"]
