import type {
  Activity,
  Conversation,
  ConversationItem,
  ToolStatus,
  TurnPhase,
} from "@aether-agent/browser";
import type {
  ContentBlock,
  ToolCallUpdate,
} from "@agentclientprotocol/sdk/experimental/v2";

/** Builds the `Conversation` snapshots the wasm client delivers. */
export class ConversationBuilder {
  private readonly items: ConversationItem[] = [];
  private turn: TurnPhase = "idle";
  private activity: Activity = { phase: "idle", thought: "" };

  user(...content: (string | ContentBlock)[]): this {
    return this.push({ kind: "user", content: content.map(block) });
  }

  assistant(...content: (string | ContentBlock)[]): this {
    return this.push({ kind: "assistant", content: content.map(block) });
  }

  tool(toolCall: ToolCallUpdate, status: ToolStatus = "success"): this {
    return this.push({
      kind: "tool",
      content: { status, subAgents: [], toolCall },
    });
  }

  notice(text: string): this {
    return this.push({ kind: "notice", content: text });
  }

  running(): this {
    this.turn = "running";
    return this;
  }

  thinking(thought: string): this {
    this.activity = { phase: "thinking", thought };
    return this.running();
  }

  build(): Conversation {
    return {
      items: [...this.items],
      turn: this.turn,
      activity: this.activity,
      plan: null,
      contextUsage: null,
      compacting: false,
    };
  }

  private push(item: DistributiveOmit<ConversationItem, Base>): this {
    this.items.push({
      id: this.items.length,
      messageId: null,
      revision: 0,
      state: "sealed",
      ...item,
    } as ConversationItem);
    return this;
  }
}

export function conversation(): ConversationBuilder {
  return new ConversationBuilder();
}

type Base = "id" | "messageId" | "revision" | "state";
type DistributiveOmit<T, K extends PropertyKey> = T extends unknown
  ? Omit<T, K>
  : never;

function block(content: string | ContentBlock): ContentBlock {
  return typeof content === "string"
    ? { type: "text", text: content }
    : content;
}
