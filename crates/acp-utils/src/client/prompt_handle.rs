use agent_client_protocol::schema::{ContentBlock, SessionId};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc;

use super::error::AcpClientError;
use crate::notifications::{
    PromptSearchParams, SessionPreviewParams, WorkspaceListParams, WorkspaceMoveParams, WorkspaceMoveTarget,
};

/// Commands sent from the main thread to the ACP client task.
#[derive(Debug)]
pub enum PromptCommand {
    Prompt { session_id: SessionId, text: String, content: Option<Vec<ContentBlock>> },
    Cancel { session_id: SessionId },
    SetConfigOption { session_id: SessionId, config_id: String, value: String },
    AuthenticateMcpServer { session_id: SessionId, server_name: String },
    Authenticate { method_id: String },
    ListSessions,
    LoadSession { session_id: SessionId, cwd: PathBuf },
    NewSession { cwd: std::path::PathBuf },
    SearchPrompts(PromptSearchParams),
    SessionPreview(SessionPreviewParams),
    ListWorkspaces(WorkspaceListParams),
    MoveWorkspace(WorkspaceMoveParams),
}

/// Send-safe handle for issuing prompt commands to the ACP client task.
#[derive(Clone)]
pub struct AcpPromptHandle {
    pub(crate) cmd_tx: mpsc::UnboundedSender<PromptCommand>,
    pub(crate) fail_signal: Option<Arc<AtomicBool>>,
}

impl AcpPromptHandle {
    /// Create a handle whose sends always succeed but are never read.
    /// Useful for tests that don't care about prompt delivery.
    pub fn noop() -> Self {
        let (cmd_tx, rx) = mpsc::unbounded_channel();
        std::mem::forget(rx);
        Self { cmd_tx, fail_signal: None }
    }

    /// Create a handle paired with a receiver for inspecting sent commands.
    /// Useful for tests that need to verify which commands were dispatched.
    pub fn recording() -> (Self, mpsc::UnboundedReceiver<PromptCommand>) {
        let (cmd_tx, rx) = mpsc::unbounded_channel();
        (Self { cmd_tx, fail_signal: None }, rx)
    }

    /// Create a handle whose sends fail when the returned signal is set to true.
    /// Useful for testing error paths without needing a real transport failure.
    /// Returns the handle, a failure flag, and a receiver for inspecting commands
    /// sent while the flag is false.
    pub fn failable() -> (Self, Arc<AtomicBool>, mpsc::UnboundedReceiver<PromptCommand>) {
        let (cmd_tx, rx) = mpsc::unbounded_channel();
        let fail = Arc::new(AtomicBool::new(false));
        (Self { cmd_tx, fail_signal: Some(Arc::clone(&fail)) }, fail, rx)
    }

    pub fn prompt(
        &self,
        session_id: &SessionId,
        text: &str,
        content: Option<Vec<ContentBlock>>,
    ) -> Result<(), AcpClientError> {
        self.send(PromptCommand::Prompt { session_id: session_id.clone(), text: text.to_string(), content })
    }

    pub fn cancel(&self, session_id: &SessionId) -> Result<(), AcpClientError> {
        self.send(PromptCommand::Cancel { session_id: session_id.clone() })
    }

    pub fn set_config_option(
        &self,
        session_id: &SessionId,
        config_id: &str,
        value: &str,
    ) -> Result<(), AcpClientError> {
        self.send(PromptCommand::SetConfigOption {
            session_id: session_id.clone(),
            config_id: config_id.to_string(),
            value: value.to_string(),
        })
    }

    pub fn authenticate_mcp_server(&self, session_id: &SessionId, server_name: &str) -> Result<(), AcpClientError> {
        self.send(PromptCommand::AuthenticateMcpServer {
            session_id: session_id.clone(),
            server_name: server_name.to_string(),
        })
    }

    pub fn authenticate(&self, method_id: &str) -> Result<(), AcpClientError> {
        self.send(PromptCommand::Authenticate { method_id: method_id.to_string() })
    }

    pub fn list_sessions(&self) -> Result<(), AcpClientError> {
        self.send(PromptCommand::ListSessions)
    }

    pub fn load_session(&self, session_id: &SessionId, cwd: &Path) -> Result<(), AcpClientError> {
        self.send(PromptCommand::LoadSession { session_id: session_id.clone(), cwd: cwd.to_path_buf() })
    }

    pub fn new_session(&self, cwd: &Path) -> Result<(), AcpClientError> {
        self.send(PromptCommand::NewSession { cwd: cwd.to_path_buf() })
    }

    pub fn search_prompts(&self, params: PromptSearchParams) -> Result<(), AcpClientError> {
        self.send(PromptCommand::SearchPrompts(params))
    }

    pub fn session_preview(&self, session_id: &SessionId) -> Result<(), AcpClientError> {
        self.send(PromptCommand::SessionPreview(SessionPreviewParams { session_id: session_id.0.to_string() }))
    }

    pub fn list_workspaces(&self, session_id: &SessionId) -> Result<(), AcpClientError> {
        self.send(PromptCommand::ListWorkspaces(WorkspaceListParams { session_id: session_id.0.to_string() }))
    }

    pub fn move_workspace(&self, session_id: &SessionId, target: WorkspaceMoveTarget) -> Result<(), AcpClientError> {
        self.send(PromptCommand::MoveWorkspace(WorkspaceMoveParams { session_id: session_id.0.to_string(), target }))
    }

