# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

## [0.7.1](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.7.0...aether-agent-core-v0.7.1) - 2026-09-04

### Other

- Bedrock Responses response.failed events with server errors are not retried ([#410](https://github.com/contextbridge/aether/pull/410))

## [0.7.0](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.6.37...aether-agent-core-v0.7.0) - 2026-09-03

### Added

- *(aether-cli)* [**breaking**] add session usage and cost tracking ([#405](https://github.com/contextbridge/aether/pull/405))

### Fixed

- *(llm)* propagate prompt cache affinity to supported providers ([#414](https://github.com/contextbridge/aether/pull/414))

### Other

- Include prompt and agent identity in traces ([#411](https://github.com/contextbridge/aether/pull/411))

## [0.6.37](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.6.36...aether-agent-core-v0.6.37) - 2026-08-31

### Fixed

- extend foreground sub-agent timeout ([#394](https://github.com/contextbridge/aether/pull/394))

## [0.6.36](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.6.35...aether-agent-core-v0.6.36) - 2026-08-31

### Other

- updated the following local packages: aether-llm, aether-mcp-utils, aether-acp-utils

## [0.6.35](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.6.34...aether-agent-core-v0.6.35) - 2026-08-27

### Other

- *(deps)* bump rust-toolchain from 1.97 to 1.98 in the rust-toolchain-minor-patch group ([#379](https://github.com/contextbridge/aether/pull/379))
- ACP sessions ([#382](https://github.com/contextbridge/aether/pull/382))
- Use new acp type for context usage, use rmcp's list_all methods and use acp native elicitation ([#376](https://github.com/contextbridge/aether/pull/376))

### Other

- Move persisted session models, log parsing, and transcript reconstruction to `aether-sessions`.

## [0.6.34](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.6.33...aether-agent-core-v0.6.34) - 2026-08-22

### Other

- scheduled code-cleanup ([#369](https://github.com/contextbridge/aether/pull/369))

## [0.6.33](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.6.32...aether-agent-core-v0.6.33) - 2026-08-20

### Fixed

- *(aether-core)* allow headless agents to shut down ([#367](https://github.com/contextbridge/aether/pull/367))

## [0.6.32](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.6.31...aether-agent-core-v0.6.32) - 2026-08-19

### Added

- Deferred (proxied) tools can now be composed via the Bash tool  ([#360](https://github.com/contextbridge/aether/pull/360))
- *(mcp)* support Client ID Metadata Documents ([#348](https://github.com/contextbridge/aether/pull/348))
- Allow proxying tools within a MCP server ([#341](https://github.com/contextbridge/aether/pull/341))
- *(mcp-servers)* Bash and Sub-agents can now be run in foreground or background by main agent ([#340](https://github.com/contextbridge/aether/pull/340))
- Support MCP tasks  ([#339](https://github.com/contextbridge/aether/pull/339))

### Fixed

- *(mcp)* classify OAuth challenges accurately ([#347](https://github.com/contextbridge/aether/pull/347))

### Other

- *(deps)* bump agent-client-protocol from 0.14.0 to 2.0.0 ([#285](https://github.com/contextbridge/aether/pull/285))
- remove nested Cargo lockfiles ([#352](https://github.com/contextbridge/aether/pull/352))
- Upgrade to rmcp 3.1.1 and MCP tool calls now use multi round trip requests (MRTR) ([#337](https://github.com/contextbridge/aether/pull/337))
- scheduled code-cleanup ([#329](https://github.com/contextbridge/aether/pull/329))

## [0.6.31](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.6.30...aether-agent-core-v0.6.31) - 2026-08-04

### Other

- *(deps)* bump rmcp from 1.8.0 to 3.0.0 ([#223](https://github.com/contextbridge/aether/pull/223))
- scheduled code-cleanup ([#315](https://github.com/contextbridge/aether/pull/315))
- scheduled code-cleanup ([#313](https://github.com/contextbridge/aether/pull/313))

## [0.6.30](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.6.29...aether-agent-core-v0.6.30) - 2026-07-29

### Added

- *(telemetry)* name agent invocation spans ([#312](https://github.com/contextbridge/aether/pull/312))

## [0.6.29](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.6.28...aether-agent-core-v0.6.29) - 2026-07-29

### Fixed

- make agent resolution canonical across runtimes ([#310](https://github.com/contextbridge/aether/pull/310))

## [0.6.28](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.6.27...aether-agent-core-v0.6.28) - 2026-07-29

### Added

- *(aether-telemetry)* Connect parent agent and subagent tracing spans together ([#305](https://github.com/contextbridge/aether/pull/305))

## [0.6.27](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.6.26...aether-agent-core-v0.6.27) - 2026-07-28

### Fixed

- *(aether-telemetry)* Include reasoning tokens and pricing information ([#301](https://github.com/contextbridge/aether/pull/301))

## [0.6.26](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.6.25...aether-agent-core-v0.6.26) - 2026-07-28

### Other

- updated the following local packages: aether-auth, aether-llm, aether-mcp-utils, aether-acp-utils

## [0.6.25](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.6.24...aether-agent-core-v0.6.25) - 2026-07-27

### Added

- *(llm)* Add support for bedrock mantle models ([#293](https://github.com/contextbridge/aether/pull/293))

### Fixed

- Set cache key based on prompt contents. ([#279](https://github.com/contextbridge/aether/pull/279))

### Other

- scheduled code-cleanup ([#291](https://github.com/contextbridge/aether/pull/291))

## [0.6.24](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.6.23...aether-agent-core-v0.6.24) - 2026-07-20

### Other

- scheduled code-cleanup ([#256](https://github.com/contextbridge/aether/pull/256))

## [0.6.23](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.6.22...aether-agent-core-v0.6.23) - 2026-07-20

### Added

- *(mcp)* support pre-registered OAuth clients ([#254](https://github.com/contextbridge/aether/pull/254))
- *(aether-session-index)* Internal tool for self-improvement  ([#240](https://github.com/contextbridge/aether/pull/240))

### Fixed

- keep agent sessions responsive during in-flight work ([#235](https://github.com/contextbridge/aether/pull/235))

### Other

- scheduled code-cleanup ([#248](https://github.com/contextbridge/aether/pull/248))
- scheduled code-cleanup ([#243](https://github.com/contextbridge/aether/pull/243))
- streamline agent and MCP integration tests ([#236](https://github.com/contextbridge/aether/pull/236))

## [0.6.22](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.6.21...aether-agent-core-v0.6.22) - 2026-07-13

### Other

- reduce development compile costs ([#231](https://github.com/contextbridge/aether/pull/231))

## [0.6.21](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.6.20...aether-agent-core-v0.6.21) - 2026-07-13

### Other

- updated the following local packages: aether-llm, aether-mcp-utils, aether-acp-utils

## [0.6.20](https://github.com/contextbridge/aether/compare/aether-agent-core-v0.6.19...aether-agent-core-v0.6.20) - 2026-07-13

### Added

- *(aether-cli)* Add support for exporting genai OTEL traces ([#219](https://github.com/contextbridge/aether/pull/219))

### Other

- cleanup tests ([#218](https://github.com/contextbridge/aether/pull/218))
- Rename AgentMessage => AgentEvent and better organize variants ([#217](https://github.com/contextbridge/aether/pull/217))

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
