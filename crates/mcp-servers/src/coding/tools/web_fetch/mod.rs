mod http_client;

pub use http_client::{
    DEFAULT_TIMEOUT_MS, HttpClient, HttpResponse, MAX_TIMEOUT_MS, ReqwestClient, WebFetchInput, WebFetchOutput,
};

use dom_smoothie::Readability;
use htmd::convert;
use reqwest::Url;
use std::time::Duration;

use crate::coding::error::WebFetchError;
use utils::display_meta::{ToolDisplayMeta, truncate};

/// HTTP client for fetching web content and converting to markdown.
pub struct WebFetcher {
    client: Box<dyn HttpClient>,
}

impl Default for WebFetcher {
    fn default() -> Self {
        Self::new()
    }
}

impl WebFetcher {
    /// Creates a new `WebFetcher` with a preconfigured reqwest client
    pub fn new() -> Self {
        Self::with_client(ReqwestClient::new())
    }

    /// Creates a `WebFetcher` with a custom HTTP client (useful for testing)
    pub fn with_client(client: impl HttpClient + 'static) -> Self {
        Self { client: Box::new(client) }
    }

    /// Fetches web content and converts it to markdown
    pub async fn fetch(&self, args: WebFetchInput) -> Result<WebFetchOutput, WebFetchError> {
        let url = normalize_url(&args.url)?;

        let timeout_ms = args.timeout.map_or(DEFAULT_TIMEOUT_MS, |t| t.min(MAX_TIMEOUT_MS));

        let response = self.client.fetch(&url, Duration::from_millis(timeout_ms)).await?;

        let is_html = response.content_type.as_deref().is_none_or(|content_type| {
            let mime = content_type.split(';').next().unwrap_or_default().trim();
            mime.eq_ignore_ascii_case("text/html") || mime.eq_ignore_ascii_case("application/xhtml+xml")
        });
        let extracted = if is_html {
            extract_content(&response.body, &response.final_url)
        } else {
            ExtractedContent { title: None, markdown: response.body, byline: None }
        };

        let display_meta =
            ToolDisplayMeta::new("Fetch URL", extracted.title.clone().unwrap_or_else(|| truncate(&url, 60)));

        Ok(WebFetchOutput {
            content: extracted.markdown,
            final_url: response.final_url,
            status_code: response.status_code,
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
    let article = Readability::new(html, Some(url), None)
        .ok()
        .and_then(|mut reader| reader.parse().ok())
        .filter(|article| !article.content.is_empty());

    match article {
        Some(article) => ExtractedContent {
            markdown: convert(&article.content).unwrap_or_else(|_| article.text_content.to_string()),
            title: non_empty(article.title),
            byline: article.byline,
        },
        None => ExtractedContent {
            title: extract_title(html),
            markdown: convert(html).unwrap_or_else(|_| html.to_string()),
            byline: None,
        },
    }
}

fn normalize_url(url: &str) -> Result<String, WebFetchError> {
    let url = if url.starts_with("http://") {
        url.replacen("http://", "https://", 1)
    } else if !url.starts_with("https://") {
        format!("https://{url}")
    } else {
        url.to_string()
    };

    Url::parse(&url).map(|u| u.to_string()).map_err(WebFetchError::InvalidUrl)
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
