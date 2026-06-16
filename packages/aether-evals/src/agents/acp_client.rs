use super::docker_agent::{AETHER_EVAL_WRAPPED_TASK_PROMPT_ENV, AgentCommandBuilder, DockerCommandConfig};
use aether_project::AetherSettings;
use llm::LlmModel;
use std::{collections::HashMap, env::vars};

pub const CONTAINER_AETHER_HOME: &str = "/root/.aether";

/// Builds the argv that drives an ACP stdio agent through `aether-evals-acp-client`. The agent
/// argv is passed verbatim after `--`, so no shell quoting is involved.
pub struct AcpClientCommand {
    client_command: Vec<String>,
    model_name: Option<String>,
    agent_argv: Vec<String>,
}

impl AcpClientCommand {
    /// Wrap an arbitrary ACP stdio agent argv (e.g. `["node", "/opt/agent/dist/eval-agent.js"]`).
    pub fn new(agent_argv: Vec<String>) -> Self {
        Self { client_command: default_client_command(), model_name: None, agent_argv }
    }

    /// Wrap the real Aether CLI (`aether acp`) with optional settings and named agent.
    pub fn aether(settings: Option<AetherSettings>, agent: Option<String>) -> Self {
        Self::new(aether_acp_agent_argv(settings, agent))
    }

    /// Override the client invocation. An empty command keeps the `aether-evals-acp-client` default.
    pub fn with_client_command(mut self, client_command: Option<Vec<String>>) -> Self {
        if let Some(command) = client_command.filter(|command| !command.is_empty()) {
            self.client_command = command;
        }
        self
    }

    pub fn with_model_name(mut self, model_name: Option<String>) -> Self {
        self.model_name = model_name;
        self
    }
}

impl AgentCommandBuilder for AcpClientCommand {
    fn build(&self, config: DockerCommandConfig<'_>) -> Vec<String> {
        let mut command = self.client_command.clone();
        command.extend([
            "--cwd".to_string(),
            config.container_cwd.display().to_string(),
            "--prompt-env".to_string(),
            AETHER_EVAL_WRAPPED_TASK_PROMPT_ENV.to_string(),
        ]);
        if let Some(model_name) = &self.model_name {
            command.push("--model-name".to_string());
            command.push(model_name.clone());
        }
        command.push("--".to_string());
        command.extend(self.agent_argv.iter().cloned());
        command
    }
}

pub fn aether_eval_env_vars() -> HashMap<String, String> {
    let mut env_vars: HashMap<String, String> = vars()
        .filter(|(key, _)| {
            key != "AETHER_HOME"
                && (LlmModel::ALL_REQUIRED_ENV_VARS.contains(&key.as_str())
                    || key == "OLLAMA_HOST"
                    || key.starts_with("AETHER_"))
        })
        .collect();
    env_vars.insert("AETHER_HOME".to_string(), CONTAINER_AETHER_HOME.to_string());
    env_vars
}

fn aether_acp_agent_argv(settings: Option<AetherSettings>, agent: Option<String>) -> Vec<String> {
    let mut argv = vec!["aether".to_string(), "acp".to_string()];
    if let Some(agent) = agent {
        argv.push("--agent".to_string());
        argv.push(agent);
    }
    if let Some(settings) = settings {
        argv.push("--settings-json".to_string());
        argv.push(serde_json::to_string(&settings).expect("AetherSettings should serialize to JSON"));
    }
    argv
}

fn default_client_command() -> Vec<String> {
    vec!["aether-evals-acp-client".to_string()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn build(command: &AcpClientCommand) -> Vec<String> {
        command.build(DockerCommandConfig { container_cwd: Path::new("/workspace") })
    }

    #[test]
    fn aether_eval_env_vars_sets_container_aether_home() {
        let env_vars = aether_eval_env_vars();

        assert_eq!(env_vars.get("AETHER_HOME").map(String::as_str), Some(CONTAINER_AETHER_HOME));
    }

    #[test]
    fn aether_wraps_acp_with_agent_and_settings() {
        let command = build(&AcpClientCommand::aether(Some(AetherSettings::default()), Some("Fast Agent".to_string())));

        assert_eq!(command[0], "aether-evals-acp-client");
        assert_eq!(command[1], "--cwd");
        assert_eq!(command[2], "/workspace");

        let separator = command.iter().position(|arg| arg == "--").expect("agent argv is passed after `--`");
        let agent_argv = &command[separator + 1..];
        assert_eq!(agent_argv[0], "aether");
        assert_eq!(agent_argv[1], "acp");
        assert_eq!(agent_argv[2], "--agent");
        assert_eq!(agent_argv[3], "Fast Agent");
        assert_eq!(agent_argv[4], "--settings-json");
        assert!(!command.iter().any(|arg| arg.contains("headless")));
    }

    #[test]
    fn aether_omits_agent_and_settings_flags_when_absent() {
        let command = AcpClientCommand::aether(None, None)
            .build(DockerCommandConfig { container_cwd: Path::new("/workspace/subdir") });

        let separator = command.iter().position(|arg| arg == "--").unwrap();
        assert_eq!(&command[separator + 1..], &["aether".to_string(), "acp".to_string()]);
        assert!(command.contains(&"/workspace/subdir".to_string()));
        assert!(!command.iter().any(|arg| arg == "--model-name"));
    }

    #[test]
    fn wraps_arbitrary_agent_with_client_override_and_model_name() {
        let command = build(
            &AcpClientCommand::new(vec!["node".to_string(), "/opt/agent/dist/eval-agent.js".to_string()])
                .with_client_command(Some(vec!["/bin/acp-client".to_string()]))
                .with_model_name(Some("typescript-acp-agent".to_string())),
        );

        assert_eq!(command[0], "/bin/acp-client");
        assert!(command.windows(2).any(|w| w == ["--model-name", "typescript-acp-agent"]));
        let separator = command.iter().position(|arg| arg == "--").expect("agent argv is passed after `--`");
        assert_eq!(&command[separator + 1..], &["node".to_string(), "/opt/agent/dist/eval-agent.js".to_string()]);
    }

    #[test]
    fn empty_client_command_override_keeps_default() {
        let command = build(&AcpClientCommand::new(vec!["agent".to_string()]).with_client_command(Some(Vec::new())));

        assert_eq!(command[0], "aether-evals-acp-client");
    }
}
