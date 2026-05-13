#![doc = include_str!(concat!(env!("OUT_DIR"), "/docs/ollama.md"))]

use super::util::get_local_config;
use crate::providers::openai::OpenAiChatProvider;
use crate::{ProviderConnectionConfig, ProviderFactory, Result};
use async_openai::{Client, config::OpenAIConfig};

pub struct OllamaProvider {
    model: String,
    client: Client<OpenAIConfig>,
}

impl OllamaProvider {
    pub fn new(model: &str, base_url: &str) -> Self {
        Self { model: model.to_string(), client: Client::with_config(get_local_config(base_url)) }
    }

    pub fn default(model: &str) -> Self {
        Self { model: model.to_string(), client: Client::with_config(get_local_config("http://localhost:11434/v1")) }
    }
}

impl ProviderFactory for OllamaProvider {
    async fn from_env() -> Result<Self> {
        Self::from_env_with_connection(ProviderConnectionConfig::default()).await
    }

    async fn from_env_with_connection(connection: ProviderConnectionConfig) -> Result<Self> {
        let base_url = connection.base_url.as_deref().unwrap_or("http://localhost:11434/v1");
        Ok(Self { model: String::new(), client: Client::with_config(get_local_config(base_url)) })
    }

    fn with_model(mut self, model: &str) -> Self {
        self.model = model.to_string();
        self
    }
}

impl OpenAiChatProvider for OllamaProvider {
    type Config = OpenAIConfig;

    fn client(&self) -> &Client<Self::Config> {
        &self.client
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn provider_name(&self) -> &'static str {
        "Ollama"
    }
}
