export type AetherEvalErrorCode =
  | "command_exit_nonzero"
  | "command_exit_missing"
  | "configuration_error"
  | "command_exit_without_terminal"
  | "agent_event_json_line"
  | "invalid_image_reference";

export class AetherEvalError extends Error {
  readonly code: AetherEvalErrorCode;
  override readonly cause?: unknown;

  constructor(code: AetherEvalErrorCode, message: string, cause?: unknown) {
    super(message);
    this.name = "AetherEvalError";
    this.code = code;
    this.cause = cause;
  }
}

export class AetherCommandExitError extends AetherEvalError {
  constructor(
    readonly exitCode: number,
    readonly stderr: string,
  ) {
    super(
      "command_exit_nonzero",
      `agent command exited with code ${exitCode}.\nstderr:\n${stderr}`,
    );
  }
}
