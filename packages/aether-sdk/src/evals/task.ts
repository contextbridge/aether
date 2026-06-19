import type { AgentMessage } from "../generated/eval-types.js";
import type { Agent, AgentRunOptions, AgentRunResult } from "./Agent.js";
import { type EvalToolCall, extractToolCalls } from "./transcript.js";
import { Workspace } from "./Workspace.js";

export interface TaskRun extends AsyncDisposable {
  readonly prompt: string;
  readonly passed: boolean;
  readonly toolCalls: EvalToolCall[];
  readonly transcript: AgentMessage[];
  readonly workspace: Workspace;
}

export class Task {
  readonly prompt: string;
  readonly workspace: Workspace;

  constructor(prompt: string, workspace: Workspace) {
    this.prompt = prompt;
    this.workspace = workspace;
  }

  async run(agent: Agent, options: AgentRunOptions = {}): Promise<TaskRun> {
    let result: AgentRunResult;
    try {
      result = await agent.run(
        { workspace: this.workspace, taskPrompt: this.prompt },
        options,
      );
    } catch (err) {
      await this.workspace.cleanup();
      throw err;
    }

    const workspace = this.workspace;
    return {
      prompt: this.prompt,
      passed: result.transcript.at(-1)?.type === "done",
      toolCalls: extractToolCalls(result.transcript),
      transcript: result.transcript,
      workspace,
      [Symbol.asyncDispose]: () => workspace.cleanup(),
    };
  }
}
