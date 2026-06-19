import { posix } from "node:path";
import { env as processEnv } from "node:process";
import { AetherSdkError, throwIfAborted } from "../errors.js";
import { DockerImage } from "./DockerImage.js";
import { DockerContainer, type BindMount } from "./DockerContainer.js";
import type {
  Agent,
  AgentConfig,
  AgentRunOptions,
  AgentRunResult,
} from "./Agent.js";

export type { BindMount } from "./DockerContainer.js";

export const CONTAINER_AETHER_HOME = "/root/.aether";
export const AETHER_EVAL_WRAPPED_TASK_PROMPT_ENV =
  "AETHER_EVAL_WRAPPED_TASK_PROMPT";
export const AETHER_EVAL_WORKSPACE_ROOT_ENV = "AETHER_EVAL_WORKSPACE_ROOT";
export const AETHER_EVAL_CWD_ENV = "AETHER_EVAL_CWD";
const REQUIRED_PROVIDER_ENV_VARS = new Set([
  "ANTHROPIC_API_KEY",
  "DEEPSEEK_API_KEY",
  "GEMINI_API_KEY",
  "MOONSHOT_API_KEY",
  "OPENAI_API_KEY",
  "OPENROUTER_API_KEY",
  "ZAI_API_KEY",
]);

export interface DockerAgentOptions {
  image: DockerImage;
  command: string[];
  env?: Record<string, string>;
  mounts?: BindMount[];
  ephemeralMounts?: string[];
}

export class DockerAgent implements Agent {
  readonly image: DockerImage;
  readonly command: string[];
  readonly env: Record<string, string>;
  readonly mounts: BindMount[];
  readonly ephemeralMounts: string[];

  constructor(options: DockerAgentOptions) {
    if (options.command.length === 0) {
      throw new AetherSdkError(
        "invalid_options",
        "DockerAgent command must not be empty",
      );
    }
    this.image = options.image;
    this.command = [...options.command];
    this.env = { ...(options.env ?? {}) };
    this.mounts = [...(options.mounts ?? [])];
    this.ephemeralMounts = [
      ...(options.ephemeralMounts ?? [CONTAINER_AETHER_HOME]),
    ];
  }

  async run(
    config: AgentConfig,
    options: AgentRunOptions = {},
  ): Promise<AgentRunResult> {
    throwIfAborted(options.signal);

    const containerCwd = config.workspace.relativeCwd
      ? posix.join("/workspace", config.workspace.relativeCwd)
      : "/workspace";

    await using container = await DockerContainer.create({
      image: this.image,
      env: {
        ...defaultEvalEnvVars(options.env),
        ...this.env,
        [AETHER_EVAL_WORKSPACE_ROOT_ENV]: "/workspace",
        [AETHER_EVAL_CWD_ENV]: containerCwd,
        [AETHER_EVAL_WRAPPED_TASK_PROMPT_ENV]: buildTaskPrompt(
          config.taskPrompt,
          containerCwd,
        ),
      },
      bindMounts: [
        {
          source: config.workspace.rootPath,
          target: "/workspace",
          mode: "rw",
        },
        ...this.mounts,
      ],
      ephemeralMounts: this.ephemeralMounts,
    });

    return await container.run({
      command: this.command,
      cwd: containerCwd,
      signal: options.signal,
      onMessage: options.onMessage,
      onStderr: options.onStderr,
    });
  }
}

function buildTaskPrompt(taskPrompt: string, containerCwd: string): string {
  return [
    "Complete the following task:",
    `<task>${taskPrompt}</task>`,
    `CRITICAL INSTRUCTIONS: when working on this task, you MUST only operate within this directory: ${containerCwd}`,
  ].join("\n");
}

function defaultEvalEnvVars(
  hostEnv: Record<string, string | undefined> = processEnv,
): Record<string, string> {
  const env: Record<string, string> = {};
  for (const [key, value] of Object.entries(hostEnv)) {
    if (value === undefined) continue;
    if (key === "AETHER_HOME") continue;
    if (
      REQUIRED_PROVIDER_ENV_VARS.has(key) ||
      key === "OLLAMA_HOST" ||
      key.startsWith("AETHER_")
    ) {
      env[key] = value;
    }
  }
  env.AETHER_HOME = CONTAINER_AETHER_HOME;
  return env;
}
