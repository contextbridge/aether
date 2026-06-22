import type { AgentMessage, ContextUsage } from "@aether-agent/sdk";

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
  readonly messages: AgentMessage[];

  constructor(messages: AgentMessage[] = []) {
    this.messages = [...messages];
  }

  static async fromStream(
    stream: AsyncIterable<AgentMessage>,
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

  add(message: AgentMessage): void {
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
  type: "context_usage",
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

export function isTerminalMessage(message: AgentMessage): boolean {
  return (
    message.type === "done" ||
    message.type === "error" ||
    message.type === "cancelled"
  );
}

function extractToolCalls(messages: AgentMessage[]): ToolCall[] {
  const calls: ToolCall[] = [];
  for (const message of messages) {
    if (message.type === "tool_result") {
      calls.push(new ToolCall(message.result.name, message.result.arguments));
    } else if (message.type === "tool_error") {
      calls.push(
        new ToolCall(message.error.name, message.error.arguments ?? ""),
      );
    }
  }
  return calls;
}

function summarizeUsage(messages: AgentMessage[]): ContextUsage {
  for (let i = messages.length - 1; i >= 0; i--) {
    const message = messages[i];
    if (message && message.type === "context_usage") return message;
  }
  return { ...ZERO_USAGE };
}
