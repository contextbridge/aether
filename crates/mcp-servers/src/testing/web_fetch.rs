use futures::future::BoxFuture;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::coding::error::WebFetchError;
use crate::coding::tools::web_fetch::{HttpClient, HttpResponse};

/// In-memory [`HttpClient`] fake that scripts responses per URL.
///
/// Requests to URLs without a scripted result fail with
/// [`WebFetchError::RequestFailed`].
#[derive(Debug, Clone)]
pub struct FakeHttpClient {
    responses: Arc<Mutex<HashMap<String, Result<HttpResponse, WebFetchError>>>>,
}

impl Default for FakeHttpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeHttpClient {
    pub fn new() -> Self {
        Self { responses: Arc::new(Mutex::new(HashMap::new())) }
    }

    /// Scripts a full response for `url`, including redirects via `final_url`.
    pub fn with_response(self, url: &str, response: HttpResponse) -> Self {
        self.with_result(url, Ok(response))
    }

    /// Scripts a 200 response whose body is `html` and whose `final_url` is `url`.
    pub fn with_html(self, url: &str, html: &str) -> Self {
        self.with_response(
            url,
            HttpResponse {
                final_url: url.to_string(),
                status_code: 200,
                body: html.to_string(),
                content_type: Some("text/html".to_string()),
            },
        )
    }

    /// Scripts a [`WebFetchError::Timeout`] carrying `timeout_ms` for `url`.
    pub fn with_timeout(self, url: &str, timeout_ms: u64) -> Self {
        self.with_result(url, Err(WebFetchError::Timeout(timeout_ms)))
    }

    /// Scripts a [`WebFetchError::RequestFailed`] carrying `message` for `url`.
    pub fn with_request_failed(self, url: &str, message: &str) -> Self {
        self.with_result(url, Err(WebFetchError::RequestFailed(message.to_string())))
    }

    fn with_result(self, url: &str, result: Result<HttpResponse, WebFetchError>) -> Self {
        self.responses.lock().unwrap().insert(url.to_string(), result);
        self
    }
}

impl HttpClient for FakeHttpClient {
    fn fetch<'a>(&'a self, url: &'a str, _timeout: Duration) -> BoxFuture<'a, Result<HttpResponse, WebFetchError>> {
        let response = self.responses.lock().unwrap().get(url).cloned();
        Box::pin(std::future::ready(response.unwrap_or_else(|| {
            Err(WebFetchError::RequestFailed(format!("No fake response configured for URL: {url}")))
        })))
    }
}
