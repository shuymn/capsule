//! Listener acquisition — abstracts how the daemon obtains its [`UnixListener`].
//!
//! [`ListenerSource`] describes where the listener comes from:
//! - [`Bind`](ListenerSource::Bind) — traditional socket file bind (standalone mode)
//! - [`Launchd`](ListenerSource::Launchd) — macOS `launch_activate_socket` (socket activation)

use std::{os::unix::net::UnixListener, path::PathBuf};

/// How the daemon obtains its listening socket.
#[derive(Debug, Clone)]
pub enum ListenerSource {
    /// Bind a new Unix socket at the given path (standalone mode).
    ///
    /// The caller is expected to guarantee exclusivity (e.g. via flock)
    /// so that unconditional removal of a stale socket file is safe.
    Bind(PathBuf),

    /// Receive the socket from macOS launchd via `launch_activate_socket`.
    ///
    /// The socket name must match the `SockServiceName` in the launchd plist.
    Launchd(String),
}

/// Acquire a [`UnixListener`] from the given [`ListenerSource`].
///
/// Returns a standard-library [`UnixListener`] in non-blocking mode.
/// The caller is responsible for converting it to a tokio listener
/// inside a runtime context.
///
/// # Errors
///
/// Returns an I/O error if the socket cannot be bound or activated.
pub fn acquire_listener(source: &ListenerSource) -> Result<UnixListener, std::io::Error> {
    match source {
        ListenerSource::Bind(path) => {
            // Unconditionally remove any existing socket file.
            // The caller guarantees exclusivity via flock, so no TOCTOU risk.
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
            let listener = UnixListener::bind(path)?;
            listener.set_nonblocking(true)?;
            Ok(listener)
        }
        ListenerSource::Launchd(name) => capsule_sys::launch_activate_socket(name),
    }
}

/// How the daemon obtained its listener — determines shutdown behavior.
///
/// In [`Bound`](ListenerMode::Bound) mode the daemon monitors the socket
/// file's inode and cleans up on shutdown. In [`Activated`](ListenerMode::Activated)
/// mode inode monitoring is skipped (the service manager owns the socket).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListenerMode {
    /// Daemon bound the socket itself (standalone).
    /// Carries the socket path for inode monitoring and cleanup.
    Bound(PathBuf),
    /// Socket was received via activation (launchd).
    /// Inode monitoring and socket cleanup are skipped.
    Activated,
}

impl ListenerSource {
    /// Returns the [`ListenerMode`] corresponding to this source.
    #[must_use]
    pub fn mode(&self) -> ListenerMode {
        match self {
            Self::Bind(path) => ListenerMode::Bound(path.clone()),
            Self::Launchd(_) => ListenerMode::Activated,
        }
    }
}
