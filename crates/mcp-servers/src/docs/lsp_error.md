Errors from LSP code intelligence operations.

# Variants

- **`Io`** -- I/O error (e.g. reading a source file to resolve symbol positions).
- **`Client`** -- LSP client or daemon communication error (wraps `aether_lspd::ClientError`).
- **`ServerUnavailable`** -- A configured language server could not start or is unavailable.
- **`UnsupportedLanguage`** -- No language server is configured for the requested language.
- **`InvalidPath`** -- A requested path cannot be represented as a file URI.
- **`SymbolNotFound`** -- The requested symbol could not be found.
- **`InvalidPosition`** -- The requested source position is invalid.
