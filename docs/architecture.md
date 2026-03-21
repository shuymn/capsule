# Architecture Baseline — capsule v1

## Goal

macOS + zsh 専用の Rust 製プロンプトエンジン。常駐 daemon が prompt の計算・レイアウト・レンダリングを担い、zsh 側は coproc relay 経由の薄い glue に徹する push-lite アーキテクチャ。

## Core Boundaries

```mermaid
graph LR
    cli["capsule-cli<br/><small>CLI entry point (clap dispatch)</small>"]
    core["capsule-core<br/><small>module system, rendering,<br/>daemon, config, init</small>"]
    protocol["capsule-protocol<br/><small>wire format (netstring +<br/>message + async codec)</small>"]
    sys["capsule-sys<br/><small>macOS FFI<br/>(launch_activate_socket)</small>"]

    cli --> core --> protocol
    cli --> sys
```

依存方向は一方向。unsafe は sys crate のみに閉じ込め。

## Constraints

- Target: macOS + zsh (Linux は socket path fallback のみ)
- Single binary (`capsule`) で daemon / connect / init を提供
- Runtime: tokio current_thread
- Lint: unsafe_code 禁止 (sys 除く)、unwrap/expect/todo/dbg!/panic 禁止 (Cargo.toml lints)
- CI: macOS runner

## System Flow

### Startup

```mermaid
sequenceDiagram
    participant zsh as zsh (.zshrc)
    participant connect as capsule connect
    participant daemon as daemon

    zsh->>zsh: eval "$(capsule init zsh)"
    zsh->>connect: coproc start

    connect->>daemon: socket connect attempt
    alt daemon not running
        connect->>connect: spawn "capsule daemon"
        connect->>connect: wait for socket (up to 1s)
    end

    connect->>daemon: Hello (version, build_id)
    daemon->>connect: HelloAck (version, build_id, env_var_names)

    alt build_id mismatch
        connect->>daemon: kill (SIGTERM)
        connect->>connect: re-spawn daemon
        connect->>daemon: re-connect
    end

    connect->>zsh: "E:VAR1,VAR2,...\n" (env metadata)
    Note over connect: enter relay loop
```

### Per-Prompt Request Pipeline

```mermaid
sequenceDiagram
    participant zsh
    participant connect as capsule connect
    participant daemon

    Note over zsh: precmd fires<br/>capture $?, duration

    zsh->>connect: tab-separated request<br/>(gen, exit, dur, cwd, cols, keymap, env_meta)
    connect->>daemon: netstring Request

    Note over daemon: run fast modules<br/>(directory, time, cmd_duration,<br/>character, fast custom)
    Note over daemon: check slow cache

    daemon->>connect: netstring RenderResult
    connect->>zsh: "R\tgen\tleft1\tleft2\n"
    Note over zsh: set PROMPT

    alt cache miss
        Note over daemon: spawn_blocking
        par
            Note over daemon: git status --porcelain=v2
        and
            Note over daemon: slow custom modules (commands)
        end
        Note over daemon: update cache, re-compose prompt
        opt prompt changed
            daemon->>connect: netstring Update
            connect->>zsh: "U\tgen\tleft1\tleft2\n"
            Note over zsh: zle reset-prompt
        end
    end
```

### Concurrent Slow Compute Coalescing

同一 cwd + config generation の slow compute が同時に複数発生した場合、最初のリクエストだけが実際に spawn し、後続は watch channel で結果を待つ。

```mermaid
sequenceDiagram
    participant A as request A
    participant state as SharedState
    participant compute as slow compute
    participant B as request B

    A->>state: cache miss, no inflight
    state->>state: insert watch::Sender
    A->>compute: spawn slow compute
    A->>A: send RenderResult (fast only)
    A->>state: subscribe (Receiver)

    Note over compute: running...

    B->>state: cache miss, inflight exists
    B->>B: send RenderResult (fast only)
    B->>state: subscribe (Receiver)

    compute->>state: insert result into cache
    compute->>state: notify Sender

    state->>A: Receiver notified
    state->>B: Receiver notified
    Note over A,B: each sends Update if prompt changed
```

### Config Hot-Reload

```mermaid
flowchart TD
    A[prompt request arrives] --> B[stat config file]
    B --> C{mtime changed?}
    C -->|no| D[use current config]
    C -->|yes| E[re-read + re-parse TOML]
    E --> F[increment config generation]
    F --> G[clear slow cache]
    G --> H[use new config]
```

### Daemon Lifecycle (Bound Mode)

```mermaid
flowchart TD
    A[capsule daemon start] --> B[flock ~/.capsule/capsule.lock]
    B --> C[bind ~/.capsule/capsule.sock]
    C --> D[record socket inode]
    D --> E[accept loop]

    E --> F{event?}
    F -->|client connects| G[spawn connection handler]
    G --> E
    F -->|inode check interval 5s| H{socket inode matches?}
    H -->|yes| E
    H -->|no / file removed| I[shutdown<br/>do NOT remove foreign socket]
    F -->|SIGTERM| J[shutdown + remove socket]
```

## Prompt Layout

```
Line 1 (info):   [directory] on [icon branch [indicators]] via [icon value] took [duration]
Line 2 (input):  at [time] [character]
```

レスポンシブ truncation: terminal width を超える場合、(1) directory を truncate、(2) line 1 の右側セグメントを順に drop。

## Key Tech Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Runtime | tokio current_thread | 1 session 1 connection。multi-thread の overhead 不要 |
| IPC | Unix domain socket | macOS native、低遅延、zsh から coproc 経由で接続 |
| Wire format | daemon-connect: Netstring + LF / shell-connect: Tab + LF | daemon 間は binary-safe netstring。shell 間は zsh native の tab split で十分 |
| Git | `Command::new("git")` + GitProvider trait | v1 は CLI 呼び出し。trait で将来の gix 移行に備える |
| Daemon startup | launchd socket activation (macOS), standalone fallback | `launch_activate_socket` FFI (sys crate) → fd → listener。standalone: flock + bind |
| zsh integration | coproc protocol translator (`capsule connect`) | zsocket 不要。connect が netstring - tab 変換を担い、shell は protocol 非依存 |
| Socket path | `~/.capsule/capsule.sock` | launchd plist で $HOME 展開可能。sun_path 104 bytes 制限回避 |
| Session ID | 64-bit random hex (16 chars) | PID は再利用される。connect 側で生成 |
| Generation | u64 monotonic counter (per-session) | stale request 検出 + slow update 破棄 |
| Module trait | sync (daemon が slow module を spawn_blocking) | async trait object の制約を回避 |
| Config | TOML, mtime-based hot-reload | daemon 再起動不要。parse error 時は defaults fallback |
| Cache | LRU (1024 entries, key = cwd + config_generation + dep_hash) | slow module 結果の再利用。dep_hash が env/file 依存を反映するため TTL 不要 |
| Slow coalescing | watch channel per cache key | 同一 cwd への concurrent request で重複 spawn を防止 |

## Revisit Trigger

- multi-user / multi-session を同一 daemon で扱う必要 -> runtime を multi-thread に変更
- gix が十分安定 -> GitProvider 実装を差し替え
- Linux を first-class support -> socket path、zsh 前提の再検討
