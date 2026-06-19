Reads a file from the local filesystem with line numbers.

## Usage

```json
{"file_path": "/absolute/path/to/file.rs"}
{"file_path": "/path/to/file.rs", "offset": 150, "limit": 50}
```

- `file_path` — **required**, must be absolute path
- `offset` — 1-indexed starting line (default: 1)
- `limit` — max lines to read (default: 2000)

Output format: `    1\tline content` (line number, tab, content)

## Tips

- Read the whole file by omitting `offset`/`limit` — use them only for large files
- Read multiple files in parallel when you need several
- Use `list_files` for directories, not this tool

## Safety

You MUST read a file before editing (`edit_file`) or overwriting (`write_file`). This prevents accidental data loss.
