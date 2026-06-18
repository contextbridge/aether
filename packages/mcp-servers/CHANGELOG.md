# Changelog

All notable changes to this project will be documented in this file.

## [0.4.18](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.17...aether-mcp-servers-v0.4.18) - 2026-06-18

### Other

- Move a bunch of errors to using thiserror ([#160](https://github.com/contextbridge/aether/pull/160))

## [0.4.17](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.16...aether-mcp-servers-v0.4.17) - 2026-06-13

### Other

- updated the following local packages: aether-llm, aether-lspd, aether-lspd, aether-mcp-utils, aether-agent-core, aether-project

## [0.4.16](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.15...aether-mcp-servers-v0.4.16) - 2026-06-13

### Added

- *(aether-cli)* Allow filtering mcp tools by annotation ([#151](https://github.com/contextbridge/aether/pull/151))
- *(aether-cli)* Add /move command to switch workspaces and bring your session + changes with you. ([#150](https://github.com/contextbridge/aether/pull/150))

## [0.4.15](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.14...aether-mcp-servers-v0.4.15) - 2026-06-10

### Added

- *(aether-cli)* Add evals command ([#142](https://github.com/contextbridge/aether/pull/142))

## [0.4.14](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.13...aether-mcp-servers-v0.4.14) - 2026-06-05

### Fixed

- model override switches to default agent ([#128](https://github.com/contextbridge/aether/pull/128))

## [0.4.13](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.12...aether-mcp-servers-v0.4.13) - 2026-06-04

### Added

- *(aether-cli)* Support encrypted file store for oauth for users that do not want full keyring ([#124](https://github.com/contextbridge/aether/pull/124))

### Fixed

- *(aether-cli)* Default Plan edit unable to create/edit plan files ([#125](https://github.com/contextbridge/aether/pull/125))

## [0.4.12](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.11...aether-mcp-servers-v0.4.12) - 2026-05-31

### Added

- Add user level settings resolution  ([#99](https://github.com/contextbridge/aether/pull/99))

### Fixed

- *(aether-cli)* Onboarding ([#112](https://github.com/contextbridge/aether/pull/112))
- *(aether-cli)* Update system prompts and mcp server connections when switching agents ([#110](https://github.com/contextbridge/aether/pull/110))
- *(mcp-servers)* Make sub-agents MCP work with LLM providers that require an OAuth store ([#107](https://github.com/contextbridge/aether/pull/107))
- *(aether-cli)* Start MCP servers concurrently to avoid blocking TUI ([#106](https://github.com/contextbridge/aether/pull/106))

## [0.4.11](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.10...aether-mcp-servers-v0.4.11) - 2026-05-21

### Other

- update Cargo.lock dependencies

## [0.4.10](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.9...aether-mcp-servers-v0.4.10) - 2026-05-18

### Other

- update Cargo.toml dependencies

## [0.4.9](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.8...aether-mcp-servers-v0.4.9) - 2026-05-18

### Other

- updated the following local packages: aether-mcp-utils, aether-agent-core, aether-project

## [0.4.8](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.7...aether-mcp-servers-v0.4.8) - 2026-05-16

### Other

- updated the following local packages: aether-llm, aether-mcp-utils, aether-agent-core, aether-project

## [0.4.7](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.6...aether-mcp-servers-v0.4.7) - 2026-05-15

### Other

- updated the following local packages: aether-llm, aether-mcp-utils, aether-agent-core, aether-project

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
