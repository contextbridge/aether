#![doc = include_str!(concat!(env!("OUT_DIR"), "/docs/llamacpp.md"))]

use super::util::get_local_config;
use crate::provider_connection::DEFAULT_STREAM_IDLE_TIMEOUT;
use crate::providers::http::{http_client, openai_client};
use crate::providers::openai::OpenAiChatProvider;
use crate::{ProviderConnectionConfig, ProviderFactory, Result};
use async_openai::{Client, config::OpenAIConfig};
use std::future::ready;
use std::time::Duration;

pub struct LlamaCppProvider {
    client: Client<OpenAIConfig>,
    idle_timeout: Duration,
}

impl LlamaCppProvider {
    pub fn new(base_url: &str) -> Self {
        Self {
            client: openai_client(get_local_config(base_url), http_client()),
            idle_timeout: DEFAULT_STREAM_IDLE_TIMEOUT,
        }
    }
}

impl Default for LlamaCppProvider {
    fn default() -> Self {
        Self::new("http://localhost:8080/v1")
    }
}

impl ProviderFactory for LlamaCppProvider {
    async fn from_env() -> Result<Self> {
        Self::from_env_with_connection(ProviderConnectionConfig::default()).await
    }

    fn from_env_with_connection(connection: ProviderConnectionConfig) -> impl Future<Output = Result<Self>> + Send {
        let base_url = connection.base_url.as_deref().unwrap_or("http://localhost:8080/v1");
        ready(Ok(Self { idle_timeout: connection.idle_timeout, ..Self::new(base_url) }))
    }

    fn with_model(self, _model: &str) -> Self {
        // LlamaCpp doesn't support model selection - it serves a single model
        self
    }
}

impl OpenAiChatProvider for LlamaCppProvider {
    type Config = OpenAIConfig;

    fn client(&self) -> &Client<Self::Config> {
        &self.client
    }

    fn model(&self) -> &'static str {
        "" // llama.cpp server serves a single model on boot and does not allow swapping models
    }

    fn provider_name(&self) -> &'static str {
        "LlamaCpp"
    }

    fn idle_timeout(&self) -> Duration {
        self.idle_timeout
    }
}
