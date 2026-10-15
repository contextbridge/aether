Scaffolds temporary projects and in-process daemons for integration testing against language servers.

Enable with the `testing` feature flag:
```toml
aether-lspd = { version = "...", features = ["testing"] }
```

# Projects

Projects are [`TestWorkspace`]s, so they share its `file`, `write`, `path`, and `path_string` helpers and are cleaned up when dropped.

- [`cargo_project(name)`](cargo_project) -- A Cargo project with `Cargo.toml` and `src/main.rs`.
- [`node_project(name)`](node_project) -- A Node.js/TypeScript project with `package.json`, `tsconfig.json`, `src/index.ts`, and TypeScript 7 installed locally with `npm install`.
- [`file_uri(path)`](file_uri) -- The LSP `file://` URI for a path.

# Daemons

- [`TestDaemon`] -- An in-process daemon with deterministic shutdown.
- [`configure_fake_server`] -- Points a language at the shared fake language server.

# Example

```rust,no_run
use aether_lspd::testing::{cargo_project, file_uri};

let project = cargo_project("my-test");
project.write("src/lib.rs", "pub fn hello() -> &'static str { \"hi\" }");
let uri = file_uri(&project.path("src/lib.rs"));
```
