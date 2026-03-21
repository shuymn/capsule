# Architecture Baseline — capsule v1

## Goal

macOS + zsh 専用の Rust 製プロンプトエンジン。常駐 daemon が prompt の計算・レイアウト・レンダリングを担い、zsh 側は coproc relay 経由の薄い glue に徹する push-lite アーキテクチャ。

## Constraints

- Target: macOS + zsh (Linux は socket path fallback のみ)
- Single binary (`capsule`) で daemon / connect / init を提供
- Runtime: tokio current_thread
- Lint: unsafe_code 禁止、unwrap/expect/todo/dbg! 禁止 (Cargo.toml lints)
- v1 で追加しない依存: gix, serde/serde_json, nix, async-trait

## Core Boundaries

```
capsule-protocol    wire format (netstring + message 型 + async codec)
                    I/O 抽象のみ、transport 非依存
capsule-core        module system, rendering, daemon, relay, init
                    ビジネスロジック全体
capsule-cli         CLI entry point (clap dispatch のみ)
                    [[bin]] name = "capsule"
```

依存方向: `cli → core → protocol` (逆方向なし)

## Key Tech Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Runtime | tokio current_thread | 1 session 1 connection。multi-thread の overhead 不要 |
| IPC | Unix domain socket | macOS native、低遅延、zsh から coproc 経由で接続 |
| Wire format | daemon↔connect: Netstring 列 + LF 終端 / shell↔connect: Tab 区切り + LF 終端 | daemon 間は binary-safe netstring。shell 間は zsh native の tab split で十分。[ADR](adr/protocol-aware-connect.md) 参照 |
| Git | `Command::new("git")` + GitProvider trait | v1 は CLI 呼び出し。trait で将来の gix 移行に備える |
| Daemon 起動 | launchd socket activation (macOS), standalone fallback | `launch_activate_socket` FFI → fd → listener。standalone: flock + bind |
| zsh 連携 | coproc protocol translator (`capsule connect`) | zsocket 不要。connect が netstring ↔ tab-separated text の変換を担い、shell は protocol 非依存 |
| Socket path | `~/.capsule/capsule.sock` | launchd plist で $HOME 展開可能。sun_path 104 bytes 制限回避。$TMPDIR は再起動ごとに変わるため不適 |
| Session ID | 64-bit random hex (16 chars) | PID は再利用される。connect 側で生成 |
| Generation | u64 monotonic counter (per-session) | stale request 検出用 |
| Module trait | sync (daemon が slow module を spawn_blocking で呼ぶ) | async trait object の制約を回避 |
| CI | macOS runner | target platform と一致させる |

## Open Questions

→ TODO.md 参照

## Revisit Trigger

- multi-user / multi-session を同一 daemon で扱う必要 → runtime を multi-thread に変更
- gix が十分安定 → GitProvider 実装を差し替え
- Linux を first-class support → socket path、zsh 前提の再検討
