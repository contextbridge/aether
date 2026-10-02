Tools powered by LSP a server.

## Operations

| Query | Operation |
|-------|-----------|
| "Where is X defined?" (including in dependencies) | `definition` |
| "Where is X used?" | `references` |
| "What type is X?" | `hover` |

## Usage

Required: `file_path` (absolute or relative to the workspace root), `symbol` (exact name as it appears)
Optional: `line` (1-indexed optimization hint; stale hints fall back to automatic resolution)

```json
{"operation": "definition", "file_path": "src/main.rs", "symbol": "HashMap"}
{"operation": "references", "file_path": "src/main.rs", "symbol": "process_request"}
```

## Output Control

- **`limit`** — cap locations (default: 50). `totalCount` always reports the full count.
- **`context_lines`** — include N lines around definition and reference locations. Eliminates the need for a separate `read_file` call.
- **`include_declaration`** — for `references` only (default: true)

## Tips

- **Cross-crate navigation:** Use `definition` on an import to jump directly into dependency source — no need to manually navigate `~/.cargo/registry/...`.
