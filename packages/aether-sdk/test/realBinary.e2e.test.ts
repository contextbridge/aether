import { existsSync } from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { AetherSession } from "../src/index.js";

const binaryPath =
  process.env.AETHER_BIN ??
  fileURLToPath(new URL("../../../target/debug/aether", import.meta.url));

describe("E2E tests", () => {
  it("initializes a stdio session", async () => {
    expect(
      existsSync(binaryPath),
      `aether binary not found at ${binaryPath}. Build it first via just command.`,
    ).toBe(true);

    const cwd = await mkdtemp(path.join(tmpdir(), "aether-e2e-"));
    try {
      const session = await AetherSession.start({
        binaryPath,
        cwd,
        logDir: cwd,
        settings: {
          agent: "Dummy",
          credentialsStore: { type: "memory" },
          agents: [
            {
              name: "Dummy",
              description: "Dummy agent for SDK real-binary startup tests",
              model: "anthropic:claude-sonnet-4-6",
              userInvocable: true,
              prompts: [
                { type: "text", text: "You are a dummy SDK test agent." },
              ],
            },
          ],
        },
        env: {
          ...process.env,
          ANTHROPIC_API_KEY: "sk-ant-e2e-dummy",
        },
      });
      expect(session.sessionId).toBeTruthy();
      await session.close();
    } finally {
      await rm(cwd, { recursive: true, force: true });
    }
  }, 30_000);
});
