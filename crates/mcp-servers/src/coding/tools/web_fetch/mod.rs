mod http_client;

#[cfg(test)]
pub use http_client::FakeHttpClient;
pub use http_client::{
    DEFAULT_TIMEOUT_MS, HttpClient, HttpResponse, MAX_CONTENT_LENGTH, MAX_TIMEOUT_MS, ReqwestClient, WebFetchInput,
    WebFetchOutput,
};

use dom_smoothie::Readability;
use htmd::convert;
use reqwest::Url;
use std::time::Duration;

use crate::coding::error::WebFetchError;
use mcp_utils::display_meta::{ToolDisplayMeta, truncate};

/// HTTP client for fetching web content and converting to markdown.
#[derive(Debug, Clone)]
pub struct WebFetcher<C: HttpClient = ReqwestClient> {
    client: C,
}

impl Default for WebFetcher<ReqwestClient> {
    fn default() -> Self {
        Self::new()
    }
}

impl WebFetcher<ReqwestClient> {
    /// Creates a new `WebFetcher` with a preconfigured reqwest client
    pub fn new() -> Self {
        Self { client: ReqwestClient::new() }
    }
}

impl<C: HttpClient> WebFetcher<C> {
    /// Creates a `WebFetcher` with a custom HTTP client (useful for testing)
    pub fn with_client(client: C) -> Self {
        Self { client }
    }

    /// Fetches web content and converts it to markdown
    pub async fn fetch(&self, args: WebFetchInput) -> Result<WebFetchOutput, WebFetchError> {
        let url = normalize_url(&args.url)?;

        let timeout_ms = args.timeout.map_or(DEFAULT_TIMEOUT_MS, |t| t.min(MAX_TIMEOUT_MS));

        let response = self.client.fetch(&url, Duration::from_millis(timeout_ms)).await?;

        let extracted = extract_content(&response.body, &url);
        let (content, truncated) = if extracted.markdown.len() > MAX_CONTENT_LENGTH {
            (truncate_str(&extracted.markdown, MAX_CONTENT_LENGTH), true)
        } else {
            (extracted.markdown, false)
        };

        let display_meta =
            ToolDisplayMeta::new("Fetch URL", extracted.title.clone().unwrap_or_else(|| truncate(&url, 60)));

        Ok(WebFetchOutput {
            content,
            final_url: response.final_url,
            status_code: response.status_code,
            truncated,
            title: extracted.title,
            byline: extracted.byline,
            meta: Some(display_meta.into()),
        })
    }
}

struct ExtractedContent {
    title: Option<String>,
    markdown: String,
    byline: Option<String>,
}

fn extract_content(html: &str, url: &str) -> ExtractedContent {
    if let Some(extracted) = try_readability(html, url) {
        return extracted;
    }
    fallback_extract(html)
}

fn try_readability(html: &str, url: &str) -> Option<ExtractedContent> {
    let mut reader = Readability::new(html, Some(url), None).ok()?;
    let article = reader.parse().ok()?;

    if article.content.is_empty() {
        return None;
    }

    let markdown = convert(&article.content).unwrap_or_else(|_| article.text_content.to_string());
    let title = non_empty(article.title);

    Some(ExtractedContent { title, markdown, byline: article.byline })
}

fn fallback_extract(html: &str) -> ExtractedContent {
    let title = extract_title(html);
    let markdown = convert(html).unwrap_or_else(|_| html.to_string());
    ExtractedContent { title, markdown, byline: None }
}

fn normalize_url(url: &str) -> Result<String, WebFetchError> {
    let url = if url.starts_with("http://") {
        url.replacen("http://", "https://", 1)
    } else if !url.starts_with("https://") {
        format!("https://{url}")
    } else {
        url.to_string()
    };

    Url::parse(&url).map(|u| u.to_string()).map_err(|e| WebFetchError::InvalidUrl(e.to_string()))
}

fn extract_title(html: &str) -> Option<String> {
    let lower = html.to_lowercase();
    let start = lower.find("<title>")?;
    let end = lower.find("</title>")?;
    if start >= end {
        return None;
    }
    non_empty(html[start + 7..end].trim().to_string())
}

fn non_empty(s: String) -> Option<String> {
    if s.trim().is_empty() { None } else { Some(s) }
}

