import type {
  ContentBlock,
  SessionUpdate,
  ToolCallContent,
} from "@agentclientprotocol/sdk";
import type {
  DataMessagePart,
  MessageStatus,
  ReasoningMessagePart,
  TextMessagePart,
  ThreadMessageLike,
  ToolCallMessagePart,
} from "@assistant-ui/react";

export type MessageCursor = {
  openMessageId: string | null;
  seeded: boolean;
};

type AssistantPart =
  | TextMessagePart
  | ReasoningMessagePart
  | ToolCallMessagePart
  | DataMessagePart;

type AssistantMessage = {
  id: string;
  role: "assistant";
  content: AssistantPart[];
  status?: MessageStatus;
};

type MessageResult = {
  messages: ThreadMessageLike[];
  cursor: MessageCursor;
};

const fileMentionPattern =
  /:file\[[^\]\n]{1,1024}\]\{name=([^}\n]{1,1024})\}/gu;

export const extractMentionedFilePaths = (text: string): string[] => [
  ...new Set([...text.matchAll(fileMentionPattern)].map((match) => match[1]!)),
];

export const finalizeOpenMessage = (
  messages: ThreadMessageLike[],
  openMessageId: string | null,
  status: NonNullable<AssistantMessage["status"]> = {
    type: "complete",
    reason: "stop",
  },
): ThreadMessageLike[] => {
  if (openMessageId === null) return messages;
  return messages.map((message) =>
    message.id === openMessageId &&
    message.role === "assistant" &&
    message.status?.type === "running"
      ? { ...message, status }
      : message,
  );
};

export const applySessionUpdate = (
  messages: ThreadMessageLike[],
  cursor: MessageCursor,
  update: SessionUpdate,
  nextAssistantMessageId: () => string,
): MessageResult => {
  switch (update.sessionUpdate) {
    case "user_message_chunk": {
      if (update.content.type !== "text" || !update.content.text) {
        return { messages, cursor };
      }
      const closed = finalizeOpenMessage(messages, cursor.openMessageId);
      const id = update.messageId ?? `user-${closed.length + 1}`;
      const last = closed[closed.length - 1];
      if (last?.role === "user" && last.id === id) {
        const text =
          typeof last.content === "string"
            ? last.content + update.content.text
            : [
                ...last.content,
                { type: "text" as const, text: update.content.text },
              ];
        return {
          messages: [...closed.slice(0, -1), { ...last, content: text }],
          cursor: { openMessageId: null, seeded: false },
        };
      }
      return {
        messages: [
          ...closed,
          { id, role: "user", content: update.content.text },
        ],
        cursor: { openMessageId: null, seeded: false },
      };
    }
    case "agent_message_chunk":
    case "agent_thought_chunk": {
      if (update.content.type !== "text" || !update.content.text) {
        return { messages, cursor };
      }
      const messageId = update.messageId ?? null;
      if (update.sessionUpdate === "agent_thought_chunk") {
        return appendPart(
          messages,
          cursor,
          messageId,
          {
            type: "reasoning",
            text: update.content.text,
          },
          nextAssistantMessageId,
        );
      }
      return appendPart(
        messages,
        cursor,
        messageId,
        {
          type: "text",
          text: update.content.text,
        },
        nextAssistantMessageId,
      );
    }
    case "tool_call":
      return appendPart(
        messages,
        cursor,
        null,
        toolCallPart(update),
        nextAssistantMessageId,
      );
    case "tool_call_update":
      return patchToolCall(
        messages,
        cursor,
        update.toolCallId,
        toolCallPatch(update),
      );
    case "plan":
      return upsertPlan(
        messages,
        cursor,
        update.entries,
        nextAssistantMessageId,
      );
    default:
      return { messages, cursor };
  }
};

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

const asAssistantMessage = (
  message: ThreadMessageLike | undefined,
): AssistantMessage | undefined =>
  message?.role === "assistant" && typeof message.content !== "string"
    ? (message as AssistantMessage)
    : undefined;

const toolResultText = (
  content: ToolCallContent[] | null | undefined,
): string | undefined => {
  const text = (content ?? [])
    .filter(
      (
        block,
      ): block is ToolCallContent & {
        content: Extract<ContentBlock, { type: "text" }>;
      } => block.type === "content" && block.content.type === "text",
    )
    .map((block) => block.content.text)
    .join("\n");
  return text || undefined;
};

const mergePart = (
  parts: readonly AssistantPart[],
  part: AssistantPart,
): AssistantPart[] => {
  const last = parts[parts.length - 1];
  if (part.type === "text" && last?.type === "text") {
    return [...parts.slice(0, -1), { ...last, text: last.text + part.text }];
  }
  if (part.type === "reasoning" && last?.type === "reasoning") {
    return [...parts.slice(0, -1), { ...last, text: last.text + part.text }];
  }
  return [...parts, part];
};

