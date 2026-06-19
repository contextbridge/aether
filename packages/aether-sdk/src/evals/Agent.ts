import type { AgentMessage } from "../generated/eval-types.js";
import type { Workspace } from "./Workspace.js";

export interface Agent {
  run(config: AgentConfig, options?: AgentRunOptions): Promise<AgentRunResult>;
}

export interface AgentConfig {
  workspace: Workspace;
  taskPrompt: string;
}

export interface AgentRunOptions {
  env?: Record<string, string | undefined>;
  onMessage?: (message: AgentMessage) => void;
  onStderr?: (chunk: string) => void;
  signal?: AbortSignal;
}

export interface AgentRunResult {
  transcript: AgentMessage[];
  stderr: string;
}
