The daemon runtime that manages language server processes and multiplexes client connections.

`LspDaemon` listens on a Unix domain socket and routes requests from multiple [`LspClient`](crate::LspClient) connections to the appropriate language server subprocess. Language servers are spawned on demand and shared across clients working in the same workspace. Requests on one connection run concurrently, so a slow diagnostics wait never blocks a hover. Start one with [`run_lspd`](crate::run_lspd).

# Lifecycle

1. **Bind** -- Acquires a lockfile and binds the Unix socket. Clients can connect as soon as `bind` returns.
2. **Listen** -- Accepts client connections and spawns a handler task per client.
3. **Shutdown** -- Triggered by any of: SIGTERM/SIGINT, idle timeout, or all workspace roots being deleted from disk. On shutdown the daemon stops all language servers and removes the socket file.

# Idle timeout

When `idle_timeout` is `Some`, the daemon shuts down after the specified duration with zero connected clients. Activity resets whenever a client connects or disconnects.

# Workspace liveness

The daemon periodically checks whether the workspace directories it manages still exist on disk. If every registered workspace root has been deleted, the daemon shuts down automatically.
