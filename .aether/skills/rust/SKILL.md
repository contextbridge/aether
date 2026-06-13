---
name: writing-rust
description: Rust best practices, idiomatic patterns, and project conventions. Use when writing Rust code, reviewing PRs, debugging issues, or working with error handling, traits, concurrency, or cargo tooling.
agent-invocable: false
triggers:
  read:
    - "**/*.rs"
---

# Rust Best Practices

Core principles for writing idiomatic, safe, and efficient Rust code.

## Contents

- [Type System](#type-system) - newtypes, enums, conversions
- [Option and Result](#option-and-result) - combinators, `?` operator
- [Error Handling](#error-handling) → [error-handling.md](./error-handling.md)
- [Traits](#traits) → [traits.md](./traits.md)
- [Memory and Ownership](#memory-and-ownership)
- [Concurrency](#concurrency) → [concurrency.md](./concurrency.md)
- [Iterators](#iterators)
- [Tooling](#tooling) → [tooling.md](./tooling.md)
- [Anti-Patterns](#anti-patterns-to-avoid)
- [Testing](./testing-fakes.md)
- [Builders](./builder-pattern.md)
- [Types](./type-safety.md)

## Type System

### Use Types to Encode Semantics

- **Make invalid states inexpressible** - design types so only valid combinations are possible at compile time
- **Use newtype pattern** (`struct Wrapper(T)`) to add semantic meaning (units, domain concepts)
- **Mark newtypes with `#[repr(transparent)]`** if binary compatibility matters
- **Prefer descriptive structs over tuples** with non-distinctive types

### Enums Over Booleans

```rust
// Bad: unclear what true/false mean
print_page(true, false);

// Good: self-documenting
print_page(Sides::Both, Output::Grayscale);
```

- Use enums for mutually exclusive states
- Exhaustive matching is enforced by the compiler
- Use enums with associated data (algebraic data types) to encode invariants

### Type Conversions

- **Implement `From<T>`** for infallible conversions (not `Into` - it's auto-generated)
- **Implement `TryFrom<T>`** for fallible conversions (returns `Result`)
- **Prefer `from`/`into` over `as` casts** - safer and more explicit
- Use `Into<T>` trait bounds on generics to accept both wrapped and unwrapped types

## Option and Result

### Prefer Transformations Over Match

```rust
// Instead of explicit match:
let value = match opt {
    Some(v) => transform(v),
    None => default,
};

// Use combinators:
let value = opt.map(transform).unwrap_or(default);
```

Key combinators: `.map()`, `.and_then()`, `.map_err()`, `.ok_or()`, `.unwrap_or_default()`

### The ? Operator

- Use `?` for error propagation - cleaner than explicit match
- Implement `From<SubError>` for your error type to enable automatic conversion

## Error Handling

See [error-handling.md](./error-handling.md) for detailed patterns.

**Quick reference:**
- Use `thiserror` + concrete `enum` error types everywhere
- Never add `anyhow` or `color-eyre`
- Outside of tests, avoid `Box<dyn std::error::Error>` — it erases the concrete type

## Memory and Ownership

### Prefer Owned Data in Structs

- Avoid lifetime parameters on structs when possible
- Allocating/cloning often leads to simpler, more maintainable code
- Optimize only when benchmarking shows it's necessary

## Concurrency

**Key points:**
- Rust prevents data races but NOT deadlocks
- Use `Send` and `Sync` marker traits
- Prefer message passing over shared state when possible
- Thread-safe alternatives: `Arc` instead of `Rc`, `Mutex`/`RwLock` instead of `RefCell`

## Iterators

### Prefer Iterator Transformations

```rust
// Instead of:
let mut result = Vec::new();
for item in collection {
    if condition(&item) {
        result.push(transform(item));
    }
}

// Use:
let result: Vec<_> = collection
    .into_iter()
    .filter(condition)
    .map(transform)
    .collect();
```

## Anti-Patterns to Avoid

- **Boolean parameters** — use enums. See [Type System](#enums-over-booleans)
- **Mock frameworks** — use Fakes. See [testing-fakes.md](./testing-fakes.md)
