Client for connecting to a running [`LspDaemon`](crate::LspDaemon) over a Unix domain socket.

`LspClient` is the primary entry point for consumers of this crate. Call [`connect`](LspClient::connect) with a workspace root and language -- the client will auto-spawn a daemon process if one isn't already running, then send an `Initialize` handshake.

# Connection lifecycle

```text
connect(workspace_root, language)
  → auto-spawn daemon if needed
  → Initialize handshake
  → ready for LSP requests
  → disconnect()
```

# LSP requests

All requests are forwarded to the underlying language server through the daemon. Each method accepts the same parameters as the corresponding LSP protocol method:

- [`goto_definition`](LspClient::goto_definition) -- Jump to a symbol's definition.
- [`find_references`](LspClient::find_references) -- Find all references to a symbol.
- [`hover`](LspClient::hover) -- Get hover information (type signature, docs).
- [`document_symbol`](LspClient::document_symbol) -- List all symbols in a document.
- [`rename`](LspClient::rename) -- Rename a symbol across the workspace.

# Diagnostics

- [`get_diagnostics`](LspClient::get_diagnostics) -- Retrieve cached diagnostics for a file (or all files if `uri` is `None`).
- [`queue_diagnostic_refresh`](LspClient::queue_diagnostic_refresh) -- Ask the daemon to re-check a file and update its diagnostics cache.

# Generic requests

[`call`](LspClient::call) sends an arbitrary LSP method with typed parameters and response. Use this for LSP methods that don't have a dedicated wrapper.

# Example

```rust,no_run
use aether_lspd::{LspClient, LanguageId, path_to_uri};
use std::path::Path;

# async fn example() -> aether_lspd::ClientResult<()> {
let client = LspClient::connect(
    Path::new("/home/user/my-project"),
    LanguageId::Rust,
).await?;

let uri = path_to_uri(Path::new("/home/user/my-project/src/main.rs")).unwrap();
let hover = client.hover(uri, 10, 5).await?;

client.disconnect().await?;
# Ok(())
# }
```
