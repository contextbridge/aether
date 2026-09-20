Use this tool to request feedback from the user on an artifact. Submit Markdown or HTML, and the user's review is returned in the response.

## Usage

```json
{"format": "markdown", "source": {"type": "content", "content": "# Question\n\nShould we use Postgres or DynamoDB?"}, "title": "Choose a database"}
{"format": "markdown", "source": {"type": "file", "path": "docs/aether/plans/feature.md"}}
{"format": "html", "source": {"type": "content", "content": "<main><h1>Ship faster</h1></main>"}, "title": "Landing page"}
{"format": "html", "source": {"type": "file", "path": "site/index.html"}}
{"format": "html", "source": {"type": "url", "url": "http://localhost:5173/settings"}, "title": "Settings page"}
```

## Returns

`approved` — proceed. `feedback` — address the user's feedback and call this tool again. `cancelled`/`declined` — the user did not complete the review.

## Tips

Call this tool when:

- You write a plan or design document and want review before proceeding.
- You generate an HTML artifact (landing page, email, report) and want element-level feedback.
- You are iterating on a running web app and want the user to point at what to change.
- A decision materially affects the task's trajectory, e.g. choosing between Postgres and `DynamoDB`.
