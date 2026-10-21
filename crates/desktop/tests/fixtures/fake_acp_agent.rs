//! An ACP agent over stdio for `LocalConnector` tests.
//!
//! It holds an exclusive lock on [`FAKE_AGENT_LOCK_FILE`] in its working directory for as long as
//! it runs, so a test can see where it was started and wait for it to die without polling. It
//! keeps running after its client hangs up, so only killing it ends it.

use acp_utils::testing::FakeAgent;
use aether_desktop::testing::FAKE_AGENT_LOCK_FILE;
use agent_client_protocol::ByteStreams;
use agent_client_protocol::schema::v2::NewSessionResponse;
use std::fs::File;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let lock = File::create(FAKE_AGENT_LOCK_FILE).expect("create the lock file in the working directory");
    lock.lock().expect("lock the lock file");
    let agent = FakeAgent::default().new_session_response(NewSessionResponse::new("local-session"));
    let stdio = ByteStreams::new(tokio::io::stdout().compat_write(), tokio::io::stdin().compat());
    let _ = agent.agent().connect_to(stdio).await;
    std::future::pending::<()>().await;
    drop(lock);
}
