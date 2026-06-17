use acp_utils::client::{AcpEvent, TokioAcpAgent, spawn_acp_session};
use aether_core::events::AcpAgentMessageMapper;
use aether_evals::AETHER_EVAL_WRAPPED_TASK_PROMPT_ENV;
use agent_client_protocol::schema::{Implementation, InitializeRequest, NewSessionRequest, ProtocolVersion};
use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Parser)]
struct Args {
    #[arg(long)]
    cwd: PathBuf,
    #[arg(long, default_value_t = AETHER_EVAL_WRAPPED_TASK_PROMPT_ENV.to_string())]
    prompt_env: String,
    #[arg(long)]
    model_name: Option<String>,
    /// The ACP agent argv, provided after `--` (e.g. `-- aether acp --agent Fast`). Passed to the
    /// agent process verbatim, with no shell parsing.
    #[arg(last = true, required = true)]
    agent_command: Vec<String>,
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), ClientError> {
    let args = Args::parse();
    let prompt = std::env::var(&args.prompt_env).map_err(|_| ClientError::MissingPromptEnv(args.prompt_env.clone()))?;
    let (command, agent_args) = args.agent_command.split_first().ok_or(ClientError::EmptyAgentCommand)?;
    let init_request = InitializeRequest::new(ProtocolVersion::LATEST)
        .client_info(Implementation::new("aether-evals-acp-client", env!("CARGO_PKG_VERSION")));
    let new_session_request = NewSessionRequest::new(args.cwd);
    let agent = TokioAcpAgent::from_command(command.clone(), agent_args.to_vec());
    let mut session = spawn_acp_session(agent, init_request, new_session_request).await?;
    let model_name = args.model_name.unwrap_or_else(|| session.agent_name.clone());
    let mut mapper = AcpAgentMessageMapper::new(model_name);

    session.prompt_handle.prompt(&session.session_id, &prompt, None)?;

    while let Some(event) = session.event_rx.recv().await {
        match event {
            AcpEvent::SessionUpdate { update, .. } => print_messages(mapper.map_update(*update))?,
            AcpEvent::PromptDone(stop_reason) => {
                print_messages(mapper.finish(stop_reason))?;
                return Ok(());
            }
            AcpEvent::PromptError(error) => {
                let mut messages = mapper.flush_buffered();
                messages.push(aether_core::events::AgentMessage::Error { message: error.to_string() });
                print_messages(messages)?;
                return Ok(());
            }
            AcpEvent::ContextCleared(_) => print_message(&aether_core::events::AgentMessage::ContextCleared)?,
            AcpEvent::ConnectionClosed => return Err(ClientError::ConnectionClosed),
            AcpEvent::ContextUsage(_)
            | AcpEvent::SubAgentProgress(_)
            | AcpEvent::AuthMethodsUpdated(_)
            | AcpEvent::McpNotification(_)
            | AcpEvent::ElicitationRequest { .. }
            | AcpEvent::AuthenticateComplete { .. }
            | AcpEvent::AuthenticateFailed { .. }
            | AcpEvent::SessionsListed { .. }
            | AcpEvent::SessionLoaded { .. }
            | AcpEvent::NewSessionCreated { .. }
            | AcpEvent::PromptSearchResults(_)
            | AcpEvent::PromptSearchFailed { .. }
            | AcpEvent::SessionPreviewLoaded(_)
            | AcpEvent::SessionPreviewFailed { .. }
            | AcpEvent::WorkspacesListed(_)
            | AcpEvent::WorkspaceListFailed { .. }
            | AcpEvent::WorkspaceMoved(_)
            | AcpEvent::WorkspaceMoveFailed { .. } => {}
        }
    }

    Err(ClientError::ConnectionClosed)
}

fn print_messages(messages: Vec<aether_core::events::AgentMessage>) -> Result<(), ClientError> {
    for message in messages {
        print_message(&message)?;
    }
    Ok(())
}

fn print_message(message: &aether_core::events::AgentMessage) -> Result<(), ClientError> {
    println!("{}", serde_json::to_string(message)?);
    Ok(())
}

#[derive(Debug, thiserror::Error)]
enum ClientError {
    #[error("prompt environment variable `{0}` is not set")]
    MissingPromptEnv(String),
    #[error("no agent command provided after `--`")]
    EmptyAgentCommand,
    #[error("ACP connection closed before prompt completed")]
    ConnectionClosed,
    #[error(transparent)]
    Acp(#[from] acp_utils::client::AcpClientError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
