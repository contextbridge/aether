import type { AgentEvent } from "@aether-agent/sdk";
import { Container } from "./containers/container.js";
import { AetherEvalError } from "./errors.js";
import type { Agent } from "./Agent.js";
import type { Task } from "./task.js";
import { isTerminalEvent } from "./transcript.js";

export const AETHER_EVAL_TASK_PROMPT_ENV = "AETHER_EVAL_TASK_PROMPT";
export const AETHER_EVAL_WORKSPACE_ROOT_ENV = "AETHER_EVAL_WORKSPACE_ROOT";
export const AETHER_EVAL_CWD_ENV = "AETHER_EVAL_CWD";

export interface DockerAgentOptions {
  container: Container;
  command: string[];
  env?: Record<string, string>;
}

export class DockerAgent implements Agent {
  readonly container: Container;
  readonly command: string[];
  readonly env: Record<string, string>;

  constructor(options: DockerAgentOptions) {
    if (options.command.length === 0) {
      throw new AetherEvalError(
        "configuration_error",
        "DockerAgent command must not be empty",
      );
    }
    this.container = options.container;
    this.command = [...options.command];
    this.env = { ...options.env };
  }

  withEnvVar(key: string, value: string): DockerAgent {
    return new DockerAgent({
      container: this.container,
      command: this.command,
      env: { ...this.env, [key]: value },
    });
  }

  withEnvVars(env: Record<string, string>): DockerAgent {
    return new DockerAgent({
      container: this.container,
      command: this.command,
      env: { ...this.env, ...env },
    });
  }

  async *run(task: Task): AsyncIterable<AgentEvent> {
    const env = {
      ...this.env,
      [AETHER_EVAL_WORKSPACE_ROOT_ENV]: this.container.workspaceRoot,
      [AETHER_EVAL_CWD_ENV]: this.container.cwd,
      [AETHER_EVAL_TASK_PROMPT_ENV]: task.prompt,
    };

    let stderr = "";
    let finished = false;
    for await (const line of this.container.execStreaming({
      command: this.command,
      cwd: this.container.cwd,
      env,
      onStderr: (chunk) => {
        stderr += chunk;
      },
    })) {
      if (finished) continue;
      const trimmed = line.trim();
      if (!trimmed) continue;
      const message = parseAgentEvent(trimmed);
      finished = isTerminalEvent(message);
      yield message;
    }

    if (!finished) {
      throw new AetherEvalError(
        "command_exit_without_terminal",
        `agent command exited without emitting a terminal AgentEvent.\nstderr:\n${stderr}`,
      );
    }
  }
}

function parseAgentEvent(line: string): AgentEvent {
  try {
    return JSON.parse(line) as AgentEvent;
  } catch (err) {
    throw new AetherEvalError(
      "agent_event_json_line",
      `agent emitted an invalid AgentEvent JSON line: ${line}`,
      err,
    );
  }
}
