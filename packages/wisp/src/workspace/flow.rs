pub use super::manager::WorkspaceManager;
use super::manager::{WorkspaceDestination, WorkspaceError, WorkspaceFork};
use super::status::WorkspaceStatus;
use crate::components::workspace_picker::WorkspacePicker;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::oneshot;

pub struct WorkspaceRuntime {
    pub cwd: PathBuf,
    pub status: WorkspaceStatus,
    pub manager: Arc<WorkspaceManager>,
}

impl WorkspaceRuntime {
    pub async fn resolve(cwd: PathBuf) -> Self {
        let manager: Arc<WorkspaceManager> = Arc::new(WorkspaceManager::new());
        let status = manager.workspace_status(&cwd).await;
        Self { cwd, status, manager }
    }

    pub fn new(cwd: PathBuf, status: WorkspaceStatus, manager: Arc<WorkspaceManager>) -> Self {
        Self { cwd, status, manager }
    }
}

pub struct WorkspaceFlow {
    cwd: PathBuf,
    manager: Arc<WorkspaceManager>,
    state: WorkspaceFlowState,
}

pub struct WorkspaceUpdate {
    pub cwd: PathBuf,
    pub status: WorkspaceStatus,
}

pub enum WorkspaceFlowEffect {
    OpenPicker(WorkspacePicker),
    UserMessage(String),
    ForkFailed(String),
    CloseModal,
    ClearScreen,
    ResetConversation,
    LoadSession { cwd: PathBuf },
}

impl WorkspaceFlow {
    pub fn new(runtime: WorkspaceRuntime) -> Self {
        Self { cwd: runtime.cwd, manager: runtime.manager, state: WorkspaceFlowState::Idle }
    }

    pub fn wants_tick(&self) -> bool {
        matches!(self.state, WorkspaceFlowState::RunningOperation { .. })
    }

    pub async fn open_picker(&mut self, prompt_in_flight: bool) -> Vec<WorkspaceFlowEffect> {
        if prompt_in_flight {
            return vec![WorkspaceFlowEffect::UserMessage("Cannot fork while a prompt is in flight.".to_string())];
        }

        match self.manager.fork_options(&self.cwd).await {
            Ok(options) => vec![WorkspaceFlowEffect::OpenPicker(WorkspacePicker::new(options, self.cwd.clone()))],
            Err(e) => vec![WorkspaceFlowEffect::UserMessage(e.to_string())],
        }
    }

    pub fn fork(&mut self, destination: WorkspaceDestination) -> Vec<WorkspaceFlowEffect> {
        let manager = Arc::clone(&self.manager);
        let src = self.cwd.clone();
        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            let result = manager.fork(&src, destination).await;
            let _ = tx.send(result);
        });
        self.state = WorkspaceFlowState::RunningOperation { rx };
        Vec::new()
    }

    pub async fn poll(&mut self) -> Vec<WorkspaceFlowEffect> {
        let WorkspaceFlowState::RunningOperation { .. } = self.state else {
            return Vec::new();
        };
        let WorkspaceFlowState::RunningOperation { mut rx } =
            std::mem::replace(&mut self.state, WorkspaceFlowState::Idle)
        else {
            return Vec::new();
        };

        let result = match rx.try_recv() {
            Err(oneshot::error::TryRecvError::Empty) => {
                self.state = WorkspaceFlowState::RunningOperation { rx };
                return Vec::new();
            }
            Ok(result) => result,
            Err(oneshot::error::TryRecvError::Closed) => Err(WorkspaceError::TaskAborted),
        };

        match result {
            Ok(fork) => self.load_forked_session(fork),
            Err(e) => vec![WorkspaceFlowEffect::ForkFailed(e.to_string())],
        }
    }

    fn load_forked_session(&mut self, fork: WorkspaceFork) -> Vec<WorkspaceFlowEffect> {
        let cwd = fork.cwd.clone();
        self.state = WorkspaceFlowState::WaitingForSessionLoad { cwd: fork.cwd, status: fork.status };
        vec![
            WorkspaceFlowEffect::CloseModal,
            WorkspaceFlowEffect::ClearScreen,
            WorkspaceFlowEffect::ResetConversation,
            WorkspaceFlowEffect::LoadSession { cwd },
        ]
    }

    pub fn complete_session_load(&mut self) -> Option<WorkspaceUpdate> {
        let WorkspaceFlowState::WaitingForSessionLoad { cwd, status } =
            std::mem::replace(&mut self.state, WorkspaceFlowState::Idle)
        else {
            return None;
        };
        self.cwd = cwd.clone();
        Some(WorkspaceUpdate { cwd, status })
    }

    pub fn cancel_session_load(&mut self) {
        if matches!(self.state, WorkspaceFlowState::WaitingForSessionLoad { .. }) {
            self.state = WorkspaceFlowState::Idle;
        }
    }

    #[cfg(test)]
    pub(crate) fn set_pending_operation_for_test(
        &mut self,
        rx: oneshot::Receiver<Result<WorkspaceFork, WorkspaceError>>,
    ) {
        self.state = WorkspaceFlowState::RunningOperation { rx };
    }
}

enum WorkspaceFlowState {
    Idle,
    RunningOperation { rx: oneshot::Receiver<Result<WorkspaceFork, WorkspaceError>> },
    WaitingForSessionLoad { cwd: PathBuf, status: WorkspaceStatus },
}
