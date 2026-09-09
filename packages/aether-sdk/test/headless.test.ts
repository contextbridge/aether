import childProcess from "node:child_process";
import { syncBuiltinESMExports } from "node:module";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { afterEach, describe, expect, it, onTestFinished, vi } from "vitest";
import { z } from "zod";

import { mcp, runHeadless, tool, type AgentEvent } from "../src/index.js";
import { sessionUsageFactory } from "./factories/sessionUsage.js";
import { TRACE_ID_CONTEXT } from "./traceContext.js";

const FAKE_AETHER = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "fakeAether.mjs",
);

let tempDirs: string[] = [];

afterEach(async () => {
  vi.restoreAllMocks();
  syncBuiltinESMExports();
  await Promise.all(
    tempDirs.map((dir) => rm(dir, { recursive: true, force: true })),
  );
  tempDirs = [];
});

describe("runHeadless()", () => {
  it("passes SDK-hosted mcp() sources through to headless via settings.mcps", async () => {
    const dir = await mkdtemp(path.join(tmpdir(), "aether-sdk-headless-"));
    tempDirs.push(dir);
    const logFile = path.join(dir, "fake-aether.jsonl");
    const submit = tool({
      name: "submit",
      description: "submit",
      inputSchema: { value: z.string() },
      handler: async () => ({ content: [{ type: "text", text: "ok" }] }),
    });

    await using weather = await mcp({ name: "weather", tools: [submit] });

    const result = await Array.fromAsync(
      runHeadless({
        binaryPath: FAKE_AETHER,
        prompt: "call the tool",
        model: "anthropic:claude-sonnet-4-5",
        settings: { agents: [], mcps: [weather.spec] },
        providers: {
          bedrock: {
            url: "http://127.0.0.1:8787",
            auth: "none",
            inferenceProfileArn: "arn:test",
          },
        },
        events: ["tool_call", "turn_ended"],
        traceContext: TRACE_ID_CONTEXT,
        env: { ...process.env, FAKE_AETHER_LOG_FILE: logFile },
      }),
    );

    expect(result).toContainEqual(
      expect.objectContaining({
        category: "message",
        event: expect.objectContaining({ chunk: "call the tool" }),
      }),
    );

    const log = JSON.parse((await readFile(logFile, "utf8")).trim());
    expect(log.event).toBe("headless");
    const optionsIndex = log.args.indexOf("--options-json");
    expect(optionsIndex).toBeGreaterThan(0);
    const options = JSON.parse(log.args[optionsIndex + 1]);
    expect(options).toMatchObject({
      prompt: "call the tool",
      settings: { agents: [] },
      model: "anthropic:claude-sonnet-4-5",
      output: "json",
      events: ["tool_call", "turn_ended"],
      traceContext: TRACE_ID_CONTEXT,
      providers: {
        bedrock: {
          url: "http://127.0.0.1:8787",
          auth: "none",
          inferenceProfileArn: "arn:test",
        },
      },
    });
    const inlineSource = options.settings.mcps[0];
    expect(inlineSource.type).toBe("inline");
    const weatherServer = inlineSource.servers.weather;
    expect(weatherServer).toMatchObject({ type: "http" });
    expect(weatherServer.headers.Authorization).toMatch(/^Bearer .+$/);
    expect(options).not.toHaveProperty("mcpConfig");
  });

  it("parses each stdout line as an AgentEvent", async () => {
    const events: AgentEvent[] = [
      textEvent("hello 🌍"),
      { category: "session_usage", event: sessionUsageFactory.build() },
      turnEnded,
    ];
    expect(
      await Array.fromAsync(stream(events, { FAKE_CHUNK_SIZE: "1" })),
    ).toEqual(events);
  });

  it.each(["", "\n\r\n  \n"])(
    "accepts empty output and blank lines: %j",
    async (raw) => {
      expect(await Array.fromAsync(streamRaw(raw))).toEqual([]);
    },
  );

  it("yields events with categories it does not know", async () => {
    const event = { category: "future", event: { type: "new" } };
    expect(await Array.fromAsync(streamRaw(JSON.stringify(event)))).toEqual([
      event,
    ]);
  });

  it.each(["{", "null", "[]", "{}", '{"category":"turn"}'])(
    "rejects malformed protocol output: %s",
    async (raw) => {
      await expect(Array.fromAsync(streamRaw(raw))).rejects.toMatchObject({
        code: "invalid_protocol_message",
      });
    },
  );

  it.each(["", "\n", "\r\n"])(
    "yields the final event before a nonzero exit with trailing delimiter %j",
    async (delimiter) => {
      const received: AgentEvent[] = [];
      await expect(
        Array.fromAsync(
          streamRaw(JSON.stringify(turnEnded) + delimiter, {
            FAKE_EXIT_CODE: "2",
            FAKE_STDERR: "provider failed",
          }),
          (event) => received.push(event),
        ),
      ).rejects.toMatchObject({
        code: "process_exited",
        message: expect.stringContaining("provider failed"),
      });
      expect(received).toEqual([turnEnded]);
    },
  );

  it("reports spawn failures", async () => {
    await expect(
      Array.fromAsync(
        runHeadless({ binaryPath: "/nonexistent/aether", prompt: "hello" }),
      ),
    ).rejects.toMatchObject({ code: "process_spawn_failed" });
  });

  it("rejects an already aborted run", async () => {
    await expect(
      Array.fromAsync(streamRaw("", {}, AbortSignal.abort())),
    ).rejects.toMatchObject({ code: "aborted" });
  });

  it("streams events while the child is alive and cleans up on break", async () => {
    let pid: number;
    for await (const event of heldStream()) {
      pid = eventPid(event);
      expect(() => process.kill(pid, 0)).not.toThrow();
      break;
    }
    expectExited(pid);
  });

  it("cleans up when the loop body throws", async () => {
    const failure = new Error("consumer failed");
    let pid: number;
    await expect(
      (async () => {
        for await (const event of heldStream()) {
          pid = eventPid(event);
          throw failure;
        }
      })(),
    ).rejects.toBe(failure);
    expectExited(pid);
  });

  it("reports signal termination as a failure", async () => {
    const { events, pid } = await startHeld();
    process.kill(pid, "SIGTERM");
    await expect(Array.fromAsync(events)).rejects.toMatchObject({
      code: "process_exited",
      message: expect.stringContaining("signal=SIGTERM"),
    });
    expectExited(pid);
  });

  it("aborts a pending read without yielding an unterminated buffered event", async () => {
    const controller = new AbortController();
    const { events, pid } = await startHeld(
      controller.signal,
      JSON.stringify(turnEnded),
    );
    const pending = events.next();
    controller.abort();
    await expect(pending).rejects.toMatchObject({ code: "aborted" });
    expectExited(pid);
  });

  it("reports cancellation after stdout EOF while waiting for the child to exit", async () => {
    const controller = new AbortController();
    const spawn = childProcess.spawn;
    vi.spyOn(childProcess, "spawn").mockImplementation((...args) => {
      const child = spawn(...args);
      child.stdout!.once("end", () => setImmediate(() => controller.abort()));
      return child;
    });
    syncBuiltinESMExports();
    const { events, pid } = await startHeld(controller.signal, "", {
      FAKE_CLOSE_STDOUT: "1",
    });
    await expect(events.next()).rejects.toMatchObject({ code: "aborted" });
    expectExited(pid);
  });

  it("cleans up a live child when an event is malformed", async () => {
    const { events, pid } = await startHeld(undefined, "{\n");
    await expect(events.next()).rejects.toMatchObject({
      code: "invalid_protocol_message",
    });
    expectExited(pid);
  });

  it("rejects conflicting options", async () => {
    await expect(
      Array.fromAsync(
        runHeadless({
          binaryPath: FAKE_AETHER,
          prompt: "hello",
          agent: "planner",
          model: "anthropic:claude-sonnet-4-5",
        } as never),
      ),
    ).rejects.toThrow(/agent and model/);
  });
});

