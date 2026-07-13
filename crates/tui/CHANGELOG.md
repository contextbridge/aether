# Changelog

All notable changes to this project will be documented in this file.

## [0.2.16](https://github.com/contextbridge/aether/compare/aether-tui-v0.2.15...aether-tui-v0.2.16) - 2026-07-13

### Other

- cleanup tests ([#218](https://github.com/contextbridge/aether/pull/218))

## [0.2.15](https://github.com/contextbridge/aether/compare/aether-tui-v0.2.14...aether-tui-v0.2.15) - 2026-06-30

### Added

- *(wisp)* Git controls for staging/unstaging and commiting in git d… ([#201](https://github.com/contextbridge/aether/pull/201))

## [0.2.14](https://github.com/contextbridge/aether/compare/aether-tui-v0.2.13...aether-tui-v0.2.14) - 2026-06-18

### Other

- Move a bunch of errors to using thiserror ([#160](https://github.com/contextbridge/aether/pull/160))

## [0.2.13](https://github.com/contextbridge/aether/compare/aether-tui-v0.2.12...aether-tui-v0.2.13) - 2026-06-13

### Added

- *(aether-cli)* Add /move command to switch workspaces and bring your session + changes with you. ([#150](https://github.com/contextbridge/aether/pull/150))
- *(wisp)* Better git diff and plan views ([#146](https://github.com/contextbridge/aether/pull/146))

## [0.2.12](https://github.com/contextbridge/aether/compare/aether-tui-v0.2.11...aether-tui-v0.2.12) - 2026-06-10

### Other

- update Cargo.toml dependencies

## [0.2.11](https://github.com/contextbridge/aether/compare/aether-tui-v0.2.10...aether-tui-v0.2.11) - 2026-05-31

### Added

- *(wisp)* Terminal bell on agent idle ([#98](https://github.com/contextbridge/aether/pull/98))

### Fixed

- *(wisp)* allow spacebar in /resume session picker ([#105](https://github.com/contextbridge/aether/pull/105))

## [0.2.10](https://github.com/contextbridge/aether/compare/aether-tui-v0.2.9...aether-tui-v0.2.10) - 2026-05-21

### Fixed

- *(wisp)* Make multi line prompt composer work properly on a Mac ([#95](https://github.com/contextbridge/aether/pull/95))
- *(acp)* Spawn parent acp process via tokio to avoid spinning cpu ([#90](https://github.com/contextbridge/aether/pull/90))

## [0.2.9](https://github.com/contextbridge/aether/compare/aether-tui-v0.2.8...aether-tui-v0.2.9) - 2026-05-18

### Fixed

- *(wisp)* Allow copying URLs when performing MCP auth  ([#82](https://github.com/contextbridge/aether/pull/82))

## [0.2.8](https://github.com/contextbridge/aether/compare/aether-tui-v0.2.7...aether-tui-v0.2.8) - 2026-05-15

### Fixed

- *(wisp)* Allow multi line input in prompt via shift+tab ([#73](https://github.com/contextbridge/aether/pull/73))
- make Tab key confirm selection in picker components ([#69](https://github.com/contextbridge/aether/pull/69))

## [0.2.7](https://github.com/contextbridge/aether/compare/aether-tui-v0.2.6...aether-tui-v0.2.7) - 2026-05-15

### Fixed

- *(tui)* Skip word keyboard shortcuts now work in user prompt with option + left/right arrow ([#64](https://github.com/contextbridge/aether/pull/64))
- *(wisp)* More visible search bar in resume session ([#62](https://github.com/contextbridge/aether/pull/62))

## [0.2.6](https://github.com/contextbridge/aether/compare/aether-tui-v0.2.5...aether-tui-v0.2.6) - 2026-05-12

### Other

- update Cargo.toml dependencies

## [0.2.5](https://github.com/contextbridge/aether/compare/aether-tui-v0.2.4...aether-tui-v0.2.5) - 2026-05-08

### Added

- *(aether-cli)* Render proxied MCP servers in a separate list from non-proxied MCPs in settings menu

## [0.2.4](https://github.com/contextbridge/aether/compare/aether-tui-v0.2.3...aether-tui-v0.2.4) - 2026-05-05

### Other

- port to contextbridge org

## [0.2.3](https://github.com/contextbridge/aether/compare/aether-tui-v0.2.2...aether-tui-v0.2.3) - 2026-05-04

### Other

- *(wisp)* Batch event renders

## [0.2.2](https://github.com/contextbridge/aether/compare/aether-tui-v0.2.1...aether-tui-v0.2.2) - 2026-04-29

### Other

- update Cargo.toml dependencies

## [aether-tui-v0.2.1] - 2026-04-20
