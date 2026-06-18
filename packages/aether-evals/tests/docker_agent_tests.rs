use aether_core::events::AgentMessage;
use std::env::var;

use aether_evals::{DockerAgent, DockerImage, Task, Workspace};

#[tokio::test]
async fn docker_agent_direct_agent_message_eval() -> Result<(), aether_evals::EvalRunError> {
    if var("AETHER_RUN_DOCKER_TESTS").ok().as_deref() != Some("1") {
        eprintln!("skipping Docker eval test; set AETHER_RUN_DOCKER_TESTS=1 to run");
        return Ok(());
    }

    let agent = DockerAgent::new(
        DockerImage::new("aether-sandbox", "latest"),
        vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            r#"printf '%s\n' '{"type":"text","message_id":"m1","chunk":"ok","is_complete":true,"model_name":"test"}' '{"type":"done"}'"#
                .to_string(),
        ],
    );

    let run = Task::new("Reply with one short sentence.", Workspace::empty()?).run(&agent).await?;
    assert!(run.transcript().messages().iter().any(|message| matches!(message, AgentMessage::Done)));
    Ok(())
}
