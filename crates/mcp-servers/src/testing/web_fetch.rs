use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::coding::error::WebFetchError;
use crate::coding::tools::web_fetch::{HttpClient, HttpResponse};

/// In-memory [`HttpClient`] fake that scripts responses per URL.
///
/// Requests to URLs without a scripted result (or without a
/// [`FakeHttpClient::with_default`] fallback) fail with
/// [`WebFetchError::RequestFailed`]. The fake is cloneable and records every
/// fetch for assertions via [`FakeHttpClient::fetch_count`] and
/// [`FakeHttpClient::fetched_urls`].
#[derive(Debug, Clone)]
pub struct FakeHttpClient {
    responses: Arc<Mutex<HashMap<String, FakeResponse>>>,
    fetch_history: Arc<Mutex<Vec<String>>>,
    default_response: Option<HttpResponse>,
}

impl Default for FakeHttpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeHttpClient {
    pub fn new() -> Self {
        Self {
            responses: Arc::new(Mutex::new(HashMap::new())),
            fetch_history: Arc::new(Mutex::new(Vec::new())),
            default_response: None,
        }
    }

    /// Scripts a full response for `url`, including redirects via `final_url`.
    pub fn with_response(self, url: &str, response: HttpResponse) -> Self {
        self.responses.lock().unwrap().insert(url.to_string(), FakeResponse::Ok(response));
        self
    }

    /// Scripts a 200 response whose body is `html` and whose `final_url` is `url`.
    pub fn with_html(self, url: &str, html: &str) -> Self {
        self.with_response(url, HttpResponse { final_url: url.to_string(), status_code: 200, body: html.to_string() })
    }

    /// Scripts a [`WebFetchError::Timeout`] carrying `timeout_ms` for `url`.
    pub fn with_timeout(self, url: &str, timeout_ms: u64) -> Self {
        self.responses.lock().unwrap().insert(url.to_string(), FakeResponse::Timeout(timeout_ms));
        self
    }

    /// Scripts a [`WebFetchError::RequestFailed`] carrying `message` for `url`.
    pub fn with_request_failed(self, url: &str, message: &str) -> Self {
        self.responses.lock().unwrap().insert(url.to_string(), FakeResponse::RequestFailed(message.to_string()));
        self
    }

    /// Responds with `response` for any URL that has no scripted result.
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

impl HttpClient for FakeHttpClient {
    fn fetch(&self, url: &str, _timeout: Duration) -> impl Future<Output = Result<HttpResponse, WebFetchError>> + Send {
        self.fetch_history.lock().unwrap().push(url.to_string());

        let responses = self.responses.lock().unwrap();
        std::future::ready(match responses.get(url) {
            Some(response) => response.clone().into_result(),
            None => self
                .default_response
                .clone()
                .ok_or_else(|| WebFetchError::RequestFailed(format!("No fake response configured for URL: {url}"))),
        })
    }
}

#[derive(Debug, Clone)]
enum FakeResponse {
    Ok(HttpResponse),
    Timeout(u64),
    RequestFailed(String),
}

impl FakeResponse {
    fn into_result(self) -> Result<HttpResponse, WebFetchError> {
        match self {
            FakeResponse::Ok(response) => Ok(response),
            FakeResponse::Timeout(timeout_ms) => Err(WebFetchError::Timeout(timeout_ms)),
            FakeResponse::RequestFailed(message) => Err(WebFetchError::RequestFailed(message)),
        }
    }
}
