import type { AgentMessage, ContextUsage } from "../generated/eval-types.js";

export interface EvalToolCall {
  name: string;
  arguments?: unknown;
  rawArguments: string;
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

/** Sum of cumulative input + output tokens. */
export function totalTokens(usage: ContextUsage): number {
  return usage.total_input_tokens + usage.total_output_tokens;
}

export function isTerminalMessage(message: AgentMessage): boolean {
  return (
    message.type === "done" ||
    message.type === "error" ||
    message.type === "cancelled"
  );
}

export function extractToolCalls(messages: AgentMessage[]): EvalToolCall[] {
  const calls: EvalToolCall[] = [];
  for (const message of messages) {
    if (message.type === "tool_result") {
      calls.push(toToolCall(message.result.name, message.result.arguments));
    } else if (message.type === "tool_error") {
      calls.push(toToolCall(message.error.name, message.error.arguments ?? ""));
    }
  }
  return calls;
}

/**
 * Returns the final `context_usage` message payload, or zeroed usage if no usage
 * was recorded.
 */
export function summarizeUsage(messages: AgentMessage[]): ContextUsage {
  for (let i = messages.length - 1; i >= 0; i--) {
    const message = messages[i];
    if (message && message.type === "context_usage") return message;
  }
  return { ...ZERO_USAGE };
}

function toToolCall(name: string, raw: string): EvalToolCall {
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    parsed = undefined;
  }
  return { name, arguments: parsed, rawArguments: raw };
}
