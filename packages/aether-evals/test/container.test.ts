import { describe, expect, it } from "vitest";

import { AetherCommandExitError } from "../src/index.js";
import { FakeRuntime } from "./FakeRuntime.js";

describe("Container", () => {
  it("execs as the container's user unless the exec names another", async () => {
    const runtime = new FakeRuntime();
    const container = runtime.container({ user: "1000:1000" });

    await container.exec({ command: ["id", "-u"] });
    await container.exec({ command: ["id", "-u"], user: "root" });

    expect(runtime.execs.map((exec) => exec.user)).toEqual([
      "1000:1000",
      "root",
    ]);
  });

  it("returns an exec's output and exit code", async () => {
    const runtime = new FakeRuntime({
      stdout: "out\n",
      stderr: "err\n",
      exitCode: 3,
    });

    const result = await runtime.container().exec({ command: ["false"] });

    expect(result).toEqual({ exitCode: 3, stdout: "out\n", stderr: "err\n" });
  });

  it("streams stdout lines and rejects a non-zero exit", async () => {
    const runtime = new FakeRuntime({
      stdout: "one\ntwo\n",
      stderr: "boom",
      exitCode: 2,
    });
    const lines: string[] = [];

    const run = (async () => {
      for await (const line of runtime.container().execStreaming({
        command: ["agent"],
      })) {
        lines.push(line);
      }
    })();

    await expect(run).rejects.toBeInstanceOf(AetherCommandExitError);
    await expect(run).rejects.toMatchObject({ exitCode: 2, stderr: "boom" });
    expect(lines).toEqual(["one", "two"]);
  });

  it("rejects an aborted exec without waiting for the command", async () => {
    const controller = new AbortController();

    const exec = new FakeRuntime({ hang: true }).container().exec({
      command: ["sleep", "60"],
      signal: controller.signal,
    });
    controller.abort();

    await expect(exec).rejects.toMatchObject({ code: "aborted" });
  });
});
