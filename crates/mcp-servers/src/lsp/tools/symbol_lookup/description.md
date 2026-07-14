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
Optional: `line` (1-indexed optimization hint; stale hints fall back to automatic resolution)

```json
{"operation": "definition", "file_path": "/path/to/file.rs", "symbol": "HashMap"}
{"operation": "references", "file_path": "/path/to/file.rs", "symbol": "process_request"}
{"operation": "incoming_calls", "file_path": "/path/to/file.rs", "symbol": "process_request", "limit": 20}
```

## Output Control

- **`limit`** — cap results. Use for `incoming_calls`/`outgoing_calls` on large functions.
- **`context_lines`** — include N lines around definition, implementation, reference, and call-site locations. Eliminates the need for a separate `read_file` call.
- **`include_declaration`** — for `references` only (default: true)
- **`callScope`** — for call hierarchy: `project` (default) filters dependencies; `all` includes them.

Call hierarchy results are deduplicated and project-local entries are sorted first. Each item includes `projectLocal` and a compact `displayPath`; pnpm store encodings are collapsed for readability. Call-site context is included only when `contextLines` is requested.

## Tips

- **Cross-crate navigation:** Use `definition` on an import to jump directly into dependency source — no need to manually navigate `~/.cargo/registry/...`.
- **`outgoing_calls` noise:** Use `callScope: "all"` only when dependency and standard-library calls are useful; the default returns project-local calls.
- **Workspace-wide search:** If you don't know which file a symbol is in, use `lsp_workspace_search` instead.
