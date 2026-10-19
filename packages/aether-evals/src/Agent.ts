import type { AgentEvent } from "@aether-agent/sdk";
import type { Task } from "./task.js";

export interface AgentRunOptions {
  signal?: AbortSignal;
}

export interface Agent {
  run(task: Task, options?: AgentRunOptions): AsyncIterable<AgentEvent>;
}
