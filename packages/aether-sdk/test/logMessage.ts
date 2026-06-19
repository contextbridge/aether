import type { AgentMessage } from "../src/generated/eval-types.js";

export function logMessage(message: AgentMessage): void {
  switch (message.type) {
    case "text":
    case "thought":
      process.stderr.write(message.chunk);
      return;
    case "tool_call":
      process.stderr.write(`\n[tool-call] ${message.request.name}\n`);
      return;
    case "tool_result":
      process.stderr.write(`\n[tool-result] ${message.result.name}\n`);
      return;
    case "tool_error":
      process.stderr.write(`\n[tool-error] ${message.error.name}\n`);
      return;
    default:
      process.stderr.write(`\n[${message.type}]\n`);
  }
}
