**Use BEFORE reading large files.** Get the map, then read only what matters.

## Workflow

```text
lsp_document(file_path) → see all symbols & line numbers → read_file(offset, limit)
```

## Usage

```json
{"file_path": "/path/to/file.rs"}
```

Note: Read file first to establish LSP context.
