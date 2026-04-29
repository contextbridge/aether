export type AetherSdkErrorCode =
  | "process_spawn_failed"
  | "process_exited"
  | "mcp_server_start_failed"
  | "mcp_server_invalid_config"
  | "session_not_started"
  | "prompt_in_progress"
  | "invalid_options"
  | "aborted";

export class AetherSdkError extends Error {
  readonly code: AetherSdkErrorCode;
  override readonly cause?: unknown;

  constructor(code: AetherSdkErrorCode, message: string, cause?: unknown) {
    super(message);
    this.name = "AetherSdkError";
    this.code = code;
    this.cause = cause;
  }
}
