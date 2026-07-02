import type { AgentEvent, ContextUsage } from "@aether-agent/sdk";

type TurnPayload = { type: string };
type ToolPayload = {
  type: string;
  result?: { name: string; arguments: string };
  error?: { name: string; arguments?: string | null };
};
type ContextPayload = { type: string; usage?: ContextUsage };

export class ToolCall {
  readonly arguments: string;

  constructor(
    readonly name: string,
    argumentsValue: string,
  ) {
    this.arguments = argumentsValue;
  }

  argumentsJson(): unknown {
    return JSON.parse(this.arguments);
  }
}

export class TranscriptError extends Error {
  readonly transcript: Transcript;

  constructor(transcript: Transcript, cause: unknown) {
    super("transcript stream failed", { cause });
    this.name = "TranscriptError";
    this.transcript = transcript;
  }
}

export class Transcript {
  readonly messages: AgentEvent[];

  constructor(messages: AgentEvent[] = []) {
    this.messages = [...messages];
  }

  static async fromStream(
    stream: AsyncIterable<AgentEvent>,
  ): Promise<Transcript> {
    const transcript = new Transcript();
    try {
      for await (const message of stream) {
        transcript.add(message);
      }
    } catch (err) {
      throw new TranscriptError(transcript, err);
    }
    return transcript;
  }

  add(message: AgentEvent): void {
    this.messages.push(message);
  }

  allToolCalls(): ToolCall[] {
    return extractToolCalls(this.messages);
  }

  toolCalls(name: string): ToolCall[] {
    return this.allToolCalls().filter((call) => call.name === name);
  }

  toolCalled(name: string): boolean {
    return this.toolCalls(name).length > 0;
  }

  toolCallCount(name: string): number {
    return this.toolCalls(name).length;
  }

  usage(): ContextUsage {
    return summarizeUsage(this.messages);
  }
}

const ZERO_USAGE: ContextUsage = {
  input_tokens: 0,
  output_tokens: 0,
  cache_read_tokens: null,
  cache_creation_tokens: null,
  reasoning_tokens: null,
  usage_ratio: null,
  context_limit: null,
  total_input_tokens: 0,
  total_output_tokens: 0,
  total_cache_read_tokens: 0,
  total_cache_creation_tokens: 0,
  total_reasoning_tokens: 0,
};

export function isTerminalMessage(event: AgentEvent): boolean {
  return (
    event.category === "turn" && (event.event as TurnPayload).type === "ended"
  );
}

function extractToolCalls(events: AgentEvent[]): ToolCall[] {
  const calls: ToolCall[] = [];
  for (const event of events) {
    if (event.category !== "tool") continue;
    const tool = event.event as ToolPayload;
    if (tool.type === "result") {
      calls.push(new ToolCall(tool.result!.name, tool.result!.arguments));
    } else if (tool.type === "error") {
      calls.push(new ToolCall(tool.error!.name, tool.error!.arguments ?? ""));
    }
  }
  return calls;
}

function summarizeUsage(events: AgentEvent[]): ContextUsage {
  for (let i = events.length - 1; i >= 0; i--) {
    const event = events[i];
    const context = event?.event as ContextPayload | undefined;
    if (
      event?.category === "context" &&
      context?.type === "usage_updated" &&
      context.usage
    )
      return context.usage;
  }
  return { ...ZERO_USAGE };
}
