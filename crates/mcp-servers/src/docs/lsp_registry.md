Holds one LSP daemon client per workspace language server for code intelligence.

The registry connects to a shared `clankerdiff-lspd` daemon that manages LSP servers for each language. This avoids spawning duplicate LSP servers when running multiple agents concurrently -- all agents share the same daemon.

# Architecture

1. A single `clankerdiff-lspd` daemon runs per machine, managing LSP servers (rust-analyzer, TypeScript 7's `tsc --lsp`, etc.).
2. The registry lazily creates one client per language server on first use; languages served by one server, such as TypeScript and TSX, share it.
3. Each client connects to its daemon on its first request and reconnects if the daemon goes away.

# Construction

```rust,ignore
use mcp_servers::lsp::LspRegistry;
use std::sync::Arc;

let registry = LspRegistry::new("/my/project".into());

// With immediate background spawning for detected languages
let registry: Arc<LspRegistry> = LspRegistry::new_and_spawn("/my/project".into());
```

[`new_and_spawn`](LspRegistry::new_and_spawn) wraps the registry in an `Arc` and immediately kicks off background LSP server spawning for languages detected in the project, so indexing starts right away.

# Key methods

- [`resolve_symbol`](LspRegistry::resolve_symbol) -- Locate a symbol in a file and select the client for its language.
- [`collect_diagnostics`](LspRegistry::collect_diagnostics) -- Diagnostics for one file, or for every connected client.
- [`queue_diagnostic_refresh`](LspRegistry::queue_diagnostic_refresh) -- Ask a file's language server to recompute its diagnostics.
- [`spawn_project_lsps`](LspRegistry::spawn_project_lsps) -- Connect to the daemon for every language detected in the project.

# See also

- [`CodingMcp`](crate::CodingMcp) -- Uses this registry when constructed with [`with_lsp`](crate::CodingMcp::with_lsp).
- [`LspError`](crate::lsp::error::LspError) -- Error types for LSP operations.
