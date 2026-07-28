# Changelog

All notable changes to this project will be documented in this file.

## [0.4.33](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.32...aether-mcp-servers-v0.4.33) - 2026-07-28

### Other

- isolate LSP timeout recovery coverage ([#300](https://github.com/contextbridge/aether/pull/300))

## [0.4.32](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.31...aether-mcp-servers-v0.4.32) - 2026-07-28

### Other

- Seepdup tests ([#294](https://github.com/contextbridge/aether/pull/294))

## [0.4.31](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.30...aether-mcp-servers-v0.4.31) - 2026-07-27

### Added

- recover from wedged or crashed language servers in lspd ([#275](https://github.com/contextbridge/aether/pull/275))

### Fixed

- Set cache key based on prompt contents. ([#279](https://github.com/contextbridge/aether/pull/279))

### Other

- scheduled code-cleanup ([#291](https://github.com/contextbridge/aether/pull/291))
- scheduled code-cleanup ([#277](https://github.com/contextbridge/aether/pull/277))
- scheduled code-cleanup ([#272](https://github.com/contextbridge/aether/pull/272))
- release ([#269](https://github.com/contextbridge/aether/pull/269))

## [0.4.30](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.29...aether-mcp-servers-v0.4.30) - 2026-07-21

### Other

- updated the following local packages: aether-lspd, aether-lspd

## [0.4.29](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.28...aether-mcp-servers-v0.4.29) - 2026-07-20

### Other

- updated the following local packages: aether-llm, aether-agent-core, aether-mcp-utils, aether-mcp-utils, aether-project

## [0.4.28](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.27...aether-mcp-servers-v0.4.28) - 2026-07-20

### Added

- strengthen LSP tool workflows ([#241](https://github.com/contextbridge/aether/pull/241))

### Fixed

- *(mcp-servers)* Make web search tool retry on rate limit ([#232](https://github.com/contextbridge/aether/pull/232))

### Other

- scheduled code-cleanup ([#253](https://github.com/contextbridge/aether/pull/253))

## [0.4.27](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.26...aether-mcp-servers-v0.4.27) - 2026-07-13

### Other

- reduce development compile costs ([#231](https://github.com/contextbridge/aether/pull/231))

## [0.4.26](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.25...aether-mcp-servers-v0.4.26) - 2026-07-13

### Other

- updated the following local packages: aether-llm, aether-project, aether-mcp-utils, aether-agent-core

## [0.4.25](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.24...aether-mcp-servers-v0.4.25) - 2026-07-13

### Added

- *(aether-cli)* Add support for exporting genai OTEL traces ([#219](https://github.com/contextbridge/aether/pull/219))

### Other

- fix lspd tests
- cleanup tests ([#218](https://github.com/contextbridge/aether/pull/218))
- Rename AgentMessage => AgentEvent and better organize variants ([#217](https://github.com/contextbridge/aether/pull/217))

## [0.4.24](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.23...aether-mcp-servers-v0.4.24) - 2026-07-09

### Other

- Update models ([#214](https://github.com/contextbridge/aether/pull/214))

## [0.4.23](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.22...aether-mcp-servers-v0.4.23) - 2026-06-30

### Added

- *(mcp-servers)* Improve find tool for agents ([#200](https://github.com/contextbridge/aether/pull/200))

### Fixed

- *(mcp-servers)* Make agent fail fast when asked to modify files after using "/plan" cmd from plan MCP ([#207](https://github.com/contextbridge/aether/pull/207))

### Other

- *(mcp-servers)* Remove MCP roots functionality  ([#199](https://github.com/contextbridge/aether/pull/199))
- *(aether-cli)* Remove old notes tools as they're subsumed by skills and rules ([#195](https://github.com/contextbridge/aether/pull/195))

## [0.4.22](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.21...aether-mcp-servers-v0.4.22) - 2026-06-22

### Other

- updated the following local packages: aether-llm, aether-lspd, aether-lspd, aether-mcp-utils, aether-agent-core, aether-project

## [0.4.21](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.20...aether-mcp-servers-v0.4.21) - 2026-06-22

### Other

- Dry up tests in mcp-servers and wisp with test builders ([#182](https://github.com/contextbridge/aether/pull/182))

## [0.4.20](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.19...aether-mcp-servers-v0.4.20) - 2026-06-19

### Added

- *(mcp-servers)* Make edit_file and edit_plan support batched edits ([#170](https://github.com/contextbridge/aether/pull/170))

## [0.4.19](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.18...aether-mcp-servers-v0.4.19) - 2026-06-18

### Added

- *(mcp-servers)* Add ast-grep tool ([#165](https://github.com/contextbridge/aether/pull/165))

## [0.4.18](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.17...aether-mcp-servers-v0.4.18) - 2026-06-18

### Other

- Move a bunch of errors to using thiserror ([#160](https://github.com/contextbridge/aether/pull/160))

## [0.4.17](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.16...aether-mcp-servers-v0.4.17) - 2026-06-13

### Other

- updated the following local packages: aether-llm, aether-lspd, aether-lspd, aether-mcp-utils, aether-agent-core, aether-project

## [0.4.16](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.15...aether-mcp-servers-v0.4.16) - 2026-06-13

### Added

- *(aether-cli)* Allow filtering mcp tools by annotation ([#151](https://github.com/contextbridge/aether/pull/151))
- *(aether-cli)* Add /move command to switch workspaces and bring your session + changes with you. ([#150](https://github.com/contextbridge/aether/pull/150))

## [0.4.15](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.14...aether-mcp-servers-v0.4.15) - 2026-06-10

### Added

- *(aether-cli)* Add evals command ([#142](https://github.com/contextbridge/aether/pull/142))

## [0.4.14](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.13...aether-mcp-servers-v0.4.14) - 2026-06-05

### Fixed

- model override switches to default agent ([#128](https://github.com/contextbridge/aether/pull/128))

## [0.4.13](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.12...aether-mcp-servers-v0.4.13) - 2026-06-04

### Added

- *(aether-cli)* Support encrypted file store for oauth for users that do not want full keyring ([#124](https://github.com/contextbridge/aether/pull/124))

### Fixed

- *(aether-cli)* Default Plan edit unable to create/edit plan files ([#125](https://github.com/contextbridge/aether/pull/125))

## [0.4.12](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.11...aether-mcp-servers-v0.4.12) - 2026-05-31

### Added

- Add user level settings resolution  ([#99](https://github.com/contextbridge/aether/pull/99))

### Fixed

- *(aether-cli)* Onboarding ([#112](https://github.com/contextbridge/aether/pull/112))
- *(aether-cli)* Update system prompts and mcp server connections when switching agents ([#110](https://github.com/contextbridge/aether/pull/110))
- *(mcp-servers)* Make sub-agents MCP work with LLM providers that require an OAuth store ([#107](https://github.com/contextbridge/aether/pull/107))
- *(aether-cli)* Start MCP servers concurrently to avoid blocking TUI ([#106](https://github.com/contextbridge/aether/pull/106))

## [0.4.11](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.10...aether-mcp-servers-v0.4.11) - 2026-05-21

### Other

- update Cargo.lock dependencies

## [0.4.10](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.9...aether-mcp-servers-v0.4.10) - 2026-05-18

### Other

- update Cargo.toml dependencies

## [0.4.9](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.8...aether-mcp-servers-v0.4.9) - 2026-05-18

### Other

- updated the following local packages: aether-mcp-utils, aether-agent-core, aether-project

## [0.4.8](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.7...aether-mcp-servers-v0.4.8) - 2026-05-16

### Other

- updated the following local packages: aether-llm, aether-mcp-utils, aether-agent-core, aether-project

## [0.4.7](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.6...aether-mcp-servers-v0.4.7) - 2026-05-15

### Other

- updated the following local packages: aether-llm, aether-mcp-utils, aether-agent-core, aether-project

## [0.4.6](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.5...aether-mcp-servers-v0.4.6) - 2026-05-15

### Other

- updated the following local packages: aether-llm, aether-mcp-utils, aether-lspd, aether-lspd, aether-agent-core, aether-project

## [0.4.5](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.4...aether-mcp-servers-v0.4.5) - 2026-05-14

### Other

- *(mcp-servers)* Update default plan prompt ([#52](https://github.com/contextbridge/aether/pull/52))

## [0.4.4](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.3...aether-mcp-servers-v0.4.4) - 2026-05-14

### Other

- update Cargo.lock dependencies

## [0.4.3](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.2...aether-mcp-servers-v0.4.3) - 2026-05-13

### Other

- updated the following local packages: aether-llm, aether-mcp-utils, aether-agent-core, aether-project

## [0.4.2](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.1...aether-mcp-servers-v0.4.2) - 2026-05-13

### Other

- *(keyring)* Add aether-keyring crate, extract OAuthCredentialStorage, and make creds store lazily initialized

## [0.4.1](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.4.0...aether-mcp-servers-v0.4.1) - 2026-05-12

### Other

- update Cargo.toml dependencies

## [0.4.0](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.3.6...aether-mcp-servers-v0.4.0) - 2026-05-11

### Added

- *(mcp-servers)* Coding mcp gains lsp workspace search, and remove confusing lsp/coding server overlap

## [0.3.6](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.3.5...aether-mcp-servers-v0.3.6) - 2026-05-08

### Other

- *(mcp-utils)* Rewrite mcp config to better use serde, schemars, and enforce 1 proxy instance

## [0.3.5](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.3.4...aether-mcp-servers-v0.3.5) - 2026-05-05

### Other

- port to contextbridge org

## [0.3.4](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.3.3...aether-mcp-servers-v0.3.4) - 2026-05-05

### Other

- update Cargo.lock dependencies

## [0.3.3](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.3.2...aether-mcp-servers-v0.3.3) - 2026-05-03

### Added

- *(aether-cli)* Support user-level settings

### Other

- *(aether-cli)* Resolve user-level settings from aether home
- *(mcp-servers)* fix flaky test

## [0.3.2](https://github.com/contextbridge/aether/compare/aether-mcp-servers-v0.3.1...aether-mcp-servers-v0.3.2) - 2026-04-29

### Added

- *(mcp-servers)* Allow disabling lsp on coding server via config flag

### Other

- *(aether-cli)* Support strings in settings as file paths
- Re-add top level prompt and mcp settings
- More consistently use the term settings over config
- *(mcp-servers)* Use new config structs from core/project

## [aether-mcp-servers-v0.3.1] - 2026-04-27
