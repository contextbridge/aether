# Changelog

All notable changes to this project will be documented in this file.

## [0.5.3](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.5.2...aether-agent-cli-v0.5.3) - 2026-05-12

### Other

- update Cargo.toml dependencies

## [0.5.2](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.5.1...aether-agent-cli-v0.5.2) - 2026-05-11

### Other

- updated the following local packages: aether-mcp-servers

## [0.5.1](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.5.0...aether-agent-cli-v0.5.1) - 2026-05-08

### Other

- updated the following local packages: aether-wisp

## [0.5.0](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.4.3...aether-agent-cli-v0.5.0) - 2026-05-08

### Fixed

- *(mcp-servers)* Allow concurrent mcp auth requests

### Other

- *(workspace)* Upgrade deps and to keyring 4.x
- *(mcp-utils)* Rewrite mcp config to better use serde, schemars, and enforce 1 proxy instance

## [0.4.3](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.4.2...aether-agent-cli-v0.4.3) - 2026-05-05

### Other

- port to contextbridge org

## [0.4.2](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.4.1...aether-agent-cli-v0.4.2) - 2026-05-05

### Other

- updated the following local packages: aether-mcp-utils, aether-acp-utils, aether-acp-utils, aether-agent-core, aether-wisp, aether-lspd, aether-mcp-servers, aether-project

## [0.4.1](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.4.0...aether-agent-cli-v0.4.1) - 2026-05-04

### Other

- updated the following local packages: aether-tui, aether-wisp

## [0.4.0](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.3.3...aether-agent-cli-v0.4.0) - 2026-05-03

### Added

- *(aether-cli)* Support user-level settings

### Other

- *(aether-cli)* Resolve user-level settings from aether home

## [0.3.3](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.3.2...aether-agent-cli-v0.3.3) - 2026-04-29

### Other

- *(aether-cli)* Fix backticks
- *(aether-cli)* correct binary references and slash command docs

## [0.3.2](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.3.1...aether-agent-cli-v0.3.2) - 2026-04-29

### Fixed

- *(aether-cli)* Auto retry on llm errors

### Other

- *(aether-cli)* Support strings in settings as file paths
- Re-add top level prompt and mcp settings
- More consistently use the term settings over config
- *(aether-cli)* Update cli to use new settings stucts
- *(aether-core)* Begin to normalize config and config sources for mcp and prompts
- *(aether-cli)* Quiet noisy acp logs

## [aether-agent-cli-v0.3.1] - 2026-04-27
