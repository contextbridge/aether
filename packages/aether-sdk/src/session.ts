import type { AetherAcpOptions } from "./generated/aether-acp-options.js";
import { addAbortListener } from "node:events";
import path from "node:path";
import * as acp from "@agentclientprotocol/sdk";
import { AsyncQueue } from "./asyncQueue.js";
import {
  startAgent,
  type AcpAgentProcess,
  type SettingsSelection,
} from "./agentProcess.js";
import { AetherSdkError, throwIfAborted } from "./errors.js";
import type {
  AetherElicitationRequest,
  AetherElicitationResponse,
  AetherMessage,
  AgentSelection,
} from "./types.js";

const SDK_VERSION = "0.3.6";

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
  onElicitation?: (
    request: AetherElicitationRequest,
  ) => Promise<AetherElicitationResponse>;
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
  private promptInProgress = false;
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

    const connection = new acp.ClientSideConnection(
      () => createAcpClient({ onPermissionRequest, onElicitation }, events),
      agentProcess.stream,
    );

    const initializeResponse = await connection.initialize({
      protocolVersion: acp.PROTOCOL_VERSION,
      clientInfo: { name: "@aether-agent/sdk", version: SDK_VERSION },
      clientCapabilities: {
        fs: { readTextFile: false, writeTextFile: false },
        terminal: false,
      },
    });

    throwIfAborted(abortSignal);

    const newSessionResponse = await connection.newSession({
      cwd: path.resolve(cwd),
      mcpServers: [],
    });

    const session = new AetherSession(
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
    private readonly connection: acp.ClientSideConnection,
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
      await this.connection.cancel({ sessionId: this.sessionId });
  }

  async close(): Promise<void> {
    if (this.closed) return;
    this.closed = true;
    this.abortCleanup?.[Symbol.dispose]();
    this.abortCleanup = null;
    this.events.close();
    await this.agentProcess.close();
  }

  async [Symbol.asyncDispose](): Promise<void> {
    await this.close();
  }

  private async *streamPrompt(
    prompt: acp.ContentBlock[],
  ): AsyncGenerator<AetherMessage> {
    if (this.closed)
      throw new AetherSdkError(
        "session_not_started",
        "AetherSession is closed",
      );
    if (this.promptInProgress) {
      throw new AetherSdkError(
        "prompt_in_progress",
        "AetherSession already has a prompt in progress",
      );
    }

    this.promptInProgress = true;
    let completed = false;
    const promptPromise = this.connection
      .prompt({ sessionId: this.sessionId, prompt })
      .then((response) => {
        completed = true;
        this.events.push({
          type: "result",
          sessionId: this.sessionId,
          stopReason: response.stopReason,
        });
        return response;
      })
      .catch((err: unknown) => {
        this.events.fail(err);
        throw err;
      });

    let yieldedResult = false;
    try {
      for await (const event of this.events) {
        if (event.type === "result" && event.sessionId === this.sessionId) {
          yieldedResult = true;
          yield event;
          break;
        }
        yield event;
      }
      await promptPromise;
    } finally {
      if (!yieldedResult && !this.closed) {
        if (!completed) await this.cancel().catch(() => undefined);
        await promptPromise.catch(() => undefined);
        try {
          for await (const event of this.events) {
            if (event.type === "result" || event.type === "error") break;
          }
        } catch {
          // Queue may be in errored state if the prompt rejected.
        }
      }
      this.promptInProgress = false;
    }
  }
}

function createAcpClient(
  {
    onPermissionRequest,
    onElicitation,
  }: {
    onPermissionRequest: PermissionRequestHandler;
    onElicitation: AetherSessionOptions["onElicitation"];
  },
  events: AsyncQueue<AetherMessage>,
): acp.Client {
  return {
    async sessionUpdate(notification: acp.SessionNotification): Promise<void> {
      events.push({
        type: "session_update",
        sessionId: notification.sessionId,
        update: notification.update,
        raw: notification,
      });
    },

    async requestPermission(
      request: acp.RequestPermissionRequest,
    ): Promise<acp.RequestPermissionResponse> {
      return onPermissionRequest(request);
    },

    async extMethod(
      method: string,
      params: Record<string, unknown>,
    ): Promise<Record<string, unknown>> {
      if (method === "_aether/elicitation" && onElicitation) {
        return (await onElicitation({
          method: "_aether/elicitation",
          params,
        })) as unknown as Record<string, unknown>;
      }
      if (method === "_aether/elicitation") return { action: "cancel" };
      throw new acp.RequestError(-32601, `Unknown extMethod: ${method}`);
    },

    async extNotification(
      method: string,
      params: Record<string, unknown>,
    ): Promise<void> {
      events.push({ type: "ext_notification", method, params });
    },
  } satisfies acp.Client;
}

function normalizePrompt(
  prompt: string | acp.ContentBlock[],
): acp.ContentBlock[] {
  return typeof prompt === "string" ? [{ type: "text", text: prompt }] : prompt;
}
