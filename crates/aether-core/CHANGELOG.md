# Changelog

All notable changes to this project will be documented in this file.

## [0.6.19](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.6.18...aether-agent-core-v0.6.19) - 2026-07-09

### Other

- updated the following local packages: aether-utils, aether-llm, aether-acp-utils, aether-mcp-utils

## [0.6.18](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.6.17...aether-agent-core-v0.6.18) - 2026-06-30

### Other

- *(mcp-servers)* Remove MCP roots functionality  ([#199](https://github.com/contextbridge/aether/pull/199))

## [0.6.17](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.6.16...aether-agent-core-v0.6.17) - 2026-06-22

### Other

- updated the following local packages: aether-llm, aether-mcp-utils, aether-acp-utils

## [0.6.16](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.6.15...aether-agent-core-v0.6.16) - 2026-06-22

### Other

- *(aether-evals)* Expose token usage stats for evals ([#179](https://github.com/contextbridge/aether/pull/179))

## [0.6.15](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.6.14...aether-agent-core-v0.6.15) - 2026-06-19

### Added

- *(aether-cli)* Add model settings to be able to control temperature, top p etc ([#168](https://github.com/contextbridge/aether/pull/168))

### Other

- Cleanup experimental eval APIs ([#171](https://github.com/contextbridge/aether/pull/171))

## [0.6.14](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.6.13...aether-agent-core-v0.6.14) - 2026-06-18

### Added

- Improved support for evals ([#167](https://github.com/contextbridge/aether/pull/167))

## [0.6.13](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.6.12...aether-agent-core-v0.6.13) - 2026-06-18

### Added

- *(aether-evals)* Drive dockerized agents under eval via ACP ([#161](https://github.com/contextbridge/aether/pull/161))

### Other

- Better experience for authoring evals ([#162](https://github.com/contextbridge/aether/pull/162))
- Move a bunch of errors to using thiserror ([#160](https://github.com/contextbridge/aether/pull/160))

## [0.6.12](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.6.11...aether-agent-core-v0.6.12) - 2026-06-13

### Other

- updated the following local packages: aether-llm, aether-mcp-utils, aether-acp-utils

## [0.6.11](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.6.10...aether-agent-core-v0.6.11) - 2026-06-13

### Added

- *(aether-cli)* Allow filtering mcp tools by annotation ([#151](https://github.com/contextbridge/aether/pull/151))

## [0.6.10](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.6.9...aether-agent-core-v0.6.10) - 2026-06-10

### Other

- *(aether-cli)* Make aether cli headless mode output serialized AgentMessages instead of abusing the tracing crate output format and run evals in isolated Docker containers ([#141](https://github.com/contextbridge/aether/pull/141))

## [0.6.9](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.6.8...aether-agent-core-v0.6.9) - 2026-06-04

### Other

- update docs ([#122](https://github.com/contextbridge/aether/pull/122))

## [0.6.8](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.6.7...aether-agent-core-v0.6.8) - 2026-05-31

### Added

- Add user level settings resolution  ([#99](https://github.com/contextbridge/aether/pull/99))

### Fixed

- *(aether-cli)* Onboarding ([#112](https://github.com/contextbridge/aether/pull/112))
- *(aether-cli)* Update system prompts and mcp server connections when switching agents ([#110](https://github.com/contextbridge/aether/pull/110))
- *(aether-cli)* Start MCP servers concurrently to avoid blocking TUI ([#106](https://github.com/contextbridge/aether/pull/106))

## [0.6.7](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.6.6...aether-agent-core-v0.6.7) - 2026-05-21

### Other

- updated the following local packages: aether-llm, aether-acp-utils, aether-mcp-utils

## [0.6.6](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.6.5...aether-agent-core-v0.6.6) - 2026-05-18

### Other

- update Cargo.toml dependencies

## [0.6.5](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.6.4...aether-agent-core-v0.6.5) - 2026-05-18

### Fixed

- *(wisp)* Allow copying URLs when performing MCP auth  ([#82](https://github.com/contextbridge/aether/pull/82))

## [0.6.4](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.6.3...aether-agent-core-v0.6.4) - 2026-05-16

### Other

- updated the following local packages: aether-llm, aether-mcp-utils, aether-acp-utils

## [0.6.3](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.6.2...aether-agent-core-v0.6.3) - 2026-05-15

### Other

- updated the following local packages: aether-auth, aether-llm, aether-mcp-utils, aether-acp-utils

## [0.6.2](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.6.1...aether-agent-core-v0.6.2) - 2026-05-15

### Other

- updated the following local packages: aether-auth, aether-llm, aether-mcp-utils, aether-acp-utils

## [0.6.1](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.6.0...aether-agent-core-v0.6.1) - 2026-05-14

### Other

- updated the following local packages: aether-llm, aether-mcp-utils, aether-acp-utils

## [0.6.0](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.5.1...aether-agent-core-v0.6.0) - 2026-05-14

### Fixed

- *(aether-core)* Give users escape hatch to set custom context window limit and set provider urls disable auth (useful for bedrock sigv4 proxy)

## [0.5.1](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.5.0...aether-agent-core-v0.5.1) - 2026-05-13

### Other

- updated the following local packages: aether-llm, aether-mcp-utils, aether-acp-utils

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
