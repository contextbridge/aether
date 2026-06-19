use std::fmt;

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextOverflowError {
    pub provider: String,
    pub model: Option<String>,
    pub requested_tokens: Option<u32>,
    pub max_tokens: Option<u32>,
    pub message: String,
}

impl ContextOverflowError {
    pub fn new(
        provider: impl Into<String>,
        model: Option<String>,
        requested_tokens: Option<u32>,
        max_tokens: Option<u32>,
        message: impl Into<String>,
    ) -> Self {
        Self { provider: provider.into(), model, requested_tokens, max_tokens, message: message.into() }
    }
}

impl fmt::Display for ContextOverflowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let model = self.model.as_deref().unwrap_or("unknown-model");
        match (self.requested_tokens, self.max_tokens) {
            (Some(requested), Some(max)) => write!(
                f,
                "{} (provider={}, model={}, requested={}, max={})",
                self.message, self.provider, model, requested, max
            ),
            _ => write!(f, "{} (provider={}, model={})", self.message, self.provider, model),
        }
    }
}

#[doc = include_str!("docs/llm_error.md")]
#[derive(Debug, Error, Clone)]
pub enum LlmError {
    /// Environment variable not set or invalid
    #[error("{0} environment variable not set")]
    MissingApiKey(String),
    /// Invalid API key format
    #[error("Invalid API key: {0}")]
    InvalidApiKey(String),
    /// HTTP client creation failed
    #[error("Failed to create HTTP client: {0}")]
    HttpClientCreation(String),
    /// API request failed
    #[error("API request failed: {0}")]
    ApiRequest(String),
    /// API returned an error response
    #[error("API error: {0}")]
    ApiError(String),
    /// HTTP 429 / provider-flagged rate limit. Retryable.
    #[error("Rate limited: {0}")]
    RateLimited(String),
    /// HTTP 5xx or provider-flagged server error. Retryable. `status` is
    /// `None` when the signal originates from a stream-level event (e.g.
    /// Anthropic SSE `overloaded_error`) rather than an HTTP response.
    #[error("Server error (status {status:?}): {message}")]
    ServerError { status: Option<u16>, message: String },
    /// Request timeout (no bytes received within client deadline). Retryable.
    #[error("Request timed out: {0}")]
    Timeout(String),
    /// Transport-level connection failure (DNS, TCP reset, TLS, request build). Retryable.
    #[error("Network error: {0}")]
    Network(String),
    /// Stream began but errored or terminated prematurely. Retryable.
    #[error("Stream interrupted: {0}")]
    StreamInterrupted(String),
    /// API rejected the request because the prompt exceeded the model's context window.
    #[error("Context overflow: {0}")]
    ContextOverflow(ContextOverflowError),
    /// IO error while reading stream
    #[error("IO error reading stream: {0}")]
    IoError(String),
    /// JSON parsing/serialization error
    #[error("JSON parsing error: {0}")]
    JsonParsing(String),
    /// Tool parameter parsing error
    #[error("Failed to parse tool parameters for {tool_name}: {error}")]
    ToolParameterParsing { tool_name: String, error: String },
    /// OAuth authentication error
    #[error("OAuth error: {0}")]
    OAuthError(String),
    /// The message contained only content types this provider doesn't support
    #[error("Unsupported content: {0}")]
    UnsupportedContent(String),
    /// Generic error for other cases
    #[error("{0}")]
    Other(String),
}

impl LlmError {
    /// Whether this error class is worth retrying. Transient transport / server
    /// failures return `true`; permanent failures (auth, schema, context size)
    /// return `false`.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            LlmError::RateLimited(_)
                | LlmError::ServerError { .. }
                | LlmError::Timeout(_)
                | LlmError::Network(_)
                | LlmError::StreamInterrupted(_)
        )
    }
}

impl From<reqwest::Error> for LlmError {
    fn from(error: reqwest::Error) -> Self {
        if error.is_timeout() {
            return LlmError::Timeout(error.to_string());
        }
        if error.is_connect() || error.is_request() {
            return LlmError::Network(error.to_string());
        }
        match error.status().map(|s| s.as_u16()) {
            Some(429) => LlmError::RateLimited(error.to_string()),
            Some(s) if (500..600).contains(&s) => LlmError::ServerError { status: Some(s), message: error.to_string() },
            _ => LlmError::ApiRequest(error.to_string()),
        }
    }
}

impl From<serde_json::Error> for LlmError {
    fn from(error: serde_json::Error) -> Self {
        LlmError::JsonParsing(error.to_string())
    }
}

impl From<std::io::Error> for LlmError {
    fn from(error: std::io::Error) -> Self {
        LlmError::IoError(error.to_string())
    }
}

impl From<reqwest::header::InvalidHeaderValue> for LlmError {
    fn from(error: reqwest::header::InvalidHeaderValue) -> Self {
        LlmError::InvalidApiKey(error.to_string())
    }
}

impl From<async_openai::error::OpenAIError> for LlmError {
    fn from(error: async_openai::error::OpenAIError) -> Self {
        use async_openai::error::OpenAIError;
        match error {
            OpenAIError::Reqwest(e) => LlmError::from(e),
            OpenAIError::StreamError(e) => LlmError::StreamInterrupted(e.to_string()),
            OpenAIError::ApiError(api_err) => LlmError::ApiError(api_err.to_string()),
            OpenAIError::JSONDeserialize(e, _) => LlmError::JsonParsing(e.to_string()),
            OpenAIError::FileSaveError(s) | OpenAIError::FileReadError(s) => LlmError::IoError(s),
            OpenAIError::InvalidArgument(s) => LlmError::Other(s),
        }
    }
}

#[cfg(feature = "codex")]
impl From<aether_auth::OAuthError> for LlmError {
    fn from(error: aether_auth::OAuthError) -> Self {
        LlmError::OAuthError(error.to_string())
    }
}

pub type Result<T> = std::result::Result<T, LlmError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_retryable() {
        assert!(LlmError::RateLimited("rl".into()).is_retryable());
        assert!(LlmError::ServerError { status: Some(503), message: "x".into() }.is_retryable());
        assert!(LlmError::ServerError { status: None, message: "stream-level".into() }.is_retryable());
        assert!(LlmError::Timeout("t".into()).is_retryable());
        assert!(LlmError::Network("n".into()).is_retryable());
        assert!(LlmError::StreamInterrupted("s".into()).is_retryable());

        assert!(!LlmError::ApiError("x".into()).is_retryable());
        assert!(!LlmError::ApiRequest("x".into()).is_retryable());
        assert!(!LlmError::MissingApiKey("x".into()).is_retryable());
        assert!(!LlmError::InvalidApiKey("x".into()).is_retryable());
        assert!(!LlmError::HttpClientCreation("x".into()).is_retryable());
        assert!(!LlmError::IoError("x".into()).is_retryable());
        assert!(!LlmError::JsonParsing("x".into()).is_retryable());
        assert!(!LlmError::ToolParameterParsing { tool_name: "t".into(), error: "e".into() }.is_retryable());
        assert!(!LlmError::OAuthError("x".into()).is_retryable());
        assert!(!LlmError::UnsupportedContent("x".into()).is_retryable());
        assert!(!LlmError::Other("x".into()).is_retryable());
        assert!(!LlmError::ContextOverflow(ContextOverflowError::new("p", None, None, None, "m")).is_retryable());
    }
}
