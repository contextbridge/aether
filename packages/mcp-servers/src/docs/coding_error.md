Error types for all coding tool operations.

`CodingError` is the top-level enum returned by [`CodingTools`](crate::CodingTools) methods and the tools exposed by [`CodingMcp`](crate::CodingMcp). Each variant wraps a more specific error type.

# Variants

- **`File`** -- File read, write, or edit failures ([`FileError`]).
- **`Bash`** -- Shell command execution failures ([`BashError`]).
- **`Grep`** -- Regex search failures ([`GrepError`]).
- **`Find`** -- Glob-based file discovery failures ([`FindError`]).
- **`ListFiles`** -- Directory listing failures ([`ListFilesError`]).
- **`WebFetch`** -- URL fetch failures ([`WebFetchError`]).
- **`WebSearch`** -- Web search API failures ([`WebSearchError`]).
- **`NotConfigured`** -- A tool was called that requires configuration not present (e.g. web search without a Brave API key).

# Sub-error types

- [`FileError`] -- `NotFound`, `ReadFailed`, `WriteFailed`, `CreateDirFailed`, `InvalidOffset`, `InvalidLine`, `InvalidEdit`, `OverlappingEdits`, `Io`.
- [`BashError`] -- `Forbidden`, `TimeoutTooLarge`, `SpawnFailed`, `InvalidRegex`, `JoinFailed`, `ShellNotFound`, `WaitFailed`.
- [`GrepError`] -- `InvalidGlobPattern`, `GlobSetBuildFailed`, `InvalidRegex`, `SearchFailed`, `PathNotFound`.
- [`FindError`] -- `PathNotFound`, `InvalidGlobPattern`, `LockFailed`.
- [`ListFilesError`] -- `ReadDirFailed`, `ReadEntryFailed`, `MetadataFailed`.
- [`WebFetchError`] -- `InvalidUrl`, `RequestFailed`, `Timeout`, `ResponseTooLarge`, `ParseFailed`.
- [`WebSearchError`] -- `InvalidQuery`, `ApiError`, `RateLimited`, `Timeout`, `ConfigError`, `ParseError`.
