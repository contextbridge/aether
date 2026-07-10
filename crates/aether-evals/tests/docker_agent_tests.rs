use aether_core::events::{AgentEvent, TurnEvent};

use aether_evals::{
    Agent, Container, ContainerError, DockerAgent, Image, Task, Transcript, TranscriptError, Workspace, WorkspaceError,
};

#[derive(Debug, thiserror::Error)]
enum DockerAgentTestError {
    #[error(transparent)]
    Workspace(#[from] WorkspaceError),
    #[error(transparent)]
    Container(#[from] ContainerError),
    #[error(transparent)]
    Transcript(#[from] TranscriptError),
}

#[tokio::test]
async fn docker_agent_direct_agent_event_eval() -> Result<(), DockerAgentTestError> {
    let workspace = Workspace::empty()?;
    let container = Container::builder(Image::parse("alpine:3").unwrap()).start(&workspace).await?;
    let agent = DockerAgent::new(
        container,
        vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            r#"printf '%s\n' '{"category":"message","event":{"type":"text","message_id":"m1","chunk":"ok","is_complete":true,"model_name":"test"}}' '{"category":"turn","event":{"type":"ended","outcome":{"status":"completed"}}}'"#
                .to_string(),
        ],
    );

    let trace = Transcript::from_stream(agent.run(Task::new("Reply with one short sentence."))).await?;
    assert!(trace.messages().iter().any(|message| matches!(message, AgentEvent::Turn(TurnEvent::Ended { .. }))));
    Ok(())
}

#[tokio::test]
async fn container_exec_shell_returns_non_zero_exit_codes() -> Result<(), DockerAgentTestError> {
    let workspace = Workspace::empty()?;
    let container = Container::builder(Image::parse("alpine:3").unwrap()).start(&workspace).await?;
    let output = container.exec_shell("echo nope >&2; exit 7").await?;

    assert_eq!(output.exit_code, 7);
    assert!(output.stderr.contains("nope"));
    Ok(())
}
