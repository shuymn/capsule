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
| Wire format | Netstring 列 + LF 終端 | zsh `read -r` で 1 message 読める。field 値に LF 不含 |
| Git | `Command::new("git")` + GitProvider trait | v1 は CLI 呼び出し。trait で将来の gix 移行に備える |
| Daemon 起動 | zsh glue から自動起動 | launchd は v2 |
| zsh 連携 | coproc relay (`capsule connect`) | zsocket 不要。stdin/stdout pipe で socket を抽象化 |
| Socket path | `$TMPDIR/capsule.sock` | macOS $TMPDIR はユーザー別。UID 付加不要。sun_path 104 bytes 制限回避 |
| Session ID | 64-bit random hex (16 chars) | PID は再利用される。zsh 側で生成 |
| Generation | u64 monotonic counter (per-session) | stale request 検出用 |
| Module trait | sync (daemon が slow module を spawn_blocking で呼ぶ) | async trait object の制約を回避 |
| CI | macOS runner | target platform と一致させる |

## Open Questions

→ TODO.md 参照

## Revisit Trigger

- multi-user / multi-session を同一 daemon で扱う必要 → runtime を multi-thread に変更
- gix が十分安定 → GitProvider 実装を差し替え
- launchd 統合が必要 → daemon 起動方式を再検討
- Linux を first-class support → socket path、zsh 前提の再検討
