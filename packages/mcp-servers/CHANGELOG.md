# Changelog

All notable changes to this project will be documented in this file.

## [0.4.6](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.5...aether-mcp-servers-v0.4.6) - 2026-05-15

### Other

- updated the following local packages: aether-llm, aether-mcp-utils, aether-lspd, aether-lspd, aether-agent-core, aether-project

## [0.4.5](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.4...aether-mcp-servers-v0.4.5) - 2026-05-14

### Other

- *(mcp-servers)* Update default plan prompt ([#52](https://github.com/contextbridge/aether/pull/52))

## [0.4.4](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.3...aether-mcp-servers-v0.4.4) - 2026-05-14

### Other

- update Cargo.lock dependencies

## [0.4.3](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.2...aether-mcp-servers-v0.4.3) - 2026-05-13

### Other

- updated the following local packages: aether-llm, aether-mcp-utils, aether-agent-core, aether-project

## [0.4.2](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.1...aether-mcp-servers-v0.4.2) - 2026-05-13

### Other

- *(keyring)* Add aether-keyring crate, extract OAuthCredentialStorage, and make creds store lazily initialized

## [0.4.1](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.0...aether-mcp-servers-v0.4.1) - 2026-05-12

### Other

- update Cargo.toml dependencies

## [0.4.0](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.3.6...aether-mcp-servers-v0.4.0) - 2026-05-11

### Added

- *(mcp-servers)* Coding mcp gains lsp workspace search, and remove confusing lsp/coding server overlap

## [0.3.6](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.3.5...aether-mcp-servers-v0.3.6) - 2026-05-08

### Other

- *(mcp-utils)* Rewrite mcp config to better use serde, schemars, and enforce 1 proxy instance

## [0.3.5](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.3.4...aether-mcp-servers-v0.3.5) - 2026-05-05

### Other

- port to contextbridge org

## [0.3.4](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.3.3...aether-mcp-servers-v0.3.4) - 2026-05-05

### Other

- update Cargo.lock dependencies

## [0.3.3](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.3.2...aether-mcp-servers-v0.3.3) - 2026-05-03

### Added

- *(aether-cli)* Support user-level settings

### Other

- *(aether-cli)* Resolve user-level settings from aether home
- *(mcp-servers)* fix flaky test

## [0.3.2](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.3.1...aether-mcp-servers-v0.3.2) - 2026-04-29

### Added

- *(mcp-servers)* Allow disabling lsp on coding server via config flag

### Other

- *(aether-cli)* Support strings in settings as file paths
- Re-add top level prompt and mcp settings
- More consistently use the term settings over config
- *(mcp-servers)* Use new config structs from core/project

## [aether-mcp-servers-v0.3.1] - 2026-04-27
