Finds files by name pattern.

## Usage

```json
{"pattern": "**/*.rs"}
{"pattern": "*.test.ts", "path": "/path/to/search"}
```

- `pattern` — **required**, glob pattern (e.g., `**/*.js`, `config*.json`)
- `path` — directory to search (default: cwd)

**Returns:** matching file paths, sorted alphabetically.

## Tips

- Run multiple searches in parallel when exploring
- For open-ended searches requiring multiple rounds, consider spawning a sub-agent
