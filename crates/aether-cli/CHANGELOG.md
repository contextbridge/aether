# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

## [0.8.1](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.8.0...aether-agent-cli-v0.8.1) - 2026-09-04

### Other

- Bedrock Responses response.failed events with server errors are not retried ([#410](https://github.com/contextbridge/aether/pull/410))

## [0.8.0](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.41...aether-agent-cli-v0.8.0) - 2026-09-03

### Added

- *(aether-cli)* [**breaking**] add session usage and cost tracking ([#405](https://github.com/contextbridge/aether/pull/405))

### Fixed

- *(llm)* propagate prompt cache affinity to supported providers ([#414](https://github.com/contextbridge/aether/pull/414))

### Other

- Include prompt and agent identity in traces ([#411](https://github.com/contextbridge/aether/pull/411))

## [0.7.41](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.40...aether-agent-cli-v0.7.41) - 2026-08-31

### Other

- updated the following local packages: aether-agent-core, aether-project, aether-sessions, aether-telemetry, aether-mcp-servers

## [0.7.40](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.39...aether-agent-cli-v0.7.40) - 2026-08-31

### Other

- Update models ([#392](https://github.com/contextbridge/aether/pull/392))
- scheduled code-cleanup ([#393](https://github.com/contextbridge/aether/pull/393))

## [0.7.39](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.38...aether-agent-cli-v0.7.39) - 2026-08-27

### Other

- *(deps)* bump rust-toolchain from 1.97 to 1.98 in the rust-toolchain-minor-patch group ([#379](https://github.com/contextbridge/aether/pull/379))
- ACP sessions ([#382](https://github.com/contextbridge/aether/pull/382))
- Use new acp type for context usage, use rmcp's list_all methods and use acp native elicitation ([#376](https://github.com/contextbridge/aether/pull/376))

### Other

- Use the shared `aether-sessions` crate for persisted session types and transcript helpers.

## [0.7.38](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.37...aether-agent-cli-v0.7.38) - 2026-08-22

### Added

- *(wisp)* Rewrite Wisp with Ratatui and remove its custom crossterm tui ([#373](https://github.com/contextbridge/aether/pull/373))

## [0.7.37](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.36...aether-agent-cli-v0.7.37) - 2026-08-20

### Other

- fix Clippy error by boxing enum

## [0.7.36](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.35...aether-agent-cli-v0.7.36) - 2026-08-19

### Added

- Deferred (proxied) tools can now be composed via the Bash tool  ([#360](https://github.com/contextbridge/aether/pull/360))
- Allow proxying tools within a MCP server ([#341](https://github.com/contextbridge/aether/pull/341))
- *(mcp-servers)* Bash and Sub-agents can now be run in foreground or background by main agent ([#340](https://github.com/contextbridge/aether/pull/340))
- Support MCP tasks  ([#339](https://github.com/contextbridge/aether/pull/339))

### Other

- *(deps)* bump agent-client-protocol from 0.14.0 to 2.0.0 ([#285](https://github.com/contextbridge/aether/pull/285))
- Refactor/credential storage ([#345](https://github.com/contextbridge/aether/pull/345))
- Upgrade to rmcp 3.1.1 and MCP tool calls now use multi round trip requests (MRTR) ([#337](https://github.com/contextbridge/aether/pull/337))
- scheduled code-cleanup ([#329](https://github.com/contextbridge/aether/pull/329))
- release ([#325](https://github.com/contextbridge/aether/pull/325))

## [0.7.35](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.34...aether-agent-cli-v0.7.35) - 2026-08-04

### Other

- *(deps)* bump rmcp from 1.8.0 to 3.0.0 ([#223](https://github.com/contextbridge/aether/pull/223))
- scheduled code-cleanup ([#318](https://github.com/contextbridge/aether/pull/318))

## [0.7.34](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.33...aether-agent-cli-v0.7.34) - 2026-07-29

### Other

- updated the following local packages: aether-agent-core, aether-lspd, aether-telemetry, aether-wisp, aether-project, aether-mcp-servers

## [0.7.33](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.32...aether-agent-cli-v0.7.33) - 2026-07-29

### Fixed

- make agent resolution canonical across runtimes ([#310](https://github.com/contextbridge/aether/pull/310))

## [0.7.32](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.31...aether-agent-cli-v0.7.32) - 2026-07-29

### Other

- updated the following local packages: aether-lspd, aether-mcp-servers

## [0.7.31](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.30...aether-agent-cli-v0.7.31) - 2026-07-29

### Added

- *(aether-telemetry)* Connect parent agent and subagent tracing spans together ([#305](https://github.com/contextbridge/aether/pull/305))

## [0.7.30](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.29...aether-agent-cli-v0.7.30) - 2026-07-28

### Fixed

- *(aether-telemetry)* Include reasoning tokens and pricing information ([#301](https://github.com/contextbridge/aether/pull/301))

## [0.7.29](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.28...aether-agent-cli-v0.7.29) - 2026-07-28

### Other

- Seepdup tests ([#294](https://github.com/contextbridge/aether/pull/294))

## [0.7.28](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.27...aether-agent-cli-v0.7.28) - 2026-07-27

### Added

- *(llm)* Add support for bedrock mantle models ([#293](https://github.com/contextbridge/aether/pull/293))

### Fixed

- Set cache key based on prompt contents. ([#279](https://github.com/contextbridge/aether/pull/279))

### Other

- release ([#269](https://github.com/contextbridge/aether/pull/269))

## [0.7.27](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.26...aether-agent-cli-v0.7.27) - 2026-07-21

### Other

- updated the following local packages: aether-lspd, aether-telemetry, aether-mcp-servers

## [0.7.26](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.25...aether-agent-cli-v0.7.26) - 2026-07-20

### Added

- Support setting custom OTEL trace id.  ([#262](https://github.com/contextbridge/aether/pull/262))

## [0.7.25](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.24...aether-agent-cli-v0.7.25) - 2026-07-20

### Added

- *(mcp)* support pre-registered OAuth clients ([#254](https://github.com/contextbridge/aether/pull/254))
- *(aether-session-index)* Internal tool for self-improvement  ([#240](https://github.com/contextbridge/aether/pull/240))
- *(telemetry)* interpolate OTLP header variables ([#238](https://github.com/contextbridge/aether/pull/238))
- add Microsoft Foundry and Fireworks providers ([#234](https://github.com/contextbridge/aether/pull/234))

### Fixed

- keep agent sessions responsive during in-flight work ([#235](https://github.com/contextbridge/aether/pull/235))

### Other

- streamline agent and MCP integration tests ([#236](https://github.com/contextbridge/aether/pull/236))

## [0.7.24](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.23...aether-agent-cli-v0.7.24) - 2026-07-13

### Other

- reduce development compile costs ([#231](https://github.com/contextbridge/aether/pull/231))

## [0.7.23](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.22...aether-agent-cli-v0.7.23) - 2026-07-13

### Added

- *(aether-cli)* Support exact trace and metric endpoints for otel e… ([#228](https://github.com/contextbridge/aether/pull/228))

## [0.7.22](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.21...aether-agent-cli-v0.7.22) - 2026-07-13

### Added

- *(aether-cli)* Add support for exporting genai OTEL traces ([#219](https://github.com/contextbridge/aether/pull/219))

### Other

- Rename AgentMessage => AgentEvent and better organize variants ([#217](https://github.com/contextbridge/aether/pull/217))

## [0.7.21](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.20...aether-agent-cli-v0.7.21) - 2026-07-09

### Other

- Update models ([#214](https://github.com/contextbridge/aether/pull/214))

## [0.7.20](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.19...aether-agent-cli-v0.7.20) - 2026-06-30

### Fixed

- *(mcp-servers)* Make agent fail fast when asked to modify files after using "/plan" cmd from plan MCP ([#207](https://github.com/contextbridge/aether/pull/207))

### Other

- *(aether-cli)* Consolidate and cleanup slash command expansion… ([#208](https://github.com/contextbridge/aether/pull/208))
- *(aether-cli)* Trim session logs by not logging partial, streamin… ([#202](https://github.com/contextbridge/aether/pull/202))
- *(mcp-servers)* Remove MCP roots functionality  ([#199](https://github.com/contextbridge/aether/pull/199))
- *(aether-cli)* Remove old notes tools as they're subsumed by skills and rules ([#195](https://github.com/contextbridge/aether/pull/195))

## [0.7.19](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.18...aether-agent-cli-v0.7.19) - 2026-06-22

### Other

- updated the following local packages: aether-llm, aether-lspd, aether-mcp-utils, aether-acp-utils, aether-acp-utils, aether-agent-core, aether-project, aether-mcp-servers, aether-wisp

## [0.7.18](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.17...aether-agent-cli-v0.7.18) - 2026-06-22

### Other

- *(aether-evals)* Expose token usage stats for evals ([#179](https://github.com/contextbridge/aether/pull/179))
- *(workspace)* Move Rust to crates/ and TS to packages/ ([#175](https://github.com/contextbridge/aether/pull/175))

## [0.7.17](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.16...aether-agent-cli-v0.7.17) - 2026-06-19

### Added

- *(aether-cli)* Add model settings to be able to control temperature, top p etc ([#168](https://github.com/contextbridge/aether/pull/168))

### Other

- Cleanup experimental eval APIs ([#171](https://github.com/contextbridge/aether/pull/171))

## [0.7.16](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.15...aether-agent-cli-v0.7.16) - 2026-06-18

### Added

- Improved support for evals ([#167](https://github.com/contextbridge/aether/pull/167))

## [0.7.15](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.14...aether-agent-cli-v0.7.15) - 2026-06-18

### Added

- *(aether-evals)* Drive dockerized agents under eval via ACP ([#161](https://github.com/contextbridge/aether/pull/161))

### Fixed

- Small fixes ([#164](https://github.com/contextbridge/aether/pull/164))

### Other

- Better experience for authoring evals ([#162](https://github.com/contextbridge/aether/pull/162))
- Move a bunch of errors to using thiserror ([#160](https://github.com/contextbridge/aether/pull/160))

## [0.7.14](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.13...aether-agent-cli-v0.7.14) - 2026-06-13

### Other

- updated the following local packages: aether-llm, aether-lspd, aether-mcp-utils, aether-acp-utils, aether-acp-utils, aether-agent-core, aether-project, aether-wisp, aether-evals, aether-mcp-servers

## [0.7.13](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.12...aether-agent-cli-v0.7.13) - 2026-06-13

### Added

- *(aether-cli)* Allow filtering mcp tools by annotation ([#151](https://github.com/contextbridge/aether/pull/151))
- *(aether-cli)* Add /move command to switch workspaces and bring your session + changes with you. ([#150](https://github.com/contextbridge/aether/pull/150))
- *(aether-cli)* Better session resume menu ([#145](https://github.com/contextbridge/aether/pull/145))

## [0.7.12](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.11...aether-agent-cli-v0.7.12) - 2026-06-10

### Added

- *(aether-cli)* Add evals command ([#142](https://github.com/contextbridge/aether/pull/142))
- *(wisp)* Allow configuring status lines via settings ([#132](https://github.com/contextbridge/aether/pull/132))

### Other

- *(aether-cli)* Make aether cli headless mode output serialized AgentMessages instead of abusing the tracing crate output format and run evals in isolated Docker containers ([#141](https://github.com/contextbridge/aether/pull/141))
- Upgrade deps ([#140](https://github.com/contextbridge/aether/pull/140))
- *(website)* Docs fixes + docs skill ([#131](https://github.com/contextbridge/aether/pull/131))

## [0.7.11](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.10...aether-agent-cli-v0.7.11) - 2026-06-05

### Added

- *(aether-cli)* Settings init command now offers to load config from other harnesses like Claude to ease onboarding ([#126](https://github.com/contextbridge/aether/pull/126))

### Fixed

- model override switches to default agent ([#128](https://github.com/contextbridge/aether/pull/128))

## [0.7.10](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.9...aether-agent-cli-v0.7.10) - 2026-06-04

### Added

- *(aether-cli)* Support encrypted file store for oauth for users that do not want full keyring ([#124](https://github.com/contextbridge/aether/pull/124))

### Fixed

- *(aether-cli)* Default Plan edit unable to create/edit plan files ([#125](https://github.com/contextbridge/aether/pull/125))

## [0.7.9](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.8...aether-agent-cli-v0.7.9) - 2026-06-02

### Fixed

- *(aether-cli)* Detect stdio file descriptors and use unix streams o… ([#118](https://github.com/contextbridge/aether/pull/118))

## [0.7.8](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.7...aether-agent-cli-v0.7.8) - 2026-05-31

### Added

- Add user level settings resolution  ([#99](https://github.com/contextbridge/aether/pull/99))

### Fixed

- *(aether-cli)* Onboarding ([#112](https://github.com/contextbridge/aether/pull/112))
- *(aether-cli)* Update system prompts and mcp server connections when switching agents ([#110](https://github.com/contextbridge/aether/pull/110))
- *(aether-cli)* Start MCP servers concurrently to avoid blocking TUI ([#106](https://github.com/contextbridge/aether/pull/106))

## [0.7.7](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.6...aether-agent-cli-v0.7.7) - 2026-05-21

### Added

- *(aether-cli)* Prompt history search ([#96](https://github.com/contextbridge/aether/pull/96))

## [0.7.6](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.5...aether-agent-cli-v0.7.6) - 2026-05-18

### Fixed

- *(aether-cli)* Crashes due to eagin being fatal in transport ([#87](https://github.com/contextbridge/aether/pull/87))

### Other

- Upgrade dependencies to latest ([#83](https://github.com/contextbridge/aether/pull/83))

## [0.7.5](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.4...aether-agent-cli-v0.7.5) - 2026-05-18

### Fixed

- *(wisp)* Allow copying URLs when performing MCP auth  ([#82](https://github.com/contextbridge/aether/pull/82))

### Other

- replace Box<dyn Error> with typed error enums, remove excessive comments ([#81](https://github.com/contextbridge/aether/pull/81))

## [0.7.4](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.3...aether-agent-cli-v0.7.4) - 2026-05-16

### Fixed

- *(llm)* Use prompt caching on Bedrock models that support prompt caching ([#75](https://github.com/contextbridge/aether/pull/75))

## [0.7.3](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.2...aether-agent-cli-v0.7.3) - 2026-05-15

### Fixed

- *(aether-cli)* Crashes due to tokio stdin surfacing eagin and acp transport treating that as fatal ([#74](https://github.com/contextbridge/aether/pull/74))

## [0.7.2](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.1...aether-agent-cli-v0.7.2) - 2026-05-15

### Fixed

- *(wisp)* Improve perf of /resume session by buffering updates instead of individually rendering each update ([#61](https://github.com/contextbridge/aether/pull/61))

## [0.7.1](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.7.0...aether-agent-cli-v0.7.1) - 2026-05-14

### Other

- updated the following local packages: aether-llm, aether-mcp-servers, aether-mcp-utils, aether-acp-utils, aether-acp-utils, aether-agent-core, aether-project, aether-wisp

## [0.7.0](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.6.3...aether-agent-cli-v0.7.0) - 2026-05-14

### Fixed

- *(aether-core)* Give users escape hatch to set custom context window limit and set provider urls disable auth (useful for bedrock sigv4 proxy)

## [0.6.3](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.6.2...aether-agent-cli-v0.6.3) - 2026-05-13

### Fixed

- *(aether-cli)* When --agent is passed, also resolve bedrock model inference arns and unify how --agent and --model check to see if a model exists

## [0.6.2](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.6.1...aether-agent-cli-v0.6.2) - 2026-05-13

### Fixed

- *(aether-cli)* Support bedrock instance profile arns, which was fixed upstream in llm package

## [0.6.1](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.6.0...aether-agent-cli-v0.6.1) - 2026-05-13

### Other

- updated the following local packages: aether-llm, aether-mcp-utils, aether-acp-utils, aether-acp-utils, aether-agent-core, aether-project, aether-wisp, aether-mcp-servers

## [0.6.0](https://github.com/contextbridge/aether/compare/aether-agent-cli-v0.5.3...aether-agent-cli-v0.6.0) - 2026-05-13

### Other

- *(keyring)* Add aether-keyring crate, extract OAuthCredentialStorage, and make creds store lazily initialized

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
