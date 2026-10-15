pub mod search_client;

pub use search_client::{BraveSearchClient, SearchClient, SearchParams};

#[cfg(test)]
pub use search_client::FakeSearchClient;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::time::Duration;
use url::Url;

use crate::coding::error::WebSearchError;
use utils::display_meta::{ToolDisplayMeta, ToolResultMeta};

const DEFAULT_COUNT: u32 = 10;
const MAX_COUNT: u32 = 20;
const MAX_RATE_LIMIT_RETRIES: u32 = 3;
const MAX_RETRY_AFTER: Duration = Duration::from_mins(1);
const DEFAULT_RETRY_DELAY: Duration = Duration::from_secs(2);

/// Input parameters for `web_search` tool
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WebSearchInput {
    /// The search query
    pub query: String,

    /// Maximum number of results (default: 10, max: 20)
    pub count: Option<u32>,

    /// Only include results from these domains
    #[serde(alias = "allowed_domains")]
    pub allowed_domains: Option<Vec<String>>,

    /// Exclude results from these domains
    #[serde(alias = "blocked_domains")]
    pub blocked_domains: Option<Vec<String>>,
}

/// Output from `web_search` tool
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WebSearchOutput {
    /// Search results
    pub results: Vec<SearchResult>,

    /// The original query
    pub query: String,

    /// Display metadata for human-friendly rendering
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub meta: Option<ToolResultMeta>,
}

/// Individual search result
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    /// Result title
    pub title: String,

    /// Result URL
    pub url: String,

    /// Result snippet/description
    #[serde(alias = "description")]
    pub snippet: String,
}

/// Web searcher that performs searches and filters results
pub struct WebSearcher {
    client: Box<dyn SearchClient>,
}

impl WebSearcher {
    pub fn with_client(client: impl SearchClient + 'static) -> Self {
        Self { client: Box::new(client) }
    }

    pub fn try_new() -> Result<Self, WebSearchError> {
        Ok(Self::with_client(BraveSearchClient::new()?))
    }

