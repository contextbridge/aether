//! Stdio transport for Aether's ACP server.
//!
//! Why this exists: `tokio::io::stdin()`'s nonblocking compat adapter can
//! surface `EAGAIN`/`WouldBlock` on terminal stdio, which the ACP JSON-RPC
//! transport actor treats as a fatal `Internal error` and kills the session.
//! `blocking::Unblock` runs synchronous `std::io` reads/writes on a thread
//! pool against the blocking handles, so the readiness error never occurs.
//!
//! Vendored from the upstream `agent-client-protocol::Stdio` which hasn't
//! published a new version of the SDK with this inside yet.
//!
//! TODO: Delete this once the SDK ships an equivalent transport.

use agent_client_protocol::{ByteStreams, ConnectTo, Error, Role};
use blocking::Unblock;
use std::io::{stdin, stdout};

pub struct Stdio;

impl Stdio {
    pub fn new() -> Self {
        Self
    }
}

impl Default for Stdio {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Role> ConnectTo<T> for Stdio {
    async fn connect_to(self, client: impl ConnectTo<T::Counterpart>) -> Result<(), Error> {
        ConnectTo::<T>::connect_to(ByteStreams::new(Unblock::new(stdout()), Unblock::new(stdin())), client).await
    }
}
