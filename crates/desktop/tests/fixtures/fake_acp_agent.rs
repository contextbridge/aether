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
