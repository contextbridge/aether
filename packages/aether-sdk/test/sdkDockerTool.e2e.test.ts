import { join } from "node:path";
import { describe, expect, it } from "vitest";
import {
  DockerAgent,
  DockerImage,
  Task,
  Workspace,
} from "../src/evals/index.js";

describe.skipIf(process.env.AETHER_SDK_DOCKER_E2E !== "1")(
  "SDK Docker tool e2e",
  () => {
    it("runs a real Aether agent in Docker and calls a TS-defined tool", async () => {
      const packageDir = new URL("..", import.meta.url).pathname;
      const image = DockerImage.fromDockerfile(
        "aether-sdk-weather-agent:latest",
        {
          context: join(packageDir, "../.."),
          dockerfile:
            "packages/aether-sdk/test/fixtures/sdk-weather-agent/Dockerfile",
        },
      );

      const task = new Task(
        'Call weather__get_weather with city "Tokyo", then answer using the tool result.',
        await Workspace.fromFiles({}),
      );
      const agent = new DockerAgent({
        image,
        command: ["node", "/app/dist/eval-agent.js"],
      });
      await using result = await task.run(agent);

      const getWeatherCall = result.toolCalls.find(
        (_) => _.name === "weather__get_weather",
      );

      expect(result.passed).toBe(true);
      expect(getWeatherCall?.arguments).toMatchObject({ city: "Tokyo" });
    }, 300_000);
  },
);
