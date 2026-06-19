Errors that can occur when interacting with LLM providers.

# Variants

## Authentication
- **`MissingApiKey`** -- A required environment variable (e.g. `ANTHROPIC_API_KEY`) is not set.
- **`InvalidApiKey`** -- The API key format is invalid or was rejected by the provider.
- **`OAuthError`** -- OAuth authentication failed (when the `oauth` feature is enabled).

## Request/Response
- **`HttpClientCreation`** -- Failed to create the HTTP client (e.g. TLS configuration error).
- **`ApiRequest`** -- The HTTP request to the provider failed (network error, timeout).
- **`ApiError`** -- The provider returned an error response (rate limit, server error).
- **`ContextOverflow`** -- The prompt exceeded the model's context window. Contains a [`ContextOverflowError`] with provider, model, and token details.

## Parsing
- **`JsonParsing`** -- Failed to parse or serialize JSON (response body, tool arguments).
- **`ToolParameterParsing`** -- Failed to parse tool parameters for a specific tool.
- **`IoError`** -- IO error while reading the response stream.

## Content
- **`UnsupportedContent`** -- The message contained only content types this provider doesn't support (e.g. sending audio to a text-only model).

## Other
- **`Other`** -- Catch-all for cases that don't fit the above categories.

# `From` implementations

`LlmError` converts automatically from common error types: `reqwest::Error`, `serde_json::Error`, `std::io::Error`, `reqwest::header::InvalidHeaderValue`, `async_openai::error::OpenAIError`, and `OAuthError` (with the `oauth` feature).

# Type alias

The crate provides `type Result<T> = std::result::Result<T, LlmError>` for convenience.
