import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import { runEval } from "../src/evals/index.js";

const runDockerE2E = process.env.AETHER_SDK_DOCKER_E2E === "1";

describe.skipIf(!runDockerE2E)("SDK Docker tool e2e", () => {
  it("runs a real Aether agent in Docker and calls a TS-defined tool", async () => {
    await using result = await runEval(
      {
        name: "sdk-weather-tool",
        docker: {
          file: "test/fixtures/sdk-weather-agent/Dockerfile",
          context: "../..",
        },
        agent: {
          command: ["node", "/app/dist/eval-agent.js"],
        },
        task: {
          prompt:
            'Call weather__get_weather with city "Tokyo", then answer using the tool result.',
          workspace: { files: { "README.md": "SDK Docker tool eval\n" } },
        },
      },
      {
        baseDir: new URL("..", import.meta.url).pathname,
      },
    );

    expect(result.passed, result.failures.join("\n")).toBe(true);

    const transcriptCall = result.toolCalls.find(
      (call) => call.name === "weather__get_weather",
    );
    expect(transcriptCall).toBeTruthy();
    expect(transcriptCall?.arguments).toMatchObject({ city: "Tokyo" });

    const log = await readFile(
      join(result.workspace.path, "tool-calls.jsonl"),
      "utf8",
    );
    const calls = log
      .trim()
      .split("\n")
      .map((line) => JSON.parse(line));
    expect(calls).toContainEqual({
      name: "weather__get_weather",
      args: { city: "Tokyo" },
    });
  });
});
