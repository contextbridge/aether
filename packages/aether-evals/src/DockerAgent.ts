import { env as processEnv } from "node:process";
import type { AgentEvent } from "@aether-agent/sdk";
import { Container } from "./containers/container.js";
import { AetherEvalError } from "./errors.js";
import type { Agent } from "./Agent.js";
import type { Task } from "./task.js";
import { isTerminalEvent } from "./transcript.js";

export const CONTAINER_AETHER_HOME = "/root/.aether";
export const AETHER_EVAL_TASK_PROMPT_ENV = "AETHER_EVAL_TASK_PROMPT";
export const AETHER_EVAL_WORKSPACE_ROOT_ENV = "AETHER_EVAL_WORKSPACE_ROOT";
export const AETHER_EVAL_CWD_ENV = "AETHER_EVAL_CWD";
const PROVIDER_ENV_VARS = new Set([
  "ANTHROPIC_API_KEY",
  "DEEPSEEK_API_KEY",
  "GEMINI_API_KEY",
  "MOONSHOT_API_KEY",
  "OPENAI_API_KEY",
  "OPENROUTER_API_KEY",
  "ZAI_API_KEY",
]);

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
    this.env = {
      ...defaultEvalEnvVars(processEnv),
      ...(options.env ?? {}),
    };
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
      const trimmed = line.trim();
      if (!trimmed) continue;
      const message = parseAgentEvent(trimmed);
      finished = isTerminalEvent(message);
      yield message;
      if (finished) break;
    }

    if (!finished) {
      throw new AetherEvalError(
        "command_exit_without_terminal",
        `agent command exited without emitting a terminal AgentEvent.\nstderr:\n${stderr}`,
      );
    }
  }
}

export function defaultEvalEnvVars(
  hostEnv: Record<string, string | undefined> = processEnv,
): Record<string, string> {
  const env: Record<string, string> = {};
  for (const [key, value] of Object.entries(hostEnv)) {
    if (value === undefined) continue;
    if (key === "AETHER_HOME") continue;
    if (
      PROVIDER_ENV_VARS.has(key) ||
      key === "OLLAMA_HOST" ||
      key.startsWith("AETHER_")
    ) {
      env[key] = value;
    }
  }
  env.AETHER_HOME = CONTAINER_AETHER_HOME;
  return env;
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
