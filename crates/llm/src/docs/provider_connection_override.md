Per-provider connection settings, merged over the built-in defaults.

Force first-party auth (e.g. an OAuth store) for a provider:

```json
{ "auth": "default" }
```

Point a provider at a custom base URL with auth disabled:

```json
{ "url": "https://gateway.internal/v1", "auth": "none" }
```

Request-target routing keeps the catalog model identity and its capabilities while sending a different provider-specific model or deployment name on the wire:

```json
{ "requestModel": "production-coding-deployment" }
```

Use `url` for providers with resource-specific endpoints, such as Microsoft Foundry. A trailing slash is normalized before `/chat/completions` is appended. A provider with `requestModel` cannot appear more than once in an alloy because the target would be ambiguous.

Pin a Bedrock application inference profile:

```json
{ "inferenceProfileArn": "arn:aws:bedrock:us-west-2:000000000000:application-inference-profile/abc" }
```
