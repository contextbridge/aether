import { fileURLToPath } from "node:url";
import path from "node:path";

import { describe, expect, it } from "vitest";

import { runCommand } from "../src/index.js";

const FAKE_COMMAND = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "fakeCommand.mjs",
);

const options = {
  cwd: process.cwd(),
  env: process.env,
  spawnFailedMessage: "spawn failed",
  exitedErrorCode: "execution_failed",
  exitedMessage: ({ exitCode, signal, stderr }) =>
    `code=${exitCode} signal=${signal} stderr=${stderr}`,
} satisfies Parameters<typeof runCommand>[2];

describe("runCommand()", () => {
  it("returns stdout and passes stdin", async () => {
    expect(
      await runCommand(process.execPath, [FAKE_COMMAND], {
        ...options,
        env: { ...process.env, FAKE_STDOUT: "out:", FAKE_ECHO_STDIN: "1" },
        stdin: "in 🌍",
      }),
    ).toBe("out:in 🌍");
  });

  it("rejects nonzero exits with stderr instead of returning partial stdout", async () => {
    await expect(
      runCommand(process.execPath, [FAKE_COMMAND], {
        ...options,
        env: {
          ...process.env,
          FAKE_STDOUT: "partial",
          FAKE_STDERR: "provider failed",
          FAKE_EXIT_CODE: "2",
        },
      }),
    ).rejects.toMatchObject({
      code: "execution_failed",
      message: "code=2 signal=null stderr=provider failed",
    });
  });
});
