Renames a symbol across the entire workspace using LSP-powered refactoring.

A single rename updates all references — no manual file-by-file editing needed.

## Usage

```json
{"file_path": "/project/src/lib.rs", "symbol": "old_name", "new_name": "better_name"}
```

- `file_path` — **required**, file containing the symbol
- `symbol` — **required**, current symbol name
- `new_name` — **required**, new symbol name
- `line` — optional 1-indexed optimization hint; stale hints fall back to automatic resolution

**Returns:** files affected, total edit count, and the edited lines grouped by file (relative to the workspace root): `src/lib.rs: 1, 14`.