    /// Performs a web search with the given parameters
    pub async fn search<T, U>(&self, args: WebSearchInput, mut on_retry: T) -> Result<WebSearchOutput, WebSearchError>
    where
        T: FnMut(Duration) -> U,
        U: Future<Output = ()>,
    {
        let query = args.query.trim();

        if query.is_empty() {
            return Err(WebSearchError::InvalidQuery("Search query cannot be empty".to_string()));
        }

        let count = args.count.unwrap_or(DEFAULT_COUNT).min(MAX_COUNT);

        let query_owned = query.to_string();
        let mut retries = 0;
        let mut results = loop {
            let params = SearchParams { query: query_owned.clone(), count };

            match self.client.search(params).await {
                Ok(results) => break results,
                Err(WebSearchError::RateLimited { retry_after, .. }) if retries < MAX_RATE_LIMIT_RETRIES => {
                    let delay = retry_after.unwrap_or(DEFAULT_RETRY_DELAY).min(MAX_RETRY_AFTER);
                    retries += 1;
                    on_retry(delay).await;
                    tokio::time::sleep(delay).await;
                }
                Err(error) => return Err(error),
            }
        };

        results.truncate(count as usize);
        results.retain(|result| {
            let url = Url::parse(&result.url).ok();
            let host = url.as_ref().and_then(Url::host_str);
            let in_any =
                |domains: &[String]| host.is_some_and(|host| domains.iter().any(|d| host.ends_with(d.as_str())));
            args.allowed_domains.as_deref().is_none_or(in_any) && !args.blocked_domains.as_deref().is_some_and(in_any)
        });

        let display_meta = ToolDisplayMeta::new("Web search", format!("'{}' ({} results)", query, results.len()));

        Ok(WebSearchOutput { results, query: query.to_string(), meta: Some(display_meta.into()) })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::*;

    fn make_result(url: &str) -> SearchResult {
        SearchResult { title: "Test".to_string(), url: url.to_string(), snippet: "Test description".to_string() }
    }

    fn input(query: &str) -> WebSearchInput {
        WebSearchInput { query: query.to_string(), count: None, allowed_domains: None, blocked_domains: None }
    }

    fn strs(vals: &[&str]) -> Vec<String> {
        vals.iter().map(std::string::ToString::to_string).collect()
    }

    fn searcher_with(query: &str, results: Vec<SearchResult>) -> WebSearcher {
        WebSearcher::with_client(FakeSearchClient::default().with_results(query, results))
    }

    fn rate_limited(retry_after: Option<Duration>) -> WebSearchError {
        WebSearchError::RateLimited { message: "too many requests".to_string(), retry_after }
    }

    #[tokio::test]
    async fn test_domain_filtering() {
        let urls = ["https://example.com/page", "https://test.org/page", "https://sub.example.com:8080/page"];
        let cases = [
            (Some(strs(&["example.com"])), None, vec![urls[0], urls[2]]),
            (None, Some(strs(&["example.com"])), vec![urls[1]]),
            (Some(strs(&["example.com", "test.org"])), Some(strs(&["test.org"])), vec![urls[0], urls[2]]),
        ];

        for (allowed_domains, blocked_domains, expected) in cases {
            let searcher = searcher_with("rust", urls.iter().map(|url| make_result(url)).collect());
            let query = WebSearchInput { allowed_domains, blocked_domains, ..input("rust") };
            let output = searcher.search(query, |_| async {}).await.unwrap();
            let urls: Vec<_> = output.results.iter().map(|r| r.url.as_str()).collect();
            assert_eq!(urls, expected);
        }
    }

    #[tokio::test]
    async fn test_empty_query_returns_error() {
        let searcher = WebSearcher::with_client(FakeSearchClient::default());
        let result = searcher.search(input("   "), |_| async {}).await;
        assert!(matches!(result, Err(WebSearchError::InvalidQuery(_))));
    }

    #[tokio::test(start_paused = true)]
    async fn retries_rate_limited_search_after_retry_after_period() {
        let client = FakeSearchClient::default()
            .with_results("rust", vec![make_result("https://example.com")])
            .with_failures("rust", [rate_limited(Some(Duration::ZERO))]);
        let searcher = WebSearcher::with_client(client);

        let retry_delays = Arc::new(Mutex::new(Vec::new()));
        let notified_delays = retry_delays.clone();
        let output = searcher
            .search(input("rust"), move |delay| {
                let notified_delays = notified_delays.clone();
                async move {
                    notified_delays.lock().unwrap().push(delay);
                }
            })
            .await
            .unwrap();

        assert_eq!(output.results.len(), 1);
        assert_eq!(*retry_delays.lock().unwrap(), vec![Duration::ZERO]);
    }

    #[tokio::test(start_paused = true)]
    async fn retries_rate_limited_search_when_retry_after_header_is_missing() {
        let client = FakeSearchClient::default()
            .with_results("rust", vec![make_result("https://example.com")])
            .with_failures("rust", [rate_limited(None)]);
        let searcher = WebSearcher::with_client(client);

        let retry_delays = Arc::new(Mutex::new(Vec::new()));
        let notified_delays = retry_delays.clone();
        let output = searcher
            .search(input("rust"), move |delay| {
                let notified_delays = notified_delays.clone();
                async move {
                    notified_delays.lock().unwrap().push(delay);
                }
            })
            .await
            .unwrap();

        assert_eq!(output.results.len(), 1);
        assert_eq!(retry_delays.lock().unwrap().len(), 1, "should have retried once");
    }

    #[tokio::test(start_paused = true)]
    async fn returns_rate_limited_error_after_retry_budget_is_exhausted() {
        let mut failures = vec![rate_limited(Some(Duration::ZERO)); 4];
        failures.push(WebSearchError::ApiError("retry budget was exceeded".to_string()));
        let searcher = WebSearcher::with_client(FakeSearchClient::default().with_failures("rust", failures));
        let result = searcher.search(input("rust"), |_| async {}).await;

        assert!(matches!(result, Err(WebSearchError::RateLimited { .. })));
    }

    #[tokio::test]
    async fn test_count_limiting() {
        let searcher =
            searcher_with("test query", (0..30).map(|i| make_result(&format!("https://example.com/{i}"))).collect());

        let output = searcher.search(WebSearchInput { count: Some(15), ..input("test query") }, |_| async {}).await;
        assert_eq!(output.unwrap().results.len(), 15);
    }

    #[test]
    fn search_result_deserializes_brave_description_as_snippet() {
        let result: SearchResult = serde_json::from_value(serde_json::json!({
            "title": "Rust Language",
            "url": "https://rust-lang.org",
            "description": "A systems language"
        }))
        .unwrap();

        assert_eq!(result.snippet, "A systems language");
    }

    #[test]
    fn web_search_input_accepts_snake_case_domains() {
        let args: WebSearchInput = serde_json::from_value(serde_json::json!({
            "query": "rust",
            "count": 5,
            "allowed_domains": ["docs.rs"],
            "blocked_domains": ["example.com"]
        }))
        .unwrap();

        assert_eq!(args.query, "rust");
        assert_eq!(args.count, Some(5));
        assert_eq!(args.allowed_domains, Some(vec!["docs.rs".to_string()]));
        assert_eq!(args.blocked_domains, Some(vec!["example.com".to_string()]));
    }
}
