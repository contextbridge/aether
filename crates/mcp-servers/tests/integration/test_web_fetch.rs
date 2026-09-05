use mcp_servers::coding::error::WebFetchError;
use mcp_servers::coding::tools::web_fetch::{HttpResponse, WebFetchInput, WebFetcher};
use mcp_servers::testing::FakeHttpClient;

fn html_page(title: &str, body: &str) -> String {
    format!("<html><head><title>{title}</title></head><body>{body}</body></html>")
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