const openMessage = (
  messages: ThreadMessageLike[],
  cursor: MessageCursor,
  messageId: string | null,
  nextAssistantMessageId: () => string,
): MessageResult & { message: AssistantMessage } => {
  const last = asAssistantMessage(messages[messages.length - 1]);

  if (last && cursor.openMessageId !== null) {
    if (messageId !== null && cursor.seeded) {
      const adopted = { ...last, id: messageId };
      return {
        messages: [...messages.slice(0, -1), adopted],
        cursor: { openMessageId: messageId, seeded: false },
        message: adopted,
      };
    }
    if (messageId === null || messageId === cursor.openMessageId) {
      return { messages, cursor, message: last };
    }
  }

  const next =
    cursor.openMessageId === null
      ? messages
      : finalizeOpenMessage(messages, cursor.openMessageId);
  const id = messageId ?? nextAssistantMessageId();
  const message: AssistantMessage = {
    id,
    role: "assistant",
    content: [],
    status: { type: "running" },
  };
  return {
    messages: [...next, message],
    cursor: { openMessageId: id, seeded: messageId === null },
    message,
  };
};

const appendPart = (
  messages: ThreadMessageLike[],
  cursor: MessageCursor,
  messageId: string | null,
  part: AssistantPart,
  nextAssistantMessageId: () => string,
): MessageResult => {
  const {
    messages: next,
    cursor: nextCursor,
    message,
  } = openMessage(messages, cursor, messageId, nextAssistantMessageId);
  const index = next.length - 1;
  return {
    messages: [
      ...next.slice(0, index),
      { ...message, content: mergePart(message.content, part) },
    ],
    cursor: nextCursor,
  };
};

const patchToolCall = (
  messages: ThreadMessageLike[],
  cursor: MessageCursor,
  toolCallId: string,
  patch: Partial<ToolCallMessagePart>,
): MessageResult => {
  if (cursor.openMessageId === null) return { messages, cursor };
  const index = messages.length - 1;
  const message = asAssistantMessage(messages[index]);
  if (!message || message.id !== cursor.openMessageId) {
    return { messages, cursor };
  }
  const content = message.content.map((part) =>
    part.type === "tool-call" && part.toolCallId === toolCallId
      ? { ...part, ...patch }
      : part,
  );
  return {
    messages: [...messages.slice(0, index), { ...message, content }],
    cursor,
  };
};

const upsertPlan = (
  messages: ThreadMessageLike[],
  cursor: MessageCursor,
  entries: Extract<SessionUpdate, { sessionUpdate: "plan" }>["entries"],
  nextAssistantMessageId: () => string,
): MessageResult => {
  const part: DataMessagePart = { type: "data", name: "plan", data: entries };
  const {
    messages: next,
    cursor: nextCursor,
    message,
  } = openMessage(messages, cursor, null, nextAssistantMessageId);
  const last = message.content[message.content.length - 1];
  if (last?.type === "data" && last.name === "plan") {
    const index = next.length - 1;
    return {
      messages: [
        ...next.slice(0, index),
        { ...message, content: [...message.content.slice(0, -1), part] },
      ],
      cursor: nextCursor,
    };
  }
  return appendPart(next, nextCursor, null, part, nextAssistantMessageId);
};

const toolCallPart = (
  update: Extract<SessionUpdate, { sessionUpdate: "tool_call" }>,
): ToolCallMessagePart => {
  const args = isRecord(update.rawInput) ? update.rawInput : {};
  const toolName = update.name ?? update._meta?.["aetherToolName"];
  return {
    type: "tool-call",
    toolCallId: update.toolCallId,
    toolName: update.title,
    args: isRecord(update.rawInput)
      ? (update.rawInput as ToolCallMessagePart["args"])
      : {},
    argsText: JSON.stringify(args, null, 2),
    providerMetadata:
      typeof toolName === "string" ? { aether: { toolName } } : undefined,
  };
};

const toolCallPatch = (
  update: Extract<SessionUpdate, { sessionUpdate: "tool_call_update" }>,
): Partial<ToolCallMessagePart> => {
  const title = update.title != null ? { toolName: update.title } : {};
  if (update.status === "completed") {
    return {
      ...title,
      result: update.rawOutput ?? toolResultText(update.content),
    };
  }
  if (update.status === "failed") {
    return {
      ...title,
      result: {
        error:
          update.rawOutput ??
          toolResultText(update.content) ??
          "Tool call failed",
      },
      isError: true,
    };
  }
  return title;
};
