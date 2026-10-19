import { join } from "node:path";
import { describe, expect, it } from "vitest";
import {
  Container,
  DockerAgent,
  Image,
  Task,
  Transcript,
  Workspace,
} from "../src/index.js";
import { logMessage } from "./logMessage.js";

describe.skipIf(process.env.AETHER_EVALS_DOCKER_E2E !== "1")(
  "DockerAgent tool e2e",
  () => {
    it("runs a real Aether agent in Docker and calls a TS-defined tool", async () => {
      const packageDir = new URL("..", import.meta.url).pathname;
      const image = Image.fromDockerfile("aether-sdk-weather-agent:latest", {
        context: join(packageDir, "../.."),
        dockerfile:
          "packages/aether-evals/test/fixtures/sdk-weather-agent/Dockerfile",
        buildkit: true,
      });

      await using workspace = await Workspace.fromFiles({});
      await using container = await Container.builder(image).start(workspace);
      const agent = new DockerAgent({
        container,
        command: ["node", "/app/dist/eval-agent.js"],
        env: hostEnv(AGENT_ENV_KEYS),
      });
      process.stderr.write(
        "[e2e] building image + running agent (first run builds, ~1 min)…\n",
      );
      const trace = new Transcript();
      for await (const message of agent.run(
        new Task(
          'Call weather__get_weather with city "Tokyo", then answer using the tool result.',
        ),
      )) {
        logMessage(message);
        trace.add(message);
      }

      const getWeatherCall = trace.toolCalls("weather__get_weather").at(0);

      expect(trace.events.at(-1)).toMatchObject({
        category: "turn",
        event: { type: "ended" },
      });
      expect(getWeatherCall?.argumentsJson()).toMatchObject({ city: "Tokyo" });
    }, 1_200_000);
  },
);

const AGENT_ENV_KEYS = [
  "AETHER_EVAL_MODEL",
  "ANTHROPIC_API_KEY",
  "DEEPSEEK_API_KEY",
  "GEMINI_API_KEY",
  "MOONSHOT_API_KEY",
  "OPENAI_API_KEY",
  "OPENROUTER_API_KEY",
  "XIAOMI_API_KEY",
  "ZAI_API_KEY",
];

function hostEnv(keys: string[]): Record<string, string> {
  return Object.fromEntries(
    keys.flatMap((key) => {
      const value = process.env[key];
      return value === undefined ? [] : [[key, value]];
    }),
  );
}
