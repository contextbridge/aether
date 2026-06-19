use crate::catalog::LlmModel;
use serde::Deserialize;
use std::time::Duration;
use tracing::debug;

pub async fn discover_local_models() -> Vec<LlmModel> {
    let (ollama, llamacpp) = tokio::join!(discover_ollama(), discover_llama_cpp());
    let mut models = ollama;
    models.extend(llamacpp);
    models
}

pub async fn discover_ollama() -> Vec<LlmModel> {
    let base = std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://localhost:11434".to_string());
    let url = format!("{base}/api/tags");

    let Some(body) = fetch(&url).await else {
        return Vec::new();
    };

    match serde_json::from_str::<OllamaTagsResponse>(&body) {
        Ok(resp) => resp.models.into_iter().map(|m| LlmModel::Ollama(m.name)).collect(),
        Err(e) => {
            debug!("Failed to parse Ollama response: {e}");
            Vec::new()
        }
    }
}

pub async fn discover_llama_cpp() -> Vec<LlmModel> {
    let base = std::env::var("LLAMA_CPP_HOST").unwrap_or_else(|_| "http://localhost:8080".to_string());
    let url = format!("{base}/v1/models");

    let Some(body) = fetch(&url).await else {
        return Vec::new();
    };

    match serde_json::from_str::<OpenAiModelsResponse>(&body) {
        Ok(resp) => resp.data.into_iter().map(|m| LlmModel::LlamaCpp(m.id)).collect(),
        Err(e) => {
            debug!("Failed to parse LlamaCpp response: {e}");
            Vec::new()
        }
    }
}

async fn fetch(url: &str) -> Option<String> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(3))
        .build()
        .ok()?;

    match client.get(url).send().await {
        Ok(resp) => resp.text().await.ok(),
        Err(e) => {
            debug!("Failed to reach {url}: {e}");
            None
        }
    }
}

#[derive(Deserialize)]
struct OllamaTagsResponse {
    #[serde(default)]
    models: Vec<OllamaModel>,
}

#[derive(Deserialize)]
struct OllamaModel {
    name: String,
}

#[derive(Deserialize)]
struct OpenAiModelsResponse {
    #[serde(default)]
    data: Vec<OpenAiModel>,
}

#[derive(Deserialize)]
struct OpenAiModel {
    id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ollama_tags_response() {
        let json = r#"{"models":[{"name":"llama3.2"},{"name":"codellama:7b"}]}"#;
        let resp: OllamaTagsResponse = serde_json::from_str(json).unwrap();
        let models: Vec<LlmModel> = resp.models.into_iter().map(|m| LlmModel::Ollama(m.name)).collect();

        assert_eq!(models.len(), 2);
        assert_eq!(models[0], LlmModel::Ollama("llama3.2".to_string()));
        assert_eq!(models[1], LlmModel::Ollama("codellama:7b".to_string()));
    }

    #[test]
    fn parse_llamacpp_models_response() {
        let json = r#"{"object":"list","data":[{"id":"my-model","object":"model"}]}"#;
        let resp: OpenAiModelsResponse = serde_json::from_str(json).unwrap();
        let models: Vec<LlmModel> = resp.data.into_iter().map(|m| LlmModel::LlamaCpp(m.id)).collect();

        assert_eq!(models.len(), 1);
        assert_eq!(models[0], LlmModel::LlamaCpp("my-model".to_string()));
    }

    #[test]
    fn parse_empty_ollama_response() {
        let json = r#"{"models":[]}"#;
        let resp: OllamaTagsResponse = serde_json::from_str(json).unwrap();
        assert!(resp.models.is_empty());
    }

    #[test]
    fn parse_malformed_json_returns_error() {
        let json = r"not valid json";
        assert!(serde_json::from_str::<OllamaTagsResponse>(json).is_err());
        assert!(serde_json::from_str::<OpenAiModelsResponse>(json).is_err());
    }

    #[test]
    fn parse_missing_models_field_defaults_to_empty() {
        let json = r"{}";
        let resp: OllamaTagsResponse = serde_json::from_str(json).unwrap();
        assert!(resp.models.is_empty());

        let resp: OpenAiModelsResponse = serde_json::from_str(json).unwrap();
        assert!(resp.data.is_empty());
    }
}
