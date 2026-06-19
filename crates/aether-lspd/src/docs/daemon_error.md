Errors from the daemon (server) side of `aether-lspd`.

# Variants

- **`Io`** -- A general IO error.
- **`BindFailed`** -- Failed to bind the Unix domain socket (e.g. address already in use, permission denied).
- **`LspSpawnFailed`** -- Failed to spawn a language server subprocess.
- **`LockfileError`** -- Failed to acquire the lockfile for the socket path, typically because another daemon instance is already running.
