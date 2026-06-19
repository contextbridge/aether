Searches for symbols across the entire workspace by name.

Use this when you don't know which file a symbol lives in. For definition/references/hover on a known file, use `lsp_symbol` instead.

## Usage

```json
{"query": "AppState"}
{"query": "Repository", "limit": 10, "context_lines": 3}
```

- `query` — **required**, symbol name (fuzzy/substring matching)
- `limit` — cap results
- `context_lines` — include N lines of source around each result

## When to Use

- "Where is `AppState` defined?" (don't know the file)
- "Find all structs matching `Repository`"
- "Which module declares `process_request`?"

## Notes

- Query matching is LSP-server dependent (typically fuzzy)
- Results deduplicated across language servers
