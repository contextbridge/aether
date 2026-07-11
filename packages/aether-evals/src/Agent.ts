import type { AgentEvent } from "@aether-agent/sdk";
import type { Task } from "./task.js";

export interface Agent {
  run(task: Task): AsyncIterable<AgentEvent>;
}
