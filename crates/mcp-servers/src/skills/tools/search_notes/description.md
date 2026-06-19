Searches notes by topic name or tag.

## Usage

```json
{"query": "agent-spec"}
```

- `query` — **required**, search term matched against topic names (substring) and tags (exact match)

## Behavior

- Scans all note files in the notes directory
- Case-insensitive matching
- Returns all matching notes with full content
