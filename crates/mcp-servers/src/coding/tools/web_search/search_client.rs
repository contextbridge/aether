//! Search client abstraction for web search operations

use futures::future::BoxFuture;
use reqwest::header::RETRY_AFTER;
use serde::Deserialize;
use std::time::Duration;

use super::SearchResult;
use crate::coding::error::WebSearchError;

const DEFAULT_TIMEOUT_MS: u64 = 10_000;
const BRAVE_API_ENDPOINT: &str = "https://api.search.brave.com/res/v1/web/search";

/// Input parameters for search client
pub struct SearchParams {
    pub query: String,
    pub count: u32,
}

/// Trait for search clients that can perform web searches
pub trait SearchClient: Send + Sync {
    fn search(&self, params: SearchParams) -> BoxFuture<'_, Result<Vec<SearchResult>, WebSearchError>>;
}

/// Production search client using Brave Search API
#[derive(Debug, Clone)]
pub struct BraveSearchClient {
    client: reqwest::Client,
    api_key: String,
}

impl BraveSearchClient {
    /// Creates a new `BraveSearchClient` with given API key
    ///
    /// The API key is read from `BRAVE_SEARCH_API_KEY` environment variable
    pub fn new() -> Result<Self, WebSearchError> {
        let api_key = std::env::var("BRAVE_SEARCH_API_KEY").map_err(|_| {
            WebSearchError::ConfigError(
                "BRAVE_SEARCH_API_KEY environment variable not set. \
                 Get a free API key from https://api.search.brave.com/app/keys"
                    .to_string(),
            )
        })?;

        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(DEFAULT_TIMEOUT_MS))
            .build()
            .map_err(|e| WebSearchError::ConfigError(format!("Failed to build HTTP client: {e}")))?;

        Ok(Self { client, api_key })
    }
}

impl SearchClient for BraveSearchClient {
    fn search(&self, params: SearchParams) -> BoxFuture<'_, Result<Vec<SearchResult>, WebSearchError>> {
        Box::pin(async move {
            let response = self
                .client
                .get(BRAVE_API_ENDPOINT)
                .header("X-Subscription-Token", &self.api_key)
                .header("Accept", "application/json")
                .query(&[("q", &params.query), ("count", &params.count.to_string())])
                .send()
                .await
                .map_err(|e| {
                    if e.is_timeout() {
                        WebSearchError::Timeout(DEFAULT_TIMEOUT_MS)
                    } else if e.is_connect() {
                        WebSearchError::ApiError(format!("Connection failed: {e}"))
                    } else {
                        WebSearchError::ApiError(format!("Request failed: {e}"))
                    }
                })?;

            let status = response.status();

            if status.is_client_error() || status.is_server_error() {
                let retry_after = parse_retry_after(response.headers().get(RETRY_AFTER));
                let error_text = response.text().await.unwrap_or_else(|_| "Unable to read error response".to_string());

                if status.as_u16() == 429 {
                    return Err(WebSearchError::RateLimited { message: error_text, retry_after });
                }

                return Err(WebSearchError::ApiError(format!("API returned {}: {error_text}", status.as_u16())));
            }

            let response_body: BraveWebResponse = response
                .json()
                .await
                .map_err(|e| WebSearchError::ParseError(format!("Failed to parse JSON response: {e}")))?;

            Ok(response_body.web.map(|web| web.results).unwrap_or_default())
        })
    }
}

fn parse_retry_after(value: Option<&reqwest::header::HeaderValue>) -> Option<Duration> {
    let value = value?.to_str().ok()?.trim();

    value.parse::<u64>().ok().map(Duration::from_secs).or_else(|| {
        chrono::DateTime::parse_from_rfc2822(value)
            .ok()?
            .with_timezone(&chrono::Utc)
            .signed_duration_since(chrono::Utc::now())
            .to_std()
            .ok()
    })
}

/// Brave API response structure
#[derive(Debug, Deserialize)]
struct BraveWebResponse {
    web: Option<BraveWebResults>,
}

#[derive(Debug, Deserialize)]
struct BraveWebResults {
    results: Vec<SearchResult>,
}

/// Fake search client that serves queued responses in call order.
#[cfg(test)]
#[derive(Debug, Default)]
pub struct FakeSearchClient {
    responses: std::sync::Mutex<std::collections::VecDeque<Result<Vec<SearchResult>, WebSearchError>>>,
}

#[cfg(test)]
impl FakeSearchClient {
    pub fn new(responses: impl IntoIterator<Item = Result<Vec<SearchResult>, WebSearchError>>) -> Self {
        Self { responses: std::sync::Mutex::new(responses.into_iter().collect()) }
    }
}

#[cfg(test)]
impl SearchClient for FakeSearchClient {
    fn search(&self, _params: SearchParams) -> BoxFuture<'_, Result<Vec<SearchResult>, WebSearchError>> {
        let response = self.responses.lock().unwrap().pop_front();
        Box::pin(std::future::ready(
            response.unwrap_or_else(|| Err(WebSearchError::ApiError("No fake response queued".to_string()))),
        ))
    }
}
