Errors from LSP code intelligence operations.

# Variants

- **`Io`** -- I/O error (e.g. reading a source file to resolve symbol positions).
- **`Client`** -- LSP client or daemon communication error (wraps `aether_lspd::ClientError`).
- **`Transport`** -- Transport or protocol error during LSP message exchange.

# Type alias

The module provides `type Result<T> = std::result::Result<T, LspError>` for convenience.
