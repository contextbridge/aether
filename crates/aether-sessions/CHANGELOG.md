# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.1](https://github.com/contextbridge/aether/compare/aether-sessions-v0.2.0...aether-sessions-v0.2.1) - 2026-09-04

### Other

- Bedrock Responses response.failed events with server errors are not retried ([#410](https://github.com/contextbridge/aether/pull/410))

## [0.2.0](https://github.com/contextbridge/aether/compare/aether-sessions-v0.1.3...aether-sessions-v0.2.0) - 2026-09-03

### Added

- *(aether-cli)* [**breaking**] add session usage and cost tracking ([#405](https://github.com/contextbridge/aether/pull/405))

## [0.1.3](https://github.com/contextbridge/aether/compare/aether-sessions-v0.1.2...aether-sessions-v0.1.3) - 2026-08-31

### Other

- updated the following local packages: aether-agent-core

## [0.1.2](https://github.com/contextbridge/aether/compare/aether-sessions-v0.1.1...aether-sessions-v0.1.2) - 2026-08-31

### Other

- updated the following local packages: aether-llm, aether-acp-utils, aether-agent-core

## [0.1.1](https://github.com/contextbridge/aether/compare/aether-sessions-v0.1.0...aether-sessions-v0.1.1) - 2026-08-27

### Other

- updated the following local packages: aether-utils, aether-llm, aether-acp-utils, aether-agent-core

### Added

- Persisted session models, JSONL log parsing, and transcript reconstruction helpers.
- Session storage, bounded previews, relocation, and derived prompt search.
- Optional SQLite analytics ingest, querying, pruning, and schema documentation.
