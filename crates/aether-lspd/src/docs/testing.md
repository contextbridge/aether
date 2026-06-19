Scaffolds temporary projects for integration testing against real language servers.

Enable with the `testing` feature flag:
```toml
aether-lspd = { version = "...", features = ["testing"] }
```

# `TestProject` trait

All test projects implement [`TestProject`], which provides:

- [`root()`](TestProject::root) -- The temporary directory path.
- [`add_file(path, content)`](TestProject::add_file) -- Write a file relative to the project root.
- [`file_uri(path)`](TestProject::file_uri) -- Get an LSP `file://` URI for a relative path.
- [`file_path_str(path)`](TestProject::file_path_str) -- Get the absolute path as a string.

# Implementations

- [`CargoProject::new(name)`](CargoProject::new) -- Creates a temporary Cargo project with `Cargo.toml` and `src/main.rs`.
- [`NodeProject::new(name)`](NodeProject::new) -- Creates a temporary Node.js/TypeScript project with `package.json`, `tsconfig.json`, `src/index.ts`, and runs `npm install typescript`.

Both are backed by [`TempDir`](tempfile::TempDir) and are automatically cleaned up when dropped.

# Example

```rust,no_run
use aether_lspd::testing::{CargoProject, TestProject};

let project = CargoProject::new("my-test").unwrap();
project.add_file("src/lib.rs", "pub fn hello() -> &'static str { \"hi\" }").unwrap();
let uri = project.file_uri("src/lib.rs");
```
