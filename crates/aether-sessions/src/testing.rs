//! Shared fixtures and event builders for exercising `aether-sessions` in tests.
//!
//! Compiled for the crate's own tests and for any consumer that enables the
//! `testing` feature. Prefer these over hand-rolling per-suite event constructors.

use std::fs;
use std::path::Path;

use aether_core::events::{AgentEvent, ContextEvent, MessageEvent, StreamState, ToolEvent, TurnEvent, TurnOutcome};
use llm::{ContentBlock, ToolCallError, ToolCallRequest, ToolCallResult};
use tempfile::TempDir;

use crate::model::{SessionControlEvent, SessionEvent, SessionMeta, UserEvent};
use crate::store::SessionStore;

/// The `createdAt` used by fixtures unless a test overrides it.
pub const DEFAULT_CREATED_AT: &str = "2026-01-01T00:00:00Z";

/// A [`SessionMeta`] with stable defaults; override fields for the scenario under test.
pub fn session_meta(session_id: &str, created_at: &str) -> SessionMeta {
    SessionMeta {
        session_id: session_id.to_string(),
        cwd: "/tmp/project".into(),
        model: "test-model".into(),
        selected_mode: Some("planner".into()),
        created_at: created_at.to_string(),
    }
}

/// A user text-message event.
pub fn user_message(text: &str) -> SessionEvent {
    user_message_with(vec![ContentBlock::text(text)])
}

/// A user message event carrying arbitrary content blocks (text, images, ...).
pub fn user_message_with(content: Vec<ContentBlock>) -> SessionEvent {
    SessionEvent::User(UserEvent::Message { content })
}

/// The complete assistant text for `message_id`.
pub fn assistant_text(message_id: &str, text: &str) -> SessionEvent {
    SessionEvent::Agent(AgentEvent::Message(MessageEvent::Text {
        message_id: message_id.into(),
        chunk: text.into(),
        is_complete: true,
    }))
}

/// A streaming assistant text chunk, which is never persisted.
pub fn partial_text(message_id: &str, chunk: &str) -> SessionEvent {
    SessionEvent::Agent(AgentEvent::text(message_id, chunk, StreamState::Partial))
}

/// A tool call issued by the agent.
pub fn tool_call(id: &str, name: &str, arguments: &str) -> SessionEvent {
    SessionEvent::Agent(AgentEvent::Tool(ToolEvent::Call {
        request: ToolCallRequest { id: id.into(), name: name.into(), arguments: arguments.into() },
    }))
}

/// A successful tool result for `id`.
pub fn tool_result(id: &str, name: &str, result: &str) -> SessionEvent {
    SessionEvent::Agent(AgentEvent::Tool(ToolEvent::Result {
        result: ToolCallResult { id: id.into(), name: name.into(), arguments: "{}".into(), result: result.into() },
        result_meta: None,
    }))
}

/// A failed tool execution for `id`.
pub fn tool_error(id: &str, name: &str, error: &str) -> SessionEvent {
    SessionEvent::Agent(AgentEvent::Tool(ToolEvent::Error {
        error: ToolCallError { id: id.into(), name: name.into(), arguments: Some("{}".into()), error: error.into() },
    }))
}

/// The terminal event of a turn.
pub fn turn_ended(outcome: TurnOutcome) -> SessionEvent {
    SessionEvent::Agent(AgentEvent::Turn(TurnEvent::Ended { outcome }))
}

/// A control event recording an agent switch.
pub fn agent_switched(from: Option<&str>, to: Option<&str>) -> SessionEvent {
    SessionEvent::Control(SessionControlEvent::AgentSwitched {
        from: from.map(str::to_string),
        to: to.map(str::to_string),
    })
}

/// A compaction result replacing `messages_removed` prior messages with `summary`.
pub fn compaction_result(summary: &str, messages_removed: usize) -> SessionEvent {
    SessionEvent::Agent(AgentEvent::Context(ContextEvent::CompactionResult {
        summary: summary.into(),
        messages_removed,
    }))
}

/// An on-disk [`SessionStore`] rooted in a temporary directory, set up fluently.
pub struct TestStore {
    directory: TempDir,
    store: SessionStore,
}

impl TestStore {
    /// Creates an empty store in a fresh temporary directory.
    pub fn new() -> Self {
        let directory = TempDir::new().expect("temporary session directory");
        let store = SessionStore::from_path(directory.path().to_path_buf());
        Self { directory, store }
    }

    /// The directory backing the store; session files live directly inside it.
    pub fn path(&self) -> &Path {
        self.directory.path()
    }

    /// The store under test.
    pub fn store(&self) -> &SessionStore {
        &self.store
    }

    /// Appends the default metadata followed by `events` for `session_id`.
    pub fn session(self, session_id: &str, events: &[SessionEvent]) -> Self {
        self.append_meta(session_id, &session_meta(session_id, DEFAULT_CREATED_AT));
        for event in events {
            self.append(session_id, event);
        }
        self
    }

    /// Appends custom metadata for `session_id`.
    pub fn append_meta(&self, session_id: &str, meta: &SessionMeta) {
        self.store.append_meta(session_id, meta).expect("failed to append metadata");
    }

    /// Appends a single event for `session_id`.
    pub fn append(&self, session_id: &str, event: &SessionEvent) {
        self.store.append_event(session_id, event).expect("failed to append event");
    }

    /// Writes a raw file into the store directory, bypassing the store API.
    pub fn write_raw(&self, name: &str, content: &str) {
        fs::write(self.path().join(name), content).expect("failed to write raw store file");
    }
}

impl Default for TestStore {
    fn default() -> Self {
        Self::new()
    }
}
