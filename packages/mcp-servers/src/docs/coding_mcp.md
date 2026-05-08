The primary MCP tool server for coding workflows. Provides file I/O, shell execution, regex search, glob-based file discovery, LSP code intelligence, and web fetch/search -- all exposed as MCP tools that an agent can invoke.

# Construction

Use the builder-style methods to configure the server:

```rust,ignore
use mcp_servers::CodingMcp;

// Default: local filesystem tools, no LSP
let server = CodingMcp::new();

// With LSP code intelligence and explicit read-rule directories
let server = CodingMcp::new()
    .with_rules_dirs(vec!["/my/project/.aether/skills".into(), "/my/project/.claude/rules".into()])
    .with_lsp("/my/project".into())
    .with_root_dir("/my/project".into())
    .with_permission_mode(mcp_servers::PermissionMode::Auto);
```

For custom tool backends, use [`CodingMcp::with_tools`]:

```rust,ignore
let server = CodingMcp::with_tools(my_custom_tools);
```

# Tools provided

**File operations:**
- `read_file` -- Read file contents with optional offset/limit and line numbers
- `write_file` -- Write content to a file (creates parent dirs)
- `edit_file` -- Line-numbered edits in an existing file
- `list_files` -- List directory contents with metadata

**Shell & search:**
- `bash` -- Execute shell commands (with background process support)
- `read_background_bash` -- Read output from a running background process
- `grep` -- Regex search across files with glob filters
- `find` -- Find files by glob pattern

**Web:**
- `web_fetch` -- Fetch a URL and extract text content
- `web_search` -- Web search via Brave Search API

**LSP** (requires [`with_lsp`](CodingMcp::with_lsp)):
- `lsp_symbol` -- Go to definition, find references, find implementations
- `lsp_document` -- List symbols in a file
- `lsp_check_errors` -- Get diagnostics for a file or workspace
- `lsp_rename` -- Rename a symbol across the project

# Safety

The server enforces several safety checks:

- **Read-before-edit** -- `write_file` and `edit_file` require the file to have been read first in the same session, preventing blind overwrites.
- **Dangerous command detection** -- Bash commands containing destructive patterns (`rm`, `git push --force`, redirect operators, etc.) are flagged.
- **Permission modes** -- [`PermissionMode`] controls whether flagged operations require user approval via MCP elicitation.

# See also

- [`CodingTools`] -- Trait abstracting the tool backend (implement for custom environments)
- [`DefaultCodingTools`] -- The default local-filesystem implementation
- [`PermissionMode`] -- User approval settings
- [`LspRegistry`](crate::lsp::LspRegistry) -- LSP daemon connection manager
- [`CodingError`](crate::coding::error::CodingError) -- Error types for tool operations
