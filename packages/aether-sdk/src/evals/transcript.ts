import type { AgentMessage } from "../generated/eval-types.js";

export interface EvalToolCall {
  name: string;
  arguments?: unknown;
  rawArguments: string;
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

function toToolCall(name: string, raw: string): EvalToolCall {
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    parsed = undefined;
  }
  return { name, arguments: parsed, rawArguments: raw };
}
