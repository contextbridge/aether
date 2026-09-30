Abstraction layer for file I/O and shell operations used by [`CodingMcp`](crate::CodingMcp).

Implement this trait to provide a custom backend -- for example, a sandboxed filesystem, remote execution, or an in-memory fake for testing. LSP operations are handled separately via [`LspRegistry`](crate::lsp::LspRegistry).

# Methods

**Required** (no default implementation):

- **`read_file`** -- Read a file's contents with optional offset and line limit.
- **`write_file`** -- Write content to a file, creating parent directories as needed.
- **`edit_file`** -- Replace a string pattern in an existing file.
- **`bash`** -- Execute a shell command, returning stdout/stderr and exit code. Supports background execution.

# See also

- [`DefaultCodingTools`](crate::DefaultCodingTools) -- The default implementation using the local filesystem and system shell.
- [`CodingMcp`](crate::CodingMcp) -- The MCP server that wraps this trait.
