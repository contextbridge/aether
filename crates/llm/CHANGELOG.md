# Changelog

All notable changes to this project will be documented in this file.

## [0.7.19](https://github.com/contextbridge/aether/compare/aether-llm-v0.7.18...aether-llm-v0.7.19) - 2026-07-13

### Other

- Update models ([#226](https://github.com/contextbridge/aether/pull/226))

## [0.7.18](https://github.com/contextbridge/aether/compare/aether-llm-v0.7.17...aether-llm-v0.7.18) - 2026-07-13

### Added

- *(aether-cli)* Add support for exporting genai OTEL traces ([#219](https://github.com/contextbridge/aether/pull/219))

### Fixed

- send supported Codex protocol version ([#215](https://github.com/contextbridge/aether/pull/215))

### Other

- Rename AgentMessage => AgentEvent and better organize variants ([#217](https://github.com/contextbridge/aether/pull/217))

## [0.7.17](https://github.com/contextbridge/aether/compare/aether-llm-v0.7.16...aether-llm-v0.7.17) - 2026-07-09

### Other

- Update models ([#214](https://github.com/contextbridge/aether/pull/214))

## [0.7.16](https://github.com/contextbridge/aether/compare/aether-llm-v0.7.15...aether-llm-v0.7.16) - 2026-06-30

### Other

- Update models ([#205](https://github.com/contextbridge/aether/pull/205))

## [0.7.15](https://github.com/contextbridge/aether/compare/aether-llm-v0.7.14...aether-llm-v0.7.15) - 2026-06-22

### Other

- update models ([#193](https://github.com/contextbridge/aether/pull/193))

## [0.7.14](https://github.com/contextbridge/aether/compare/aether-llm-v0.7.13...aether-llm-v0.7.14) - 2026-06-19

### Added

- *(aether-cli)* Add model settings to be able to control temperature, top p etc ([#168](https://github.com/contextbridge/aether/pull/168))

## [0.7.13](https://github.com/contextbridge/aether/compare/aether-llm-v0.7.12...aether-llm-v0.7.13) - 2026-06-18

### Added

- Improved support for evals ([#167](https://github.com/contextbridge/aether/pull/167))

## [0.7.12](https://github.com/contextbridge/aether/compare/aether-llm-v0.7.11...aether-llm-v0.7.12) - 2026-06-18

### Other

- update models ([#158](https://github.com/contextbridge/aether/pull/158))

## [0.7.11](https://github.com/contextbridge/aether/compare/aether-llm-v0.7.10...aether-llm-v0.7.11) - 2026-06-13

### Other

- update models ([#153](https://github.com/contextbridge/aether/pull/153))

## [0.7.10](https://github.com/contextbridge/aether/compare/aether-llm-v0.7.9...aether-llm-v0.7.10) - 2026-06-13

### Added

- *(aether-cli)* Allow filtering mcp tools by annotation ([#151](https://github.com/contextbridge/aether/pull/151))

### Other

- update models ([#149](https://github.com/contextbridge/aether/pull/149))
- pin deps to prevent build error ([#148](https://github.com/contextbridge/aether/pull/148))

## [0.7.9](https://github.com/contextbridge/aether/compare/aether-llm-v0.7.8...aether-llm-v0.7.9) - 2026-06-10

### Added

- *(aether-cli)* Add evals command ([#142](https://github.com/contextbridge/aether/pull/142))

### Other

- update models ([#143](https://github.com/contextbridge/aether/pull/143))
- update models ([#135](https://github.com/contextbridge/aether/pull/135))

## [0.7.8](https://github.com/contextbridge/aether/compare/aether-llm-v0.7.7...aether-llm-v0.7.8) - 2026-06-04

### Other

- update docs ([#122](https://github.com/contextbridge/aether/pull/122))

## [0.7.7](https://github.com/contextbridge/aether/compare/aether-llm-v0.7.6...aether-llm-v0.7.7) - 2026-05-31

### Fixed

- *(aether-cli)* Onboarding ([#112](https://github.com/contextbridge/aether/pull/112))
- *(aether-cli)* Update system prompts and mcp server connections when switching agents ([#110](https://github.com/contextbridge/aether/pull/110))
- *(llm)* Codex provider streaming / parsing ([#108](https://github.com/contextbridge/aether/pull/108))
- *(aether-cli)* Start MCP servers concurrently to avoid blocking TUI ([#106](https://github.com/contextbridge/aether/pull/106))

## [0.7.6](https://github.com/contextbridge/aether/compare/aether-llm-v0.7.5...aether-llm-v0.7.6) - 2026-05-21

### Other

- update models to get Gemini 3.5 flash ([#91](https://github.com/contextbridge/aether/pull/91))

## [0.7.5](https://github.com/contextbridge/aether/compare/aether-llm-v0.7.4...aether-llm-v0.7.5) - 2026-05-18

### Other

- update Cargo.toml dependencies

## [0.7.4](https://github.com/contextbridge/aether/compare/aether-llm-v0.7.3...aether-llm-v0.7.4) - 2026-05-16

### Fixed

- *(llm)* Use prompt caching on Bedrock models that support prompt caching ([#75](https://github.com/contextbridge/aether/pull/75))

### Other

- *(llm-codegen)* Cleanup string munging and use quote crate ([#77](https://github.com/contextbridge/aether/pull/77))

## [0.7.3](https://github.com/contextbridge/aether/compare/aether-llm-v0.7.2...aether-llm-v0.7.3) - 2026-05-15

### Fixed

- *(mcp)* populate token_received_at in MCP credential store to enable rmcp refresh ([#72](https://github.com/contextbridge/aether/pull/72))

## [0.7.2](https://github.com/contextbridge/aether/compare/aether-llm-v0.7.1...aether-llm-v0.7.2) - 2026-05-15

### Fixed

- *(llm)* Refresh Codex auth tokens ([#59](https://github.com/contextbridge/aether/pull/59))

## [0.7.1](https://github.com/contextbridge/aether/compare/aether-llm-v0.7.0...aether-llm-v0.7.1) - 2026-05-14

### Fixed

- *(llm)* When bedrock provider is requested to have no auth (e.g. for a proxy), construct the bedrock client with no credentials

## [0.7.0](https://github.com/contextbridge/aether/compare/aether-llm-v0.6.0...aether-llm-v0.7.0) - 2026-05-14

### Fixed

- *(aether-core)* Give users escape hatch to set custom context window limit and set provider urls disable auth (useful for bedrock sigv4 proxy)

## [0.6.0](https://github.com/contextbridge/aether/compare/aether-llm-v0.5.0...aether-llm-v0.6.0) - 2026-05-13

### Added

- *(llm)* Support bedrock inferance profile arns in model strings

## [0.5.0](https://github.com/contextbridge/aether/compare/aether-llm-v0.4.0...aether-llm-v0.5.0) - 2026-05-13

### Other

- *(keyring)* Add aether-keyring crate, extract OAuthCredentialStorage, and make creds store lazily initialized
- *(llm)* Updatem models and async openai
- *(llm)* Update models

## [0.4.0](https://github.com/contextbridge/aether/compare/aether-llm-v0.3.0...aether-llm-v0.4.0) - 2026-05-12

### Fixed

- *(llm)* Retry llm calls on more retryable failures for bedrock, codex and openai compatible providers

## [0.3.0](https://github.com/contextbridge/aether/compare/aether-llm-v0.2.7...aether-llm-v0.3.0) - 2026-05-08

### Other

- *(workspace)* Upgrade deps and to keyring 4.x

## [0.2.7](https://github.com/contextbridge/aether/compare/aether-llm-v0.2.6...aether-llm-v0.2.7) - 2026-05-05

### Other

- port to contextbridge org

## [0.2.6](https://github.com/contextbridge/aether/compare/aether-llm-v0.2.5...aether-llm-v0.2.6) - 2026-05-03

### Fixed

- *(llm)* Set codex context window limit for gpt to 272k subscription limit

### Other

- *(aether-evals)* Simplify aether-evals to rely on normal rust tests and cargo next test

## [0.2.5](https://github.com/contextbridge/aether/compare/aether-llm-v0.2.4...aether-llm-v0.2.5) - 2026-04-29

### Fixed

- *(aether-cli)* Auto retry on llm errors

## [aether-llm-v0.2.4] - 2026-04-27
