//! Platform-specific FFI bindings for capsule.
//!
//! This crate isolates all `unsafe` FFI code so the rest of the workspace
//! can keep `unsafe_code = "forbid"`.

#![warn(clippy::pedantic, clippy::nursery, clippy::cargo)]

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "linux")]
pub use linux::systemd_activated_socket;
#[cfg(target_os = "macos")]
pub use macos::launch_activate_socket;

/// Stub for non-macOS platforms.
///
/// # Errors
///
/// Always returns [`std::io::ErrorKind::Unsupported`].
#[cfg(not(target_os = "macos"))]
pub fn launch_activate_socket(
    _name: &str,
) -> Result<std::os::unix::net::UnixListener, std::io::Error> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "launch_activate_socket is only available on macOS",
    ))
}

/// Stub for non-Linux platforms.
///
/// # Errors
///
/// Always returns [`std::io::ErrorKind::Unsupported`].
#[cfg(not(target_os = "linux"))]
pub fn systemd_activated_socket() -> Result<std::os::unix::net::UnixListener, std::io::Error> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "systemd socket activation is only available on Linux",
    ))
}
