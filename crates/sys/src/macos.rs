//! macOS-specific FFI: `launch_activate_socket`.

use std::os::unix::{io::RawFd, net::UnixListener};

/// Call macOS `launch_activate_socket(name)` and return a [`UnixListener`].
///
/// Retrieves the first socket fd from launchd, closes any extras, and
/// wraps the fd in a non-blocking [`UnixListener`].
///
/// # Errors
///
/// Returns an I/O error if launchd socket activation fails (e.g. the
/// daemon was not launched by launchd, or the socket name does not match).
pub fn launch_activate_socket(name: &str) -> Result<UnixListener, std::io::Error> {
    let fd = activate_socket_fd(name)?;
    fd_to_listener(fd)
}

/// Raw FFI call to `launch_activate_socket`.
fn activate_socket_fd(name: &str) -> Result<RawFd, std::io::Error> {
    // int launch_activate_socket(const char *name, int **fds, size_t *cnt);
    unsafe extern "C" {
        fn launch_activate_socket(
            name: *const std::ffi::c_char,
            fds: *mut *mut RawFd,
            cnt: *mut usize,
        ) -> std::ffi::c_int;
        fn free(ptr: *mut std::ffi::c_void);
        fn close(fd: std::ffi::c_int) -> std::ffi::c_int;
    }

    let c_name = std::ffi::CString::new(name).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("invalid socket name: {e}"),
        )
    })?;

    let mut fds: *mut RawFd = std::ptr::null_mut();
    let mut cnt: usize = 0;

    // SAFETY: launch_activate_socket is a well-defined C API on macOS.
    // We pass valid pointers and check the return code before accessing fds.
    let ret = unsafe {
        launch_activate_socket(
            c_name.as_ptr(),
            std::ptr::addr_of_mut!(fds),
            std::ptr::addr_of_mut!(cnt),
        )
    };

    if ret != 0 {
        return Err(std::io::Error::from_raw_os_error(ret));
    }

    if cnt == 0 || fds.is_null() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "launch_activate_socket returned 0 fds",
        ));
    }

    // SAFETY: launch_activate_socket guarantees `cnt` valid fds at `fds`.
    let fd = unsafe { *fds };

    // Close extra fds if launchd gave us more than one.
    // SAFETY: fds[i] for i in 1..cnt are valid fds from launch_activate_socket.
    for i in 1..cnt {
        unsafe {
            close(*fds.add(i));
        }
    }

    // SAFETY: fds was allocated by launch_activate_socket via malloc; we must free it.
    unsafe {
        free(fds.cast());
    }

    Ok(fd)
}

/// Convert a raw fd into a non-blocking [`UnixListener`].
fn fd_to_listener(fd: RawFd) -> Result<UnixListener, std::io::Error> {
    use std::os::unix::io::FromRawFd;

    // SAFETY: fd is a valid socket fd from launch_activate_socket.
    // We take exclusive ownership — activate_socket_fd closed any extra fds
    // and freed the fd array, so no other code will close or use this fd.
    let listener = unsafe { UnixListener::from_raw_fd(fd) };
    listener.set_nonblocking(true)?;
    Ok(listener)
}
