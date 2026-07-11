import type { AgentEvent } from "@aether-agent/sdk";

export function eventName(event: AgentEvent): string {
  return `${event.category}:${event.event.type}`;
}

export function logMessage(message: AgentEvent): void {
  if (message.category === "message") {
    process.stderr.write(message.event.chunk);
    return;
  }

  if (message.category === "tool") {
    switch (message.event.type) {
      case "call":
        process.stderr.write(`\n[tool-call] ${message.event.request.name}\n`);
        return;
      case "result":
        process.stderr.write(`\n[tool-result] ${message.event.result.name}\n`);
        return;
      case "error":
        process.stderr.write(`\n[tool-error] ${message.event.error.name}\n`);
        return;
    }
  }

  process.stderr.write(`\n[${message.category}:${message.event.type}]\n`);
}
