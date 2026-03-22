# ADR: multi-platform daemon management

## Status

Accepted

## Context

capsule daemon needs to start automatically and stay running. Two options:

1. **zsh glue auto-start**: `capsule connect` spawns daemon on demand (Theme 6 approach)
2. **Service manager socket activation**: the OS service manager creates the socket and launches daemon on first connection

zsh auto-start has latency on first prompt and no process supervision. Native service managers provide socket activation, automatic restart, and OS-native lifecycle management.

## Decision

### Listener abstraction

`ListenerSource` enum with three variants:
- `Bind(PathBuf)` — standalone bind mode (all platforms)
- `Launchd(String)` — macOS `launch_activate_socket` socket activation
- `Systemd` — Linux `$LISTEN_FDS` socket activation

`acquire_listener()` returns a `std::os::unix::net::UnixListener`. `ListenerMode::Activated` is used for both `Launchd` and `Systemd` variants (inode monitoring and flock are skipped — the service manager owns the socket lifecycle and guarantees single instance).

### Auto-detection

`daemon::run()` tries platform socket activation first, falls back to standalone bind:
- macOS: `launch_activate_socket("Listeners")`
- Linux: `$LISTEN_PID`/`$LISTEN_FDS` validation, then fd 3
- Fallback: flock + bind at `~/.capsule/capsule.sock`

### Socket path

`~/.capsule/capsule.sock` (stable across reboots; service managers can expand `$HOME`).

### ServiceManager trait

`capsule daemon install/uninstall` is handled by a `ServiceManager` trait with three methods:
- `install(home, socket_path)` — write service definition, start daemon, block until ready
- `uninstall(home)` — stop daemon, remove service definition
- `restart()` — restart daemon, block until new instance is ready

Implementations:
- `Launchd` (macOS) — generates plist to `~/Library/LaunchAgents/`, uses `launchctl bootstrap/bootout/kickstart`
- `Systemd` (Linux) — generates `.service`/`.socket` to `~/.config/systemd/user/`, uses `systemctl --user`

Both `install` and `restart` use `wait_until_daemon_ready` (Hello/HelloAck protocol handshake) to confirm the daemon is ready before returning.

### FFI isolation

- `launch_activate_socket` — raw C FFI, isolated in `capsule-sys` crate
- `systemd_activated_socket` — uses `FromRawFd` (unsafe), isolated in `capsule-sys` crate
- Workspace keeps `unsafe_code = "forbid"`; only `capsule-sys` allows unsafe
- Non-macOS platforms get a stub for `launch_activate_socket` that returns `Unsupported`
- Non-Linux platforms get a stub for `systemd_activated_socket` that returns `Unsupported`

### Module structure

```
crates/cli/src/daemon/service/
  mod.rs      — ServiceManager trait, shared helpers (wait_until_daemon_ready, daemon_needs_restart)
  launchd.rs  — Launchd impl (#[cfg(target_os = "macos")])
  systemd.rs  — Systemd impl (#[cfg(target_os = "linux")])
crates/sys/src/
  macos.rs    — launch_activate_socket FFI
  linux.rs    — systemd_activated_socket + parse_listen_vars
```

### zsh glue preserved

`capsule connect` auto-start remains as fallback for non-service-manager environments.

## Consequences

- First prompt under service management has ~0ms daemon startup (socket already exists; daemon starts in background on first connect)
- `capsule daemon install` / `uninstall` provide full lifecycle management on macOS and Linux
- `unsafe_code = "forbid"` is maintained at workspace level; FFI is isolated in `capsule-sys`
- Standalone mode (without service management) continues to work on all platforms
- `$CAPSULE_SOCK_DIR` env var allows tests to override the socket directory
- Unit file generation tests run as pure-function tests (no service manager process required)
