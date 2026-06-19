//! Piped Stdio transport for ACP.
//!
//! `agent_client_protocol::Stdio` (from acp crate) drives stdin/stdout via
//! `blocking::Unblock` (blocking syscalls on a thread-pool worker) and treats
//! every io error as fatal. That's a problem when the parent process spawns
//! this binary with non-blocking pipe fds: a `read`/`write` on the child side
//! can return `EAGAIN`, which the upstream transport surfaces as a fatal io error
//! and tears the session down.
//!
//! We poll the fds via epoll instead, so `EAGAIN` isn't fatal. Which tokio type
//! backs that depends on the fd: when spawned with `stdio: 'pipe'`, Node/libuv
//! backs stdin/stdout with `AF_UNIX` socketpairs (not FIFOs), so we route sockets
//! through `tokio::net::UnixStream` and real FIFOs through
//! `tokio::net::unix::pipe`.

use agent_client_protocol::{ByteStreams, ConnectTo, Error, Role};
use futures::{AsyncRead, AsyncWrite};
use std::fs::File;
use std::io;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::UnixStream as StdUnixStream;
use tokio::net::UnixStream;
use tokio::net::unix::pipe::{Receiver, Sender};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

pub struct Stdio;

impl Stdio {
    pub fn new() -> Self {
        Self
    }
}

impl<T: Role> ConnectTo<T> for Stdio {
    async fn connect_to(self, client: impl ConnectTo<T::Counterpart>) -> Result<(), Error> {
        let streams = ByteStreams::new(
            stdout(io::stdout().as_fd()).map_err(Error::into_internal_error)?,
            stdin(io::stdin().as_fd()).map_err(Error::into_internal_error)?,
        );
        ConnectTo::<T>::connect_to(streams, client).await
    }
}

impl Default for Stdio {
    fn default() -> Self {
        Self::new()
    }
}

type BoxRead = Box<dyn AsyncRead + Unpin + Send>;
type BoxWrite = Box<dyn AsyncWrite + Unpin + Send>;

fn stdin(fd: BorrowedFd) -> io::Result<BoxRead> {
    let owned = fd.try_clone_to_owned()?;
    if is_socket(&owned)? {
        Ok(Box::new(unix_stream(owned)?.compat()))
    } else {
        Ok(Box::new(Receiver::from_owned_fd(owned)?.compat()))
    }
}

fn stdout(fd: BorrowedFd) -> io::Result<BoxWrite> {
    let owned = fd.try_clone_to_owned()?;
    if is_socket(&owned)? {
        Ok(Box::new(unix_stream(owned)?.compat_write()))
    } else {
        Ok(Box::new(Sender::from_owned_fd(owned)?.compat_write()))
    }
}

fn unix_stream(owned: OwnedFd) -> io::Result<UnixStream> {
    let std = StdUnixStream::from(owned);
    std.set_nonblocking(true)?;
    UnixStream::from_std(std)
}

fn is_socket(fd: &OwnedFd) -> io::Result<bool> {
    let probe = File::from(fd.try_clone()?);
    Ok(probe.metadata()?.file_type().is_socket())
}
