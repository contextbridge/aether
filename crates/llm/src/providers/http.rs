use crate::{LlmError, ProviderError};
use reqwest::{Response, header::HeaderMap};
use serde_json::{Value, from_str};

#[derive(Debug, Clone)]
pub(crate) struct HttpResponseMetadata {
    pub(crate) status: u16,
    pub(crate) request_id: Option<String>,
}

impl From<&Response> for HttpResponseMetadata {
    fn from(response: &Response) -> Self {
        Self { status: response.status().as_u16(), request_id: extract_request_id(response.headers()) }
    }
}

pub(crate) fn extract_request_id(headers: &HeaderMap) -> Option<String> {
    for name in ["x-amzn-requestid", "x-amz-request-id", "x-request-id", "request-id"] {
        if let Some(value) = headers.get(name)
            && let Ok(text) = value.to_str()
        {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

pub(crate) async fn rejected(provider: &str, response: Response, code: fn(&str) -> Option<String>) -> LlmError {
    let metadata = HttpResponseMetadata::from(&response);
    let body = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
    let message = format!("{provider} request failed with status {}: {body}", metadata.status);
    ProviderError::from_http_status(metadata.status, message)
        .with_code(code(&body))
        .with_request_id(metadata.request_id)
        .into()
}

pub(crate) fn extract_json_code(body: &str, pointer: &str) -> Option<String> {
    from_str::<Value>(body).ok()?.pointer(pointer)?.as_str().map(String::from)
}

pub(crate) fn responses_code(body: &str) -> Option<String> {
    extract_json_code(body, "/error/code")
}

pub(crate) fn anthropic_code(body: &str) -> Option<String> {
    extract_json_code(body, "/error/type")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_id_prefers_amazon_headers() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-request-id", "openai-1".parse().unwrap());
        headers.insert("x-amzn-requestid", "amzn-1".parse().unwrap());
        assert_eq!(extract_request_id(&headers).as_deref(), Some("amzn-1"));
    }

    #[test]
    fn request_id_ignores_blank_values() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-request-id", "   ".parse().unwrap());
        assert_eq!(extract_request_id(&headers), None);
    }

    #[test]
    fn extracts_nested_json_codes() {
        assert_eq!(anthropic_code(r#"{"error":{"type":"rate_limit_error"}}"#).as_deref(), Some("rate_limit_error"));
        assert_eq!(responses_code(r#"{"error":{"code":"server_error"}}"#).as_deref(), Some("server_error"));
    }
}
