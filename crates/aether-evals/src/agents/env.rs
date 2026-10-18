use llm::catalog::ModelSpec;
use std::collections::{HashMap, HashSet};
use std::env::vars;

pub const CONTAINER_AETHER_HOME: &str = "/root/.aether";

pub fn eval_env_vars(model: &ModelSpec) -> HashMap<String, String> {
    eval_env_vars_from(model, vars())
}

fn eval_env_vars_from(model: &ModelSpec, vars: impl IntoIterator<Item = (String, String)>) -> HashMap<String, String> {
    let wanted: HashSet<&str> = model.models().iter().flat_map(|model| model.provider_enum().env_vars()).collect();
    let mut env_vars: HashMap<String, String> =
        vars.into_iter().filter(|(key, _)| wanted.contains(key.as_str())).collect();
    env_vars.insert("AETHER_HOME".to_string(), CONTAINER_AETHER_HOME.to_string());
    env_vars
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host_env() -> Vec<(String, String)> {
        [
            ("ANTHROPIC_API_KEY", "anthropic-key"),
            ("OPENAI_API_KEY", "openai-key"),
            ("OLLAMA_HOST", "http://gpu-box:11434"),
            ("AETHER_CREDENTIALS_PASSWORD", "hunter2"),
            ("AETHER_HOME", "/host/aether"),
            ("AWS_ACCESS_KEY_ID", "id"),
            ("AWS_SECRET_ACCESS_KEY", "secret"),
            ("AWS_SESSION_TOKEN", "token"),
            ("AWS_REGION", "us-east-1"),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
    }

    fn forwarded(model: &str) -> Vec<String> {
        let mut keys: Vec<String> = eval_env_vars_from(&model.parse().unwrap(), host_env()).into_keys().collect();
        keys.sort();
        keys
    }

    #[test]
    fn forwards_only_the_eval_models_provider_credentials() {
        assert_eq!(forwarded("anthropic:claude-opus-4-6"), ["AETHER_HOME", "ANTHROPIC_API_KEY"]);
        assert_eq!(forwarded("ollama:llama3"), ["AETHER_HOME", "OLLAMA_HOST"]);
    }

    #[test]
    fn forwards_every_alloy_members_credentials() {
        assert_eq!(
            forwarded("anthropic:claude-opus-4-6,openai:gpt-5"),
            ["AETHER_HOME", "ANTHROPIC_API_KEY", "OPENAI_API_KEY"]
        );
    }

    #[test]
    fn forwards_bedrock_credentials() {
        let keys = forwarded("bedrock:anthropic.claude-opus-4-6-v1");

        for key in ["AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY", "AWS_SESSION_TOKEN", "AWS_REGION"] {
            assert!(keys.iter().any(|forwarded| forwarded == key), "{key} should be forwarded: {keys:?}");
        }
        assert!(!keys.iter().any(|key| key == "ANTHROPIC_API_KEY"), "{keys:?}");
    }

    #[test]
    fn the_container_gets_its_own_aether_home() {
        let env_vars = eval_env_vars_from(&"ollama:llama3".parse().unwrap(), host_env());

        assert_eq!(env_vars.get("AETHER_HOME"), Some(&CONTAINER_AETHER_HOME.to_string()));
    }
}
