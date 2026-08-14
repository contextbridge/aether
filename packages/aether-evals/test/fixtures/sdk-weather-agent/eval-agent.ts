import { appendFile } from "node:fs/promises";
import { mcp, runHeadless, tool } from "@aether-agent/sdk";
import { z } from "zod";

const prompt = process.env.AETHER_EVAL_TASK_PROMPT;
if (!prompt) throw new Error("AETHER_EVAL_TASK_PROMPT is required");

const getWeather = tool({
  name: "get_weather",
  description: "Get deterministic weather for a city.",
  inputSchema: { city: z.string() },
  handler: async ({ city }) => {
    await appendFile(
      "/workspace/tool-calls.jsonl",
      `${JSON.stringify({ name: "weather__get_weather", args: { city } })}\n`,
    );
    return {
      content: [{ type: "text", text: `Weather in ${city}: sunny and 72F.` }],
    };
  },
});

await using weather = await mcp({ name: "weather", tools: [getWeather] });
await runHeadless({
  binaryPath: process.env.AETHER_BIN ?? "/usr/local/bin/aether",
  prompt,
  cwd: process.env.AETHER_EVAL_CWD ?? process.cwd(),
  model: process.env.AETHER_EVAL_MODEL ?? "zai:glm-5.3",
  settings: { agents: [], mcps: [weather.spec] },
  output: "json",
  stdout: "inherit",
  stderr: "inherit",
});
