use aether_core::events::AgentMessage;
use std::env::var;

use aether_evals::{DockerAetherAgent, DockerImage, Workspace, run_eval};
use aether_project::{AetherSettings, AgentConfig as AetherAgentConfig, PromptSource};

#[tokio::test]
async fn docker_agent_aether_acp_eval() -> Result<(), aether_evals::EvalRunError> {
    if var("AETHER_RUN_DOCKER_TESTS").ok().as_deref() != Some("1") {
        eprintln!("skipping Docker eval test; set AETHER_RUN_DOCKER_TESTS=1 to run");
        return Ok(());
    }

    let image = DockerImage::new("aether-sandbox", "latest");
    let agent_name = std::env::var("AETHER_EVAL_AGENT").unwrap_or_else(|_| "eval-smoke".to_string());
    let settings = AetherSettings {
        agent: Some(agent_name.clone()),
        prompts: vec![PromptSource::Text { text: "You are in an eval. Keep responses concise.".to_string() }],
        agents: vec![AetherAgentConfig {
            name: agent_name.clone(),
            description: "Eval smoke agent".to_string(),
            model: std::env::var("AETHER_EVAL_MODEL").unwrap_or_else(|_| "anthropic:claude-sonnet-4-5".to_string()),
            user_invocable: true,
            ..AetherAgentConfig::default()
        }],
        ..AetherSettings::default()
    };
    let agent = DockerAetherAgent::new(image).with_settings(settings).with_agent(agent_name);

    let report = run_eval(&agent, "Reply with one short sentence.", Workspace::empty()?).await?;
    assert!(report.messages().iter().any(|message| matches!(message, AgentMessage::Done)));
    Ok(())
}
