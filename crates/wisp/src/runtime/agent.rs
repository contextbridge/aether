use crate::command::{AgentCommand, CommandResult};
use crate::runtime::tasks::TaskSupervisor;
use acp_utils::client::{AcpClientError, AcpClientHandle};
use acp_utils::notifications::{McpRequest, SessionPreviewParams, WorkspaceListParams, WorkspaceMoveParams};
use agent_client_protocol::schema::v1::{
    AuthenticateRequest, CancelNotification, ContentBlock, ListSessionsRequest, LoadSessionRequest, NewSessionRequest,
    PromptRequest, SetSessionConfigOptionRequest, TextContent,
};

#[allow(clippy::too_many_lines)]
pub(super) fn execute(
    handle: &AcpClientHandle,
    command: AgentCommand,
    tasks: &mut TaskSupervisor,
) -> Option<CommandResult> {
    let failure = command.failure();
    match command {
        AgentCommand::Prompt { session_id, text, content } => {
            let handle = handle.clone();
            tasks.spawn_mutation(async move {
                let mut prompt = vec![ContentBlock::Text(TextContent::new(text))];
                if let Some(content) = content {
                    prompt.extend(content);
                }
                handle
                    .prompt(PromptRequest::new(session_id, prompt))
                    .await
                    .map_or_else(|error| failed(failure, &error), |_| CommandResult::AgentCommandAccepted)
            });
            None
        }
        AgentCommand::Cancel { session_id } => {
            let handle = handle.clone();
            tasks.spawn_mutation(async move {
                handle
                    .cancel(CancelNotification::new(session_id))
                    .await
                    .map_or_else(|error| failed(failure, &error), |()| CommandResult::AgentCommandAccepted)
            });
            None
        }
        AgentCommand::SetConfigOption { session_id, config_id, value } => {
            let handle = handle.clone();
            tasks.spawn_mutation(async move {
                handle
                    .set_config_option(SetSessionConfigOptionRequest::new(session_id, config_id, value.as_str()))
                    .await
                    .map_or_else(|error| CommandResult::ConfigOptionUpdateFailed { error: error.to_string() }, |response| {
                        CommandResult::ConfigOptionsUpdated(response.config_options)
                    })
            });
            None
        }
        AgentCommand::AuthenticateMcpServer { session_id, server_name } => {
            let handle = handle.clone();
            tasks.spawn_mutation(async move {
                let request = McpRequest::Authenticate { session_id: session_id.0.to_string(), server_name };
                handle
                    .authenticate_mcp_server(request)
                    .await
                    .map_or_else(|error| failed(failure, &error), |()| CommandResult::AgentCommandAccepted)
            });
            None
        }
        AgentCommand::Authenticate { method_id } => {
            let handle = handle.clone();
            tasks.spawn_mutation(async move {
                let failed_method_id = method_id.clone();
                handle
                    .authenticate(AuthenticateRequest::new(method_id.clone()))
                    .await
                    .map_or_else(
                        |_| CommandResult::AuthenticationFailed { method_id: failed_method_id },
                        |_| CommandResult::AuthenticationCompleted { method_id },
                    )
            });
            None
        }
        AgentCommand::ListSessions => {
            let handle = handle.clone();
            tasks.spawn_mutation(async move {
                handle
                    .list_sessions(ListSessionsRequest::new())
                    .await
                    .map_or_else(|error| failed(failure, &error), CommandResult::SessionsListed)
            });
            None
        }
        AgentCommand::LoadSession { session_id, cwd } => {
            let handle = handle.clone();
            tasks.spawn_mutation(async move {
                handle
                    .load_session(LoadSessionRequest::new(session_id, cwd))
                    .await
                    .map_or_else(|error| failed(failure, &error), CommandResult::SessionLoaded)
            });
            None
        }
        AgentCommand::NewSession { cwd } => {
            let handle = handle.clone();
            tasks.spawn_mutation(async move {
                handle
                    .new_session(NewSessionRequest::new(cwd))
                    .await
                    .map_or_else(|error| failed(failure, &error), CommandResult::NewSessionCreated)
            });
            None
        }
        AgentCommand::SearchPrompts(params) => {
            let query = params.query.clone();
            let handle = handle.clone();
            tasks.spawn_mutation(async move {
                match handle.search_prompts(params).await {
                    Ok(response) => CommandResult::PromptSearchResults(response),
                    Err(error) => CommandResult::PromptSearchFailed { query, error: error.to_string() },
                }
            });
            None
        }
        AgentCommand::SessionPreview { session_id } => {
            let failed_id = session_id.clone();
            let handle = handle.clone();
            tasks.spawn_mutation(async move {
                handle
                    .preview_session(SessionPreviewParams { session_id })
                    .await
                    .map_or_else(
                        |error| CommandResult::SessionPreviewFailed { session_id: failed_id, error: error.to_string() },
                        CommandResult::SessionPreviewLoaded,
                    )
            });
            None
        }
        AgentCommand::ListWorkspaces { session_id } => {
            let handle = handle.clone();
            tasks.spawn_mutation(async move {
                handle
                    .list_workspaces(WorkspaceListParams { session_id })
                    .await
                    .map_or_else(
                        |error| CommandResult::WorkspaceListFailed { error: error.to_string() },
                        CommandResult::WorkspacesListed,
                    )
            });
            None
        }
        AgentCommand::MoveWorkspace { session_id, target } => {
            let handle = handle.clone();
            tasks.spawn_mutation(async move {
                handle
                    .move_workspace(WorkspaceMoveParams { session_id, target })
                    .await
                    .map_or_else(
                        |error| CommandResult::WorkspaceMoveFailed { error: error.to_string() },
                        CommandResult::WorkspaceMoved,
                    )
            });
            None
        }
    }
}


fn failed(failure: crate::command::FailedCommand, error: &AcpClientError) -> CommandResult {
    CommandResult::Failed { command: failure, error: error.to_string() }
}
