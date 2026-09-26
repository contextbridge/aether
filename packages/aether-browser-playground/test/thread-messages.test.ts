import { describe, expect, it } from "vitest";
import { toThreadMessages } from "../src/aether/thread-messages";
import { conversation } from "./conversation-builder";

describe("toThreadMessages", () => {
  it("joins agent messages and tool calls between prompts into one assistant message", () => {
    const messages = toThreadMessages(
      conversation()
        .user("list files")
        .assistant("Looking.")
        .tool({ toolCallId: "t1", name: "ls" })
        .assistant("Found two.")
        .user("thanks")
        .build(),
    );

    expect(messages.map(({ role, id }) => ({ role, id }))).toEqual([
      { role: "user", id: "user-0" },
      { role: "assistant", id: "reply-user-0" },
      { role: "user", id: "user-4" },
    ]);
    expect(messages[1]?.content).toMatchObject([
      { type: "text", text: "Looking." },
      { type: "tool-call", toolCallId: "t1", toolName: "ls" },
      { type: "text", text: "Found two." },
    ]);
  });

  it("gives notices their own system message", () => {
    const messages = toThreadMessages(
      conversation()
        .assistant("before")
        .notice("context cleared")
        .assistant("after")
        .build(),
    );

    expect(messages.map(({ role }) => role)).toEqual([
      "assistant",
      "system",
      "assistant",
    ]);
    expect(messages[1]?.content).toEqual([
      { type: "text", text: "context cleared" },
    ]);
  });

  it("leaves a running tool call without a result so assistant-ui shows it running", () => {
    const [message] = toThreadMessages(
      conversation()
        .tool({ toolCallId: "t1", rawInput: { path: "a.rs" } }, "running")
        .build(),
    );

    expect(message?.content).toEqual([
      expect.objectContaining({
        args: { path: "a.rs" },
        argsText: '{\n  "path": "a.rs"\n}',
      }),
    ]);
    expect(message?.content[0]).not.toHaveProperty("result");
  });

  it("marks a failed tool call as an error and carries its state as the artifact", () => {
    const failed = { error: "no such file" };
    const [message] = toThreadMessages(
      conversation()
        .tool({ toolCallId: "t1", rawInput: "a.rs", rawOutput: "boom" }, failed)
        .build(),
    );

    expect(message?.content).toEqual([
      expect.objectContaining({
        args: { input: "a.rs" },
        result: "boom",
        isError: true,
        artifact: {
          status: failed,
          subAgents: [],
          toolCall: expect.objectContaining({ toolCallId: "t1" }),
        },
      }),
    ]);
  });

  it("ends a running turn with the reply, which keeps its id as the agent's output arrives", () => {
    const started = toThreadMessages(
      conversation().user("why?").running().build(),
    );
    const thinking = toThreadMessages(
      conversation().user("why?").thinking("Let me see").build(),
    );
    const answered = toThreadMessages(
      conversation().user("why?").assistant("Because.").build(),
    );

    expect(started.at(-1)).toEqual({
      role: "assistant",
      id: "reply-user-0",
      content: [],
      status: { type: "running" },
    });
    expect(thinking.at(-1)).toMatchObject({
      id: "reply-user-0",
      content: [{ type: "reasoning", text: "Let me see" }],
    });
    expect(answered.at(-1)).toMatchObject({
      id: "reply-user-0",
      status: { type: "complete" },
    });
  });

  it("shows user images and links", () => {
    const [message] = toThreadMessages(
      conversation()
        .user(
          { type: "image", mimeType: "image/png", data: "AAAA" },
          { type: "resource_link", name: "main.rs", uri: "file:///main.rs" },
        )
        .build(),
    );

    expect(message?.content).toEqual([
      { type: "image", image: "data:image/png;base64,AAAA" },
      { type: "text", text: "[main.rs](file:///main.rs)" },
    ]);
  });
});
