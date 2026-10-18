Messages sent from an [`LspClient`](crate::LspClient) to the daemon.

The daemon protocol uses length-prefixed JSON frames over a Unix domain socket. Each frame is a 4-byte big-endian length prefix followed by a JSON-serialized `DaemonRequest` or [`DaemonResponse`]. The protocol is private to this crate: [`PROTOCOL_VERSION`] is part of every socket name, so a client never talks to a daemon built from a different version.

# Request lifecycle

```text
Client                          Daemon
  ── Initialize ──────────────►
  ◄─────────────── Initialized ─
  ── Call { id: 1, Lsp } ─────►
  ── Call { id: 2, Diagnostics }►
  ◄────────── Reply { id: 1 } ──
  ◄────────── Reply { id: 2 } ──
  ── Disconnect ──────────────►
```

# Variants

- **`Initialize`** -- Handshake that binds the connection to a workspace root and language. Must be the first message. The daemon answers with `Initialized` or `InitializeFailed`.
- **`Call`** -- A [`DaemonCall`] tagged with a client-chosen `id`. Calls on one connection run concurrently and their replies can arrive in any order, so a diagnostics wait never blocks a hover.
- **`Disconnect`** -- Graceful client disconnect.
