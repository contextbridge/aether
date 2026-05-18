//! Piped Stdio transport for ACP.
//!
//! `agent_client_protocol::Stdio` (from acp crate) drives stdin/stdout via
//! `blocking::Unblock` (blocking syscalls on a thread-pool worker) and treats
//! every io error as fatal. That's a provlem when the parent process spawns
//! this binary with non-blocking pipe fds: a `read`/`write` on the child side
//! can return `EAGAIN`, which the upstream transport surfaces as a fatal io error
//! and tears the session down.
//!
//! `tokio::net::unix::pipe` polls the fds via epoll instead, so `EAGAIN` isn't
//! a fatal error.

use agent_client_protocol::{ByteStreams, ConnectTo, Error, Role};
use std::io;
use std::os::fd::AsFd;
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
        let stdin = io::stdin()
            .as_fd()
            .try_clone_to_owned()
            .and_then(Receiver::from_owned_fd)
            .map_err(Error::into_internal_error)?;

        let stdout = io::stdout()
            .as_fd()
            .try_clone_to_owned()
            .and_then(Sender::from_owned_fd)
            .map_err(Error::into_internal_error)?;

        let streams = ByteStreams::new(stdout.compat_write(), stdin.compat());
        ConnectTo::<T>::connect_to(streams, client).await
    }
}

impl Default for Stdio {
    fn default() -> Self {
        Self::new()
    }
}
