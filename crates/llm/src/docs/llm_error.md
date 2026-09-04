Errors that can occur when interacting with LLM providers.

# Other variants

## Authentication
- **`MissingApiKey`** -- A required environment variable (e.g. `ANTHROPIC_API_KEY`) is not set.
- **`OAuthError`** -- OAuth authentication failed (when the `oauth` feature is enabled).

## Request/Response
- **`HttpClientCreation`** -- Failed to create the HTTP client (e.g. TLS configuration error).

## Parsing
- **`JsonParsing`** -- Failed to parse or serialize JSON (response body, tool arguments).
- **`ToolParameterParsing`** -- Failed to parse tool parameters for a specific tool.
- **`IoError`** -- IO error while reading the response stream.

## Content
- **`UnsupportedContent`** -- The message contained only content types this provider doesn't support (e.g. sending audio to a text-only model).

## Model parsing
- **`UnknownProvider`** -- Provider name is not registered with the model parser.
- **`EmptyModelSpec`** -- The model spec did not yield any usable provider.
- **`DuplicateProvider`** -- A single-model-only config field was reused across multiple models of the same provider within one alloy spec (e.g. bedrock `inferenceProfileArn` or openai-compatible `requestModel`).
- **`InvalidModelSpec`** -- A `provider:model` identity could not be parsed.
- **`MissingProviderUrl`** -- Provider endpoint URL has not been configured.
- **`ProviderRequest`** -- A provider request, header, or signature could not be constructed (e.g. an SDK builder rejected the input or contained malformed data).
- **`InvalidArgument`** -- An upstream client library rejected an argument as invalid.


# Type alias

The crate provides `type Result<T> = std::result::Result<T, LlmError>` for convenience.
