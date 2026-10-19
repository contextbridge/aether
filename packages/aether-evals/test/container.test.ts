import type { StartedTestContainer } from "testcontainers";
import { describe, expect, it } from "vitest";

import { Container } from "../src/index.js";

describe("Container.exec", () => {
  it("runs as the container's user unless the exec names another", async () => {
    const { started, execUsers } = fakeStartedContainer();
    const container = new Container(started, {
      cwd: "/workspace",
      user: "1000:1000",
    });

    await container.exec({ command: ["id", "-u"] });
    await container.exec({ command: ["id", "-u"], user: "root" });

    expect(execUsers).toEqual(["1000:1000", "root"]);
  });
});

function fakeStartedContainer(
  result: Promise<unknown> = Promise.resolve({
    exitCode: 0,
    output: "",
    stdout: "",
    stderr: "",
  }),
): { started: StartedTestContainer; execUsers: Array<string | undefined> } {
  const execUsers: Array<string | undefined> = [];
  const started = {
    exec: (_command: string[], options: { user?: string }) => {
      execUsers.push(options.user);
      return result;
    },
  } as unknown as StartedTestContainer;
  return { started, execUsers };
}
