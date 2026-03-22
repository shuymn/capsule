//! Linux-specific: systemd socket activation.

use std::os::unix::net::UnixListener;

/// Retrieve a socket fd from systemd socket activation and wrap it as a
/// non-blocking [`UnixListener`].
///
/// Checks that `$LISTEN_PID` matches the current process PID and that
/// `$LISTEN_FDS >= 1`, then wraps fd 3 (the first activated socket) as a
/// non-blocking [`UnixListener`].
///
/// # Errors
///
/// Returns an I/O error if:
/// - `$LISTEN_PID` or `$LISTEN_FDS` are absent, unparseable, or do not pass
///   validation.
/// - The fd cannot be made non-blocking.
pub fn systemd_activated_socket() -> Result<UnixListener, std::io::Error> {
    let listen_pid = std::env::var("LISTEN_PID").map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("LISTEN_PID not set (not launched by systemd socket activation): {error}"),
        )
    })?;
    let listen_fds = std::env::var("LISTEN_FDS").map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("LISTEN_FDS not set (not launched by systemd socket activation): {error}"),
        )
    })?;

    let fd_count = parse_listen_vars(&listen_pid, &listen_fds, std::process::id())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    if fd_count == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "LISTEN_FDS is 0, no sockets passed by systemd",
        ));
    }

    // SAFETY: fd 3 is the first socket passed by systemd socket activation.
    // parse_listen_vars verified that LISTEN_PID matches us and LISTEN_FDS >= 1,
    // so fd 3 is a valid socket that systemd handed to this process.
    // We take exclusive ownership here — no other code in this process uses it.
    let listener = unsafe {
        use std::os::unix::io::FromRawFd;
        UnixListener::from_raw_fd(3)
    };
    listener.set_nonblocking(true)?;
    Ok(listener)
}

/// Validate `$LISTEN_PID` and `$LISTEN_FDS` against the current process.
///
/// Returns the number of activated fds on success.
///
/// # Errors
///
/// Returns a descriptive error string if validation fails.
fn parse_listen_vars(listen_pid: &str, listen_fds: &str, my_pid: u32) -> Result<u32, String> {
    let pid: u32 = listen_pid
        .trim()
        .parse()
        .map_err(|error| format!("LISTEN_PID is not a valid integer: {listen_pid:?}: {error}"))?;

    if pid != my_pid {
        return Err(format!(
            "LISTEN_PID {pid} does not match current PID {my_pid}"
        ));
    }

    let fds: u32 = listen_fds
        .trim()
        .parse()
        .map_err(|error| format!("LISTEN_FDS is not a valid integer: {listen_fds:?}: {error}"))?;

    Ok(fds)
}

#[cfg(test)]
mod tests {
    use super::parse_listen_vars;

    #[test]
    fn test_parse_listen_vars_valid() -> Result<(), Box<dyn std::error::Error>> {
        let fds = parse_listen_vars("1234", "1", 1234)?;
        assert_eq!(fds, 1);
        Ok(())
    }

    #[test]
    fn test_parse_listen_vars_pid_mismatch() {
        let result = parse_listen_vars("9999", "1", 1234);
        assert!(result.is_err(), "should fail when PID does not match");
        if let Err(e) = result {
            assert!(
                e.contains("does not match"),
                "error should mention PID mismatch: {e}"
            );
        }
    }

    #[test]
    fn test_parse_listen_vars_invalid_pid() {
        let result = parse_listen_vars("abc", "1", 1234);
        assert!(result.is_err(), "should fail on non-numeric LISTEN_PID");
    }

    #[test]
    fn test_parse_listen_vars_invalid_fds() {
        let result = parse_listen_vars("1234", "xyz", 1234);
        assert!(result.is_err(), "should fail on non-numeric LISTEN_FDS");
    }

    #[test]
    fn test_parse_listen_vars_zero_fds() -> Result<(), Box<dyn std::error::Error>> {
        let fds = parse_listen_vars("1234", "0", 1234)?;
        assert_eq!(fds, 0);
        Ok(())
    }

    #[test]
    fn test_parse_listen_vars_multiple_fds() -> Result<(), Box<dyn std::error::Error>> {
        let fds = parse_listen_vars("42", "3", 42)?;
        assert_eq!(fds, 3);
        Ok(())
    }
}
