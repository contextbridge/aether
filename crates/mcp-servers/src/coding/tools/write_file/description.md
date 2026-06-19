Writes a file to the filesystem, replacing entire contents. Creates parent directories if needed.

## Usage

```json
{"filePath": "/path/to/new_file.rs", "content": "fn main() {}"}
```

- `filePath` — **required**, absolute path
- `content` — **required**, full file contents

## Tips

- Prefer `edit_file` for existing files — only use `write_file` for new files or complete rewrites
- Don't proactively create documentation (*.md, README) unless explicitly requested

## Safety

If the file exists, you MUST read it with `read_file` first. This prevents accidental data loss. New files don't require reading.
