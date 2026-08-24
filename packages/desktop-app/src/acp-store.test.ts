import { beforeEach, describe, expect, it, vi } from "vitest";
import type {
  AvailableCommandsUpdate,
  SessionConfigOption,
  SessionUpdate,
} from "@agentclientprotocol/sdk";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({
  Channel: class {},
  invoke,
}));

import type { ThreadMessageLike } from "@assistant-ui/react";
import { AppActions } from "./app-actions";
import { createChatStore, type ChatStore } from "./acp-store";

const CONNECTION_ID = "conn-1";

let store: ChatStore;
let actions: AppActions;

const connect = () => {
  store.setState({
    connection: {
      status: "connected",
      connectionId: CONNECTION_ID,
      sessionId: "s1",
      agentName: "Aether",
    },
    configOptions: [],
    availableCommands: [],
    workspaceFiles: [],
    messages: [],
    isRunning: true,
    error: null,
    openMessageId: null,
    openMessageSeeded: false,
  });
};

const event = (update: SessionUpdate) => ({
  kind: "sessionUpdate" as const,
  connectionId: CONNECTION_ID,
  sessionId: "s1",
  update,
});

const textChunk = (text: string, messageId = "m1"): SessionUpdate => ({
  sessionUpdate: "agent_message_chunk",
  content: { type: "text", text },
  messageId,
});

const thoughtChunk = (text: string, messageId = "m1"): SessionUpdate => ({
  sessionUpdate: "agent_thought_chunk",
  content: { type: "text", text },
  messageId,
});

const toolCall = (toolCallId = "t1"): SessionUpdate => ({
  sessionUpdate: "tool_call",
  toolCallId,
  title: "Read file",
  status: "in_progress",
  rawInput: { path: "src/lib.rs" },
  _meta: { aetherToolName: "coding__read_file" },
});

const toolCallUpdate = (
  toolCallId: string,
  status: "completed" | "failed",
  result: string,
): SessionUpdate => ({
  sessionUpdate: "tool_call_update",
  toolCallId,
  status,
  content: [{ type: "content", content: { type: "text", text: result } }],
});

const lastMessage = (): ThreadMessageLike =>
  store.getState().messages[store.getState().messages.length - 1];

const parts = (message: ThreadMessageLike) =>
  typeof message.content === "string" ? [] : [...message.content];

beforeEach(() => {
  invoke.mockReset();
  store = createChatStore();
  actions = new AppActions(store);
  connect();
});

const selectConfig = (
  id: string,
  currentValue: string,
  values: string[],
  category: "mode" | "model" | "thought_level",
): SessionConfigOption => ({
  type: "select",
  id,
  name: id,
  category,
  currentValue,
  options: values.map((value) => ({ value, name: value })),
});

describe("session configuration", () => {
  it("publishes the agent's available slash commands", () => {
    const update: SessionUpdate = {
      sessionUpdate: "available_commands_update",
      availableCommands: [
        {
          name: "review",
          description: "Review the current changes",
          input: { hint: "optional focus" },
        },
      ],
    } satisfies AvailableCommandsUpdate & {
      sessionUpdate: "available_commands_update";
    };

    actions.handleEvent(event(update));

    expect(store.getState().availableCommands).toEqual(
      update.availableCommands,
    );
  });

  it("replaces configuration when ACP publishes an update", () => {
    const configOptions = [
      selectConfig("model", "anthropic:sonnet", ["anthropic:sonnet"], "model"),
      selectConfig("mode", "Coder", ["Coder", "Planner"], "mode"),
    ];

    actions.handleEvent(
      event({ sessionUpdate: "config_option_update", configOptions }),
    );

    expect(store.getState().configOptions).toEqual(configOptions);
  });

  it("sends configuration changes to the active ACP session", async () => {
    invoke.mockResolvedValue(undefined);

    await actions.setConfigOption("reasoning_effort", "high");

    expect(invoke).toHaveBeenCalledWith("set_session_config_option", {
      sessionId: "s1",
      configId: "reasoning_effort",
      value: "high",
    });
  });
});

describe("file mentions", () => {
  it("sends selected file directives as ACP file paths", async () => {
    invoke.mockResolvedValue(undefined);

    await actions.send("Read :file[src/lib.rs]{name=/workspace/src/lib.rs}");

    expect(invoke).toHaveBeenCalledWith("send_prompt", {
      sessionId: "s1",
      text: "Read :file[src/lib.rs]{name=/workspace/src/lib.rs}",
      filePaths: ["/workspace/src/lib.rs"],
    });
  });
});

