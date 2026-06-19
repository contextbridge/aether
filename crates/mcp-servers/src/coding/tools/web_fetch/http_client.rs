use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::time::Duration;

use crate::coding::error::WebFetchError;

pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;
pub const MAX_TIMEOUT_MS: u64 = 60_000;
pub const MAX_CONTENT_LENGTH: usize = 50_000;
const USER_AGENT: &str = "Mozilla/5.0 (compatible; MCP-Lexicon/1.0)";

/// Input parameters for the `web_fetch` tool
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WebFetchInput {
    /// The URL to fetch content from (must be a valid HTTP/HTTPS URL)
    pub url: String,

    /// A prompt describing what information to extract from the page (optional).
    /// This is for documentation purposes and helps focus the user's intent.
    pub prompt: Option<String>,

    /// Optional timeout in milliseconds (default: 30000, max: 60000)
    pub timeout: Option<u64>,
}

/// Output from the `web_fetch` tool
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WebFetchOutput {
    /// The fetched content converted to markdown
    pub content: String,

    /// The final URL after any redirects
    pub final_url: String,

    /// HTTP status code
    pub status_code: u16,

    /// Whether the content was truncated
    pub truncated: bool,

    /// Page title if available
    pub title: Option<String>,

    /// Author or byline if available
    #[serde(skip_serializing_if = "Option::is_none")]
    pub byline: Option<String>,

    /// Display metadata for human-friendly rendering
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub meta: Option<mcp_utils::display_meta::ToolResultMeta>,
}

/// Response from an HTTP client fetch operation
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub final_url: String,
    pub status_code: u16,
    pub body: String,
}

/// Trait for HTTP clients that can fetch web content
pub trait HttpClient: Send + Sync {
    fn fetch(&self, url: &str, timeout: Duration) -> impl Future<Output = Result<HttpResponse, WebFetchError>> + Send;
}

/// Production HTTP client using reqwest
#[derive(Debug, Clone)]
pub struct ReqwestClient {
    client: reqwest::Client,
}

impl Default for ReqwestClient {
    fn default() -> Self {
        Self::new()
    }
}

impl ReqwestClient {
    pub fn new() -> Self {
        let client = reqwest::Client::builder().user_agent(USER_AGENT).build().expect("Failed to build HTTP client");

        Self { client }
    }
}

impl HttpClient for ReqwestClient {
    async fn fetch(&self, url: &str, timeout: Duration) -> Result<HttpResponse, WebFetchError> {
        let response = self.client.get(url).timeout(timeout).send().await.map_err(|e| {
            if e.is_timeout() {
                WebFetchError::Timeout(u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX))
            } else {
                WebFetchError::RequestFailed(e.to_string())
            }
        })?;

        let final_url = response.url().to_string();
        let status_code = response.status().as_u16();

        let body = response.text().await.map_err(|e| WebFetchError::RequestFailed(e.to_string()))?;

        Ok(HttpResponse { final_url, status_code, body })
    }
}

/// Fake HTTP client for testing without network calls
#[cfg(test)]
#[derive(Debug, Clone)]
pub struct FakeHttpClient {
    responses: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, HttpResponse>>>,
    fetch_history: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    default_response: Option<HttpResponse>,
}

#[cfg(test)]
impl Default for FakeHttpClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl FakeHttpClient {
    pub fn new() -> Self {
        Self {
            responses: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            fetch_history: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            default_response: None,
        }
    }

    pub fn with_response(self, url: &str, response: HttpResponse) -> Self {
        self.responses.lock().unwrap().insert(url.to_string(), response);
        self
    }

    pub fn with_html(self, url: &str, html: &str) -> Self {
        self.with_response(url, HttpResponse { final_url: url.to_string(), status_code: 200, body: html.to_string() })
    }

    pub fn with_default(mut self, response: HttpResponse) -> Self {
        self.default_response = Some(response);
        self
    }

    pub fn fetch_count(&self) -> usize {
        self.fetch_history.lock().unwrap().len()
    }

    pub fn fetched_urls(&self) -> Vec<String> {
        self.fetch_history.lock().unwrap().clone()
    }
}

#[cfg(test)]
impl HttpClient for FakeHttpClient {
    async fn fetch(&self, url: &str, _timeout: Duration) -> Result<HttpResponse, WebFetchError> {
        self.fetch_history.lock().unwrap().push(url.to_string());

        let responses = self.responses.lock().unwrap();
        if let Some(response) = responses.get(url) {
            Ok(response.clone())
        } else if let Some(ref default) = self.default_response {
            Ok(default.clone())
        } else {
            Err(WebFetchError::RequestFailed(format!("No fake response configured for URL: {url}")))
        }
    }
}
