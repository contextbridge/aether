use mcp_servers::coding::error::WebFetchError;
use mcp_servers::coding::tools::web_fetch::{HttpResponse, WebFetchInput, WebFetchOutput, WebFetcher};
use mcp_servers::testing::FakeHttpClient;

fn html_page(title: &str, body: &str) -> String {
    format!("<html><head><title>{title}</title></head><body>{body}</body></html>")
}

#[tokio::test]
async fn test_fetch_truncates_unicode_without_panicking() {
    let result = WebFetchTest::new(html_page("Unicode", &format!("<p>{}</p>", "界".repeat(20_000)))).fetch().await;

    assert!(result.truncated);
    assert!(result.content.ends_with("[Content truncated...]"));
    assert!(result.content.starts_with('界'));
}

#[tokio::test]
async fn test_fetch_preserves_plain_text_documentation() {
    let body = "<SYSTEM>Abridged documentation</SYSTEM>\n\n# Rust\n\n```rust\nVec<String>\n```\n";
    let result = WebFetchTest::new(body)
        .url("https://example.com/llms-small.txt")
        .content_type(Some("text/plain; charset=utf-8"))
        .fetch()
        .await;

    assert_eq!(result.content, body);
    assert_eq!(result.title, None);
    assert!(!result.truncated);
}

#[tokio::test]
async fn test_fetch_html_content_types() {
    for content_type in [None, Some("text/html; charset=utf-8"), Some("Text/HTML"), Some("application/xhtml+xml")] {
        let result = WebFetchTest::new(html_page("HTML", "<h1>Heading</h1>"))
            .url("https://example.com/page.txt")
            .content_type(content_type)
            .fetch()
            .await;

        assert!(result.content.contains("# Heading"), "{content_type:?}");
        assert!(!result.content.contains("<h1>"));
    }
}

#[tokio::test]
async fn test_fetch_real_page() {
    let fetcher = WebFetcher::with_client(FakeHttpClient::new().with_html(
        "https://example.com/html",
        &html_page("Herman Melville", "<h1>Moby-Dick</h1><p>By Herman Melville</p>"),
    ));
    let result = fetcher
        .fetch(WebFetchInput { url: "https://example.com/html".to_string(), prompt: None, timeout: Some(10_000) })
        .await
        .unwrap();

    assert_eq!(result.status_code, 200);
    assert!(!result.content.is_empty());
    assert!(!result.truncated);
    assert!(result.content.contains("Melville") || result.content.contains("Moby"));
}

#[tokio::test]
async fn test_fetch_with_redirect() {
    let fetcher = WebFetcher::with_client(FakeHttpClient::new().with_response(
        "https://example.com/redirect",
        HttpResponse {
            final_url: "https://example.com/html".to_string(),
            status_code: 200,
            body: html_page("Redirect Target", "<h1>Redirected</h1>"),
            content_type: Some("text/html".to_string()),
        },
    ));
    let result = fetcher
        .fetch(WebFetchInput { url: "https://example.com/redirect".to_string(), prompt: None, timeout: Some(10_000) })
        .await
        .unwrap();

    assert_eq!(result.status_code, 200);
    assert_eq!(result.final_url, "https://example.com/html");
}

#[tokio::test]
async fn test_fetch_http_upgrades_to_https() {
    let fetcher = WebFetcher::with_client(
        FakeHttpClient::new().with_html("https://example.com/html", &html_page("Upgraded", "<h1>HTTPS</h1>")),
    );
    let result = fetcher
        .fetch(WebFetchInput { url: "http://example.com/html".to_string(), prompt: None, timeout: Some(10_000) })
        .await
        .unwrap();

    assert_eq!(result.final_url, "https://example.com/html");
}

#[tokio::test]
async fn test_fetch_timeout() {
    let fetcher = WebFetcher::with_client(FakeHttpClient::new().with_timeout("https://example.com/delay", 1000));
    let result = fetcher
        .fetch(WebFetchInput { url: "https://example.com/delay".to_string(), prompt: None, timeout: Some(1000) })
        .await;

    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), WebFetchError::Timeout(_)));
}

#[tokio::test]
async fn test_fetch_invalid_url() {
    let fetcher = WebFetcher::new();
    // Use a URL with invalid characters that can't be parsed
    let result =
        fetcher.fetch(WebFetchInput { url: "https://[invalid".to_string(), prompt: None, timeout: None }).await;

    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), WebFetchError::InvalidUrl(_)));
}

#[tokio::test]
async fn test_fetch_with_prompt() {
    let fetcher = WebFetcher::with_client(
        FakeHttpClient::new().with_html("https://example.com/html", &html_page("Prompt", "<h1>Main heading</h1>")),
    );
    // The prompt is currently just for documentation, but we should handle it gracefully
    let result = fetcher
        .fetch(WebFetchInput {
            url: "https://example.com/html".to_string(),
            prompt: Some("Extract the main heading".to_string()),
            timeout: Some(10_000),
        })
        .await
        .unwrap();

    assert_eq!(result.status_code, 200);
    assert!(!result.content.is_empty());
}

#[tokio::test]
async fn test_fetch_non_existent_host() {
    let fetcher = WebFetcher::with_client(
        FakeHttpClient::new()
            .with_request_failed("https://this-domain-definitely-does-not-exist-12345.com/", "dns error"),
    );
    let result = fetcher
        .fetch(WebFetchInput {
            url: "https://this-domain-definitely-does-not-exist-12345.com".to_string(),
            prompt: None,
            timeout: Some(5000),
        })
        .await;

    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), WebFetchError::RequestFailed(_)));
}

#[tokio::test]
async fn test_fetcher_reusable() {
    // Test that a single WebFetcher can be reused for multiple requests without relying on an external service.
    let client = FakeHttpClient::new()
        .with_html("https://example.com/page1", "<html><body><h1>Page 1</h1></body></html>")
        .with_html("https://example.com/page2", "<html><body><h1>Page 2</h1></body></html>");
    let fetcher = WebFetcher::with_client(client);

    let result1 = fetcher
        .fetch(WebFetchInput { url: "https://example.com/page1".to_string(), prompt: None, timeout: Some(10_000) })
        .await
        .unwrap();

    let result2 = fetcher
        .fetch(WebFetchInput { url: "https://example.com/page2".to_string(), prompt: None, timeout: Some(10_000) })
        .await
        .unwrap();

    assert_eq!(result1.status_code, 200);
    assert_eq!(result2.status_code, 200);
    assert!(result1.content.contains("Page 1"));
    assert!(result2.content.contains("Page 2"));
}

struct WebFetchTest {
    response: HttpResponse,
}

impl WebFetchTest {
    fn new(body: impl Into<String>) -> Self {
        Self {
            response: HttpResponse {
                final_url: "https://example.com/".to_string(),
                status_code: 200,
                body: body.into(),
                content_type: Some("text/html".to_string()),
            },
        }
    }

    fn url(mut self, url: &str) -> Self {
        self.response.final_url = url.to_string();
        self
    }

    fn content_type(mut self, content_type: Option<&str>) -> Self {
        self.response.content_type = content_type.map(str::to_owned);
        self
    }

    async fn fetch(self) -> WebFetchOutput {
        let url = self.response.final_url.clone();
        let client = FakeHttpClient::new().with_response(&url, self.response);
        WebFetcher::with_client(client).fetch(WebFetchInput { url, prompt: None, timeout: None }).await.unwrap()
    }
}
