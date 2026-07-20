use crate::command::{AgentCommand, CommandResult};
use acp_utils::client::{AcpClientError, AcpPromptHandle};
use agent_client_protocol::schema::v1::SessionId;

pub(super) fn execute(handle: &AcpPromptHandle, command: AgentCommand) -> Option<CommandResult> {
    let failure = command.failure();
    let query = match &command {
        AgentCommand::SearchPrompts(params) => Some(params.query.clone()),
        _ => None,
    };
    send(handle, command).err().map(|error| match query {
        Some(query) => CommandResult::PromptSearchFailed { query, error: error.to_string() },
        None => CommandResult::Failed { command: failure, error: error.to_string() },
    })
}

fn send(handle: &AcpPromptHandle, command: AgentCommand) -> Result<(), AcpClientError> {
    match command {
        AgentCommand::Prompt { session_id, text, content } => handle.prompt(&session_id, &text, content),
        AgentCommand::Cancel { session_id } => handle.cancel(&session_id),
        AgentCommand::SetConfigOption { session_id, config_id, value } => {
            handle.set_config_option(&session_id, &config_id, &value)
        }
        AgentCommand::AuthenticateMcpServer { session_id, server_name } => {
            handle.authenticate_mcp_server(&session_id, &server_name)
        }
        AgentCommand::Authenticate { method_id } => handle.authenticate(&method_id),
        AgentCommand::ListSessions => handle.list_sessions(),
        AgentCommand::LoadSession { session_id, cwd } => handle.load_session(&session_id, &cwd),
        AgentCommand::NewSession { cwd } => handle.new_session(&cwd),
        AgentCommand::SearchPrompts(params) => handle.search_prompts(params),
        AgentCommand::SessionPreview { session_id } => handle.session_preview(&SessionId::new(session_id)),
        AgentCommand::ListWorkspaces { session_id } => handle.list_workspaces(&SessionId::new(session_id)),
        AgentCommand::MoveWorkspace { session_id, target } => {
            handle.move_workspace(&SessionId::new(session_id), target)
        }
    }
}
