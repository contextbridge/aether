Gets instant compiler errors and warnings without running a build.

**Prefer this over `cargo check`, `npm run build`, `tsc`, `go build`.**

## Usage

The tool has two explicit modes:

**Workspace-wide diagnostics:**

```json
{"input":{"scope":"workspace"}}
```

**Single-file diagnostics:**

```json
{"input":{"scope":"file","filePath":"/absolute/path/to/file.rs"}}
```

## Parameters

- `input` — **required**, wrapper object for the diagnostics query
- `input.scope` — **required**, either `"workspace"` or `"file"`
- `input.filePath` — required when `input.scope="file"`, must be an absolute path to an existing file
