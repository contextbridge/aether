//! Search client abstraction for web search operations

use futures::future::BoxFuture;
use reqwest::header::RETRY_AFTER;
use serde::Deserialize;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crate::coding::error::WebSearchError;

const DEFAULT_TIMEOUT_MS: u64 = 10_000;
const BRAVE_API_ENDPOINT: &str = "https://api.search.brave.com/res/v1/web/search";

/// Raw search result from search API
#[derive(Debug, Clone)]
pub struct RawSearchResult {
    pub title: String,
    pub url: String,
    pub description: String,
}

/// Input parameters for search client
pub struct SearchParams {
    pub query: String,
    pub count: u32,
}

/// Trait for search clients that can perform web searches
pub trait SearchClient: Send + Sync {
    fn search(&self, params: SearchParams) -> BoxFuture<'_, Result<Vec<RawSearchResult>, WebSearchError>>;
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
    fn search(&self, params: SearchParams) -> BoxFuture<'_, Result<Vec<RawSearchResult>, WebSearchError>> {
        Box::pin(async move {
            if params.query.trim().is_empty() {
                return Err(WebSearchError::InvalidQuery("Search query cannot be empty".to_string()));
            }

            let count = params.count.min(20); // Max 20 results per request

            let response = self
                .client
                .get(BRAVE_API_ENDPOINT)
                .header("X-Subscription-Token", &self.api_key)
                .header("Accept", "application/json")
                .query(&[("q", &params.query), ("count", &count.to_string())])
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

            let results = response_body
                .web
                .map(|w| {
                    w.results
                        .into_iter()
                        .map(|r| RawSearchResult { title: r.title, url: r.url, description: r.description })
                        .collect()
                })
                .unwrap_or_default();

            Ok(results)
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
    results: Vec<BraveResult>,
}

#[derive(Debug, Deserialize)]
struct BraveResult {
    title: String,
    url: String,
    description: String,
}

/// The result of a single search call: either matching results or an error.
#[cfg(test)]
type SearchOutcome = Result<Vec<RawSearchResult>, WebSearchError>;

/// Fake search client for testing without network calls
#[cfg(test)]
#[derive(Debug, Clone)]
pub struct FakeSearchClient {
    responses: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, Vec<RawSearchResult>>>>,
    call_count: std::sync::Arc<AtomicUsize>,
    /// When set, responses are served in call order (indexed by call count)
    /// instead of being looked up by query. Use for retry/error tests.
    sequential_responses: Option<std::sync::Arc<std::sync::Mutex<Vec<SearchOutcome>>>>,
}

#[cfg(test)]
impl Default for FakeSearchClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl FakeSearchClient {
    pub fn new() -> Self {
        Self {
            responses: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            call_count: std::sync::Arc::default(),
            sequential_responses: None,
        }
    }

    pub fn with_results(self, query: &str, results: Vec<RawSearchResult>) -> Self {
        self.responses.lock().unwrap().insert(query.to_string(), results);
        self
    }

    /// Queue a sequence of responses served in call order, regardless of query.
    /// Enables retry/error tests where the same query returns different results.
    pub fn with_sequential_results(mut self, responses: Vec<SearchOutcome>) -> Self {
        self.sequential_responses = Some(std::sync::Arc::new(std::sync::Mutex::new(responses)));
        self
    }
}

#[cfg(test)]
impl SearchClient for FakeSearchClient {
    fn search(&self, params: SearchParams) -> BoxFuture<'_, Result<Vec<RawSearchResult>, WebSearchError>> {
        let call_index = self.call_count.fetch_add(1, Ordering::SeqCst);

        if let Some(ref sequential) = self.sequential_responses {
            return Box::pin(std::future::ready(sequential.lock().unwrap().get(call_index).cloned().unwrap_or_else(
                || Err(WebSearchError::ApiError(format!("No sequential response at index {call_index}"))),
            )));
        }

        let responses = self.responses.lock().unwrap();
        Box::pin(std::future::ready(responses.get(&params.query).cloned().ok_or_else(|| {
            WebSearchError::ApiError(format!("No fake response configured for query: {}", params.query))
        })))
    }
}