const turnEnded: AgentEvent = {
  category: "turn",
  event: { type: "ended", outcome: { status: "completed" } },
};

function textEvent(chunk: string): AgentEvent {
  return {
    category: "message",
    event: { type: "text", message_id: "text", chunk, is_complete: true },
  };
}

function stream(events: AgentEvent[], env: Record<string, string> = {}) {
  return streamRaw(
    events.map((event) => JSON.stringify(event)).join("\r\n"),
    env,
  );
}

function streamRaw(
  stdout: string,
  env: Record<string, string> = {},
  abortSignal?: AbortSignal,
) {
  return runHeadless({
    binaryPath: FAKE_AETHER,
    prompt: "hello",
    env: { ...process.env, ...env, FAKE_STDOUT: stdout },
    abortSignal,
  });
}

function heldStream(
  abortSignal?: AbortSignal,
  suffix = "",
  env: Record<string, string> = {},
) {
  const events = streamRaw(
    JSON.stringify(textEvent("$PID")) + "\n" + suffix,
    { ...env, FAKE_HOLD: "1" },
    abortSignal,
  );
  onTestFinished(async () => {
    await events.return();
  });
  return events;
}

async function startHeld(
  abortSignal?: AbortSignal,
  suffix = "",
  env: Record<string, string> = {},
) {
  const events = heldStream(abortSignal, suffix, env);
  const first = await events.next();
  expect(first.done).toBe(false);
  return { events, pid: eventPid(first.value as AgentEvent) };
}

function eventPid(event: AgentEvent): number {
  if (event.category !== "message" || event.event.type !== "text") {
    throw new Error("Expected a text event containing the child pid");
  }
  const pid = Number(event.event.chunk);
  expect(pid).toBeGreaterThan(0);
  return pid;
}

function expectExited(pid: number) {
  expect(pid).toBeGreaterThan(0);
  expect(() => process.kill(pid, 0)).toThrow(
    expect.objectContaining({ code: "ESRCH" }),
  );
}
