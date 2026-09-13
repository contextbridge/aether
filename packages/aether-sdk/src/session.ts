import type { AetherAcpOptions } from "./generated/aether-acp-options.js";
import type { SessionUsageEvent } from "./generated/eval-types.js";
import { addAbortListener } from "node:events";
import path from "node:path";
import * as acp from "@agentclientprotocol/sdk/experimental/v2";
import { AsyncQueue } from "./asyncQueue.js";
import {
  startAgent,
  type AcpAgentProcess,
  type SettingsSelection,
} from "./agentProcess.js";
import { AetherSdkError, throwIfAborted } from "./errors.js";
import { SDK_VERSION } from "./generated/sdk-version.js";
import type { AetherMessage, AgentSelection } from "./types.js";

export type PermissionRequestHandler = (
  request: acp.RequestPermissionRequest,
) => Promise<acp.RequestPermissionResponse>;

export type CommonAetherSessionOptions = Pick<
  AetherAcpOptions,
  "logDir" | "providers" | "traceContext"
> & {
  cwd?: string;
  binaryPath?: string;
  env?: Record<string, string | undefined>;
  abortSignal?: AbortSignal;
  /** Defaults to {@link autoApprovePermissions}. */
  onPermissionRequest?: PermissionRequestHandler;
  /**
   * Handles native ACP `elicitation/create` requests from the agent. When
   * omitted, the SDK responds with `{ action: "cancel" }` and does not
   * advertise the `elicitation` client capability.
   */
  onElicitation?: (
    request: acp.CreateElicitationRequest,
  ) => Promise<acp.CreateElicitationResponse>;
};

export type AetherSessionOptions = CommonAetherSessionOptions &
  SettingsSelection &
  AgentSelection;

/**
 * Permission handler that selects the first `allow_*` option, or cancels if
 * none exist. This is the default when `onPermissionRequest` is not supplied —
 * suitable for trusted/dev contexts. For untrusted agents or production hosts,
 * pass your own handler that prompts the user or applies a policy.
 */
export const autoApprovePermissions: PermissionRequestHandler = async (
  request,
) => {
  const allowOption = request.options.find((o) => o.kind.startsWith("allow_"));
  if (allowOption)
    return {
      outcome: { outcome: "selected", optionId: allowOption.optionId },
    };
  return { outcome: { outcome: "cancelled" } };
};

export class AetherSession {
  readonly sessionId: string;
  readonly initializeResponse: acp.InitializeResponse;
  readonly newSessionResponse: acp.NewSessionResponse;

  private closed = false;
  private turn: Turn | null = null;
  private abortCleanup: Disposable | null = null;

  static async start(
    options: AetherSessionOptions = {},
  ): Promise<AetherSession> {
    const {
      abortSignal,
      cwd = process.cwd(),
      onPermissionRequest = autoApprovePermissions,
      onElicitation,
      ...agentOptions
    } = options;

    throwIfAborted(abortSignal);

    const events = new AsyncQueue<AetherMessage>();
    await using stack = new AsyncDisposableStack();

    throwIfAborted(abortSignal);

    const agentProcess = startAgent({ ...agentOptions, cwd, events });

    stack.defer(() => agentProcess.close());

    let session: AetherSession | undefined;
    const connection = createAcpClient(
      { onPermissionRequest, onElicitation },
      events,
      (notification) => session?.onSessionUpdate(notification),
    ).connect(agentProcess.stream);

    stack.defer(() => connection.close());

    const initializeResponse = await connection.agent.request("initialize", {
      protocolVersion: acp.PROTOCOL_VERSION,
      info: { name: "@aether-agent/sdk", version: SDK_VERSION },
      capabilities: {
        ...(onElicitation ? { elicitation: { form: {}, url: {} } } : {}),
      },
    });

    throwIfAborted(abortSignal);

    const newSessionResponse = await connection.agent.request("session/new", {
      cwd: path.resolve(cwd),
    });

    session = new AetherSession(
      agentProcess,
      connection,
      events,
      initializeResponse,
      newSessionResponse,
      abortSignal,
    );

    // Hand resources off to the session; close() now owns their cleanup.
    stack.move();
    return session;
  }

  private constructor(
    private readonly agentProcess: AcpAgentProcess,
    private readonly connection: acp.ClientConnection,
    private readonly events: AsyncQueue<AetherMessage>,
    initializeResponse: acp.InitializeResponse,
    newSessionResponse: acp.NewSessionResponse,
    abortSignal: AbortSignal | undefined,
  ) {
    this.initializeResponse = initializeResponse;
    this.newSessionResponse = newSessionResponse;
    this.sessionId = newSessionResponse.sessionId;
    void this.connection.closed.then(() => this.events.close());
    if (abortSignal) {
      this.abortCleanup = addAbortListener(abortSignal, () => {
        this.events.push({
          type: "error",
          error: new AetherSdkError("aborted", "Aborted by caller"),
        });
        void this.cancel()
          .catch(() => undefined)
          .finally(() => void this.close());
      });
    }
  }

  prompt(prompt: string | acp.ContentBlock[]): AsyncIterable<AetherMessage> {
    return this.streamPrompt(normalizePrompt(prompt));
  }

