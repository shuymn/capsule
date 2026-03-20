# capsule — macOS zsh Prompt Engine v1

## Goal

macOS + zsh 専用 Rust 製プロンプトエンジン v1。daemon が prompt の計算・レイアウト・レンダリングを担い、zsh は coproc relay 経由の薄い glue に徹する。

## Constraints

- Target: macOS + zsh (Linux は best-effort fallback)
- Architecture: [docs/architecture.md](docs/architecture.md) 参照
- Runtime: tokio current_thread
- Lint: 既存 Cargo.toml lints を workspace に移行して維持
- v1 で追加しない依存: gix, serde/serde_json, nix, async-trait

## Open Questions

- Question: coproc の read/write fd 番号取得が zsh バージョンにより異なる可能性
  - Class: `risk-bearing`
  - Resolution: `spike` (macOS 同梱 zsh 5.9 で動作確認、Theme 6 実装時)
  - Status: `open`

- Question: CI を macOS runner に変更するか、macOS job を追加するか
  - Class: `risk-bearing`
  - Resolution: `decision`
  - Status: `resolved` — macOS runner に変更。capsule は macOS 専用ツールのため target platform で CI を実行する

- Question: Async trait object の制約 (dyn Trait + async fn)
  - Class: `non-blocking`
  - Resolution: v1 は固定モジュールセット。enum dispatch または static dispatch で回避
  - Status: `resolved`

- Question: spawn_blocking は current_thread runtime でも別スレッドで動作するか
  - Class: `non-blocking`
  - Resolution: する (tokio の blocking thread pool は runtime flavor に依存しない)
  - Status: `resolved`

## Theme Backlog

- [x] Theme 1: Workspace Foundation
  - Outcome: 3-crate workspace がコンパイルし、CLI skeleton が `--help` を返す
  - Goal: workspace 構造 (protocol/core/cli)、共通 lint/deps 定義、CLI サブコマンド骨格、既存 `src/main.rs` 削除、CI macOS 化
  - Must Not Break: `task check` (既存の lint/fmt/build パイプライン)
  - Non-goals: サブコマンドの実装、依存の実使用、テストヘルパー以上のテスト基盤
  - Acceptance (EARS):
    - When `task check` is run, the system shall pass all checks on the workspace
    - When `cargo run -p capsule-cli -- --help` is run, the output shall list `daemon`, `connect`, `init` subcommands
    - When CI runs, all jobs shall execute on macOS runner
  - Evidence: `run=cargo run -p capsule-cli -- --help; oracle=stdout contains "daemon" and "connect" and "init"; visibility=independent; controls=[context]; missing=[agent]; companion=none`
  - Gates: `static`
  - Executable doc: `cargo run -p capsule-cli -- --help` → 出力に `daemon`, `connect`, `init` が含まれる
  - Why not split vertically further?: crate 骨格と CLI skeleton は相互依存 (cli は core/protocol に依存)。片方だけでは `task check` で検証不能
  - Escalate if: `workspace.lints` の移行で既存 lint 設定の挙動が変わる

- [x] Theme 2: Wire Protocol
  - Outcome: netstring codec と Request/RenderResult/Update のシリアライズが round-trip テストを通る
  - Goal: `capsule-protocol` に netstring encode/decode、message 型 (Request/RenderResult/Update)、SessionId newtype、async codec (MessageReader/MessageWriter) を実装
  - Must Not Break: `task check`
  - Non-goals: 実際の socket I/O、daemon 統合、wire format の拡張性
  - Acceptance (EARS):
    - When arbitrary bytes are encoded then decoded, the system shall return the original bytes
    - When a Request is serialized to wire format then deserialized, the system shall return an equivalent Request
    - When invalid wire input is decoded, the system shall return Err without panicking
    - When messages are written to and read from an async duplex channel, the system shall preserve message content
  - Evidence: `run=cargo test -p capsule-protocol; oracle=all tests pass; visibility=independent; controls=[context]; missing=[agent]; companion=none`
  - Gates: `static`, `integration`
  - Executable doc: `cargo test -p capsule-protocol` — netstring round-trip (空/ASCII/UTF-8/長い文字列)、message round-trip (Request/RenderResult/Update)、不正入力エラー、async codec round-trip
  - Why not split vertically further?: netstring・message・codec は 1 つの wire format を構成し、round-trip テストが分離不能
  - Escalate if: wire format の field 順序や encoding 方式で追加の設計判断が必要になった場合

