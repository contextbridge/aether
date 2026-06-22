# Changelog

All notable changes to this project will be documented in this file.

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
