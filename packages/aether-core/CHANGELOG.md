# Changelog

All notable changes to this project will be documented in this file.

## [0.5.0](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.4.1...aether-agent-core-v0.5.0) - 2026-05-13

### Fixed

- *(aether-core)* Enable codex provider feature

### Other

- *(keyring)* Add aether-keyring crate, extract OAuthCredentialStorage, and make creds store lazily initialized

## [0.4.1](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.4.0...aether-agent-core-v0.4.1) - 2026-05-12

### Other

- update Cargo.toml dependencies

## [0.4.0](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.3.5...aether-agent-core-v0.4.0) - 2026-05-08

### Added

- *(aether-cli)* Render proxied MCP servers in a separate list from non-proxied MCPs in settings menu

### Fixed

- *(mcp-servers)* Allow concurrent mcp auth requests

### Other

- *(mcp-utils)* Rewrite mcp config to better use serde, schemars, and enforce 1 proxy instance

## [0.3.5](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.3.4...aether-agent-core-v0.3.5) - 2026-05-05

### Other

- port to contextbridge org

## [0.3.4](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.3.3...aether-agent-core-v0.3.4) - 2026-05-05

### Fixed

- *(mcp-utils)* Allow re-authing proxied mcps

## [0.3.3](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.3.2...aether-agent-core-v0.3.3) - 2026-05-03

### Added

- *(aether-cli)* Support user-level settings

## [0.3.2](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.3.1...aether-agent-core-v0.3.2) - 2026-04-29

### Fixed

- *(aether-cli)* Auto retry on llm errors

### Other

- clippy
- *(aether-cli)* Support strings in settings as file paths
- *(aether-core)* Begin to normalize config and config sources for mcp and prompts

## [aether-agent-core-v0.3.1] - 2026-04-27
