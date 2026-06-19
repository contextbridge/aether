Lists files and directories in a path with detailed metadata.

## Usage

```json
{"path": "/absolute/path"}
{"path": "/path", "include_hidden": true}
```

- `path` — directory to list (default: current directory)
- `include_hidden` — include files starting with '.' (default: false)

**Returns:** name, path, type (file/directory/symlink), size, permissions, modification time. Sorted alphabetically.
