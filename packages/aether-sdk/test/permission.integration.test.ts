import { fileURLToPath } from "node:url";
import path from "node:path";

import { describe, expect, it } from "vitest";

import { AetherSession, type AetherMessage } from "../src/index.js";

const FAKE_AETHER = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "fakeAether.mjs",
);

describe("default permission handler", () => {
  it("auto-selects the first allow_* option when no handler is supplied", async () => {
    const session = await AetherSession.start({
      binaryPath: FAKE_AETHER,
      env: { PATH: process.env.PATH, FAKE_AETHER_REQUEST_PERMISSION: "1" },
    });
    const messages: AetherMessage[] = [];

    try {
      for await (const message of session.prompt("anything")) {
        messages.push(message);
      }
    } finally {
      await session.close();
    }

    const update = messages.find((m) => m.type === "session_update");
    if (
      update?.type === "session_update" &&
      update.update.sessionUpdate === "agent_message_chunk"
    ) {
      const text =
        update.update.content.type === "text" ? update.update.content.text : "";
      expect(text).toContain('"selected"');
      expect(text).toContain('"allow"');
    } else {
      throw new Error("expected agent_message_chunk update");
    }
  });

  it("uses the user-supplied permission handler when provided", async () => {
    const session = await AetherSession.start({
      binaryPath: FAKE_AETHER,
      env: { PATH: process.env.PATH, FAKE_AETHER_REQUEST_PERMISSION: "1" },
      onPermissionRequest: async () => ({
        outcome: { outcome: "selected", optionId: "reject" },
      }),
    });
    const messages: AetherMessage[] = [];

    try {
      for await (const message of session.prompt("anything")) {
        messages.push(message);
      }
    } finally {
      await session.close();
    }

    const update = messages.find((m) => m.type === "session_update");
    if (
      update?.type === "session_update" &&
      update.update.sessionUpdate === "agent_message_chunk"
    ) {
      const text =
        update.update.content.type === "text" ? update.update.content.text : "";
      expect(text).toContain('"reject"');
    } else {
      throw new Error("expected agent_message_chunk update");
    }
  });
});
