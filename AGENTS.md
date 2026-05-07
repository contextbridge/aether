# Agent Instructions

## Key Commands

- **Compile** -- use LSP tools, or `just check` if you must
- **Tests** -- `just test`
- **Lint** -- `just lint`
- **Format** -- `just fmt`

## Coding Style

1. Backwards compatibility and fallbacks are not a concern when planning or implementing code.
2. Favor simplicity over complexity. Good architecture results in simple systems.
3. Non-doc (double slash) comments should be used extremely sparringly and only to explain the "why?" a certain piece of code exits. Comments should never be used for file separators.

### File organization

- Put `pub` sturcts, traits, functions, and types at the top of the file.
- Put private functions below `pub` constructs, near the bottom of the file.
- The reader's eye should flow from the most important, high-level things (top) to the less important nitty-gritty details (bottom). Example `eval_runner.rs` should have `pub struct EvalRunner` near the top

### Testing

1. Write `Fake` objects that mimic the _real_ behavior of the thing being faked using an in-memory implementation instead of producing side-effects. For example, a `FakeFilesystem` should work just like a real file system, but `write_file()` might write to a `HashMap` instead of the file-system.
2. When fixing a bug, always write test(s) first. Confirm the test(s) fail _before_ attempting to fix the bug.
3. Prefer integration tests that assert against state using fakes (e.g. in memory file system that asserts file contents) over mocks that test behavior (e.g. how many times did we call write_file?)
4. Don't put timeouts into tests, this always leads to flaky tests on CPUs with different speeds.
5. All tests must test _only_ the public API. Never write tests that test a private method as that couples the tests to implementation details and makes the code harder to refactor.

### Error handling

1. Never add `anyhow` or `color-eyere` as dependencies. Use standard Rust `enum`'s instead, e.g. `enum ApiError { ... }`
2. Prefer using specific enum types over `Box<dyn std::error::Error>` as the later makes it impossible for the caller to pattern match on specific errors.
3. Leverage `map`, `flat_map`, `and_then` etc to flatten nested `match` statements like `Ok(Ok(foo)) => {...}`.

## Workflows

1. Use `gh` (Github CLI) for all Github operations.
