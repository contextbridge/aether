Messages sent from an [`LspClient`](crate::LspClient) to the [`LspDaemon`](crate::LspDaemon).

The daemon protocol uses length-prefixed JSON frames over a Unix domain socket. Each frame is a 4-byte big-endian length prefix followed by a JSON-serialized `DaemonRequest` or [`DaemonResponse`]. The maximum frame size is [`MAX_MESSAGE_SIZE`] (16 MB).

# Request lifecycle

```text
Client                          Daemon
  ── Initialize ──────────────►
  ◄─────────────── Initialized ─
  ── LspCall ─────────────────►
  ◄──────────────── LspResult ──
  ── GetDiagnostics ──────────►
  ◄──────────────── LspResult ──
  ── Disconnect ──────────────►
```

# Variants

- **`Initialize`** -- Handshake that specifies the workspace root and language. Must be the first message. Contains an [`InitializeRequest`].
- **`LspCall`** -- Forward an LSP method call to the language server. Each call carries a `client_id` for response correlation.
- **`GetDiagnostics`** -- Retrieve cached diagnostics. Pass `uri: None` to get diagnostics for all files in the workspace.
- **`QueueDiagnosticRefresh`** -- Ask the daemon to re-check a file and update its diagnostics cache.
- **`Disconnect`** -- Graceful client disconnect.
- **`Ping`** -- Keepalive check; the daemon responds with [`DaemonResponse::Pong`].

# Response types

[`DaemonResponse`] variants:
- **`Initialized`** -- The handshake succeeded.
- **`Pong`** -- Reply to `Ping`.
- **`LspResult`** -- The result of an `LspCall` or `GetDiagnostics`, containing either a JSON `Value` or an [`LspErrorResponse`].
- **`Error`** -- A [`ProtocolError`] for protocol-level failures.

# Helper function

[`extract_document_uri`] extracts the target file URI from an LSP request's params, used by the daemon to auto-open documents before forwarding requests.
