# Type Safety

**Location**: `packages/aether/src/types.rs`, `packages/aether-evals/src/eval_assertion.rs`

## Pattern 1: Newtype Wrapper

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IsoString(pub String);

impl IsoString {
    pub fn now() -> Self { Self(Utc::now().to_rfc3339()) }
}
```

**Use for**: Timestamps, IDs, URLs, validated strings - prevents mixing conceptually different primitives

## Pattern 2: Enum Builder Methods

```rust
pub enum EvalAssertion {
    FileExists { path: String },
    ToolCall { name: String, count: Option<usize> },
}

impl EvalAssertion {
    pub fn file_exists(path: &str) -> Self {
        Self::FileExists { path: path.to_string() }
    }

    pub fn tool_call_exact(name: &str, count: usize) -> Self {
        Self::ToolCall { name: name.to_string(), count: Some(count) }
    }
}

// Usage: EvalAssertion::file_exists("out.txt") vs verbose struct construction
```

**Use for**: Complex enum variants, hiding `Arc<dyn Fn>` wrapping, ergonomic APIs

## Core Principles

- **Newtype for domain concepts** - Zero-cost compile-time guarantees
- **Smart constructors** - `now()`, `from_datetime()` enforce invariants
- **Derive Serialize/Deserialize** - Works seamlessly with JSON/TOML
- **Builder methods on enums** - Hide complexity, improve discoverability
- **Arc for closures in enums** - `Arc<dyn Fn(...) + Send + Sync>` enables Clone

## Why

- Catch type errors at compile time, not runtime
- IDE autocomplete shows available constructors
- Self-documenting - type names convey meaning
