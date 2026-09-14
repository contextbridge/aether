import { fileURLToPath } from "node:url";
import path from "node:path";

import { describe, expect, it } from "vitest";

import { AetherSession, acp, type AetherMessage } from "../src/index.js";

const FAKE_AETHER = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "fakeAether.mjs",
);

function agentMessageText(messages: AetherMessage[]): string {
  const update = messages.find(
    (m) =>
      m.type === "session_update" &&
      acp.SessionUpdate.isAgentMessageChunk(m.update),
  );
  if (
    update?.type === "session_update" &&
    acp.SessionUpdate.isAgentMessageChunk(update.update)
  ) {
    return acp.ContentBlock.isText(update.update.content)
      ? update.update.content.text
      : "";
  }
  throw new Error("expected agent_message_chunk update");
}

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

    const text = agentMessageText(messages);
    expect(text).toContain('"selected"');
    expect(text).toContain('"allow"');
  });

  it("uses the user-supplied permission handler when provided", async () => {
    const session = await AetherSession.start({
      binaryPath: FAKE_AETHER,
      env: { PATH: process.env.PATH, FAKE_AETHER_REQUEST_PERMISSION: "1" },
      onPermissionRequest: async (request) => {
        expect(request.title).toBe("Run test tool?");
        expect(request.subject).toMatchObject({
          type: "tool_call",
          toolCall: { toolCallId: "tc-1" },
        });
        return { outcome: { outcome: "selected", optionId: "reject" } };
      },
    });
    const messages: AetherMessage[] = [];

    try {
      for await (const message of session.prompt("anything")) {
        messages.push(message);
      }
    } finally {
      await session.close();
    }

    expect(agentMessageText(messages)).toContain('"reject"');
  });
});
