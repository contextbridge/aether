# Changelog

All notable changes to this project will be documented in this file.

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

- *(crucible)* Simplify crucible to rely on normal rust tests and cargo next test

## [0.2.5](https://github.com/contextbridge/aether/compare/aether-llm-v0.2.4...aether-llm-v0.2.5) - 2026-04-29

### Fixed

- *(aether-cli)* Auto retry on llm errors

## [aether-llm-v0.2.4] - 2026-04-27
