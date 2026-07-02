The root of an Aether settings file (`.aether/settings.json`). It selects the
default agent and defines the agents, prompt sources, MCP servers, and provider
overrides available to a project.

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

OpenTelemetry `GenAI` telemetry is disabled by default and is configured only from
settings. Aether does not read `OTEL_*` environment variables for its own
telemetry configuration. Telemetry is process-level infrastructure: it is
resolved once at startup from the settings visible in the directory the CLI was
launched in, and every session in that process shares it. The configuration is
validated when telemetry initializes (not at settings load): headless runs fail
at startup on an invalid configuration, while interactive sessions log a warning
and run with telemetry disabled. Prompt, response, reasoning, and tool argument
content is not exported unless `captureContent` is explicitly set to `true`;
enabling it may export secrets.

Each user turn is exported as one trace: an `invoke_agent` span parenting a
`chat` span per LLM call (context-compaction calls are tagged
`aether.llm.purpose=compaction`, retries carry `aether.llm.attempt`) and an
`execute_tool` span per tool execution. Sub-agents spawned during a turn emit
their own traces.

```json
{
  "telemetry": {
    "enabled": true,
    "serviceName": "aether",
    "sampleRatio": 1.0,
    "captureContent": false,
    "traces": { "enabled": true },
    "metrics": { "enabled": true },
    "otlp": {
      "endpoint": "http://localhost:4317",
      "protocol": "grpc"
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

Minimal local OTLP collector configuration:

```yaml
receivers:
  otlp:
    protocols:
      grpc:
        endpoint: 0.0.0.0:4317
      http:
        endpoint: 0.0.0.0:4318
exporters:
  debug:
service:
  pipelines:
    traces:
      receivers: [otlp]
      exporters: [debug]
    metrics:
      receivers: [otlp]
      exporters: [debug]
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
