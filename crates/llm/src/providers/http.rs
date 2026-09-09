use crate::ProviderError;
use async_openai::{Client, config::Config, error::OpenAIError, middleware::HttpRequestFactory};
use reqwest::{Request, Response, header::HeaderMap};
use serde_json::{Value, from_str};
use tower::{Service, ServiceExt, service_fn};

/// Preserve HTTP diagnostics before async-openai deserializes rejected responses.
pub(crate) fn openai_client<T, U>(config: T, service: U) -> Client<T>
where
    T: Config,
    U: Service<Request, Response = Response, Error = reqwest::Error> + Clone + Send + Sync + 'static,
    U::Future: Send + 'static,
{
    Client::with_config(config).with_http_service(service_fn(move |factory: HttpRequestFactory| {
        let service = service.clone();
        async move {
            let response = service.oneshot(factory.build().await?).await.map_err(OpenAIError::Reqwest)?;
            if response.status().is_success() {
                return Ok(response);
            }
            Err(OpenAIError::Boxed(Box::new(rejected(response, responses_code).await)))
        }
    }))
}

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

pub(crate) async fn rejected(response: Response, code: fn(&str) -> Option<String>) -> ProviderError {
    let metadata = HttpResponseMetadata::from(&response);
    let body = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
    let message = format!("request failed with status {}: {body}", metadata.status);
    ProviderError::from_http_status(metadata.status, message)
        .with_code(code(&body))
        .with_request_id(metadata.request_id)
}

pub(crate) fn extract_json_code(body: &str, pointer: &str) -> Option<String> {
    match from_str::<Value>(body).ok()?.pointer(pointer)? {
        Value::String(code) => Some(code.clone()),
        Value::Number(code) => Some(code.to_string()),
        _ => None,
    }
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
