# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0](https://github.com/contextbridge/aether/compare/aether-telemetry-v0.1.18...aether-telemetry-v0.2.0) - 2026-09-03

### Added

- *(aether-cli)* [**breaking**] add session usage and cost tracking ([#405](https://github.com/contextbridge/aether/pull/405))

### Other

- Include prompt and agent identity in traces ([#411](https://github.com/contextbridge/aether/pull/411))

## [0.1.18](https://github.com/contextbridge/aether/compare/aether-telemetry-v0.1.17...aether-telemetry-v0.1.18) - 2026-08-31

### Other

- updated the following local packages: aether-agent-core, aether-agent-core

## [0.1.17](https://github.com/contextbridge/aether/compare/aether-telemetry-v0.1.16...aether-telemetry-v0.1.17) - 2026-08-31

### Other

- updated the following local packages: aether-llm, aether-agent-core, aether-agent-core

## [0.1.16](https://github.com/contextbridge/aether/compare/aether-telemetry-v0.1.15...aether-telemetry-v0.1.16) - 2026-08-27

### Other

- updated the following local packages: aether-utils, aether-llm, aether-agent-core, aether-agent-core

## [0.1.15](https://github.com/contextbridge/aether/compare/aether-telemetry-v0.1.14...aether-telemetry-v0.1.15) - 2026-08-22

### Other

- updated the following local packages: aether-utils, aether-agent-core, aether-agent-core, aether-llm

## [0.1.14](https://github.com/contextbridge/aether/compare/aether-telemetry-v0.1.13...aether-telemetry-v0.1.14) - 2026-08-20

### Other

- updated the following local packages: aether-llm, aether-agent-core, aether-agent-core

## [0.1.13](https://github.com/contextbridge/aether/compare/aether-telemetry-v0.1.12...aether-telemetry-v0.1.13) - 2026-08-19

### Other

- *(deps)* bump agent-client-protocol from 0.14.0 to 2.0.0 ([#285](https://github.com/contextbridge/aether/pull/285))
- Upgrade to rmcp 3.1.1 and MCP tool calls now use multi round trip requests (MRTR) ([#337](https://github.com/contextbridge/aether/pull/337))
- scheduled code-cleanup ([#329](https://github.com/contextbridge/aether/pull/329))

## [0.1.12](https://github.com/contextbridge/aether/compare/aether-telemetry-v0.1.11...aether-telemetry-v0.1.12) - 2026-08-04

### Other

- scheduled code-cleanup ([#315](https://github.com/contextbridge/aether/pull/315))

## [0.1.11](https://github.com/contextbridge/aether/compare/aether-telemetry-v0.1.10...aether-telemetry-v0.1.11) - 2026-07-29

### Added

- *(telemetry)* name agent invocation spans ([#312](https://github.com/contextbridge/aether/pull/312))

## [0.1.10](https://github.com/contextbridge/aether/compare/aether-telemetry-v0.1.9...aether-telemetry-v0.1.10) - 2026-07-29

### Other

- updated the following local packages: aether-agent-core, aether-agent-core

## [0.1.9](https://github.com/contextbridge/aether/compare/aether-telemetry-v0.1.8...aether-telemetry-v0.1.9) - 2026-07-29

### Added

- *(aether-telemetry)* Connect parent agent and subagent tracing spans together ([#305](https://github.com/contextbridge/aether/pull/305))

## [0.1.8](https://github.com/contextbridge/aether/compare/aether-telemetry-v0.1.7...aether-telemetry-v0.1.8) - 2026-07-28

### Fixed

- *(aether-telemetry)* Include reasoning tokens and pricing information ([#301](https://github.com/contextbridge/aether/pull/301))

## [0.1.7](https://github.com/contextbridge/aether/compare/aether-telemetry-v0.1.6...aether-telemetry-v0.1.7) - 2026-07-28

### Fixed

- *(aether-telemetry)* Small improvements for telemetry, include stop… ([#297](https://github.com/contextbridge/aether/pull/297))

## [0.1.6](https://github.com/contextbridge/aether/compare/aether-telemetry-v0.1.5...aether-telemetry-v0.1.6) - 2026-07-27

### Other

- updated the following local packages: aether-llm, aether-agent-core, aether-agent-core

## [0.1.5](https://github.com/contextbridge/aether/compare/aether-telemetry-v0.1.4...aether-telemetry-v0.1.5) - 2026-07-21

### Added

- support trace IDs without parent spans ([#268](https://github.com/contextbridge/aether/pull/268))

## [0.1.4](https://github.com/contextbridge/aether/compare/aether-telemetry-v0.1.3...aether-telemetry-v0.1.4) - 2026-07-20

### Added

- Support setting custom OTEL trace id.  ([#262](https://github.com/contextbridge/aether/pull/262))

## [0.1.3](https://github.com/contextbridge/aether/compare/aether-telemetry-v0.1.2...aether-telemetry-v0.1.3) - 2026-07-20

### Added

- *(telemetry)* interpolate OTLP header variables ([#238](https://github.com/contextbridge/aether/pull/238))

### Fixed

- keep agent sessions responsive during in-flight work ([#235](https://github.com/contextbridge/aether/pull/235))

### Other

- streamline agent and MCP integration tests ([#236](https://github.com/contextbridge/aether/pull/236))

## [0.1.2](https://github.com/contextbridge/aether/compare/aether-telemetry-v0.1.1...aether-telemetry-v0.1.2) - 2026-07-13

### Other

- reduce development compile costs ([#231](https://github.com/contextbridge/aether/pull/231))

## [0.1.1](https://github.com/contextbridge/aether/compare/aether-telemetry-v0.1.0...aether-telemetry-v0.1.1) - 2026-07-13

### Added

- *(aether-cli)* Support exact trace and metric endpoints for otel e… ([#228](https://github.com/contextbridge/aether/pull/228))

## [0.1.0](https://github.com/contextbridge/aether/releases/tag/aether-telemetry-v0.1.0) - 2026-07-13

### Added

- *(aether-cli)* Add support for exporting genai OTEL traces ([#219](https://github.com/contextbridge/aether/pull/219))