fn truncate_str(content: &str, max_len: usize) -> String {
    if content.len() <= max_len {
        return content.to_string();
    }

    let truncated = &content[..max_len];

    if let Some(last_para) = truncated.rfind("\n\n") {
        format!("{}\n\n[Content truncated...]", &truncated[..last_para])
    } else if let Some(last_newline) = truncated.rfind('\n') {
        format!("{}\n\n[Content truncated...]", &truncated[..last_newline])
    } else {
        format!("{truncated}\n\n[Content truncated...]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(url: &str) -> WebFetchInput {
        WebFetchInput { url: url.to_string(), prompt: None, timeout: None }
    }

    #[test]
    fn test_normalize_url() {
        let cases = [
            ("example.com", "https://example.com/"),
            ("http://example.com", "https://example.com/"),
            ("https://example.com", "https://example.com/"),
        ];
        for (input, expected) in cases {
            assert_eq!(normalize_url(input).unwrap(), expected, "input: {input}");
        }
        assert!(normalize_url("not a valid url!!!").is_err());
    }

    #[test]
    fn test_extract_title() {
        assert_eq!(extract_title("<html><head><title>Test Page</title></head></html>"), Some("Test Page".to_string()));
        assert_eq!(extract_title("<html><head></head><body></body></html>"), None);
        assert_eq!(extract_title("<html><head><title>  </title></head></html>"), None);
    }

    fn article_html(body: &str) -> String {
        format!(
            r#"<html><head><title>Test Article</title>
            <meta name="author" content="Jane Doe">
            <meta property="og:description" content="A test excerpt">
            </head><body>
            <nav><a href="/">Home</a><a href="/about">About</a></nav>
            <article>{body}</article>
            <footer><p>Copyright 2026</p></footer>
            </body></html>"#
        )
    }

    #[test]
    fn test_readability_extracts_article_content() {
        let body = "<h1>Main Article</h1>";
        let paragraphs: Vec<String> = (0..10)
            .map(|i| format!("<p>This is paragraph {i} with enough content to be considered substantial by the readability algorithm.</p>"))
            .collect();
        let html = article_html(&format!("{body}{}", paragraphs.join("")));

        let result = extract_content(&html, "https://example.com/article");

        assert!(result.markdown.contains("Main Article"));
        assert!(result.markdown.contains("paragraph 0"));
        assert!(!result.markdown.contains("Copyright 2026"), "Footer should be stripped by readability");
    }

    #[test]
    fn test_readability_extracts_metadata() {
        let paragraphs: Vec<String> =
            (0..10).map(|i| format!("<p>Paragraph {i} with enough content for readability to engage.</p>")).collect();
        let html = article_html(&paragraphs.join(""));

        let result = extract_content(&html, "https://example.com/article");

        assert!(result.title.is_some());
    }

    #[test]
    fn test_fallback_when_no_article_content() {
        let html = "<html><head><title>Sparse</title></head><body><p>Hi</p></body></html>";
        let result = extract_content(html, "https://example.com/sparse");

        assert_eq!(result.title, Some("Sparse".to_string()));
        assert!(result.markdown.contains("Hi"));
    }

    #[test]
    fn test_truncate_str() {
        assert_eq!(truncate_str("Short content", 100), "Short content");

        let content = "First paragraph.\n\nSecond paragraph.\n\nThird paragraph.";
        let result = truncate_str(content, 35);
        assert!(result.contains("First paragraph."));
        assert!(result.contains("[Content truncated...]"));
        assert!(!result.contains("Third paragraph"));
    }

    #[tokio::test]
    async fn test_fake_client_returns_configured_response() {
        let fake = FakeHttpClient::new().with_html(
            "https://example.com/",
            "<html><head><title>Example</title></head><body><h1>Hello</h1></body></html>",
        );
        let fetcher = WebFetcher::with_client(fake.clone());
        let result = fetcher.fetch(input("example.com")).await.unwrap();

        assert_eq!(result.status_code, 200);
        assert_eq!(result.title, Some("Example".to_string()));
        assert!(result.content.contains("Hello"));
        assert_eq!(fake.fetch_count(), 1);
    }

    #[tokio::test]
    async fn test_fake_client_tracks_fetch_history() {
        let fake = FakeHttpClient::new()
            .with_html("https://example.com/page1", "<h1>Page 1</h1>")
            .with_html("https://example.com/page2", "<h1>Page 2</h1>");
        let fetcher = WebFetcher::with_client(fake.clone());

        fetcher.fetch(input("https://example.com/page1")).await.unwrap();
        fetcher.fetch(input("https://example.com/page2")).await.unwrap();

        assert_eq!(fake.fetch_count(), 2);
        assert_eq!(fake.fetched_urls(), vec!["https://example.com/page1", "https://example.com/page2"]);
    }

    #[tokio::test]
    async fn test_fake_client_missing_url_returns_error() {
        let fetcher = WebFetcher::with_client(FakeHttpClient::new());
        assert!(fetcher.fetch(input("https://not-configured.com/")).await.is_err());
    }

    #[tokio::test]
    async fn test_fake_client_with_default_response() {
        let fake = FakeHttpClient::new().with_default(HttpResponse {
            final_url: "https://fallback.com/".to_string(),
            status_code: 404,
            body: "<h1>Not Found</h1>".to_string(),
        });
        let result = WebFetcher::with_client(fake).fetch(input("https://any-url.com/")).await.unwrap();
        assert_eq!(result.status_code, 404);
    }
}
