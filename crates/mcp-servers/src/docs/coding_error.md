Error types for all coding tool operations.

`CodingError` is the top-level enum returned by [`CodingTools`](crate::CodingTools) methods and the tools exposed by [`CodingMcp`](crate::CodingMcp). Each variant wraps a more specific error type.

# Variants

- **`File`** -- File read, write, or edit failures ([`FileError`]).
- **`Bash`** -- Shell command execution failures ([`BashError`]).
- **`WebFetch`** -- URL fetch failures ([`WebFetchError`]).
- **`WebSearch`** -- Web search API failures ([`WebSearchError`]).
- **`Lsp`** -- LSP code-intelligence tool failures ([`LspError`]).
- **`NotConfigured`** -- A tool was called that requires configuration not present (e.g. web search without a Brave API key).
- **`EmptyFilePath`** -- A required file-path argument was empty.
- **`NotReadBeforeOverwrite`** / **`NotReadBeforeEdit`** -- Read-before-overwrite/edit safety checks failed.
- **`ExistsCheckFailed`** -- Failed to check whether the target file exists.

# Sub-error types

- [`FileError`] -- `NotFound`, `ReadFailed`, `WriteFailed`, `CreateDirFailed`, `InvalidOffset`, `PatternNotFound`, `Io`.
- [`BashError`] -- `Forbidden`, `TimeoutTooLarge`, `SpawnFailed`.
- [`WebFetchError`] -- `InvalidUrl`, `RequestFailed`, `Timeout`, `ResponseTooLarge`, `ParseFailed`.
- [`WebSearchError`] -- `InvalidQuery`, `ApiError`, `RateLimited`, `Timeout`, `ConfigError`, `ParseError`.
