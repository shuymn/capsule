# ADR: launchd daemon management

## Status

Accepted (Theme 13)

## Context

capsule daemon needs to start automatically and stay running. Two options:

1. **zsh glue auto-start**: `capsule connect` spawns daemon on demand (Theme 6 approach)
2. **launchd socket activation**: launchd creates the socket and launches daemon on first connection

zsh auto-start has latency on first prompt and no process supervision. launchd provides socket activation, automatic restart, and OS-native lifecycle management.

## Decision

- **Listener abstraction**: `ListenerSource` enum (`Bind` / `Launchd` / `Systemd`) with `acquire_listener()` returning a `std::os::unix::net::UnixListener`. `Systemd` variant is defined but not implemented.
- **Auto-detection**: daemon tries `launch_activate_socket("Listeners")` first. On failure (not under launchd), falls back to standalone bind.
- **Socket path**: `~/.capsule/capsule.sock` (was `$TMPDIR/capsule.sock`). `$TMPDIR` changes across reboots; `~/.capsule/` is stable and launchd can expand `$HOME` in plist.
- **ListenerMode**: `Bound` (standalone) vs `Activated` (launchd). In `Activated` mode, inode monitoring and flock are skipped — launchd owns the socket lifecycle and guarantees single instance.
- **Install/uninstall**: `capsule daemon install` generates plist, writes to `~/Library/LaunchAgents/`, and runs `launchctl bootstrap`. Idempotent — skips reload if plist is identical. `capsule daemon uninstall` runs `launchctl bootout` and removes plist.
- **FFI**: `launch_activate_socket` called via raw FFI (1 C function), isolated in `capsule-sys` crate. Workspace keeps `unsafe_code = "forbid"`; only `capsule-sys` allows unsafe. Non-macOS platforms get a stub that returns `Unsupported`.
- **zsh glue preserved**: `capsule connect` auto-start remains as fallback for non-launchd environments.

## Consequences

- First prompt under launchd has ~0ms daemon startup (socket already exists; daemon starts in background on first connect)
- `capsule daemon install` / `uninstall` provide full lifecycle management
- `unsafe_code = "forbid"` is maintained at workspace level; FFI is isolated in `capsule-sys`
- Standalone mode (without launchd) continues to work identically to pre-Theme 13
- `$CAPSULE_SOCK_DIR` env var allows tests to override the socket directory
