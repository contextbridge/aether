# Changelog

All notable changes to this project will be documented in this file.

## [0.4.32](https://github.com/contextbridge/aether/compare/aether-wisp-v0.4.31...aether-wisp-v0.4.32) - 2026-07-20

### Added

- stage and unstage directories in the git diff file tree ([#249](https://github.com/contextbridge/aether/pull/249))
- *(wisp)* add staged/unstaged/both scope toggle to git diff mode ([#237](https://github.com/contextbridge/aether/pull/237))

### Fixed

- keep file picker open when pasting ([#250](https://github.com/contextbridge/aether/pull/250))
- keep agent sessions responsive during in-flight work ([#235](https://github.com/contextbridge/aether/pull/235))

### Other

- scheduled code-cleanup ([#246](https://github.com/contextbridge/aether/pull/246))
- streamline agent and MCP integration tests ([#236](https://github.com/contextbridge/aether/pull/236))

## [0.4.31](https://github.com/contextbridge/aether/compare/aether-wisp-v0.4.30...aether-wisp-v0.4.31) - 2026-07-13

### Other

- reduce development compile costs ([#231](https://github.com/contextbridge/aether/pull/231))

## [0.4.30](https://github.com/contextbridge/aether/compare/aether-wisp-v0.4.29...aether-wisp-v0.4.30) - 2026-07-13

### Other

- updated the following local packages: aether-acp-utils, aether-acp-utils

## [0.4.29](https://github.com/contextbridge/aether/compare/aether-wisp-v0.4.28...aether-wisp-v0.4.29) - 2026-07-13

### Other

- Rename AgentMessage => AgentEvent and better organize variants ([#217](https://github.com/contextbridge/aether/pull/217))

## [0.4.28](https://github.com/contextbridge/aether/compare/aether-wisp-v0.4.27...aether-wisp-v0.4.28) - 2026-07-09

### Other

- Update models ([#214](https://github.com/contextbridge/aether/pull/214))

## [0.4.27](https://github.com/contextbridge/aether/compare/aether-wisp-v0.4.26...aether-wisp-v0.4.27) - 2026-06-30

### Added

- *(wisp)* Git controls for staging/unstaging and commiting in git d… ([#201](https://github.com/contextbridge/aether/pull/201))

### Other

- *(mcp-servers)* Remove MCP roots functionality  ([#199](https://github.com/contextbridge/aether/pull/199))

## [0.4.26](https://github.com/contextbridge/aether/compare/aether-wisp-v0.4.25...aether-wisp-v0.4.26) - 2026-06-22

### Other

- updated the following local packages: aether-acp-utils, aether-acp-utils

## [0.4.25](https://github.com/contextbridge/aether/compare/aether-wisp-v0.4.24...aether-wisp-v0.4.25) - 2026-06-22

### Other

- Dry up tests in mcp-servers and wisp with test builders ([#182](https://github.com/contextbridge/aether/pull/182))
- *(workspace)* Move Rust to crates/ and TS to packages/ ([#175](https://github.com/contextbridge/aether/pull/175))

## [0.4.24](https://github.com/contextbridge/aether/compare/aether-wisp-v0.4.23...aether-wisp-v0.4.24) - 2026-06-19

### Other

- updated the following local packages: aether-acp-utils, aether-acp-utils

## [0.4.23](https://github.com/contextbridge/aether/compare/aether-wisp-v0.4.22...aether-wisp-v0.4.23) - 2026-06-18

### Other

- updated the following local packages: aether-acp-utils, aether-acp-utils

## [0.4.22](https://github.com/contextbridge/aether/compare/aether-wisp-v0.4.21...aether-wisp-v0.4.22) - 2026-06-18

### Added

- *(aether-evals)* Drive dockerized agents under eval via ACP ([#161](https://github.com/contextbridge/aether/pull/161))

### Fixed

- Small fixes ([#164](https://github.com/contextbridge/aether/pull/164))

### Other

- Move a bunch of errors to using thiserror ([#160](https://github.com/contextbridge/aether/pull/160))

## [0.4.21](https://github.com/contextbridge/aether/compare/aether-wisp-v0.4.20...aether-wisp-v0.4.21) - 2026-06-13

### Other

- updated the following local packages: aether-acp-utils, aether-acp-utils

## [0.4.20](https://github.com/contextbridge/aether/compare/aether-wisp-v0.4.19...aether-wisp-v0.4.20) - 2026-06-13

### Added

- *(aether-cli)* Add /move command to switch workspaces and bring your session + changes with you. ([#150](https://github.com/contextbridge/aether/pull/150))
- *(wisp)* Better git diff and plan views ([#146](https://github.com/contextbridge/aether/pull/146))
- *(aether-cli)* Better session resume menu ([#145](https://github.com/contextbridge/aether/pull/145))

### Fixed

- *(wisp)* Mouse scroll in git diff view no longer jumps ([#147](https://github.com/contextbridge/aether/pull/147))

## [0.4.19](https://github.com/contextbridge/aether/compare/aether-wisp-v0.4.18...aether-wisp-v0.4.19) - 2026-06-10

### Added

- *(wisp)* Allow configuring status lines via settings ([#132](https://github.com/contextbridge/aether/pull/132))

## [0.4.18](https://github.com/contextbridge/aether/compare/aether-wisp-v0.4.17...aether-wisp-v0.4.18) - 2026-06-05

### Fixed

- Status line wrapping ([#129](https://github.com/contextbridge/aether/pull/129))

## [0.4.17](https://github.com/contextbridge/aether/compare/aether-wisp-v0.4.16...aether-wisp-v0.4.17) - 2026-06-04

### Other

- update Cargo.lock dependencies

## [0.4.16](https://github.com/contextbridge/aether/compare/aether-wisp-v0.4.15...aether-wisp-v0.4.16) - 2026-05-31

### Added

- *(wisp)* Terminal bell on agent idle ([#98](https://github.com/contextbridge/aether/pull/98))

### Fixed

- *(aether-cli)* Update system prompts and mcp server connections when switching agents ([#110](https://github.com/contextbridge/aether/pull/110))
- *(wisp)* allow spacebar in /resume session picker ([#105](https://github.com/contextbridge/aether/pull/105))
- *(aether-cli)* Start MCP servers concurrently to avoid blocking TUI ([#106](https://github.com/contextbridge/aether/pull/106))

## [0.4.15](https://github.com/contextbridge/aether/compare/aether-wisp-v0.4.14...aether-wisp-v0.4.15) - 2026-05-21

### Added

- *(aether-cli)* Prompt history search ([#96](https://github.com/contextbridge/aether/pull/96))

### Fixed

- *(wisp)* Make multi line prompt composer work properly on a Mac ([#95](https://github.com/contextbridge/aether/pull/95))

## [0.4.14](https://github.com/contextbridge/aether/compare/aether-wisp-v0.4.13...aether-wisp-v0.4.14) - 2026-05-18

### Other

- update Cargo.toml dependencies

## [0.4.13](https://github.com/contextbridge/aether/compare/aether-wisp-v0.4.12...aether-wisp-v0.4.13) - 2026-05-18

### Fixed

- *(wisp)* Allow copying URLs when performing MCP auth  ([#82](https://github.com/contextbridge/aether/pull/82))
- *(wisp)* Show cwd and git branch to avoid confusing users with multiple terminal tabs open ([#79](https://github.com/contextbridge/aether/pull/79))

## [0.4.12](https://github.com/contextbridge/aether/compare/aether-wisp-v0.4.11...aether-wisp-v0.4.12) - 2026-05-16

### Other

- updated the following local packages: aether-acp-utils, aether-acp-utils

## [0.4.11](https://github.com/contextbridge/aether/compare/aether-wisp-v0.4.10...aether-wisp-v0.4.11) - 2026-05-15

### Fixed

- *(wisp)* Allow multi line input in prompt via shift+tab ([#73](https://github.com/contextbridge/aether/pull/73))
- make Tab key confirm selection in picker components ([#69](https://github.com/contextbridge/aether/pull/69))

## [0.4.10](https://github.com/contextbridge/aether/compare/aether-wisp-v0.4.9...aether-wisp-v0.4.10) - 2026-05-15

### Fixed

- *(wisp)* More visible search bar in resume session ([#62](https://github.com/contextbridge/aether/pull/62))
- *(wisp)* Show dot files in "@" search ([#63](https://github.com/contextbridge/aether/pull/63))
- *(wisp)* Improve perf of /resume session by buffering updates instead of individually rendering each update ([#61](https://github.com/contextbridge/aether/pull/61))

## [0.4.9](https://github.com/contextbridge/aether/compare/aether-wisp-v0.4.8...aether-wisp-v0.4.9) - 2026-05-14

### Other

- updated the following local packages: aether-acp-utils, aether-acp-utils

## [0.4.8](https://github.com/contextbridge/aether/compare/aether-wisp-v0.4.7...aether-wisp-v0.4.8) - 2026-05-14

### Other

- update Cargo.lock dependencies

## [0.4.7](https://github.com/contextbridge/aether/compare/aether-wisp-v0.4.6...aether-wisp-v0.4.7) - 2026-05-13

### Other

- updated the following local packages: aether-acp-utils, aether-acp-utils

## [0.4.6](https://github.com/contextbridge/aether/compare/aether-wisp-v0.4.5...aether-wisp-v0.4.6) - 2026-05-13

### Other

- update Cargo.lock dependencies

## [0.4.5](https://github.com/contextbridge/aether/compare/aether-wisp-v0.4.4...aether-wisp-v0.4.5) - 2026-05-12

### Other

- update Cargo.toml dependencies

## [0.4.4](https://github.com/contextbridge/aether/compare/aether-wisp-v0.4.3...aether-wisp-v0.4.4) - 2026-05-08

### Other

- *(wisp)* DRY up tests with better helpers

## [0.4.3](https://github.com/contextbridge/aether/compare/aether-wisp-v0.4.2...aether-wisp-v0.4.3) - 2026-05-08

### Added

- *(aether-cli)* Render proxied MCP servers in a separate list from non-proxied MCPs in settings menu

### Fixed

- *(mcp-servers)* Allow concurrent mcp auth requests

## [0.4.2](https://github.com/contextbridge/aether/compare/aether-wisp-v0.4.1...aether-wisp-v0.4.2) - 2026-05-05

### Other

- port to contextbridge org

## [0.4.1](https://github.com/contextbridge/aether/compare/aether-wisp-v0.4.0...aether-wisp-v0.4.1) - 2026-05-05

### Fixed

- *(mcp-utils)* Allow re-authing proxied mcps

## [0.4.0](https://github.com/contextbridge/aether/compare/aether-wisp-v0.3.3...aether-wisp-v0.4.0) - 2026-05-04

### Other

- *(wisp)* Improve rendering performance of git diff view
- *(wisp)* Batch event renders

## [0.3.3](https://github.com/contextbridge/aether/compare/aether-wisp-v0.3.2...aether-wisp-v0.3.3) - 2026-05-03

### Other

- updated the following local packages: aether-acp-utils, aether-acp-utils

## [0.3.2](https://github.com/contextbridge/aether/compare/aether-wisp-v0.3.1...aether-wisp-v0.3.2) - 2026-04-29

### Other

- updated the following local packages: aether-utils, aether-tui, aether-tui, aether-acp-utils, aether-acp-utils

## [aether-wisp-v0.3.1] - 2026-04-27
