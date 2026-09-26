import type { Conversation, ToolCallState } from "@aether-agent/browser";
import type { ContentBlock } from "@agentclientprotocol/sdk/experimental/v2";
import type { ThreadMessageLike } from "@assistant-ui/react";
import { hasType } from "./acp";

export type ThreadPart = Exclude<ThreadMessageLike["content"], string>[number];

/**
 * Project a conversation onto assistant-ui messages: each user item and notice becomes its own
 * message, and every run of agent messages and tool calls after one becomes a single assistant
 * reply. While a turn runs, the thread ends in a reply carrying the agent's streamed reasoning,
 * so assistant-ui never adds a placeholder of its own.
 *
 * A reply's id names the message it follows, so it keeps its id from the moment the turn starts.
 * A tool call's `artifact` carries its full `ToolCallState` for the tool card to render.
 */
export function toThreadMessages(
  conversation: Conversation,
): ThreadMessageLike[] {
  const messages: Draft[] = [];
  for (const item of conversation.items) {
    switch (item.kind) {
      case "user":
        push(messages, "user", `user-${item.id}`).content.push(
          ...item.content.map(userPart),
        );
        break;
      case "notice":
        push(messages, "system", `notice-${item.id}`).content.push({
          type: "text",
          text: item.content,
        });
        break;
      case "assistant":
        reply(messages).content.push(...item.content.map(assistantPart));
        break;
      case "tool":
        reply(messages).content.push(toolPart(item.content));
        break;
    }
  }
  const running = conversation.turn !== "idle";
  if (running) {
    const { phase, thought } = conversation.activity;
    const trailing = reply(messages);
    if (phase === "thinking" && thought)
      trailing.content.push({ type: "reasoning", text: thought });
  }
  return messages.map(({ role, id, content }, index) =>
    role === "assistant"
      ? {
          role,
          id,
          content,
          status: running && index === messages.length - 1 ? RUNNING : COMPLETE,
        }
      : { role, id, content },
  );
}

const RUNNING = { type: "running" } as const;
const COMPLETE = { type: "complete", reason: "unknown" } as const;

interface Draft {
  role: ThreadMessageLike["role"];
  id: string;
  content: ThreadPart[];
}

function push(messages: Draft[], role: Draft["role"], id: string): Draft {
  const draft: Draft = { role, id, content: [] };
  messages.push(draft);
  return draft;
}

/** The assistant reply that agent output joins: the last message, unless a prompt or notice ended it. */
function reply(messages: Draft[]): Draft {
  const last = messages.at(-1);
  return last?.role === "assistant"
    ? last
    : push(messages, "assistant", `reply-${last?.id ?? "start"}`);
}

function userPart(block: ContentBlock): ThreadPart {
  if (hasType(block, "image")) {
    return {
      type: "image",
      image: `data:${block.mimeType};base64,${block.data}`,
    };
  }
  return assistantPart(block);
}

function assistantPart(block: ContentBlock): ThreadPart {
  if (hasType(block, "text")) return { type: "text", text: block.text };
  if (hasType(block, "resource_link"))
    return { type: "text", text: `[${block.name}](${block.uri})` };
  return { type: "text", text: `[${block.type}]` };
}

function toolPart(tool: ToolCallState): ThreadPart {
  const { toolCall, status } = tool;
  const done =
    status === "running"
      ? {}
      : {
          result: toolCall.rawOutput ?? toolCall.content ?? null,
          isError: status !== "success",
        };
  return {
    type: "tool-call",
    toolCallId: toolCall.toolCallId,
    toolName: toolCall.name ?? toolCall.kind ?? "tool",
    args: asArgs(toolCall.rawInput),
    argsText:
      toolCall.rawInput === undefined
        ? ""
        : JSON.stringify(toolCall.rawInput, null, 2),
    artifact: tool,
    ...done,
  };
}

type JsonObject = NonNullable<
  Extract<ThreadPart, { type: "tool-call" }>["args"]
>;

function asArgs(input: unknown): JsonObject {
  if (input === undefined || input === null) return {};
  if (typeof input === "object" && !Array.isArray(input))
    return input as JsonObject;
  return { input: input as JsonObject[string] };
}
