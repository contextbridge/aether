use super::client_info;
use crate::connection::{Connection, ConnectionError, Session};
use crate::runtime::SessionRuntime;
use crate::session_core::{Effect, SessionCore};
use acp_utils::testing::FakeAgent;
use agent_client_protocol::schema::v2::SessionId;
use std::path::PathBuf;

pub struct LiveSession {
    pub core: SessionCore,
    pub runtime: SessionRuntime,
}

impl LiveSession {
    pub async fn connect(
        agent: FakeAgent,
        working_dir: impl Into<PathBuf>,
        session_id: Option<SessionId>,
    ) -> Result<Self, ConnectionError> {
        let connection = Connection::connect_to(client_info(), agent.serve()).await?;
        Ok(Self::start(connection.open(session_id, working_dir.into()).await?))
    }

    pub fn start(session: Session) -> Self {
        let (config, runtime) = session.into_core();
        Self { core: SessionCore::new(config), runtime }
    }

    pub async fn run_until(&mut self, done: impl Fn(&SessionCore) -> bool) {
        loop {
            for effect in self.core.take_effects() {
                if let Effect::Dispatch(command) = effect {
                    self.runtime.dispatch(command);
                }
            }
            if done(&self.core) {
                return;
            }
            let input = self.runtime.next().await;
            self.core.on_input(input);
        }
    }

    pub async fn shutdown(mut self) {
        self.runtime.shutdown().await;
    }
}