describe("structured parts", () => {
  it("keeps reasoning separate from the visible text", () => {
    actions.handleEvent(event(thoughtChunk("thinking ")));
    actions.handleEvent(event(thoughtChunk("hard")));
    actions.handleEvent(event(textChunk("Hello")));

    const message = lastMessage();
    expect(message.id).toBe("m1");
    expect(parts(message)).toEqual([
      { type: "reasoning", text: "thinking hard" },
      { type: "text", text: "Hello" },
    ]);
  });

  it("merges consecutive text chunks into one part", () => {
    actions.handleEvent(event(textChunk("Hel")));
    actions.handleEvent(event(textChunk("lo")));

    expect(parts(lastMessage())).toEqual([{ type: "text", text: "Hello" }]);
  });

  it("tracks the tool call lifecycle from running to completed", () => {
    actions.handleEvent(event(textChunk("Let me check")));
    actions.handleEvent(event(toolCall("t1")));
    actions.handleEvent(event(toolCallUpdate("t1", "completed", "42 lines")));

    const message = lastMessage();
    expect(message.content[1]).toMatchObject({
      type: "tool-call",
      toolCallId: "t1",
      toolName: "Read file",
      args: { path: "src/lib.rs" },
      result: "42 lines",
    });
    expect(parts(message)[1].type).toBe("tool-call");
  });

  it("marks failed tool calls as errors", () => {
    actions.handleEvent(event(toolCall("t1")));
    actions.handleEvent(
      event(toolCallUpdate("t1", "failed", "permission denied")),
    );

    const toolPart = parts(lastMessage()).find(
      (part) => part.type === "tool-call",
    );
    expect(toolPart).toMatchObject({
      type: "tool-call",
      isError: true,
      result: { error: "permission denied" },
    });
  });
});

describe("message segmentation", () => {
  it("starts a new assistant message when messageId changes", () => {
    actions.handleEvent(event(textChunk("first", "m1")));
    actions.handleEvent(event(textChunk("second", "m2")));

    const messages = store.getState().messages;
    expect(messages).toHaveLength(2);
    expect(messages[0]).toMatchObject({
      id: "m1",
      status: { type: "complete", reason: "stop" },
    });
    expect(messages[1]).toMatchObject({
      id: "m2",
      status: { type: "running" },
    });
  });

  it("adopts a real messageId into a message seeded by a tool call", () => {
    actions.handleEvent(event(toolCall("t1")));
    actions.handleEvent(event(textChunk("result", "m1")));

    const messages = store.getState().messages;
    expect(messages).toHaveLength(1);
    expect(messages[0].id).toBe("m1");
    expect(parts(messages[0])).toHaveLength(2);
  });

  it("replaces the plan in place", () => {
    actions.handleEvent(
      event({
        sessionUpdate: "plan",
        entries: [
          { content: "step one", priority: "high", status: "in_progress" },
        ],
      }),
    );
    actions.handleEvent(
      event({
        sessionUpdate: "plan",
        entries: [
          { content: "step one", priority: "high", status: "completed" },
          { content: "step two", priority: "medium", status: "pending" },
        ],
      }),
    );

    const message = lastMessage();
    expect(parts(message)).toHaveLength(1);
    expect(parts(message)[0]).toMatchObject({
      type: "data",
      name: "plan",
      data: [
        { content: "step one", priority: "high", status: "completed" },
        { content: "step two", priority: "medium", status: "pending" },
      ],
    });
  });
});

describe("turn lifecycle", () => {
  it("finalizes the open message on promptDone", () => {
    actions.handleEvent(event(textChunk("done")));
    actions.handleEvent({
      kind: "promptDone",
      connectionId: CONNECTION_ID,
      sessionId: "s1",
      stopReason: "endTurn",
    });

    expect(lastMessage()).toMatchObject({
      id: "m1",
      status: { type: "complete", reason: "stop" },
    });
    const state = store.getState();
    expect(state.openMessageId).toBeNull();
    expect(state.isRunning).toBe(false);
  });

  it("marks the message incomplete and surfaces the error on promptError", () => {
    actions.handleEvent(event(textChunk("partial")));
    actions.handleEvent({
      kind: "promptError",
      connectionId: CONNECTION_ID,
      sessionId: "s1",
      error: "boom",
    });

    expect(lastMessage()).toMatchObject({
      status: { type: "incomplete", reason: "error", error: "boom" },
    });
    expect(store.getState().error).toBe("boom");
    expect(store.getState().isRunning).toBe(false);
  });

  it("starts a fresh message on the next turn", () => {
    actions.handleEvent(event(textChunk("first", "m1")));
    actions.handleEvent({
      kind: "promptDone",
      connectionId: CONNECTION_ID,
      sessionId: "s1",
      stopReason: "endTurn",
    });
    actions.handleEvent(event(textChunk("second", "m2")));

    const messages = store.getState().messages;
    expect(messages).toHaveLength(2);
    expect(messages[0]).toMatchObject({
      id: "m1",
      status: { type: "complete" },
    });
    expect(messages[1]).toMatchObject({
      id: "m2",
      status: { type: "running" },
    });
  });

  it("ignores events from a stale connection", () => {
    actions.handleEvent({
      kind: "sessionUpdate",
      connectionId: "conn-2",
      sessionId: "s2",
      update: textChunk("stale"),
    });

    expect(store.getState().messages).toEqual([]);
  });
});
