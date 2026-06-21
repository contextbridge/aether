import type { AgentMessage } from "../generated/eval-types.js";
import type { Task } from "./task.js";

export interface Agent {
  run(task: Task): AsyncIterable<AgentMessage>;
}
