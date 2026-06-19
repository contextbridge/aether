# Testing with Fakes

**Location**: `crates/aether-core/src/testing/fake_llm.rs`, `crates/aether-core/src/testing/fake_mcp.rs`

## Rule: Use "Fake", Never "Mock"

## Pattern: Fake with In-Memory State

```rust
pub struct FakeLlmProvider {
    responses: Vec<String>,
    call_count: Arc<AtomicUsize>,
}

impl FakeLlmProvider {
    pub fn new(responses: Vec<String>) -> Self {
        Self {
            responses,
            call_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn call_count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }
}

impl StreamingModelProvider for FakeLlmProvider {
    async fn stream_completion(&self, _req: CompletionRequest) -> Result<impl Stream> {
        let index = self.call_count.fetch_add(1, Ordering::SeqCst);
        let response = self.responses.get(index).unwrap_or(&self.responses[0]).clone();
        Ok(stream::once(async move { Chunk::Text { text: response } }))
    }
}
```

## Core Principles

- **In-memory state** - HashMap, Vec instead of file I/O or network
- **Same trait as real impl** - Seamless substitution in tests
- **Stateful behavior** - Return different results on each call, track history
- **Thread safety** - `Arc<AtomicUsize>` for counters, `Arc<Mutex<T>>` for complex state
- **Query methods** - `call_count()`, `get_history()` for assertions
- **Builder pattern** - `with_tool()`, `with_file()` for fluent setup

## Example: Comprehensive Fake

```rust
pub struct FakeMcp {
    tools: Arc<Mutex<HashMap<String, Tool>>>,
    call_history: Arc<Mutex<Vec<ToolCall>>>,
}

impl FakeMcp {
    pub fn with_tool(self, name: &str, tool: Tool) -> Self {
        self.tools.lock().unwrap().insert(name.to_string(), tool);
        self
    }

    pub fn call_count(&self, tool_name: &str) -> usize {
        self.call_history.lock().unwrap()
            .iter()
            .filter(|c| c.name == tool_name)
            .count()
    }
}
```

## Why Fakes > Mocks

- **Maintainable** - Refactoring doesn't break tests
- **Self-documenting** - Shows how real implementation works
- **Fast** - No I/O, network, or external dependencies
- **Deterministic** - Same input = same output
- **Integration-friendly** - Test multiple components together

## Anti-Pattern

❌ Mock frameworks with expectations:
```rust
mock.expect_call().times(1).with(eq("foo")).returning(|_| Ok(()));
```

✅ Fakes with realistic behavior:
```rust
let fake = FakeMcp::new().with_tool("read_file", tool);
agent.run(&fake).await?;
assert_eq!(fake.call_count("read_file"), 1);
```
