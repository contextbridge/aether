Searches the web via Brave Search API for up-to-date information.

## Usage

```json
{"query": "rust async best practices 2024"}
{"query": "tokio documentation", "count": 5, "allowed_domains": ["docs.rs"]}
```

- `query` — **required**, search query (be specific)
- `count` — max results (default: 10, max: 20)
- `allowed_domains` — only include these domains
- `blocked_domains` — exclude these domains

## Tips

- Use snippets to assess relevance before fetching full pages
- Always cite sources using the returned URLs
- Use `web_fetch` to get full content from promising results
