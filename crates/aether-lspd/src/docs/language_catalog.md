Identifies a programming language for LSP server selection.

`LanguageId` maps languages to their LSP server configuration, file extensions, and protocol identifiers. The daemon uses this to determine which language server to spawn and which files to associate with it.

# Detection

- [`from_extension`](LanguageId::from_extension) -- Detect from a file extension (e.g. `"rs"` → `Rust`).
- [`from_path`](LanguageId::from_path) -- Detect from a file path. Falls back to `PlainText` for unknown extensions.
- [`as_str`](LanguageId::as_str) -- Get the LSP language ID string for this variant.
- [`detect_project_languages`](crate::detect_project_languages) -- Detect configured languages from project manifests.

# Supported language servers

| Server | Languages | Env override |
|--------|-----------|-------------|
| `rust-analyzer` | Rust | `AETHER_LSPD_SERVER_COMMAND_RUST_ANALYZER` |
| `tsc --lsp` (TypeScript 7+) | JavaScript, JSX, TypeScript, TSX | `AETHER_LSPD_SERVER_COMMAND_TSC` |
| `pyright-langserver` | Python | `AETHER_LSPD_SERVER_COMMAND_PYRIGHT` |
| `gopls` | Go | `AETHER_LSPD_SERVER_COMMAND_GOPLS` |
| `clangd` | C, C++ | `AETHER_LSPD_SERVER_COMMAND_CLANGD` |

Languages without a configured server (Java, Ruby, etc.) can still be identified but won't have LSP support.

# Server pooling

Languages that share a server implementation also share a daemon socket. For example, TypeScript and TSX both use `tsc`, so [`socket_path`](crate::socket_path()) returns the same path for both. This avoids spawning duplicate server processes.

# Metadata

[`LANGUAGES`](crate::LANGUAGES) lists every language with its LSP language ID, file extensions and language server, if any. [`metadata`](LanguageId::metadata) looks up a single language, and [`server`](LanguageId::server) returns its server's display name and installation guidance.
