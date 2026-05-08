Applies line-numbered edits to a file atomically.

## Usage

```json
{
  "filePath": "/path/to/file.rs",
  "edits": [
    { "type": "set_line", "line": 12, "newText": "let timeout = 30_000;" },
    { "type": "replace_lines", "startLine": 20, "endLine": 24, "newText": "replacement\nblock" },
    { "type": "delete_lines", "startLine": 28, "endLine": 28 },
    { "type": "delete_lines", "startLine": 31, "endLine": 33 },
    { "type": "insert_before", "line": 40, "text": "use std::collections::HashMap;" },
    { "type": "insert_after", "line": 44, "text": "new line\nsecond new line" }
  ]
}
```

- `filePath` — **required**, absolute path
- `edits` — **required**, non-empty array of line edit operations
- `set_line` — replace one line; `newText: ""` makes it blank
- `replace_lines` — replace an inclusive line range
- `delete_lines` — delete an inclusive line range; set `startLine` and `endLine` to the same value to delete one line
- `insert_before` — insert literal `text` before the line
- `insert_after` — insert literal `text` after the line

## Important

- Line numbers are 1-indexed. Use `read_file` to get file contents with line numbers.
- `newText` and `text` should be literal file content, not line-number-prefixed `read_file` output.
- Deletions must use the explicit `delete_lines` operation.
- Multiple edits are validated first and then applied atomically; partial success is not possible.
- Overlapping or ambiguous edits are rejected.
- For renaming symbols across the codebase, use `lsp_rename` instead.

## Safety

You MUST read a file with `read_file` before editing it. This prevents accidental data loss.