- [x] Theme 3: Module System
  - Outcome: Module trait + 7 モジュール (directory, status, cmd_duration, time, character, git, toolchain) が RenderContext から正しい出力を返す
  - Goal: `capsule-core` に Module trait (sync)、ModuleSpeed enum、RenderContext、ModuleOutput を定義し、全 v1 モジュールを実装。GitProvider trait + CommandGitProvider (std::process::Command)
  - Must Not Break: `task check`
  - Non-goals: ANSI スタイリング (Theme 4)、モジュール出力の合成 (Theme 4)、daemon 統合 (Theme 5)
  - Acceptance (EARS):
    - When cwd is `$HOME`, directory module shall output `~`
    - When last_exit_code is 0, status module shall return None
    - When last_exit_code is non-zero, character module shall output content distinguishable from success case
    - When duration_ms is below 2000, cmd_duration module shall return None
    - When a git repository with staged changes exists, git module shall report staged count > 0
    - When `Cargo.toml` exists in cwd, toolchain module shall output a string containing "rust"
  - Evidence: `run=cargo test -p capsule-core -- module; oracle=all module tests pass; visibility=independent; controls=[context]; missing=[agent]; companion=none; notes=git tests use tempfile + git init`
  - Gates: `static`, `integration`
  - Executable doc: `cargo test -p capsule-core -- module` — 各モジュールの入出力テスト (temp git repo 使用)
  - Why not split vertically further?: 全モジュールが同一 trait と RenderContext を共有。個別 theme は同一構造の繰り返しになり overhead が勝つ
  - Escalate if: GitProvider trait の設計が `git status --porcelain=v2` のパース結果と整合しない

- [x] Theme 4: Rendering Pipeline
  - Outcome: Module 出力群が left1 (info line) / left2 (input line) prompt 文字列に合成される
  - Goal: style.rs (ANSI + zsh `%{..%}` escape)、layout.rs (display width 計算 + truncation)、Composer (info_left + info_right → right-padded line1、input_left → line2) を実装
  - Must Not Break: `task check`
  - Non-goals: daemon 統合、実際のターミナル出力、カスタマイズ可能な色設定
  - Acceptance (EARS):
    - When module outputs fit within terminal width, the info line shall right-align right-side modules with space padding
    - When module outputs exceed terminal width, directory shall be truncated before right-side modules are dropped
    - When ANSI escape sequences are present in input, display width calculation shall exclude them
    - When zsh `%{..%}` wrappers are present in input, display width calculation shall exclude them
  - Evidence: `run=cargo test -p capsule-core -- render; oracle=all render tests pass; visibility=independent; controls=[context]; missing=[agent]; companion=none`
  - Gates: `static`, `integration`
  - Executable doc: `cargo test -p capsule-core -- render` — width 計算 (ANSI/CJK/zsh escape)、padding、truncation、full composition
  - Why not split vertically further?: style → layout → composition は 1 パイプライン。分離すると合成テストが成立しない
  - Escalate if: CJK 文字の width 計算が unicode-width crate で不正確な場合

- [ ] Theme 5: Daemon Server
  - Outcome: Unix socket で listen する daemon が request を受けて fast module の RenderResult を即時返し、slow module 完了後に Update を送信する
  - Goal: socket lifecycle (bind/accept/shutdown)、Session + SessionMap、BoundedCache (TTL + size limit)、request pipeline (fast → cache check → respond → slow recompute → update) を実装
  - Must Not Break: `task check`
  - Non-goals: CLI サブコマンド統合 (Theme 6)、zsh glue (Theme 6)、構造化ログ出力 (Theme 7)
  - Acceptance (EARS):
    - When a client connects and sends a valid Request, the daemon shall respond with a RenderResult containing fast module outputs
    - When slow modules complete with results different from the initial response, the daemon shall send an Update message
    - When a stale socket file exists and no daemon is listening, the daemon shall remove it and bind successfully
    - When SIGTERM is received, the daemon shall shut down gracefully and remove the socket file
    - When request generation is not greater than session's last generation, the daemon shall discard the request
    - When cache contains fresh slow module results for the same cwd, the daemon shall use cached values
  - Evidence: `run=cargo test -p capsule-core -- daemon; oracle=all daemon tests pass; visibility=independent; controls=[context]; missing=[agent]; companion=none; notes=temp socket + MockGitProvider for deterministic slow module`
  - Gates: `static`, `integration`, `system`
  - Executable doc: `cargo test -p capsule-core -- daemon` — temp socket daemon 起動 → 接続 → request → RenderResult 検証 → slow module 完了 → Update 検証 → SIGTERM → socket cleanup
  - Why not split vertically further?: session・cache・handler は request pipeline の構成要素。pipeline 統合なしに daemon の正しさを検証できない
  - Escalate if: tokio current_thread + spawn_blocking の組み合わせで想定外の挙動が発生

