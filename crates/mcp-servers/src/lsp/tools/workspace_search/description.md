Searches for symbols across the entire workspace by name.

Use this when you don't know which file a symbol lives in. For definition/references/hover on a known file, use `lsp_symbol` instead.

## Usage

```json
{"query": "AppState", "language": "rust"}
{"query": "Repository", "language": "typescript", "limit": 10, "context_lines": 3}
```

- `query` — **required**, symbol name (fuzzy/substring matching)
- `language` — **required**, the single LSP language to query, such as `rust`, `typescript`, or `python`
- `limit` — cap results
- `context_lines` — include N lines of source around each result

## When to Use

- "Where is `AppState` defined?" (don't know the file)
- "Find all structs matching `Repository`"
- "Which module declares `process_request`?"

## Notes

- The requested language's LSP is the only server queried. To search another language, issue a separate call.
- Query matching is LSP-server dependent (typically fuzzy).
- Results are deduplicated.
- When the workspace index returns no matches, Aether searches candidate files for the requested language and validates matches with document symbols.
- Each result reports `source` as `workspaceSymbol` or `documentSymbolFallback`.
