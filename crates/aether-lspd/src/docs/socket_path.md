Generate a deterministic Unix socket path for a workspace and language.

The path is derived from a SHA-256 hash of the canonical workspace root, combined with the language server identity (not the language itself -- languages sharing a server get the same socket).

# Path format

```text
{socket_dir}/lsp-{server}-v{protocol_version}-{hash}.sock
```

Where `socket_dir` is `$XDG_RUNTIME_DIR/aether-lspd` if available, or `/tmp/aether-lspd-{uid}` otherwise. The UID suffix prevents permission conflicts between users on shared machines. The protocol version keeps a newer client from connecting to an older daemon that is still running.

The daemon keeps its lockfile and log next to the socket, with `.lock` and `.log` extensions.