- [ ] Theme 6: CLI + zsh Integration
  - Outcome: `eval "$(capsule init zsh)"` で zsh session に prompt が表示され、`capsule connect` が daemon との coproc relay を提供する
  - Goal: `capsule daemon` / `capsule connect` / `capsule init zsh` サブコマンド実装、zsh glue スクリプト生成 (precmd/preexec/update handler/fallback/coproc 監視)、coproc relay (stdin/stdout ↔ Unix socket)、daemon 自動起動
  - Must Not Break: `task check`、ユーザーの既存 zsh 設定 (ZDOTDIR 隔離で保証)
  - Non-goals: launchd 統合、設定ファイル、テーマカスタマイズ、Linux 対応
  - Acceptance (EARS):
    - When `capsule init zsh` is run, the output shall be valid zsh script containing `_capsule_` prefixed functions
    - When the zsh glue is eval'd in a clean zsh session (ZDOTDIR isolated), PROMPT shall be set to a non-empty value
    - When `capsule connect` is run and daemon is not running, it shall auto-start the daemon then relay
    - When coproc dies, the next precmd shall attempt reconnection
    - If reconnection fails, the system shall display fallback prompt `%~ %# `
  - Evidence: `run=cargo test -p capsule-core -- init && cargo test -p capsule-cli; oracle=all tests pass; visibility=independent; controls=[context]; missing=[agent]; companion=none; notes=zsh integration test uses ZDOTDIR/HOME isolation in temp dir`
  - Gates: `static`, `integration`, `system`
  - Executable doc: `ZDOTDIR=$tmp HOME=$tmp zsh -c 'eval "$(capsule init zsh)" && [[ -n "$PROMPT" ]]'` — temp dir 隔離下で PROMPT が設定されることを検証
  - Why not split vertically further?: init/connect/zsh-glue は 1 つのユーザーフローを構成。connect なしに init の出力は検証不能
  - Escalate if: macOS 同梱 zsh の coproc fd 取得方法が想定と異なる (Open Question #1)

- [ ] Theme 7: E2E Integration + Observability
  - Outcome: binary build → daemon 起動 → client 接続 → request/response → shutdown の全フローが動作し、構造化ログが出力される
  - Goal: workspace-level E2E テスト、tracing-subscriber によるログ出力 (`$TMPDIR/capsule.log`、`CAPSULE_LOG` でレベル制御)、error recovery の検証
  - Must Not Break: `task check`、既存の全テスト
  - Non-goals: パフォーマンス最適化 (latency 計測・記録のみ)、設定ファイル
  - Acceptance (EARS):
    - When E2E test runs the full flow (daemon start → connect → request → response → cwd change → re-request → response change → shutdown → cleanup), the system shall succeed
    - When `CAPSULE_LOG=debug` is set, the daemon shall output structured log lines to `$TMPDIR/capsule.log`
    - When cwd changes between requests, the response shall reflect the new directory
    - When daemon is stopped and restarted, stale socket shall be detected and removed automatically
  - Evidence: `run=cargo test --workspace -- e2e; oracle=all e2e tests pass; visibility=independent; controls=[context]; missing=[agent]; companion=none`
  - Gates: `static`, `integration`, `system`
  - Executable doc: `cargo test --workspace -- e2e` — binary build → temp socket daemon → connect → request → RenderResult 検証 → cwd 変更 → 再 request → response 変化確認 → shutdown → socket cleanup
  - Why not split vertically further?: E2E とログ出力は最終統合の 1 スライス。ログなしに E2E のデバッグが困難
  - Escalate if: fast-only latency が 5ms 目標を大幅に超過し、アーキテクチャ変更が必要な場合
