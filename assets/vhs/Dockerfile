# syntax=docker/dockerfile:1.22@sha256:4a43a54dd1fedceb30ba47e76cfcf2b47304f4161c0caeac2db1c61804ea3c91

FROM rust:bookworm@sha256:fdb91abf3cb33f1ebc84a76461d2472fd8cf606df69c181050fa7474bade2895

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
