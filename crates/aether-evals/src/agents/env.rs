use llm::is_provider_env_var;
use std::collections::HashMap;
use std::env::vars;

pub const CONTAINER_AETHER_HOME: &str = "/root/.aether";

pub fn default_eval_env_vars() -> HashMap<String, String> {
    default_eval_env_vars_from(vars())
}

fn default_eval_env_vars_from(vars: impl IntoIterator<Item = (String, String)>) -> HashMap<String, String> {
    let mut env_vars: HashMap<String, String> = vars
        .into_iter()
        .filter(|(key, _)| key != "AETHER_HOME" && (is_provider_env_var(key) || key.starts_with("AETHER_")))
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

    #[test]
    fn default_eval_env_vars_forwards_bedrock_credentials() {
        let bedrock = [
            ("AWS_ACCESS_KEY_ID".to_string(), "id".to_string()),
            ("AWS_SECRET_ACCESS_KEY".to_string(), "secret".to_string()),
            ("AWS_SESSION_TOKEN".to_string(), "token".to_string()),
            ("AWS_REGION".to_string(), "us-east-1".to_string()),
        ];

        let env_vars = default_eval_env_vars_from(bedrock.clone());

        for (key, value) in bedrock {
            assert_eq!(env_vars.get(&key), Some(&value), "{key} should be forwarded");
        }
    }
}
