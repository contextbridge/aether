The root of an Aether settings file (`.aether/settings.json`). It selects the
default agent and defines the agents, prompt sources, MCP servers, and provider
overrides available to a project.

## Basic Examples
A minimal project with a single user-invocable agent:

```json
{
  "agent": "Build",
  "agents": [
    {
      "name": "Build",
      "description": "Builds features and fixes bugs",
      "model": "anthropic:claude-sonnet-4-5-20250929",
      "userInvocable": true
    }
  ]
}
```

A fuller setup with shared prompts, an MCP source, and a provider override:

```json
{
  "agent": "Build",
  "prompts": ["AGENTS.md"],
  "mcps": [".aether/mcp.json"],
  "providers": {
    "anthropic": { "auth": "default" }
  },
  "agents": [
    {
      "name": "Build",
      "description": "Builds features and fixes bugs",
      "model": "anthropic:claude-sonnet-4-5-20250929",
      "reasoningEffort": "high",
      "userInvocable": true,
      "prompts": [".aether/BUILD.md", "AGENTS.md"]
    }
  ]
}
```

An encrypted file credential store using a passphrase from the environment:

```json
{
  "credentialsStore": {
    "type": "encryptedFile",
    "passwordEnv": "PASSWORD_ENV_VAR_NAME"
  },
  "agents": [
    {
      "name": "Build",
      "description": "Builds features and fixes bugs",
      "model": "anthropic:claude-sonnet-4-5-20250929",
      "userInvocable": true
    }
  ]
}
```

## OpenTelemetry

OpenTelemetry is enabled by adding a `telemetry` section to
settings. User-level telemetry settings merge with project-level, with project-level taking priority. 
Prompt, response, reasoning, and tool argument content is not exported unless `captureContent` is explicitly set to `true`.

```json
{
  "telemetry": {
    "serviceName": "aether",
    "sampleRatio": 1.0,
    "captureContent": false,
    "traces": { "enabled": true },
    "metrics": { "enabled": true },
    "otlp": {
      "endpoint": "http://localhost:4318"
    }
  },
  "agents": [
    {
      "name": "Build",
      "description": "Builds features and fixes bugs",
      "model": "anthropic:claude-sonnet-4-5-20250929",
      "userInvocable": true
    }
  ]
}
```
