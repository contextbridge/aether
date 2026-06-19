Text/pattern search using ripgrep.

**For code structure (definitions, usages, types), use `lsp_symbol` instead.** Grep is for string literals, TODOs, logs, and non-code files.

## Usage

```json
{"pattern": "TODO|FIXME"}
{"pattern": "error.*failed", "outputMode": "content", "lineNumbers": true}
{"pattern": "impl Handler", "type": "rust"}
{"pattern": "api_key", "glob": "*.ts", "caseInsensitive": true}
{"pattern": "struct \\{[\\s\\S]*?field", "multiline": true}
```

- `pattern` — **required**, regex pattern to search for
- `path` — absolute path to file or directory (defaults to cwd)
- `outputMode` — `content` (default, shows matching lines), `filesWithMatches` (file paths only), or `count` (match counts per file)
- `type` — file type filter (`js`, `py`, `rust`, etc.)
- `glob` — glob pattern filter (e.g., `*.ts`, `*.{js,jsx}`)
- `caseInsensitive` — case insensitive search (default: false)
- `lineNumbers` — show line numbers (content mode only, default: true)
- `contextBefore` — lines before each match (content mode only)
- `contextAfter` — lines after each match (content mode only)
- `contextAround` — lines before and after each match (content mode only, overrides contextBefore/contextAfter)
- `headLimit` — limit output to first N entries
- `multiline` — for cross-line patterns where `.` matches newlines
