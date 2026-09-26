// Type-checked by `pnpm typecheck` to prove the generated declarations resolve against the ACP v2 types.
import init, {
  AetherClient,
  type AetherClientError,
  type AetherClientEvent,
  type Conversation,
  type ConversationItem,
} from "@aether-agent/browser";
import type {
  PromptResponse,
  SessionUpdate,
} from "@agentclientprotocol/sdk/experimental/v2";

export async function usage(url: string): Promise<string> {
  await init();
  const client = await AetherClient.connect(url, onEvent, {
    protocols: ["gateway.auth"],
  });
  const agent: string = client.initializeResponse.info.name;
  const session = await client.newSession({ cwd: "/workspace" });
  const response: PromptResponse = await client.prompt({
    sessionId: session.sessionId,
    prompt: [{ type: "text", text: `hello ${agent}` }],
  });
  const conversation: Conversation | undefined = client.conversation(
    session.sessionId,
  );
  if (conversation?.turn === "idle") summarize(conversation);
  client.cancel(session.sessionId);
  await client.resumeSession(
    { sessionId: session.sessionId, cwd: "/workspace" },
    true,
  );
  await client.disconnect();
  client.free();
  return response.messageId;
}

export function errorCode(
  error: unknown,
): AetherClientError["code"] | undefined {
  return error instanceof Error && "code" in error
    ? (error as AetherClientError).code
    : undefined;
}

export function closeCode(error: AetherClientError): number | undefined {
  return error.close?.code;
}

function onEvent(event: AetherClientEvent): void {
  switch (event.type) {
    case "session_update":
      render(event.notification.update);
      break;
    case "elicitation_request":
      event.respond({ action: "decline" });
      break;
    case "connection_closed":
      console.log(event.close?.code, event.close?.reason);
      break;
    case "conversation_changed":
      console.log(event.sessionId, summarize(event.conversation));
      break;
    default:
      console.log(event.type, event.params);
  }
}

function summarize(conversation: Conversation): string[] {
  const thinking =
    conversation.activity.phase === "thinking"
      ? [conversation.activity.thought]
      : [];
  return [...thinking, ...conversation.items.map(describe)];
}

function describe(item: ConversationItem): string {
  switch (item.kind) {
    case "user":
    case "assistant":
      return item.content
        .map((block) => (block.type === "text" ? block.text : block.type))
        .join("");
    case "tool": {
      const status = item.content.status;
      const outcome = typeof status === "string" ? status : status.error;
      return `${item.content.toolCall.title ?? "tool"}: ${outcome} (${item.content.subAgents.length} sub-agents)`;
    }
    case "notice":
      return item.content;
  }
}

function render(update: SessionUpdate): void {
  console.log(update.sessionUpdate);
}
