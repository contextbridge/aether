Per-provider connection settings, merged over the built-in defaults.

Force first-party auth (e.g. an OAuth store) for a provider:

```json
{ "auth": "default" }
```

Point a provider at a custom base URL with auth disabled:

```json
{ "url": "https://gateway.internal/v1", "auth": "none" }
```

Pin a Bedrock application inference profile:

```json
{ "inferenceProfileArn": "arn:aws:bedrock:us-west-2:000000000000:application-inference-profile/abc" }
```
