Generate a deterministic Unix socket path for a workspace and language.

The path is derived from a SHA-256 hash of the canonical workspace root, combined with the language server identity (not the language itself -- languages sharing a server get the same socket).

# Path format

```text
{socket_dir}/lsp-{server}-{hash}.sock
```

Where `socket_dir` is `$XDG_RUNTIME_DIR/aether-lspd` if available, or `/tmp/aether-lspd-{uid}` otherwise. The UID suffix prevents permission conflicts between users on shared machines.

# Related functions

- [`ensure_socket_dir`] -- Creates the socket directory if needed and returns the socket path.
- [`lockfile_path`] -- Returns the `.lock` path for a given socket (used by the daemon for single-instance enforcement).
- [`log_file_path`] -- Returns the `.log` path for a given socket (used for daemon log output).
