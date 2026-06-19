# Builder Pattern

**Location**: `crates/aether-core/src/agent/agent_builder.rs`, `crates/aether-core/src/mcp/mcp_builder.rs`

## Pattern

```rust
pub struct AgentBuilder<T: StreamingModelProvider> {
    llm: T,
    tools: Vec<Tool>,
    max_iterations: Option<usize>,
}

impl<T: StreamingModelProvider> AgentBuilder<T> {
    pub fn with_tool(mut self, tool: Tool) -> Self {
        self.tools.push(tool);
        self
    }

    pub async fn spawn(self) -> Agent {  // Consumes self
        Agent { tools: self.tools, /* ... */ }
    }
}
```

## Core Principles

- **Take `self` by value, return `Self`** - Enables method chaining
- **Consume in final method** - `spawn(self)` prevents reuse
- **Use generics for type safety** - `<T: StreamingModelProvider>` not `Box<dyn>`
- **Separate config from runtime** - Builder holds config, `spawn()` creates runtime object
- **Async spawn when needed** - Use `async fn spawn()` for async initialization

## Why

- Fluent API with method chaining
- Prevents invalid states (builder consumed)
- Zero-cost abstraction with generics
- Clear separation of configuration and execution
