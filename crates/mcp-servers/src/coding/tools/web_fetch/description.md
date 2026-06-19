Fetches content from a URL, extracts the main article content, and converts it to Markdown.

Uses a readability algorithm to strip navigation, sidebars, footers, and other non-content elements, giving you clean article text instead of the full page.

## Usage

```json
{"url": "https://docs.rs/serde"}
{"url": "https://example.com", "prompt": "Find API documentation", "timeout": 60000}
```

- `url` — **required**, HTTP/HTTPS URL (HTTP auto-upgraded)
- `prompt` — describe what you're looking for (optional)
- `timeout` — max wait in ms (default: 30000, max: 60000)

## Tips

- Use specific URLs rather than relying on search
- Handle 4xx/5xx status codes gracefully
- Falls back to full-page conversion when readability extraction fails
