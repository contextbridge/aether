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
    responses: Arc<Mutex<HashMap<String, FakeResponse>>>,
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
        self.responses.lock().unwrap().insert(url.to_string(), FakeResponse::Ok(response));
        self
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
        self.responses.lock().unwrap().insert(url.to_string(), FakeResponse::Timeout(timeout_ms));
        self
    }

    /// Scripts a [`WebFetchError::RequestFailed`] carrying `message` for `url`.
    pub fn with_request_failed(self, url: &str, message: &str) -> Self {
        self.responses.lock().unwrap().insert(url.to_string(), FakeResponse::RequestFailed(message.to_string()));
        self
    }
}

impl HttpClient for FakeHttpClient {
    fn fetch<'a>(&'a self, url: &'a str, _timeout: Duration) -> BoxFuture<'a, Result<HttpResponse, WebFetchError>> {
        let responses = self.responses.lock().unwrap();
        Box::pin(std::future::ready(match responses.get(url) {
            Some(response) => response.clone().into_result(),
            None => Err(WebFetchError::RequestFailed(format!("No fake response configured for URL: {url}"))),
        }))
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
