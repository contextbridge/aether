# Changelog

All notable changes to this project will be documented in this file.

## [0.5.2](https://github.com/contextbridge/aether/compare/aether-project-v0.5.1...aether-project-v0.5.2) - 2026-05-15

### Other

- updated the following local packages: aether-llm, aether-mcp-utils, aether-agent-core

## [0.5.1](https://github.com/contextbridge/aether/compare/aether-project-v0.5.0...aether-project-v0.5.1) - 2026-05-14

### Other

- updated the following local packages: aether-llm, aether-mcp-utils, aether-agent-core

## [0.5.0](https://github.com/contextbridge/aether/compare/aether-project-v0.4.6...aether-project-v0.5.0) - 2026-05-14

### Fixed

- *(aether-core)* Give users escape hatch to set custom context window limit and set provider urls disable auth (useful for bedrock sigv4 proxy)

## [0.4.6](https://github.com/contextbridge/aether/compare/aether-project-v0.4.5...aether-project-v0.4.6) - 2026-05-13

### Other

- updated the following local packages: aether-llm, aether-mcp-utils, aether-agent-core

## [0.4.5](https://github.com/contextbridge/aether/compare/aether-project-v0.4.4...aether-project-v0.4.5) - 2026-05-13

### Other

- update Cargo.lock dependencies

## [0.4.4](https://github.com/contextbridge/aether/compare/aether-project-v0.4.3...aether-project-v0.4.4) - 2026-05-12

### Other

- update Cargo.toml dependencies

## [0.4.3](https://github.com/contextbridge/aether/compare/aether-project-v0.4.2...aether-project-v0.4.3) - 2026-05-08

### Other

- *(mcp-utils)* Rewrite mcp config to better use serde, schemars, and enforce 1 proxy instance

## [0.4.2](https://github.com/contextbridge/aether/compare/aether-project-v0.4.1...aether-project-v0.4.2) - 2026-05-05

### Other

- port to contextbridge org

## [0.4.1](https://github.com/contextbridge/aether/compare/aether-project-v0.4.0...aether-project-v0.4.1) - 2026-05-05

### Other

- updated the following local packages: aether-mcp-utils, aether-agent-core

## [0.4.0](https://github.com/contextbridge/aether/compare/aether-project-v0.3.2...aether-project-v0.4.0) - 2026-05-03

### Added

- *(aether-cli)* Support user-level settings

### Other

- *(aether-cli)* Resolve user-level settings from aether home

## [0.3.2](https://github.com/contextbridge/aether/compare/aether-project-v0.3.1...aether-project-v0.3.2) - 2026-04-29

### Other

- *(aether-cli)* Support strings in settings as file paths
- Re-add top level prompt and mcp settings
- More consistently use the term settings over config
- *(aether-project)* Begin to cleanup settings so we can code generate json schemas from structs

## [aether-project-v0.3.1] - 2026-04-27
