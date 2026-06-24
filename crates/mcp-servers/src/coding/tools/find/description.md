Finds files by glob pattern.

## Usage

```json
{"pattern": "README*"}
{"pattern": "justfile"}
{"pattern": "tsconfig*.json"}
{"pattern": "**/*.rs"}
{"pattern": "crates/**/*.rs", "limit": 50}
{"pattern": "settings.json", "includeHidden": true}
{"pattern": "readme*", "caseInsensitive": true}
```

- `pattern` — **required**, glob pattern. Bare patterns like `README*`, `justfile`, and `*.rs` match file names recursively under `path`. Patterns containing `/`, such as `crates/**/*.rs`, match paths relative to `path`.
- `path` — directory to search (default: cwd)
- `limit` — maximum number of matches to return. When reached, the search stops early and `truncated` is set.
- `includeHidden` — include hidden files and directories (default: false)
- `caseInsensitive` — match patterns case-insensitively (default: false)

**Returns:** matching file paths, sorted alphabetically. `count` is the number returned, and `truncated` indicates the search stopped early because more matches were available.

## Tips

- Use bare patterns for common file-name searches like `README*`, `justfile`, `tsconfig*.json`, and `*.rs`
- Use slash-containing patterns like `crates/**/*.rs` when you need to constrain matches by directory
- Run multiple searches in parallel when exploring
- For open-ended searches requiring multiple rounds, consider spawning a sub-agent
