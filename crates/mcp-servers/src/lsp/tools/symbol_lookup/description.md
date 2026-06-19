Code navigation powered by a LSP server. **Prefer this over `grep` and `find`**.

## Operations

| Query | Operation |
|-------|-----------|
| "Where is X defined?" | `definition` |
| "Where is X used?" | `references` |
| "What type is X?" | `hover` |
| "What implements this trait?" | `implementation` |
| "What calls X?" | `incoming_calls` |
| "What does X call?" | `outgoing_calls` |

## Usage

Required: `file_path`, `symbol` (exact name as it appears)
Optional: `line` (1-indexed, skips auto-resolution — faster)

```json
{"operation": "definition", "file_path": "/path/to/file.rs", "symbol": "HashMap"}
{"operation": "references", "file_path": "/path/to/file.rs", "symbol": "process_request"}
{"operation": "incoming_calls", "file_path": "/path/to/file.rs", "symbol": "process_request", "limit": 20}
```

## Output Control

- **`limit`** — cap results. Use for `incoming_calls`/`outgoing_calls` on large functions.
- **`context_lines`** — include N lines of source around each result (definition/implementation/references only). Eliminates need for `read_file`.
- **`include_declaration`** — for `references` only (default: true)

## Tips

- **Cross-crate navigation:** Use `definition` on an import to jump directly into dependency source — no need to manually navigate `~/.cargo/registry/...`.
- **`outgoing_calls` noise:** Returns ALL calls including stdlib/dep calls (`map_err`, `collect`, etc.). Use `limit` and filter by `file_path` for project-local calls.
- **Workspace-wide search:** If you don't know which file a symbol is in, use `lsp_workspace_search` instead.
