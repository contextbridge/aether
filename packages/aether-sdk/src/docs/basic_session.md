```ts
import { AetherSession, type AetherSettings } from "@aether-agent/sdk";

async function example() {
  const settings: AetherSettings = {
    agent: "Example",
    telemetry: {
      content: {
        systemInstructions: true,
        inputMessages: true,
        outputMessages: true,
        toolDefinitions: true,
        toolCalls: true,
      },
      traces: { enabled: true },
      metrics: { enabled: true },
      otlp: {
        endpoint: "http://localhost:4318",
        headers: { Authorization: "Bearer $OTEL_TOKEN" },
      },
    },
    agents: [
      {
        name: "Example",
        description: "A helpful assistant.",
        model: "zai:glm-5.1",
        userInvocable: true,
        prompts: [{ type: "text", text: "You are a helpful assistant." }],
      },
    ],
  };

  await using session = await AetherSession.start({ settings });
}
```
