use crate::command::{AgentCommand, CommandResult, PromptRejection, Reply};
use crate::connection::resume_request;
use crate::session_model::initial_workspace_status;
use acp_utils::client::{AcpClientError, AcpClientHandle};
use acp_utils::notifications::{
    McpRequest, SessionPreviewParams, WorkspaceListParams, WorkspaceMoveParams, WorkspaceStatusPayload,
};
use agent_client_protocol::schema::v2::{
    CancelSessionNotification, ListSessionsRequest, LoginAuthRequest, NewSessionRequest, PromptRequest,
    SetSessionConfigOptionRequest,
};
use agent_client_protocol::{ErrorCode, JsonRpcRequest};
use tokio::task::JoinSet;

pub(super) fn execute(
    handle: &AcpClientHandle,
    command: AgentCommand,
    tasks: &mut JoinSet<CommandResult>,
) -> Option<CommandResult> {
    match command {
        AgentCommand::Prompt { request_id, session_id, content } => {
            submit_request(handle, tasks, PromptRequest::new(session_id, content), move |result| {
                CommandResult::Prompt { request_id, result: result.map(drop).map_err(prompt_rejection) }
            });
        }
        AgentCommand::Cancel { session_id } => {
            return Some(CommandResult::Notified(handle.notify(CancelSessionNotification::new(session_id))));
        }
        AgentCommand::SetConfigOption { conversation_id, session_id, config_id, value } => submit_request(
            handle,
            tasks,
            SetSessionConfigOptionRequest::new(session_id, config_id, value),
            move |result| CommandResult::ConfigOptionsUpdated { conversation_id, result },
        ),
        AgentCommand::AuthenticateMcpServer { session_id, server_name } => {
            let request = McpRequest::Authenticate { session_id: session_id.to_string(), server_name };
            return Some(CommandResult::Notified(handle.notify(request)));
        }
        AgentCommand::Authenticate { method_id } => {
            submit_request(handle, tasks, LoginAuthRequest::new(method_id.clone()), move |result| {
                CommandResult::Reply(Reply::AuthenticationCompleted { method_id, result })
            });
        }
        AgentCommand::ListSessions => {
            submit_request(handle, tasks, ListSessionsRequest::new(), CommandResult::SessionsListed);
        }
        AgentCommand::ResumeSession { session_id, cwd } => {
            submit_request(handle, tasks, resume_request(session_id.clone(), cwd), move |result| {
                CommandResult::ResumeSession { session_id, result }
            });
        }
        AgentCommand::NewSession { cwd } => {
            submit_request(handle, tasks, NewSessionRequest::new(cwd), CommandResult::NewSession);
        }
        AgentCommand::SearchPrompts(params) => {
            let query = params.query.clone();
            submit_request(handle, tasks, params, move |result| CommandResult::PromptSearchResults { query, result });
        }
        AgentCommand::SessionPreview { session_id } => {
            let request = SessionPreviewParams { session_id: session_id.to_string() };
            submit_request(handle, tasks, request, move |result| {
                CommandResult::Reply(Reply::SessionPreviewLoaded { session_id, result })
            });
        }
        AgentCommand::ListWorkspaces { session_id } => {
            let request = WorkspaceListParams { session_id: session_id.to_string() };
            submit_request(handle, tasks, request, CommandResult::WorkspacesListed);
        }
        AgentCommand::MoveWorkspace { session_id, target } => {
            let request = WorkspaceMoveParams { session_id: session_id.to_string(), target };
            submit_request(handle, tasks, request, CommandResult::WorkspaceMoved);
        }
        AgentCommand::FetchWorkspaceStatus { session_id, cwd } => {
            let request = WorkspaceStatusPayload { session_id: session_id.to_string() };
            submit_request(handle, tasks, request, move |result| {
                let status = result.unwrap_or_else(|_| initial_workspace_status(&cwd));
                CommandResult::WorkspaceResolved { cwd, status }
            });
        }
    }
    None
}

fn prompt_rejection(error: AcpClientError) -> PromptRejection {
    match error {
        AcpClientError::Protocol(error) if error.code == ErrorCode::RequestCancelled => PromptRejection::Cancelled,
        error => PromptRejection::Failed(error.to_string()),
    }
}

fn submit_request<T: JsonRpcRequest + Send + 'static>(
    handle: &AcpClientHandle,
    tasks: &mut JoinSet<CommandResult>,
    request: T,
    complete: impl FnOnce(Result<T::Response, AcpClientError>) -> CommandResult + Send + 'static,
) {
    let response = handle.request(request);
    tasks.spawn(async move { complete(response.await) });
}
