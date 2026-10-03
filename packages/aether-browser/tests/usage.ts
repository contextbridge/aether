import init, {
  AetherClient,
  type AetherClientError,
  type AetherClientEvent,
  type Conversation,
  type ConversationItem,
  type Elicitation,
  type RemoteServerInfo,
  type SubAgentEvent,
} from "@aether-agent/browser";
import type {
  CreateElicitationRequest,
  PromptResponse,
  SessionUpdate,
  ToolCallUpdate,
} from "@agentclientprotocol/sdk/experimental/v2";

export async function usage(url: string): Promise<string> {
  await init();
  const client = await AetherClient.connect(url, onEvent, {
    protocols: ["gateway.auth"],
  });
  const agent: string = client.initializeResponse.info.name;
  const remote: RemoteServerInfo | null = client.remote;
  const cwd = remote?.cwd ?? "/workspace";
  const session = await client.newSession({ cwd });
  const response: PromptResponse = await client.prompt([
    { type: "text", text: `hello ${agent}` },
  ]);
  await client.cancel();
  await client.resumeSession({
    sessionId: remote?.sessionId ?? session.sessionId,
    cwd,
    replayFrom: { type: "start" },
  });
  await client.closeSession();
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

export function subAgentToolCallId(event: SubAgentEvent): string | undefined {
  switch (event.type) {
    case "started":
    case "done":
      return undefined;
    case "tool_call_update": {
      const update: ToolCallUpdate = event;
      return update.toolCallId;
    }
  }
}

function onEvent(event: AetherClientEvent): void {
  switch (event.type) {
    case "session_update":
      render(event.notification.update);
      break;
    case "sub_agent_progress":
      console.log(event.params.parentToolId, event.params.event.type);
      break;
    case "mcp_notification":
      console.log(event.params.servers.map((server) => server.status.type));
      break;
    case "elicitation_request":
      answer(event.elicitation);
      break;
    case "connection_closed":
      console.log(event.close?.code, event.close?.reason);
      break;
    case "conversation_changed":
      console.log(event.conversation && summarize(event.conversation));
      break;
    default:
      console.log(event.type, event.params);
  }
}

function answer(elicitation: Elicitation): void {
  const request: CreateElicitationRequest = elicitation.request;
  console.log(request.message);
  elicitation.respond({ action: "decline" });
}

function summarize(conversation: Conversation): string[] {
  return [
    conversation.sessionId,
    conversation.activity,
    ...conversation.items.map(describe),
  ];
}

function describe(item: ConversationItem): string {
  switch (item.kind) {
    case "user":
    case "assistant":
    case "thought":
      return item.content
        .map((block) => (block.type === "text" ? block.text : block.type))
        .join("");
    case "tool": {
      const { status, subAgents, toolCall } = item.content;
      return `${toolCall.title ?? "tool"}: ${status} (${subAgents.length} sub-agents)`;
    }
    case "notice":
      return item.content;
  }
}

function render(update: SessionUpdate): void {
  console.log(update.sessionUpdate);
}