  async cancel(): Promise<void> {
    if (!this.closed)
      await this.connection.agent.notify("session/cancel", {
        sessionId: this.sessionId,
      });
  }

  async close(): Promise<void> {
    if (this.closed) return;
    this.closed = true;
    this.abortCleanup?.[Symbol.dispose]();
    this.abortCleanup = null;
    this.events.close();
    this.connection.close();
    await this.agentProcess.close();
  }

  async [Symbol.asyncDispose](): Promise<void> {
    await this.close();
  }

  private onSessionUpdate(notification: acp.UpdateSessionNotification): void {
    if (
      notification.sessionId !== this.sessionId ||
      !this.turn ||
      this.turn.completed
    )
      return;
    const update = notification.update;
    if (
      acp.SessionUpdate.isStateUpdate(update) &&
      acp.StateUpdate.isIdle(update)
    ) {
      this.turn.stopReason ??= update.stopReason ?? "end_turn";
      this.completeTurn();
    }
  }

  private completeTurn(): void {
    const turn = this.turn;
    if (!turn?.accepted || turn.stopReason === undefined || turn.completed)
      return;
    turn.completed = true;
    const stopReason = turn.stopReason;

    setImmediate(() => {
      if (this.turn !== turn || this.closed || this.connection.signal.aborted)
        return;
      this.events.push({
        type: "result",
        sessionId: this.sessionId,
        stopReason,
      });
    });
  }

  private async *streamPrompt(
    prompt: acp.ContentBlock[],
  ): AsyncGenerator<AetherMessage> {
    if (this.closed)
      throw new AetherSdkError(
        "session_not_started",
        "AetherSession is closed",
      );
    if (this.turn) {
      throw new AetherSdkError(
        "prompt_in_progress",
        "AetherSession already has a prompt in progress",
      );
    }

    const turn: Turn = { accepted: false, completed: false };
    this.turn = turn;
    let rejection: Extract<AetherMessage, { type: "error" }> | undefined;
    const promptPromise = this.connection.agent
      .request("session/prompt", { sessionId: this.sessionId, prompt })
      .then(
        () => {
          turn.accepted = true;
          this.completeTurn();
        },
        (error: unknown) => {
          rejection = { type: "error", error };
          this.events.push(rejection);
        },
      );

    let yieldedResult = false;
    try {
      for await (const event of this.events) {
        if (event === rejection) throw rejection.error;
        yieldedResult =
          event.type === "result" && event.sessionId === this.sessionId;
        yield event;
        if (yieldedResult) return;
      }
      await promptPromise;
      if (!this.closed) {
        throw new AetherSdkError(
          "process_exited",
          "ACP connection closed before the turn reached idle",
        );
      }
    } finally {
      if (!yieldedResult && !this.closed && !rejection) {
        if (!turn.completed) await this.cancel().catch(() => undefined);
        await promptPromise;
        try {
          for await (const event of this.events) {
            if (event.type === "result" || event.type === "error") break;
          }
        } catch {
          // Transport or malformed extension errors can fail the shared queue.
        }
      }
      this.turn = null;
    }
  }
}

type Turn = {
  accepted: boolean;
  stopReason?: acp.StopReason;
  completed: boolean;
};

function createAcpClient(
  {
    onPermissionRequest,
    onElicitation,
  }: {
    onPermissionRequest: PermissionRequestHandler;
    onElicitation: AetherSessionOptions["onElicitation"];
  },
  events: AsyncQueue<AetherMessage>,
  onSessionUpdate: (notification: acp.UpdateSessionNotification) => void,
): acp.ClientApp {
  return acp
    .client()
    .onNotification("session/update", ({ params: notification }) => {
      events.push({
        type: "session_update",
        sessionId: notification.sessionId,
        update: notification.update,
        raw: notification,
      });
      onSessionUpdate(notification);
    })
    .onRequest("session/request_permission", ({ params }) =>
      onPermissionRequest(params),
    )
    .onRequest("elicitation/create", ({ params }) =>
      onElicitation ? onElicitation(params) : { action: "cancel" },
    )
    .onNotification("elicitation/complete", ({ params: notification }) => {
      events.push({
        type: "elicitation_complete",
        elicitationId: notification.elicitationId,
      });
    })
    .onNotification(
      "_aether/session_usage",
      (params) => params,
      ({ params }) => {
        try {
          const usage = parseUsageNotification(params);
          if (usage) events.push({ type: "usage", usage });
        } catch (error) {
          events.fail(error);
        }
      },
    );
}

function parseUsageNotification(params: unknown): SessionUsageEvent {
  const usage =
    params && typeof params === "object" && "usage" in params
      ? params.usage
      : undefined;
  if (!usage || typeof usage !== "object") {
    throw new AetherSdkError(
      "invalid_protocol_message",
      "Aether CLI sent an invalid session usage update",
    );
  }

  return usage as SessionUsageEvent;
}

function normalizePrompt(
  prompt: string | acp.ContentBlock[],
): acp.ContentBlock[] {
  return typeof prompt === "string" ? [{ type: "text", text: prompt }] : prompt;
}
