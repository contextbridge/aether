use crate::command::{AgentCommand, CommandResult};
use crate::runtime::tasks::TaskSupervisor;
use acp_utils::client::{AcpClientError, AcpClientHandle};
use acp_utils::notifications::{McpRequest, SessionPreviewParams, WorkspaceListParams, WorkspaceMoveParams};
use agent_client_protocol::JsonRpcRequest;
use agent_client_protocol::schema::v2::{
    CancelSessionNotification, ContentBlock, ListSessionsRequest, LoginAuthRequest, NewSessionRequest, PromptRequest,
    ResumeSessionRequest, SetSessionConfigOptionRequest,
};

pub(super) fn execute(
    handle: &AcpClientHandle,
    command: AgentCommand,
    tasks: &mut TaskSupervisor,
) -> Option<CommandResult> {
    let failure = command.failure();
    match command {
        AgentCommand::Prompt { session_id, text, content } => {
            let mut prompt = vec![ContentBlock::from(text)];
            if let Some(content) = content {
                prompt.extend(content);
            }
            let response = handle.prompt(PromptRequest::new(session_id, prompt));
            tasks.spawn_network(async move {
                response.await.map_or_else(|error| failed(failure, &error), |_| CommandResult::PromptAccepted)
            });
        }
        AgentCommand::Cancel { session_id } => return Some(
            handle.cancel(CancelSessionNotification::new(session_id))
                .map_or_else(|error| failed(failure, &error), |()| CommandResult::AgentCommandAccepted),
        ),
        AgentCommand::SetConfigOption { conversation_id, session_id, config_id, value } => spawn_request(
            handle, tasks, SetSessionConfigOptionRequest::new(session_id, config_id, value),
            move |response| CommandResult::ConfigOptionsUpdated { conversation_id, options: response.config_options },
            move |error| CommandResult::ConfigOptionUpdateFailed { conversation_id, error: error.to_string() },
        ),
        AgentCommand::AuthenticateMcpServer { session_id, server_name } => {
            let request = McpRequest::Authenticate { session_id: session_id.0.to_string(), server_name };
            return Some(handle.notify(request)
                .map_or_else(|error| failed(failure, &error), |()| CommandResult::AgentCommandAccepted));
        }
        AgentCommand::Authenticate { method_id } => {
            let failed_method_id = method_id.clone();
            spawn_request(handle, tasks, LoginAuthRequest::new(method_id.clone()),
                move |_| CommandResult::AuthenticationCompleted { method_id },
                move |_| CommandResult::AuthenticationFailed { method_id: failed_method_id });
        }
        AgentCommand::ListSessions => spawn_request(handle, tasks, ListSessionsRequest::new(),
            CommandResult::SessionsListed, move |error| failed(failure, &error)),
        AgentCommand::ResumeSession { session_id, cwd } => {
            let handle = handle.clone();
            tasks.spawn_network(async move {
                handle.resume_session_with_replay(ResumeSessionRequest::new(session_id, cwd)).await
                    .map_or_else(|error| failed(failure, &error), |_| CommandResult::AgentCommandAccepted)
            });
        }
        AgentCommand::NewSession { cwd } => {
            let handle = handle.clone();
            tasks.spawn_network(async move {
                handle.new_session(NewSessionRequest::new(cwd)).await
                    .map_or_else(|error| failed(failure, &error), CommandResult::NewSessionCreated)
            });
        }
        AgentCommand::SearchPrompts(params) => {
            let query = params.query.clone();
            spawn_request(handle, tasks, params, CommandResult::PromptSearchResults,
                move |error| CommandResult::PromptSearchFailed { query, error: error.to_string() });
        }
        AgentCommand::SessionPreview { session_id } => {
            let failed_id = session_id.clone();
            spawn_request(handle, tasks, SessionPreviewParams { session_id }, CommandResult::SessionPreviewLoaded,
                move |error| CommandResult::SessionPreviewFailed { session_id: failed_id, error: error.to_string() });
        }
        AgentCommand::ListWorkspaces { session_id } => spawn_request(
            handle, tasks, WorkspaceListParams { session_id }, CommandResult::WorkspacesListed,
            |error| CommandResult::WorkspaceListFailed { error: error.to_string() },
        ),
        AgentCommand::MoveWorkspace { session_id, target } => spawn_request(
            handle, tasks, WorkspaceMoveParams { session_id, target }, CommandResult::WorkspaceMoved,
            |error| CommandResult::WorkspaceMoveFailed { error: error.to_string() },
        ),
    }
    None
}

fn spawn_request<R: JsonRpcRequest + Send + 'static>(
    handle: &AcpClientHandle,
    tasks: &mut TaskSupervisor,
    request: R,
    on_ok: impl FnOnce(R::Response) -> CommandResult + Send + 'static,
    on_err: impl FnOnce(AcpClientError) -> CommandResult + Send + 'static,
) {
    let handle = handle.clone();
    tasks.spawn_network(async move { handle.request(request).await.map_or_else(on_err, on_ok) });
}

fn failed(failure: crate::command::FailedCommand, error: &AcpClientError) -> CommandResult {
    CommandResult::Failed { command: failure, error: error.to_string() }
}
