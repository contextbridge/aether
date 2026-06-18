use llm::LlmModel;
use std::collections::HashMap;
use std::env::vars;

pub const CONTAINER_AETHER_HOME: &str = "/root/.aether";

pub fn default_eval_env_vars() -> HashMap<String, String> {
    default_eval_env_vars_from(vars())
}

fn default_eval_env_vars_from(vars: impl IntoIterator<Item = (String, String)>) -> HashMap<String, String> {
    let mut env_vars: HashMap<String, String> = vars
        .into_iter()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_eval_env_vars_sets_container_aether_home() {
        let env_vars = default_eval_env_vars_from([]);

        assert_eq!(env_vars.get("AETHER_HOME"), Some(&CONTAINER_AETHER_HOME.to_string()));
    }

    #[test]
    fn default_eval_env_vars_does_not_inherit_host_aether_home() {
        let env_vars = default_eval_env_vars_from([("AETHER_HOME".to_string(), "/host/aether".to_string())]);

        assert_eq!(env_vars.get("AETHER_HOME"), Some(&CONTAINER_AETHER_HOME.to_string()));
    }
}
