# ADR: Protocol-aware connect

## Status

Accepted (Theme 22)

## Context

capsule connect was originally a dumb byte relay: bytes from shell stdin → daemon socket, bytes from socket → shell stdout. The wire protocol (netstring + LF) was chosen so zsh could directly read/write messages via `read -r` and simple string operations.

Over time, connect gained protocol awareness:

- Theme 15 added Hello/HelloAck negotiation in connect (build ID check + env var names)
- init.zsh grew ~60 lines of protocol code: netstring encode (`_capsule_ns`), netstring decode (`_capsule_parse_wire`), 10-field Request construction (`_capsule_send_request`)

This violated the architecture baseline's stated goal: "zsh 側は coproc relay 経由の薄い glue に徹する". Connect was neither fully protocol-unaware (Hello/HelloAck) nor fully protocol-aware (Request/Response still handled by shell).

## Decision

Make capsule connect the protocol translator between shell and daemon:

- Shell ↔ connect: tab-separated text protocol (simple, zsh-native)
- Connect ↔ daemon: netstring wire protocol (unchanged)

Connect owns:

- Session ID generation (was shell's `od | tr` invocation)
- Protocol version, message type tagging
- Netstring encoding/decoding for all message types

Shell owns:

- Generation counter (authority for stale Update detection)
- Exit code, duration, cwd, cols, keymap capture
- Env var value collection (names from `E:` metadata line)

Shell ↔ connect text protocol:

- Request: `<gen>\t<exit>\t<dur>\t<cwd>\t<cols>\t<keymap>\t<env_meta>\n`
- Response: `<type>\t<gen>\t<left1>\t<left2>\n`
- Startup: `E:<var1>,<var2>,...\n` (unchanged)

Tab separator chosen because: no field contains literal tabs (verified: integers, paths, ANSI prompt output, keymap values). Env meta uses `\0` as internal separator, not `\t`. The existing wire format already forbids LF in field values.

## Rejected Alternatives

1. **Keep dumb relay** (status quo): connect is already half protocol-aware (Hello/HelloAck). Shell carries ~60 lines of brittle protocol code with zsh-specific byte-counting hacks (`setopt no_multibyte`). Inconsistent split of responsibility.

2. **Per-prompt subcommand**: `capsule prompt` invoked per precmd. Adds fork+exec (~3-7ms) per prompt. Loses persistent connection and async Update support. Defeats the daemon architecture's latency advantage.

3. **Length-prefixed fields on shell↔connect**: Adds encoding complexity for no benefit. The whole point is to remove encoding complexity from the shell.

## Consequence

- init.zsh loses ~60 lines of protocol code (netstring encode/decode, Request construction, Response parsing)
- All protocol logic consolidated in Rust (single source of truth, testable)
- Shell code is genuinely "thin glue": capture state → print tab-separated line → read tab-separated response
- connect relay function changes from byte-copy (`tokio::io::copy`) to message loop (read/translate/write)
- Existing e2e tests updated to use new text protocol when interacting with connect's stdin/stdout
- init.rs tests for `_capsule_ns` and `_capsule_parse_wire` become obsolete and are removed

## Revisit trigger

- A prompt module produces output containing literal tab characters
- Shell needs to send structured data that doesn't fit tab-separated format
- Multi-shell support requires different shell-facing protocols
