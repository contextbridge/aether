# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.9](https://github.com/contextbridge/aether/compare/aether-auth-v0.1.8...aether-auth-v0.1.9) - 2026-07-27

### Other

- update Cargo.toml dependencies

## [0.1.8](https://github.com/contextbridge/aether/compare/aether-auth-v0.1.7...aether-auth-v0.1.8) - 2026-07-20

### Added

- *(mcp)* support pre-registered OAuth clients ([#254](https://github.com/contextbridge/aether/pull/254))

### Other

- scheduled code-cleanup ([#253](https://github.com/contextbridge/aether/pull/253))

## [0.1.7](https://github.com/contextbridge/aether/compare/aether-auth-v0.1.6...aether-auth-v0.1.7) - 2026-07-13

### Other

- reduce development compile costs ([#231](https://github.com/contextbridge/aether/pull/231))

## [0.1.6](https://github.com/contextbridge/aether/compare/aether-auth-v0.1.5...aether-auth-v0.1.6) - 2026-06-10

### Other

- update Cargo.toml dependencies

## [0.1.5](https://github.com/contextbridge/aether/compare/aether-auth-v0.1.4...aether-auth-v0.1.5) - 2026-06-04

### Added

- *(aether-cli)* Support encrypted file store for oauth for users that do not want full keyring ([#124](https://github.com/contextbridge/aether/pull/124))

## [0.1.4](https://github.com/contextbridge/aether/compare/aether-auth-v0.1.3...aether-auth-v0.1.4) - 2026-05-31

### Other

- update Cargo.toml dependencies

## [0.1.3](https://github.com/contextbridge/aether/compare/aether-auth-v0.1.2...aether-auth-v0.1.3) - 2026-05-18

### Other

- update Cargo.toml dependencies

## [0.1.2](https://github.com/contextbridge/aether/compare/aether-auth-v0.1.1...aether-auth-v0.1.2) - 2026-05-15

### Fixed

- *(mcp)* populate token_received_at in MCP credential store to enable rmcp refresh ([#72](https://github.com/contextbridge/aether/pull/72))

## [0.1.1](https://github.com/contextbridge/aether/compare/aether-auth-v0.1.0...aether-auth-v0.1.1) - 2026-05-15

### Fixed

- *(llm)* Refresh Codex auth tokens ([#59](https://github.com/contextbridge/aether/pull/59))
- *(aether-auth)* Do not overwrite refresh tokens with None if we already have a refresh token stored ([#55](https://github.com/contextbridge/aether/pull/55))
