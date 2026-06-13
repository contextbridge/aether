# Rust Error Handling

This project uses concrete `enum` error types with `thiserror`.

## Contents

- [Canonical Pattern](#canonical-pattern) - enum + thiserror + Result alias
- [Variants and Sources](#variants-and-sources) - `#[error]`, `#[from]`
- [Adding Context](#adding-context) - without anyhow
- [When to Use `panic!`](#when-to-use-panic)
- [Banned Dependencies](#banned-dependencies)

## Canonical Pattern

Define a specific enum per component and a `Result` type alias:

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("MCP error: {0}")]
    McpError(#[from] mcp_utils::client::McpError),
    #[error("LLM error: {0}")]
    LlmError(#[from] llm::LlmError),
    #[error("IO error: {0}")]
    IoError(String),
}

pub type Result<T> = std::result::Result<T, AgentError>;
```

## Variants and Sources

- `#[error("...")]` — sets the `Display` message (supports interpolation)
- `#[from]` — auto-implements `From`, enabling `?` to convert source errors
- Named fields make messages self-documenting:

```rust
#[derive(Debug, Error)]
pub enum DataError {
    #[error("data not found")]
    NotFound,

    #[error("invalid data at position {position}")]
    Invalid { position: usize },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
```

## When to Use `panic!`

**DO use panic for:**
- Programmer errors / violated invariants
- Unrecoverable states that indicate bugs

**DON'T use panic for:**
- Expected error conditions (IO, parsing, user input)
- Anything a caller might want to handle

```rust
// Good: panic for invariant violation
fn get_element(slice: &[i32], index: usize) -> i32 {
    assert!(index < slice.len(), "index out of bounds: bug in caller");
    slice[index]
}

// Good: return Result for expected failures
fn parse_config(input: &str) -> Result<Config, ConfigError> {
    serde_json::from_str(input).map_err(ConfigError::from)
}
```


## Quick Reference

| Context | Use |
|---------|-----|
| Any component | `#[derive(Debug, Error)] pub enum MyError { ... }` |
| Propagate | `?` with `#[from]` on the source variant |
| Add context | `.map_err(\|e\| MyError::Variant(ctx, e))` |
| Type alias | `pub type Result<T> = std::result::Result<T, MyError>;` |
| Bugs/invariants | `panic!` / `assert!` |
