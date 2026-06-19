# Changelog

All notable changes to this project will be documented in this file.

## [0.5.19](https://github.com/contextbridge/aether/compare/aether-mcp-utils-v0.5.18...aether-mcp-utils-v0.5.19) - 2026-06-19

### Other

- updated the following local packages: aether-llm

## [0.5.18](https://github.com/contextbridge/aether/compare/aether-mcp-utils-v0.5.17...aether-mcp-utils-v0.5.18) - 2026-06-18

### Added

- Improved support for evals ([#167](https://github.com/contextbridge/aether/pull/167))

## [0.5.17](https://github.com/contextbridge/aether/compare/aether-mcp-utils-v0.5.16...aether-mcp-utils-v0.5.17) - 2026-06-18

### Other

- Move a bunch of errors to using thiserror ([#160](https://github.com/contextbridge/aether/pull/160))

## [0.5.16](https://github.com/contextbridge/aether/compare/aether-mcp-utils-v0.5.15...aether-mcp-utils-v0.5.16) - 2026-06-13

### Other

- updated the following local packages: aether-llm

## [0.5.15](https://github.com/contextbridge/aether/compare/aether-mcp-utils-v0.5.14...aether-mcp-utils-v0.5.15) - 2026-06-13

### Added

- *(aether-cli)* Allow filtering mcp tools by annotation ([#151](https://github.com/contextbridge/aether/pull/151))
- *(aether-cli)* Add /move command to switch workspaces and bring your session + changes with you. ([#150](https://github.com/contextbridge/aether/pull/150))

## [0.5.14](https://github.com/contextbridge/aether/compare/aether-mcp-utils-v0.5.13...aether-mcp-utils-v0.5.14) - 2026-06-10

### Added

- *(aether-cli)* Add evals command ([#142](https://github.com/contextbridge/aether/pull/142))

## [0.5.13](https://github.com/contextbridge/aether/compare/aether-mcp-utils-v0.5.12...aether-mcp-utils-v0.5.13) - 2026-06-04

### Other

- update docs ([#122](https://github.com/contextbridge/aether/pull/122))

## [0.5.12](https://github.com/contextbridge/aether/compare/aether-mcp-utils-v0.5.11...aether-mcp-utils-v0.5.12) - 2026-05-31

### Added

- Add user level settings resolution  ([#99](https://github.com/contextbridge/aether/pull/99))

### Fixed

- *(aether-cli)* Update system prompts and mcp server connections when switching agents ([#110](https://github.com/contextbridge/aether/pull/110))
- *(aether-cli)* Start MCP servers concurrently to avoid blocking TUI ([#106](https://github.com/contextbridge/aether/pull/106))

## [0.5.11](https://github.com/contextbridge/aether/compare/aether-mcp-utils-v0.5.10...aether-mcp-utils-v0.5.11) - 2026-05-21

### Other

- updated the following local packages: aether-llm

## [0.5.10](https://github.com/contextbridge/aether/compare/aether-mcp-utils-v0.5.9...aether-mcp-utils-v0.5.10) - 2026-05-18

### Other

- update Cargo.toml dependencies

## [0.5.9](https://github.com/contextbridge/aether/compare/aether-mcp-utils-v0.5.8...aether-mcp-utils-v0.5.9) - 2026-05-18

### Fixed

- *(wisp)* Allow copying URLs when performing MCP auth  ([#82](https://github.com/contextbridge/aether/pull/82))

## [0.5.8](https://github.com/contextbridge/aether/compare/aether-mcp-utils-v0.5.7...aether-mcp-utils-v0.5.8) - 2026-05-16

### Other

- updated the following local packages: aether-llm

## [0.5.7](https://github.com/contextbridge/aether/compare/aether-mcp-utils-v0.5.6...aether-mcp-utils-v0.5.7) - 2026-05-15

### Fixed

- *(mcp)* populate token_received_at in MCP credential store to enable rmcp refresh ([#72](https://github.com/contextbridge/aether/pull/72))

## [0.5.6](https://github.com/contextbridge/aether/compare/aether-mcp-utils-v0.5.5...aether-mcp-utils-v0.5.6) - 2026-05-15

### Fixed

- *(mcp-utils)* Pipe stdio MCP server stderr to tracing instead of inheriting, which prevnts MCP stderr showing up in terminal ([#60](https://github.com/contextbridge/aether/pull/60))

## [0.5.5](https://github.com/contextbridge/aether/compare/aether-mcp-utils-v0.5.4...aether-mcp-utils-v0.5.5) - 2026-05-14

### Other

- updated the following local packages: aether-llm

## [0.5.4](https://github.com/contextbridge/aether/compare/aether-mcp-utils-v0.5.3...aether-mcp-utils-v0.5.4) - 2026-05-14

### Other

- updated the following local packages: aether-llm

## [0.5.3](https://github.com/contextbridge/aether/compare/aether-mcp-utils-v0.5.2...aether-mcp-utils-v0.5.3) - 2026-05-13

### Other

- updated the following local packages: aether-llm

## [0.5.2](https://github.com/contextbridge/aether/compare/aether-mcp-utils-v0.5.1...aether-mcp-utils-v0.5.2) - 2026-05-13

### Other

- *(keyring)* Add aether-keyring crate, extract OAuthCredentialStorage, and make creds store lazily initialized

## [0.5.1](https://github.com/contextbridge/aether/compare/aether-mcp-utils-v0.5.0...aether-mcp-utils-v0.5.1) - 2026-05-12

### Other

- update Cargo.toml dependencies

## [0.5.0](https://github.com/contextbridge/aether/compare/aether-mcp-utils-v0.4.1...aether-mcp-utils-v0.5.0) - 2026-05-08

### Added

- *(aether-cli)* Render proxied MCP servers in a separate list from non-proxied MCPs in settings menu

### Fixed

- *(mcp-servers)* Allow concurrent mcp auth requests

### Other

- *(mcp-utils)* Rewrite mcp config to better use serde, schemars, and enforce 1 proxy instance

## [0.4.1](https://github.com/contextbridge/aether/compare/aether-mcp-utils-v0.4.0...aether-mcp-utils-v0.4.1) - 2026-05-05

### Other

- port to contextbridge org

## [0.4.0](https://github.com/contextbridge/aether/compare/aether-mcp-utils-v0.3.3...aether-mcp-utils-v0.4.0) - 2026-05-05

### Fixed

- *(mcp-utils)* Allow re-authing proxied mcps

## [0.3.3](https://github.com/contextbridge/aether/compare/aether-mcp-utils-v0.3.2...aether-mcp-utils-v0.3.3) - 2026-05-03

### Added

- *(aether-cli)* Support user-level settings

### Other

- *(aether-evals)* Simplify aether-evals to rely on normal rust tests and cargo next test

## [0.3.2](https://github.com/contextbridge/aether/compare/aether-mcp-utils-v0.3.1...aether-mcp-utils-v0.3.2) - 2026-04-29

### Other

- *(mcp-utils)* Generate json schemas for RawMcpConfig

## [aether-mcp-utils-v0.3.1] - 2026-04-27
