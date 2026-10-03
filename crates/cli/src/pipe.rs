//! Nonblocking inherited pipes. Credit wakes zsh when a partial write can progress.

use std::{
    io,
    os::fd::{AsFd, OwnedFd},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, ready},
};

use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf, unix::AsyncFd},
    sync::Notify,
};

pub fn configure_shell_endpoints() -> io::Result<()> {
    nonblocking(io::stdin())?;
    nonblocking(io::stdout())
}

fn nonblocking(fd: impl AsFd) -> io::Result<()> {
    let flags = fcntl_getfl(&fd)?;
    fcntl_setfl(&fd, flags | OFlags::NONBLOCK)?;
    Ok(())
}

pub struct Pipe {
    fd: AsyncFd<OwnedFd>,
    credit: Option<Arc<Notify>>,
}

impl Pipe {
    pub(crate) fn new(fd: OwnedFd, credit: Option<Arc<Notify>>) -> io::Result<Self> {
        nonblocking(&fd)?;
        Ok(Self {
            fd: AsyncFd::new(fd)?,
            credit,
        })
    }
}

impl AsyncRead for Pipe {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            let mut guard = ready!(self.fd.poll_read_ready(cx))?;
            match guard.try_io(|fd| {
                rustix::io::read(fd, buf.initialize_unfilled()).map_err(io::Error::from)
            }) {
                Ok(Ok(count)) => {
                    buf.advance(count);
                    if count > 0
                        && let Some(credit) = &self.credit
                    {
                        credit.notify_one();
                    }
                    return Poll::Ready(Ok(()));
                }
                Ok(Err(error)) => return Poll::Ready(Err(error)),
                Err(_) => {}
            }
        }
    }
}

impl AsyncWrite for Pipe {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            let mut guard = ready!(self.fd.poll_write_ready(cx))?;
            if let Ok(result) =
                guard.try_io(|fd| rustix::io::write(fd, buf).map_err(io::Error::from))
            {
                return Poll::Ready(result);
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