    fn send(&self, cmd: PromptCommand) -> Result<(), AcpClientError> {
        if self.fail_signal.as_ref().is_some_and(|s| s.load(Ordering::Relaxed)) {
            return Err(AcpClientError::AgentCrashed("failable handle set to fail".into()));
        }
        self.cmd_tx.send(cmd).map_err(|_| AcpClientError::AgentCrashed("command channel closed".into()))
    }
}

#[cfg(test)]
mod tests {
    use agent_client_protocol::schema::TextContent;

    use super::*;

    #[test]
    fn test_noop_handle_succeeds_silently() {
        let handle = AcpPromptHandle::noop();
        let session_id = SessionId::new("test");

        assert!(handle.prompt(&session_id, "hello", None).is_ok());
        assert!(handle.cancel(&session_id).is_ok());
    }

    #[test]
    fn test_prompt_sends_command() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let handle = AcpPromptHandle { cmd_tx: tx, fail_signal: None };
        let session_id = SessionId::new("sess-1");

        handle.prompt(&session_id, "hello", None).unwrap();

        let cmd = rx.try_recv().unwrap();
        match cmd {
            PromptCommand::Prompt { session_id, text, .. } => {
                assert_eq!(session_id.0.as_ref(), "sess-1");
                assert_eq!(text, "hello");
            }
            _ => panic!("Expected Prompt command"),
        }
    }

    #[test]
    fn test_cancel_sends_command() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let handle = AcpPromptHandle { cmd_tx: tx, fail_signal: None };
        let session_id = SessionId::new("sess-1");

        handle.cancel(&session_id).unwrap();

        let cmd = rx.try_recv().unwrap();
        assert!(matches!(cmd, PromptCommand::Cancel { .. }));
    }

    #[test]
    fn test_set_config_option_sends_command() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let handle = AcpPromptHandle { cmd_tx: tx, fail_signal: None };
        let session_id = SessionId::new("sess-1");

        handle.set_config_option(&session_id, "model", "gpt-4o").unwrap();

        let cmd = rx.try_recv().unwrap();
        match cmd {
            PromptCommand::SetConfigOption { session_id, config_id, value } => {
                assert_eq!(session_id.0.as_ref(), "sess-1");
                assert_eq!(config_id, "model");
                assert_eq!(value, "gpt-4o");
            }
            _ => panic!("Expected SetConfigOption command"),
        }
    }

    #[test]
    fn test_prompt_with_content_sends_blocks() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let handle = AcpPromptHandle { cmd_tx: tx, fail_signal: None };
        let session_id = SessionId::new("sess-1");
        let content = vec![ContentBlock::Text(TextContent::new("attached"))];

        handle.prompt(&session_id, "hello", Some(content.clone())).unwrap();

        let cmd = rx.try_recv().unwrap();
        match cmd {
            PromptCommand::Prompt { session_id, text, content: Some(extra) } => {
                assert_eq!(session_id.0.as_ref(), "sess-1");
                assert_eq!(text, "hello");
                assert_eq!(extra, content);
            }
            _ => panic!("Expected Prompt command with content"),
        }
    }

    #[test]
    fn test_list_sessions_sends_command() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let handle = AcpPromptHandle { cmd_tx: tx, fail_signal: None };

        handle.list_sessions().unwrap();

        let cmd = rx.try_recv().unwrap();
        assert!(matches!(cmd, PromptCommand::ListSessions));
    }

    #[test]
    fn test_load_session_sends_command() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let handle = AcpPromptHandle { cmd_tx: tx, fail_signal: None };
        let session_id = SessionId::new("sess-restore");
        let cwd = Path::new("/tmp/project");

        handle.load_session(&session_id, cwd).unwrap();

        let cmd = rx.try_recv().unwrap();
        match cmd {
            PromptCommand::LoadSession { session_id, cwd } => {
                assert_eq!(session_id.0.as_ref(), "sess-restore");
                assert_eq!(cwd, std::path::PathBuf::from("/tmp/project"));
            }
            _ => panic!("Expected LoadSession command"),
        }
    }

    #[test]
    fn test_list_workspaces_sends_command() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let handle = AcpPromptHandle { cmd_tx: tx, fail_signal: None };
        handle.list_workspaces(&SessionId::new("sess-1")).unwrap();

        let cmd = rx.try_recv().unwrap();
        match cmd {
            PromptCommand::ListWorkspaces(params) => assert_eq!(params.session_id, "sess-1"),
            _ => panic!("Expected ListWorkspaces command"),
        }
    }

    #[test]
    fn test_move_workspace_sends_command() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let handle = AcpPromptHandle { cmd_tx: tx, fail_signal: None };
        handle.move_workspace(&SessionId::new("sess-1"), WorkspaceMoveTarget::New { name: "ws".into() }).unwrap();

        let cmd = rx.try_recv().unwrap();
        match cmd {
            PromptCommand::MoveWorkspace(params) => {
                assert_eq!(params.session_id, "sess-1");
                assert_eq!(params.target, WorkspaceMoveTarget::New { name: "ws".into() });
            }
            _ => panic!("Expected MoveWorkspace command"),
        }
    }

    #[test]
    fn test_new_session_sends_command() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let handle = AcpPromptHandle { cmd_tx: tx, fail_signal: None };
        let cwd = std::path::Path::new("/tmp/project");

        handle.new_session(cwd).unwrap();

        let cmd = rx.try_recv().unwrap();
        match cmd {
            PromptCommand::NewSession { cwd } => {
                assert_eq!(cwd, std::path::PathBuf::from("/tmp/project"));
            }
            _ => panic!("Expected NewSession command"),
        }
    }
}
