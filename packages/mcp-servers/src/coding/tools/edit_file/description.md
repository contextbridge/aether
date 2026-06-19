Use this tool to edit a file. This tool accepts an array of batched exact-string replacements. If 1 edit fails, no edits in the batch are applied.

## Usage

```json
{"filePath": "/path/to/file.rs", "edits": [
  {"oldString": "foo", "newString": "bar"},
  {"oldString": "old_name", "newString": "new_name", "replaceAll": true}
]}
```

- `filePath` — **required**, absolute path
- `edits` — **required**, a non-empty array. Each edit replaces `oldString` with `newString`.
  - `oldString` — exact string to find (must be unique unless `replaceAll`).
  - `newString` — replacement text.
  - `replaceAll` — replace all occurrences (default: false).

## Tips

- Batch every edit to a file into one call; edits must target non-overlapping regions.
- Preserve exact indentation from `read_file` output — match text AFTER the tab character, not the line-number prefix.
- To rename symbols across the codebase, prefer `lsp_rename` instead (if available).
- Unless your edits require overwriting an entire file, prefer this tool over `create_file`.

## Safety

You MUST read a file with `read_file` before editing it. This prevents accidental data loss.
