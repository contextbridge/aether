import type {
  AgentEvent,
  ContextUsage,
  SessionUsageTotals,
  TurnOutcome,
} from "@aether-agent/sdk";

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
  readonly events: AgentEvent[];

  constructor(events: AgentEvent[] = []) {
    this.events = [...events];
  }

  static async fromStream(
    stream: AsyncIterable<AgentEvent>,
  ): Promise<Transcript> {
    const transcript = new Transcript();
    try {
      for await (const event of stream) {
        transcript.add(event);
      }
    } catch (err) {
      throw new TranscriptError(transcript, err);
    }
    return transcript;
  }

  add(event: AgentEvent): void {
    this.events.push(event);
  }

  allToolCalls(): ToolCall[] {
    return extractToolCalls(this.events);
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
    return summarizeUsage(this.events);
  }

  sessionUsage(): SessionUsageTotals {
    return lastSessionUsage(this.events);
  }
}

const ZERO_USAGE: ContextUsage = {
  usage_ratio: null,
  context_limit: null,
  input_tokens: 0,
};

const ZERO_SESSION_USAGE: SessionUsageTotals = {
  tokens: {
    input_tokens: 0,
    output_tokens: 0,
    cache_read_tokens: null,
    cache_creation_tokens: null,
    input_audio_tokens: null,
    input_video_tokens: null,
    reasoning_tokens: null,
    output_audio_tokens: null,
    accepted_prediction_tokens: null,
    rejected_prediction_tokens: null,
  },
  estimated_usd: 0,
  estimated_input_usd: 0,
  estimated_output_usd: 0,
  estimated_cache_read_usd: 0,
  estimated_cache_creation_usd: 0,
  unpriced_calls: 0,
};

export function isTerminalEvent(event: AgentEvent): boolean {
  return event.category === "turn" && event.event.type === "ended";
}

/** Build the terminal turn event, defaulting to a completed turn. */
export function turnEnded(
  outcome: TurnOutcome = { status: "completed" },
): AgentEvent {
  return { category: "turn", event: { type: "ended", outcome } };
}

function extractToolCalls(events: AgentEvent[]): ToolCall[] {
  const calls: ToolCall[] = [];
  for (const event of events) {
    if (event.category !== "tool") continue;
    const tool = event.event;
    if (tool.type === "result") {
      calls.push(new ToolCall(tool.result.name, tool.result.arguments));
    } else if (tool.type === "error") {
      calls.push(new ToolCall(tool.error.name, tool.error.arguments ?? ""));
    }
  }
  return calls;
}

function summarizeUsage(events: AgentEvent[]): ContextUsage {
  for (let i = events.length - 1; i >= 0; i--) {
    const event = events[i];
    if (event?.category === "context" && event.event.type === "usage_updated")
      return event.event.usage;
  }
  return { ...ZERO_USAGE };
}

function lastSessionUsage(events: AgentEvent[]): SessionUsageTotals {
  for (let i = events.length - 1; i >= 0; i--) {
    const event = events[i];
    if (event?.category === "session_usage") return event.event.totals;
  }
  return structuredClone(ZERO_SESSION_USAGE);
}
