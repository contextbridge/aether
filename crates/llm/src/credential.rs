use serde::{Deserialize, Serialize};

/// Credential for an LLM provider (e.g., Anthropic, `OpenRouter`)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ProviderCredential {
    ApiKey { key: String },
}

impl ProviderCredential {
    pub fn api_key(key: &str) -> Self {
        Self::ApiKey { key: key.to_string() }
    }
}
