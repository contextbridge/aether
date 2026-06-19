# aether-lspd

An LSP daemon that manages language server processes and shares them across multiple Aether agents. Communicates over Unix domain sockets.

## Table of Contents

<!-- START doctoc generated TOC please keep comment here to allow auto update -->
<!-- DON'T EDIT THIS SECTION, INSTEAD RE-RUN doctoc TO UPDATE -->

- [Quick start](#quick-start)
- [Documentation](#documentation)
- [Key Types](#key-types)
- [Feature Flags](#feature-flags)
- [License](#license)

<!-- END doctoc generated TOC please keep comment here to allow auto update -->

## Quick start

Connect to a workspace, query hover info, and disconnect:

```rust,no_run
use aether_lspd::{LspClient, LanguageId, path_to_uri};
use std::path::Path;

#[tokio::main]
async fn main() -> aether_lspd::ClientResult<()> {
    let client = LspClient::connect(
        Path::new("/home/user/my-project"),
        LanguageId::Rust,
    ).await?;

    let uri = path_to_uri(Path::new("/home/user/my-project/src/main.rs")).unwrap();
    let hover = client.hover(uri, 10, 5).await?;

    client.disconnect().await?;
    Ok(())
}
```

The client auto-spawns a daemon process if one isn't already running. Multiple clients can share the same daemon for a given workspace and language.

## Documentation

Full API documentation is available on [docs.rs](https://docs.rs/aether-lspd).

Key entry points:
- [`LspClient`] -- connect to a daemon and make LSP requests
- [`LspDaemon`] -- the daemon runtime that manages language servers
- [`LanguageId`] -- supported languages and their LSP server configurations
- [`DaemonRequest`] / [`DaemonResponse`] -- the wire protocol between client and daemon

## Key Types

- **`LspClient`** -- Client for connecting to a running daemon. Supports go-to-definition, references, hover, diagnostics, rename, and more.
- **`LspDaemon`** -- Main daemon runtime. Listens on a Unix socket and manages language server lifecycles.
- **`DaemonRequest` / `DaemonResponse`** -- Protocol messages between client and daemon.
- **`LanguageId`** -- Supported language identifiers with associated LSP server configurations.

## Feature Flags

| Feature | Description |
|---------|-------------|
| `testing` | Test utilities for integration tests |

## License

MIT
