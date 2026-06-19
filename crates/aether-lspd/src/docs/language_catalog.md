Identifies a programming language for LSP server selection.

`LanguageId` maps languages to their LSP server configuration, file extensions, and protocol identifiers. The daemon uses this to determine which language server to spawn and which files to associate with it.

# Detection

- [`from_extension`](LanguageId::from_extension) -- Detect from a file extension (e.g. `"rs"` → `Rust`).
- [`from_path`](LanguageId::from_path) -- Detect from a file path. Falls back to `PlainText` for unknown extensions.
- [`from_lsp_id`](crate::from_lsp_id) -- Parse an LSP language ID string (e.g. `"typescriptreact"` → `TypeScriptReact`).
- [`as_str`](LanguageId::as_str) -- Get the LSP language ID string for this variant.

# Supported language servers

| Server | Languages | Env override |
|--------|-----------|-------------|
| `rust-analyzer` | Rust | `AETHER_LSPD_SERVER_COMMAND_RUST_ANALYZER` |
| `typescript-language-server` | JavaScript, JSX, TypeScript, TSX | `AETHER_LSPD_SERVER_COMMAND_TYPESCRIPT_LANGUAGE_SERVER` |
| `pyright-langserver` | Python | `AETHER_LSPD_SERVER_COMMAND_PYRIGHT` |
| `gopls` | Go | `AETHER_LSPD_SERVER_COMMAND_GOPLS` |
| `clangd` | C, C++ | `AETHER_LSPD_SERVER_COMMAND_CLANGD` |

Languages without a configured server (Java, Ruby, etc.) can still be identified but won't have LSP support.

# Server pooling

Languages that share a server implementation also share a daemon socket. For example, TypeScript and TSX both use `typescript-language-server`, so [`socket_path`](crate::socket_path()) returns the same path for both. This avoids spawning duplicate server processes.

# Metadata

[`LANGUAGE_METADATA`](crate::LANGUAGE_METADATA) provides a static list of all language entries with their extensions, aliases, and primary extension. Use [`metadata_for`](crate::metadata_for) to look up a single language, or [`extensions_for_alias`](crate::extensions_for_alias) to find all file extensions matching a language name.
