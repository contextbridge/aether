# Changelog

All notable changes to this project will be documented in this file.

## [0.5.26](https://github.com/contextbridge/aether/compare/aether-project-v0.5.25...aether-project-v0.5.26) - 2026-07-27

### Other

- update Cargo.lock dependencies

## [0.5.25](https://github.com/contextbridge/aether/compare/aether-project-v0.5.24...aether-project-v0.5.25) - 2026-07-21

### Other

- update Cargo.lock dependencies

## [0.5.24](https://github.com/contextbridge/aether/compare/aether-project-v0.5.23...aether-project-v0.5.24) - 2026-07-20

### Other

- updated the following local packages: aether-llm, aether-agent-core, aether-mcp-utils

## [0.5.23](https://github.com/contextbridge/aether/compare/aether-project-v0.5.22...aether-project-v0.5.23) - 2026-07-20

### Added

- *(telemetry)* interpolate OTLP header variables ([#238](https://github.com/contextbridge/aether/pull/238))

## [0.5.22](https://github.com/contextbridge/aether/compare/aether-project-v0.5.21...aether-project-v0.5.22) - 2026-07-13

### Other

- reduce development compile costs ([#231](https://github.com/contextbridge/aether/pull/231))

## [0.5.21](https://github.com/contextbridge/aether/compare/aether-project-v0.5.20...aether-project-v0.5.21) - 2026-07-13

### Added

- *(aether-cli)* Support exact trace and metric endpoints for otel e… ([#228](https://github.com/contextbridge/aether/pull/228))

## [0.5.20](https://github.com/contextbridge/aether/compare/aether-project-v0.5.19...aether-project-v0.5.20) - 2026-07-13

### Added

- *(aether-cli)* Add support for exporting genai OTEL traces ([#219](https://github.com/contextbridge/aether/pull/219))

## [0.5.19](https://github.com/contextbridge/aether/compare/aether-project-v0.5.18...aether-project-v0.5.19) - 2026-07-09

### Other

- Update models ([#214](https://github.com/contextbridge/aether/pull/214))

## [0.5.18](https://github.com/contextbridge/aether/compare/aether-project-v0.5.17...aether-project-v0.5.18) - 2026-06-30

### Other

- updated the following local packages: aether-llm, aether-mcp-utils, aether-agent-core

## [0.5.17](https://github.com/contextbridge/aether/compare/aether-project-v0.5.16...aether-project-v0.5.17) - 2026-06-22

### Other

- updated the following local packages: aether-llm, aether-mcp-utils, aether-agent-core

## [0.5.16](https://github.com/contextbridge/aether/compare/aether-project-v0.5.15...aether-project-v0.5.16) - 2026-06-22

### Other

- *(workspace)* Move Rust to crates/ and TS to packages/ ([#175](https://github.com/contextbridge/aether/pull/175))

## [0.5.15](https://github.com/contextbridge/aether/compare/aether-project-v0.5.14...aether-project-v0.5.15) - 2026-06-19

### Added

- *(aether-cli)* Add model settings to be able to control temperature, top p etc ([#168](https://github.com/contextbridge/aether/pull/168))

## [0.5.14](https://github.com/contextbridge/aether/compare/aether-project-v0.5.13...aether-project-v0.5.14) - 2026-06-18

### Other

- updated the following local packages: aether-llm, aether-mcp-utils, aether-agent-core

## [0.5.13](https://github.com/contextbridge/aether/compare/aether-project-v0.5.12...aether-project-v0.5.13) - 2026-06-18

### Other

- Fix ci by re-adding missing schema generator ([#163](https://github.com/contextbridge/aether/pull/163))
- Better experience for authoring evals ([#162](https://github.com/contextbridge/aether/pull/162))
- Move a bunch of errors to using thiserror ([#160](https://github.com/contextbridge/aether/pull/160))

## [0.5.12](https://github.com/contextbridge/aether/compare/aether-project-v0.5.11...aether-project-v0.5.12) - 2026-06-13

### Other

- updated the following local packages: aether-llm, aether-mcp-utils, aether-agent-core

## [0.5.11](https://github.com/contextbridge/aether/compare/aether-project-v0.5.10...aether-project-v0.5.11) - 2026-06-13

### Added

- *(aether-cli)* Allow filtering mcp tools by annotation ([#151](https://github.com/contextbridge/aether/pull/151))

### Other

- release ([#130](https://github.com/contextbridge/aether/pull/130))

## [0.5.10](https://github.com/contextbridge/aether/compare/aether-project-v0.5.9...aether-project-v0.5.10) - 2026-06-10

### Added

- *(aether-cli)* Add evals command ([#142](https://github.com/contextbridge/aether/pull/142))

### Other

- *(aether-cli)* Make aether cli headless mode output serialized AgentMessages instead of abusing the tracing crate output format and run evals in isolated Docker containers ([#141](https://github.com/contextbridge/aether/pull/141))
- Upgrade deps ([#140](https://github.com/contextbridge/aether/pull/140))

## [0.5.9](https://github.com/contextbridge/aether/compare/aether-project-v0.5.8...aether-project-v0.5.9) - 2026-06-04

### Added

- *(aether-cli)* Support encrypted file store for oauth for users that do not want full keyring ([#124](https://github.com/contextbridge/aether/pull/124))

### Other

- update docs ([#122](https://github.com/contextbridge/aether/pull/122))

## [0.5.8](https://github.com/contextbridge/aether/compare/aether-project-v0.5.7...aether-project-v0.5.8) - 2026-05-31

### Added

- Add user level settings resolution  ([#99](https://github.com/contextbridge/aether/pull/99))

### Fixed

- *(aether-cli)* Onboarding ([#112](https://github.com/contextbridge/aether/pull/112))

## [0.5.7](https://github.com/contextbridge/aether/compare/aether-project-v0.5.6...aether-project-v0.5.7) - 2026-05-21

### Other

- update Cargo.lock dependencies

## [0.5.6](https://github.com/contextbridge/aether/compare/aether-project-v0.5.5...aether-project-v0.5.6) - 2026-05-18

### Other

- update Cargo.lock dependencies

## [0.5.5](https://github.com/contextbridge/aether/compare/aether-project-v0.5.4...aether-project-v0.5.5) - 2026-05-18

### Other

- updated the following local packages: aether-mcp-utils, aether-agent-core

## [0.5.4](https://github.com/contextbridge/aether/compare/aether-project-v0.5.3...aether-project-v0.5.4) - 2026-05-16

### Other

- updated the following local packages: aether-llm, aether-mcp-utils, aether-agent-core

## [0.5.3](https://github.com/contextbridge/aether/compare/aether-project-v0.5.2...aether-project-v0.5.3) - 2026-05-15

### Other

- updated the following local packages: aether-llm, aether-mcp-utils, aether-agent-core

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
