Errors that can occur when using [`LspClient`].

# Connection errors

- **`ConnectionFailed`** -- Failed to connect to the daemon's Unix socket.
- **`SpawnFailed`** -- The client tried to auto-spawn a daemon process but the spawn failed.
- **`SpawnTimeout`** -- A daemon was spawned but didn't become ready within the timeout window.
- **`DaemonBinaryNotFound`** -- The `aether-lspd` binary could not be found on `PATH` or in known locations.
- **`InitializationFailed`** -- Connected to the daemon but the `Initialize` handshake was rejected.

# Runtime errors

- **`Io`** -- An IO error on the Unix socket connection.
- **`DaemonError`** -- The daemon returned a protocol-level error.
- **`ProtocolError`** -- A framing or serialization error in the daemon protocol.

# LSP errors

- **`LspError`** -- The language server returned an error response. Contains the LSP error `code` and `message` (e.g. "method not supported", "request failed").
