Abstraction layer for file I/O, shell, and search operations used by [`CodingMcp`](crate::CodingMcp).

Implement this trait to provide a custom backend -- for example, a sandboxed filesystem, remote execution, or an in-memory fake for testing. LSP operations are handled separately via [`LspRegistry`](crate::lsp::LspRegistry).

# Methods

**Required** (no default implementation):

- **`read_file`** -- Read a file's contents with optional offset and line limit.
- **`write_file`** -- Write content to a file, creating parent directories as needed.
- **`edit_file`** -- Apply line-numbered edits to an existing file.
- **`list_files`** -- List directory entries with metadata (size, type, modified time).
- **`bash`** -- Execute a shell command, returning stdout/stderr and exit code. Supports background execution.
- **`read_background_bash`** -- Read accumulated output from a running background process.

**Provided** (default implementations using standalone functions):

- **`grep`** -- Regex search across files. Delegates to [`perform_grep`](crate::coding::tools::grep::perform_grep).
- **`find`** -- Glob-based file discovery. Delegates to [`find_files_by_name`](crate::coding::tools::find::find_files_by_name).

# See also

- [`DefaultCodingTools`](crate::DefaultCodingTools) -- The default implementation using the local filesystem and system shell.
- [`CodingMcp`](crate::CodingMcp) -- The MCP server that wraps this trait.
