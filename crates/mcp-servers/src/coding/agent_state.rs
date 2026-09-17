use mcp_utils::request_context::{AgentIdentity, GatewayRequestContext, RequestContextError};
use rmcp::model::MetaObject;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::RwLock, time::Instant};

/// Mutable safety state belonging to one agent runtime, not to its transport.
#[derive(Default)]
pub struct CodingAgentState {
    pub(super) files_read: RwLock<HashSet<String>>,
    pub(super) activated_rules: Mutex<HashSet<String>>,
}

#[derive(Debug, thiserror::Error)]
pub enum AgentStateError {
    #[error(transparent)]
    Context(#[from] RequestContextError),
    #[error("coding agent identity capacity reached")]
    Capacity,
}

pub(super) struct CodingAgentStates {
    local: Arc<Entry>,
    remote: Mutex<HashMap<AgentIdentity, Arc<Entry>>>,
}

#[derive(Clone)]
pub(super) struct AgentStateLease(Arc<Entry>);

impl CodingAgentStates {
    pub(super) fn new() -> Self {
        Self { local: Arc::new(Entry::new()), remote: Mutex::new(HashMap::new()) }
    }

    pub(super) fn local(&self) -> AgentStateLease {
        AgentStateLease(self.local.clone())
    }

    pub(super) fn resolve(&self, meta: &MetaObject) -> Result<AgentStateLease, AgentStateError> {
        let identity = match GatewayRequestContext::from_meta(Some(meta)) {
            Ok(context) => context.identity,
            Err(RequestContextError::Missing) => return Ok(self.local()),
            Err(error) => return Err(error.into()),
        };
        let mut states = self.remote.lock().expect("agent state lock poisoned");
        let now = Instant::now();
        states.retain(|_, entry| {
            Arc::strong_count(entry) > 1
                || now.duration_since(*entry.idle_since.lock().expect("idle clock lock poisoned")) < IDLE_TTL
        });
        if let Some(entry) = states.get(&identity) {
            return Ok(AgentStateLease(entry.clone()));
        }
        if states.len() >= MAX_IDENTITIES {
            return Err(AgentStateError::Capacity);
        }
        let entry = Arc::new(Entry::new());
        states.insert(identity, entry.clone());
        Ok(AgentStateLease(entry))
    }
}

impl std::ops::Deref for AgentStateLease {
    type Target = CodingAgentState;

    fn deref(&self) -> &Self::Target {
        &self.0.state
    }
}

impl Drop for AgentStateLease {
    fn drop(&mut self) {
        *self.0.idle_since.lock().expect("idle clock lock poisoned") = Instant::now();
    }
}

const IDLE_TTL: Duration = Duration::from_secs(3600);
const MAX_IDENTITIES: usize = 1024;

struct Entry {
    state: CodingAgentState,
    idle_since: Mutex<Instant>,
}

impl Entry {
    fn new() -> Self {
        Self { state: CodingAgentState::default(), idle_since: Mutex::new(Instant::now()) }
    }
}
