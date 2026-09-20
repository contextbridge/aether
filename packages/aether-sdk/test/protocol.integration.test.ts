import path from "node:path";
import { fileURLToPath } from "node:url";
import { expect, it } from "vitest";
import { acp, startAetherAcpAgentProcess } from "../src/index.js";

const binaryPath = fileURLToPath(new URL("./fakeAether.mjs", import.meta.url));

it("exposes v2 auth and plain/replay resume over the public process transport", async () => {
  await using process = startAetherAcpAgentProcess({ binaryPath });
  const updates: acp.UpdateSessionNotification[] = [];
  const connection = acp
    .client()
    .onNotification("session/update", ({ params }) => {
      updates.push(params);
    })
    .connect(process.stream);
  try {
    const initialized = await connection.agent.request("initialize", {
      protocolVersion: acp.PROTOCOL_VERSION,
      info: { name: "sdk-wire-test", version: "1" },
      capabilities: {},
    });
    expect(initialized.protocolVersion).toBe(2);
    await connection.agent.request("auth/login", { methodId: "fake" });
    await connection.agent.request("auth/logout", {});
    const cwd = path.resolve(".");
    const session = await connection.agent.request("session/new", { cwd });
    const request = { cwd, sessionId: session.sessionId };
    await connection.agent.request("session/resume", request);
    expect(updates).toEqual([]);
    await connection.agent.request("session/resume", {
      ...request,
      replayFrom: { type: "start" },
    });
    expect(updates.map((n) => n.update.sessionUpdate)).toEqual([
      "user_message",
      "state_update",
    ]);
    expect(updates[0]?.update).toMatchObject({ messageId: "history-user" });
  } finally {
    connection.close();
  }
});
