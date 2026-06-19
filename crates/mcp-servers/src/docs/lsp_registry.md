Manages LSP daemon clients with lazy per-language connection for code intelligence.

The registry connects to a shared `aether-lspd` daemon that manages LSP servers for each language. This avoids spawning duplicate LSP servers when running multiple agents concurrently -- all agents share the same daemon.

# Architecture

1. A single `aether-lspd` daemon runs per machine, managing LSP servers (rust-analyzer, typescript-language-server, etc.).
2. The registry lazily connects to the daemon on first access for each language.
3. Connections are cached and reused for subsequent requests.

# Construction

```rust,ignore
use mcp_servers::lsp::LspRegistry;
use std::sync::Arc;

// Basic: connect lazily on demand
let registry = LspRegistry::new("/my/project".into());

// With immediate background spawning for detected languages
let registry: Arc<LspRegistry> = LspRegistry::new_and_spawn("/my/project".into());
```

[`new_and_spawn`](LspRegistry::new_and_spawn) wraps the registry in an `Arc` and immediately kicks off background LSP server spawning for languages detected in the project, so indexing starts right away.

# Key methods

- [`get_or_spawn`](LspRegistry::get_or_spawn) -- Get or create a client for the language of a given file path.
- [`get_or_spawn_for_language`](LspRegistry::get_or_spawn_for_language) -- Get or create a client for a specific language ID.
- [`active_clients`](LspRegistry::active_clients) -- List all currently connected LSP clients.
- [`has_config_for`](LspRegistry::has_config_for) -- Check if the daemon has an LSP config for a file's language.

# See also

- [`CodingMcp`](crate::CodingMcp) -- Uses this registry when constructed with [`with_lsp`](crate::CodingMcp::with_lsp).
- [`LspError`](crate::lsp::error::LspError) -- Error types for LSP operations.
