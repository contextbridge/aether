use crate::command::{AgentCommand, CommandResult};
use crate::runtime::tasks::TaskSupervisor;
use crate::session::workspace_status::WorkspaceStatus;
use acp_utils::client::AcpClientHandle;
use acp_utils::notifications::{
    McpRequest, SessionPreviewParams, WorkspaceListParams, WorkspaceMoveParams, WorkspaceStatusPayload,
};
use agent_client_protocol::JsonRpcRequest;
use agent_client_protocol::schema::v2::{
    CancelSessionNotification, ContentBlock, ListSessionsRequest, LoginAuthRequest, NewSessionRequest, PromptRequest,
    ReplayFrom, ReplayFromStart, ResumeSessionRequest, SetSessionConfigOptionRequest,
};

pub(super) fn execute(
    handle: &AcpClientHandle,
    command: AgentCommand,
    tasks: &mut TaskSupervisor,
) -> Option<CommandResult> {
    match command {
        AgentCommand::Prompt { session_id, text, content } => {
            let mut prompt = vec![ContentBlock::from(text)];
            prompt.extend(content.into_iter().flatten());
            submit_request(handle, tasks, PromptRequest::new(session_id, prompt), CommandResult::Prompt);
        }
        AgentCommand::Cancel { session_id } => {
            return Some(CommandResult::Cancel(
                handle.cancel(CancelSessionNotification::new(session_id)).map_err(|error| error.to_string()),
            ));
        }
        AgentCommand::SetConfigOption { conversation_id, session_id, config_id, value } => submit_request(
            handle,
            tasks,
            SetSessionConfigOptionRequest::new(session_id, config_id, value),
            move |result| CommandResult::ConfigOptionsUpdated { conversation_id, result },
        ),
        AgentCommand::AuthenticateMcpServer { session_id, server_name } => {
            let request = McpRequest::Authenticate { session_id: session_id.0.to_string(), server_name };
            return Some(CommandResult::AuthenticateMcp(handle.notify(request).map_err(|error| error.to_string())));
        }
        AgentCommand::Authenticate { method_id } => {
            submit_request(handle, tasks, LoginAuthRequest::new(method_id.clone()), move |result| {
                CommandResult::AuthenticationCompleted { method_id, result }
            });
        }
        AgentCommand::ListSessions => {
            submit_request(handle, tasks, ListSessionsRequest::new(), CommandResult::SessionsListed);
        }
        AgentCommand::ResumeSession { session_id, cwd } => submit_request(
            handle,
            tasks,
            ResumeSessionRequest::new(session_id.clone(), cwd).replay_from(ReplayFrom::Start(ReplayFromStart::new())),
            move |result| CommandResult::ResumeSession { session_id, result },
        ),
        AgentCommand::NewSession { cwd } => {
            submit_request(handle, tasks, NewSessionRequest::new(cwd), CommandResult::NewSession);
        }
        AgentCommand::SearchPrompts(params) => {
            let query = params.query.clone();
            submit_request(handle, tasks, params, move |result| CommandResult::PromptSearchResults { query, result });
        }
        AgentCommand::SessionPreview { session_id } => {
            submit_request(handle, tasks, SessionPreviewParams { session_id: session_id.clone() }, move |result| {
                CommandResult::SessionPreviewLoaded { session_id, result }
            });
        }
        AgentCommand::ListWorkspaces { session_id } => {
            submit_request(handle, tasks, WorkspaceListParams { session_id }, CommandResult::WorkspacesListed);
        }
        AgentCommand::MoveWorkspace { session_id, target } => {
            submit_request(handle, tasks, WorkspaceMoveParams { session_id, target }, CommandResult::WorkspaceMoved);
        }
        AgentCommand::FetchWorkspaceStatus { session_id, cwd } => {
            let request = WorkspaceStatusPayload { session_id };
            let response = handle.request(request);
            tasks.submit_network(async move {
                let status = response.await.map_or_else(|_| WorkspaceStatus::initial(&cwd), WorkspaceStatus::from);
                CommandResult::WorkspaceResolved { cwd, status }
            });
        }
    }
    None
}

fn submit_request<R: JsonRpcRequest + Send + 'static>(
    handle: &AcpClientHandle,
    tasks: &mut TaskSupervisor,
    request: R,
    complete: impl FnOnce(Result<R::Response, String>) -> CommandResult + Send + 'static,
) {
    let response = handle.request(request);
    tasks.submit_network(async move { complete(response.await.map_err(|error| error.to_string())) });
}
