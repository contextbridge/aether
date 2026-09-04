use std::fmt;

use thiserror::Error;

#[doc = include_str!("docs/llm_error.md")]
#[derive(Debug, Error, Clone)]
pub enum LlmError {
    /// Environment variable not set or invalid
    #[error("{0} environment variable not set")]
    MissingApiKey(String),
    /// HTTP client creation failed
    #[error("Failed to create HTTP client: {0}")]
    HttpClientCreation(String),
    /// Normalized provider-side failure carrying retry classification and
    /// support diagnostics (HTTP status, request ID, provider error code).
    #[error("{0}")]
    Provider(#[from] ProviderError),
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
    /// Provider endpoint URL has not been configured.
    #[error("Provider '{provider}' requires a URL configured via providers.{provider}.url")]
    MissingProviderUrl { provider: String },
    /// Provider name is not registered with the model parser.
    #[error("Unknown provider: {provider}")]
    UnknownProvider { provider: String },
    /// The model spec did not yield any usable provider.
    #[error("No models provided")]
    EmptyModelSpec,
    /// A single-model-only config field was reused across multiple models of the
    /// same provider within one alloy spec (e.g. bedrock `inferenceProfileArn`
    /// or openai-compatible `requestModel`).
    #[error("providers.{provider}.{field} cannot be used with multiple {provider} models in one alloy spec")]
    DuplicateProvider { provider: String, field: String },
    /// A `provider:model` identity could not be parsed.
    #[error("Invalid model spec: {0}")]
    InvalidModelSpec(String),
    /// A provider request body could not be constructed (e.g. an SDK builder
    /// rejected the input or contained malformed data).
    #[error("Failed to build provider request: {0}")]
    ProviderRequest(String),
    /// An upstream client library rejected an argument as invalid.
    #[error("Invalid argument: {0}")]
    InvalidArgument(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderErrorKind {
    Authentication,
    Api,
    RateLimit,
    Server,
    Timeout,
    Network,
    StreamInterrupted,
}

impl ProviderErrorKind {
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::RateLimit | Self::Server | Self::Timeout | Self::Network | Self::StreamInterrupted)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderError {
    pub kind: ProviderErrorKind,
    pub message: String,
    pub http_status: Option<u16>,
    pub request_id: Option<String>,
    pub code: Option<String>,
}

impl ProviderError {
    pub fn new(kind: ProviderErrorKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into(), http_status: None, request_id: None, code: None }
    }

    pub fn authentication(message: impl Into<String>) -> Self {
        Self::new(ProviderErrorKind::Authentication, message)
    }

    pub fn api(message: impl Into<String>) -> Self {
        Self::new(ProviderErrorKind::Api, message)
    }

    pub fn rate_limit(message: impl Into<String>) -> Self {
        Self::new(ProviderErrorKind::RateLimit, message)
    }

    pub fn server(message: impl Into<String>) -> Self {
        Self::new(ProviderErrorKind::Server, message)
    }

    pub fn timeout(message: impl Into<String>) -> Self {
        Self::new(ProviderErrorKind::Timeout, message)
    }

    pub fn network(message: impl Into<String>) -> Self {
        Self::new(ProviderErrorKind::Network, message)
    }

    pub fn stream_interrupted(message: impl Into<String>) -> Self {
        Self::new(ProviderErrorKind::StreamInterrupted, message)
    }

    pub fn from_http_status(status: u16, message: impl Into<String>) -> Self {
        match status {
            401 | 403 => Self::authentication(message),
            408 | 504 => Self::timeout(message),
            429 => Self::rate_limit(message),
            s if (500..600).contains(&s) => Self::server(message),
            _ => Self::api(message),
        }
        .with_http_status(status)
    }

    pub fn with_http_status(mut self, status: u16) -> Self {
        self.http_status = Some(status);
        self
    }

    pub fn with_http_metadata(mut self, status: Option<u16>, request_id: Option<String>) -> Self {
        self.http_status = status;
        self.request_id = request_id;
        self
    }

    pub fn with_code(mut self, code: Option<String>) -> Self {
        self.code = code;
        self
    }

    pub fn with_request_id(mut self, request_id: Option<String>) -> Self {
        self.request_id = request_id;
        self
    }

    pub fn is_retryable(&self) -> bool {
        self.kind.is_retryable()
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let prefix = match self.kind {
            ProviderErrorKind::Authentication => "Authentication error",
            ProviderErrorKind::Api => "API error",
            ProviderErrorKind::RateLimit => "Rate limited",
            ProviderErrorKind::Server => "Server error",
            ProviderErrorKind::Timeout => "Request timed out",
            ProviderErrorKind::Network => "Network error",
            ProviderErrorKind::StreamInterrupted => "Stream interrupted",
        };
        write!(f, "{prefix}: {}", self.message)?;
        let mut diagnostics = Vec::new();
        if let Some(status) = self.http_status {
            diagnostics.push(format!("status {status}"));
        }
        if let Some(code) = &self.code {
            diagnostics.push(format!("code {code}"));
        }
        if let Some(request_id) = &self.request_id {
            diagnostics.push(format!("request_id {request_id}"));
        }
        if !diagnostics.is_empty() {
            write!(f, " ({})", diagnostics.join(", "))?;
        }
        Ok(())
    }
}

impl std::error::Error for ProviderError {}

impl LlmError {
    pub fn is_retryable(&self) -> bool {
        self.provider().is_some_and(ProviderError::is_retryable)
    }

    pub fn provider(&self) -> Option<&ProviderError> {
        match self {
            Self::Provider(error) => Some(error),
            _ => None,
        }
    }
}

impl From<reqwest::Error> for LlmError {
    fn from(error: reqwest::Error) -> Self {
        if error.is_timeout() {
            return ProviderError::timeout(error.to_string()).into();
        }
        if error.is_connect() || error.is_request() {
            return ProviderError::network(error.to_string()).into();
        }
        match error.status().map(|s| s.as_u16()) {
            Some(status) => ProviderError::from_http_status(status, error.to_string()).into(),
            None => ProviderError::network(error.to_string()).into(),
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
        LlmError::ProviderRequest(error.to_string())
    }
}

impl From<async_openai::error::OpenAIError> for LlmError {
    fn from(error: async_openai::error::OpenAIError) -> Self {
        use async_openai::error::OpenAIError;
        match error {
            OpenAIError::Reqwest(e) => LlmError::from(e),
            OpenAIError::StreamError(e) => ProviderError::stream_interrupted(e.to_string()).into(),
            OpenAIError::ApiError(api_err) => {
                let status = api_err.status_code.as_u16();
                let code = api_err.api_error.code.clone();
                let message = format!("{status} {}", api_err.api_error);
                ProviderError::from_http_status(status, message).with_code(code).into()
            }
            OpenAIError::JSONDeserialize(e, _) => LlmError::JsonParsing(e.to_string()),
            OpenAIError::FileSaveError(s) | OpenAIError::FileReadError(s) => LlmError::IoError(s),
            OpenAIError::InvalidArgument(s) => LlmError::InvalidArgument(s),
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
    fn retryable_kinds_cover_transient_failures() {
        assert!(!ProviderErrorKind::Authentication.is_retryable());
        assert!(!ProviderErrorKind::Api.is_retryable());
        assert!(ProviderErrorKind::RateLimit.is_retryable());
        assert!(ProviderErrorKind::Server.is_retryable());
        assert!(ProviderErrorKind::Timeout.is_retryable());
        assert!(ProviderErrorKind::Network.is_retryable());
        assert!(ProviderErrorKind::StreamInterrupted.is_retryable());
    }

    #[test]
    fn http_status_classification_covers_known_statuses() {
        assert_eq!(ProviderError::from_http_status(401, "d").kind, ProviderErrorKind::Authentication);
        assert_eq!(ProviderError::from_http_status(403, "d").kind, ProviderErrorKind::Authentication);
        assert_eq!(ProviderError::from_http_status(408, "d").kind, ProviderErrorKind::Timeout);
        assert!(ProviderError::from_http_status(408, "d").is_retryable());
        assert_eq!(ProviderError::from_http_status(429, "d").kind, ProviderErrorKind::RateLimit);
        assert_eq!(ProviderError::from_http_status(500, "d").kind, ProviderErrorKind::Server);
        assert_eq!(ProviderError::from_http_status(503, "d").kind, ProviderErrorKind::Server);
        assert_eq!(ProviderError::from_http_status(400, "d").kind, ProviderErrorKind::Api);
        assert!(!ProviderError::from_http_status(400, "d").is_retryable());
        assert_eq!(ProviderError::from_http_status(404, "d").kind, ProviderErrorKind::Api);
    }

    #[test]
    fn display_includes_diagnostics_when_present() {
        let error = ProviderError::server("boom").with_http_status(200).with_code(Some("server_error".into()));
        let text = error.to_string();
        assert!(text.contains("boom"));
        assert!(text.contains("status 200"));
        assert!(text.contains("code server_error"));
        let error = error.with_request_id(Some("req-1".into()));
        assert!(error.to_string().contains("request_id req-1"));
    }

    #[test]
    fn display_omits_suffix_when_no_metadata() {
        let error = ProviderError::api("bad");
        assert_eq!(error.to_string(), "API error: bad");
    }

    #[test]
    fn is_retryable() {
        assert!(LlmError::from(ProviderError::rate_limit("rl")).is_retryable());
        assert!(LlmError::from(ProviderError::server("x").with_http_status(503)).is_retryable());
        assert!(LlmError::from(ProviderError::server("stream-level")).is_retryable());
        assert!(LlmError::from(ProviderError::timeout("t")).is_retryable());
        assert!(LlmError::from(ProviderError::network("n")).is_retryable());
        assert!(LlmError::from(ProviderError::stream_interrupted("s")).is_retryable());

        assert!(!LlmError::from(ProviderError::api("x")).is_retryable());
        assert!(!LlmError::from(ProviderError::authentication("x")).is_retryable());
        assert!(!LlmError::MissingApiKey("x".into()).is_retryable());
        assert!(!LlmError::HttpClientCreation("x".into()).is_retryable());
        assert!(!LlmError::IoError("x".into()).is_retryable());
        assert!(!LlmError::JsonParsing("x".into()).is_retryable());
        assert!(!LlmError::ToolParameterParsing { tool_name: "t".into(), error: "e".into() }.is_retryable());
        assert!(!LlmError::OAuthError("x".into()).is_retryable());
        assert!(!LlmError::UnsupportedContent("x".into()).is_retryable());
        assert!(!LlmError::MissingProviderUrl { provider: "azure-foundry".into() }.is_retryable());
        assert!(!LlmError::UnknownProvider { provider: "foo".into() }.is_retryable());
        assert!(!LlmError::EmptyModelSpec.is_retryable());
        assert!(
            !LlmError::DuplicateProvider { provider: "bedrock".into(), field: "inferenceProfileArn".into() }
                .is_retryable()
        );
        assert!(!LlmError::InvalidModelSpec("x".into()).is_retryable());
        assert!(!LlmError::ProviderRequest("x".into()).is_retryable());
        assert!(!LlmError::InvalidArgument("x".into()).is_retryable());
    }

    #[test]
    fn async_openai_api_error_preserves_status_and_code() {
        use async_openai::error::{ApiError, ApiErrorResponse};
        let response = ApiErrorResponse {
            status_code: reqwest::StatusCode::SERVICE_UNAVAILABLE,
            api_error: ApiError {
                message: "overloaded".to_string(),
                r#type: None,
                param: None,
                code: Some("server_error".to_string()),
            },
        };
        let error = LlmError::from(async_openai::error::OpenAIError::ApiError(response));
        let provider = error.provider().expect("expected provider error");
        assert_eq!(provider.kind, ProviderErrorKind::Server);
        assert_eq!(provider.http_status, Some(503));
        assert_eq!(provider.code.as_deref(), Some("server_error"));
        assert!(error.is_retryable());
    }

    #[test]
    fn async_openai_stream_error_is_interruption() {
        let io = std::io::Error::other("eof");
        let error = LlmError::from(async_openai::error::OpenAIError::StreamError(Box::new(
            async_openai::error::StreamError::EventStream(io.to_string()),
        )));
        let provider = error.provider().expect("expected provider error");
        assert_eq!(provider.kind, ProviderErrorKind::StreamInterrupted);
        assert!(error.is_retryable());
    }
}
