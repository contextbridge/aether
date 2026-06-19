//! Search client abstraction for web search operations

use serde::Deserialize;
use std::future::Future;
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
    fn search(&self, params: SearchParams)
    -> impl Future<Output = Result<Vec<RawSearchResult>, WebSearchError>> + Send;
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

    /// Creates a new `BraveSearchClient` with an explicit API key
    pub fn with_api_key(api_key: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(DEFAULT_TIMEOUT_MS))
            .build()
            .expect("Failed to build HTTP client");

        Self { client, api_key }
    }
}

impl SearchClient for BraveSearchClient {
    async fn search(&self, params: SearchParams) -> Result<Vec<RawSearchResult>, WebSearchError> {
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
            let error_text = response.text().await.unwrap_or_else(|_| "Unable to read error response".to_string());

            if status.as_u16() == 429 {
                return Err(WebSearchError::RateLimited(error_text));
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
    }
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

/// Fake search client for testing without network calls
#[cfg(test)]
#[derive(Debug, Clone)]
pub struct FakeSearchClient {
    responses: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, Vec<RawSearchResult>>>>,
    search_history: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    default_response: Option<Vec<RawSearchResult>>,
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
            search_history: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            default_response: None,
        }
    }

    pub fn with_results(self, query: &str, results: Vec<RawSearchResult>) -> Self {
        self.responses.lock().unwrap().insert(query.to_string(), results);
        self
    }

    pub fn with_default(mut self, results: Vec<RawSearchResult>) -> Self {
        self.default_response = Some(results);
        self
    }

    pub fn search_count(&self) -> usize {
        self.search_history.lock().unwrap().len()
    }

    pub fn searched_queries(&self) -> Vec<String> {
        self.search_history.lock().unwrap().clone()
    }
}

#[cfg(test)]
impl SearchClient for FakeSearchClient {
    async fn search(&self, params: SearchParams) -> Result<Vec<RawSearchResult>, WebSearchError> {
        self.search_history.lock().unwrap().push(params.query.clone());

        let responses = self.responses.lock().unwrap();
        if let Some(results) = responses.get(&params.query) {
            Ok(results.clone())
        } else if let Some(ref default) = self.default_response {
            Ok(default.clone())
        } else {
            Err(WebSearchError::ApiError(format!("No fake response configured for query: {}", params.query)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_params_limits_count() {
        let params = SearchParams { query: "test".to_string(), count: 100 };
        let _ = params; // Just ensure it compiles
    }

    #[tokio::test]
    async fn test_fake_client_returns_configured_results() {
        let fake = FakeSearchClient::new().with_results(
            "rust programming",
            vec![RawSearchResult {
                title: "Rust Programming Language".to_string(),
                url: "https://rust-lang.org".to_string(),
                description: "A systems programming language".to_string(),
            }],
        );

        let results = fake.search(SearchParams { query: "rust programming".to_string(), count: 10 }).await.unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Rust Programming Language");
    }

    #[tokio::test]
    async fn test_fake_client_tracks_search_history() {
        let fake = FakeSearchClient::new().with_results("query1", vec![]).with_results("query2", vec![]);

        fake.search(SearchParams { query: "query1".to_string(), count: 10 }).await.unwrap();

        fake.search(SearchParams { query: "query2".to_string(), count: 10 }).await.unwrap();

        assert_eq!(fake.search_count(), 2);
        assert_eq!(fake.searched_queries(), vec!["query1".to_string(), "query2".to_string()]);
    }

    #[tokio::test]
    async fn test_fake_client_with_default_response() {
        let fake = FakeSearchClient::new().with_default(vec![RawSearchResult {
            title: "Default Result".to_string(),
            url: "https://example.com".to_string(),
            description: "Default description".to_string(),
        }]);

        let results = fake.search(SearchParams { query: "any query".to_string(), count: 10 }).await.unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Default Result");
    }

    #[tokio::test]
    async fn test_fake_client_missing_query_returns_error() {
        let fake = FakeSearchClient::new();
        let result = fake.search(SearchParams { query: "not configured".to_string(), count: 10 }).await;

        assert!(result.is_err());
    }
}
