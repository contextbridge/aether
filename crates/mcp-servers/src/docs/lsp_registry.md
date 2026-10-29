Holds one LSP daemon client per workspace language server for code intelligence.

The registry connects to a shared `clankerdiff-lspd` daemon that manages LSP servers for each language. This avoids spawning duplicate LSP servers when running multiple agents concurrently -- all agents share the same daemon.

# Architecture

1. A single `clankerdiff-lspd` daemon runs per machine, managing LSP servers (rust-analyzer, TypeScript 7's `tsc --lsp`, etc.).
2. The registry creates one client per language server; languages served by one server, such as TypeScript and TSX, share it.
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

- [`client_for`](LspRegistry::client_for) -- The client for the language of a given file path.
- [`client`](LspRegistry::client) -- The client for a specific language ID.
- [`active_clients`](LspRegistry::active_clients) -- List all currently connected LSP clients.

# See also

- [`CodingMcp`](crate::CodingMcp) -- Uses this registry when constructed with [`with_lsp`](crate::CodingMcp::with_lsp).
- [`LspError`](crate::lsp::error::LspError) -- Error types for LSP operations.
