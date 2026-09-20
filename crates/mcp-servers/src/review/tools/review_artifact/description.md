Use this tool to request feedback from the user. You submit markdown (or a markdown file), and the response user's line-level feedback is returned in the response.

## Usage

```json
{"source": {"type": "content", "content": "# Question\n\nShould we use Postgres or DynamoDB?"}, "title": "Choose a database"}
{"source": {"type": "file", "path": "docs/aether/plans/feature.md"}}
```

`source` is required and tagged by `type`:

- `content` — include Markdown in `source.content`. No temporary file is created.
- `file` — include an existing path in `source.path`. Relative paths resolve against the configured workspace root.
- `title` — optional top-level display title for either source. Inline content defaults to `Review`.

**Returns:** `approved` (no feedback) -- proceed, `feedback` -- address the user's feedback and call this tool again, or `cancelled`/`declined` (user didn't respond).

## Tips

Call this tool when:

- You write a plan file to the filesystem and want the user to give feedback on the plan.
- You want feedback on something that materially affects the trajectory of the task at hand, e.g. there's multiple high level optinos the user should make a decision on (e.g. postgres vs mysql vs dynamoDB). 
- If discussing code, it's often helpful to include markdown code fences and show the user high-level interfaces and/or schemas.
