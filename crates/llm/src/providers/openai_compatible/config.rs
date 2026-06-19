use async_openai::config::{Config, OpenAIConfig};
use reqwest::header::{AUTHORIZATION, HeaderMap};
use secrecy::SecretString;

use crate::ProviderAuthMode;

#[derive(Clone, Debug)]
pub struct AetherOpenAiConfig {
    inner: OpenAIConfig,
    auth_mode: ProviderAuthMode,
}

impl AetherOpenAiConfig {
    pub fn new(inner: OpenAIConfig, auth_mode: ProviderAuthMode) -> Self {
        Self { inner, auth_mode }
    }
}

impl Config for AetherOpenAiConfig {
    fn headers(&self) -> HeaderMap {
        let mut headers = self.inner.headers();
        if self.auth_mode == ProviderAuthMode::None {
            headers.remove(AUTHORIZATION);
        }
        headers
    }

    fn url(&self, path: &str) -> String {
        self.inner.url(path)
    }

    fn query(&self) -> Vec<(&str, &str)> {
        self.inner.query()
    }

    fn api_base(&self) -> &str {
        self.inner.api_base()
    }

    fn api_key(&self) -> &SecretString {
        self.inner.api_key()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_auth_keeps_authorization_header() {
        let config = AetherOpenAiConfig::new(OpenAIConfig::new().with_api_key("token"), ProviderAuthMode::Default);
        assert!(config.headers().contains_key(AUTHORIZATION));
    }

    #[test]
    fn none_auth_removes_authorization_header() {
        let config = AetherOpenAiConfig::new(OpenAIConfig::new().with_api_key("token"), ProviderAuthMode::None);
        assert!(!config.headers().contains_key(AUTHORIZATION));
    }
}
