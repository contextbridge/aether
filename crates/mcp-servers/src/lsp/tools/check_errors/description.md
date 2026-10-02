Gets instant compiler errors and warnings without running a build.

**Prefer this over `cargo check`, `npm run build`, `tsc`, `go build`.**

## Usage

The tool infers scope from `filePath`:

**Workspace-wide diagnostics:**

```json
{}
```

**Single-file diagnostics:**

```json
{"filePath":"src/main.rs"}
```

## Parameters

- `filePath` — optional path to an existing file, absolute or relative to the workspace root. When omitted, checks the workspace.

If the required language server cannot start, the tool returns the startup error. For a missing TypeScript server, the error includes local and global npm installation commands.
