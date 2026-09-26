import init, {
  AetherClient,
  type AetherClientError,
  type AetherClientEvent,
  type Conversation,
  type WebSocketClose,
} from "@aether-agent/browser";
import type {
  ContentBlock,
  CreateElicitationRequest,
  CreateElicitationResponse,
  InitializeResponse,
} from "@agentclientprotocol/sdk/experimental/v2";
import { create } from "zustand";

export type ConnectionStatus =
  | { kind: "disconnected"; close?: WebSocketClose | null }
  | { kind: "connecting" }
  | { kind: "connected" };

/** `aether server`'s working directory and live session, advertised on the initialize response. */
export interface RemoteServerInfo {
  cwd: string;
  sessionId: string | null;
}

export interface PendingElicitation {
  id: number;
  request: CreateElicitationRequest;
  respond: (response: CreateElicitationResponse) => void;
}

export interface LoggedEvent {
  seq: number;
  at: Date;
  event: AetherClientEvent;
}

export interface AetherState {
  status: ConnectionStatus;
  client: AetherClient | null;
  agentName: string | null;
  remote: RemoteServerInfo | null;
  sessionId: string | null;
  conversations: Record<string, Conversation>;
  elicitations: PendingElicitation[];
  events: LoggedEvent[];
  /** The last failed action, shown until the next one starts. */
  error: string | null;
}

export const useAether = create<AetherState>(() => ({
  status: { kind: "disconnected" },
  client: null,
  agentName: null,
  remote: null,
  sessionId: null,
  conversations: {},
  elicitations: [],
  events: [],
  error: null,
}));

export function useConversation(): Conversation | null {
  return useAether((state) =>
    state.sessionId ? (state.conversations[state.sessionId] ?? null) : null,
  );
}

/**
 * Connect to `aether server` and attach to its live session, replaying its history, or start a
 * new session in its working directory.
 */
export async function connect(url: string): Promise<void> {
  if (useAether.getState().status.kind !== "disconnected") return;
  useAether.setState({
    status: { kind: "connecting" },
    agentName: null,
    remote: null,
    sessionId: null,
    conversations: {},
    elicitations: [],
    events: [],
    error: null,
  });
  let client: AetherClient;
  try {
    await wasmReady();
    client = await AetherClient.connect(url, record);
  } catch (error) {
    useAether.setState({
      status: { kind: "disconnected", close: closeOf(error) },
      error: describe(error),
    });
    return;
  }
  const remote = remoteServerInfo(client.initializeResponse);
  useAether.setState({
    status: { kind: "connected" },
    client,
    agentName: client.initializeResponse.info.name,
    remote,
  });
  if (!remote) {
    useAether.setState({
      error:
        "The agent did not advertise a working directory, so no session was opened.",
    });
    return;
  }
  await attempt(async () => {
    if (remote.sessionId) {
      useAether.setState({ sessionId: remote.sessionId });
      await client.resumeSession(
        { sessionId: remote.sessionId, cwd: remote.cwd },
        true,
      );
    } else {
      await openNewSession(client, remote.cwd);
    }
  });
}

/** Close the connection. The session keeps running on the server. */
export async function disconnect(): Promise<void> {
  await useAether.getState().client?.disconnect();
}

export async function newSession(): Promise<void> {
  const { client, remote } = useAether.getState();
  if (!client || !remote) return;
  await attempt(() => openNewSession(client, remote.cwd));
}

/** Send a prompt. It resolves once the agent accepts it; the turn streams in as conversation changes. */
export async function sendPrompt(prompt: ContentBlock[]): Promise<void> {
  const { client, sessionId } = useAether.getState();
  if (!client || !sessionId) return;
  await attempt(() => client.prompt({ sessionId, prompt }));
}

export async function cancelTurn(): Promise<void> {
  const { client, sessionId } = useAether.getState();
  if (!client || !sessionId) return;
  await attempt(async () => client.cancel(sessionId));
}

export function answerElicitation(
  id: number,
  response: CreateElicitationResponse,
): void {
  const elicitation = useAether
    .getState()
    .elicitations.find((pending) => pending.id === id);
  if (!elicitation) return;
  useAether.setState((state) => ({
    elicitations: state.elicitations.filter((pending) => pending.id !== id),
  }));
  try {
    elicitation.respond(response);
  } catch (error) {
    useAether.setState({ error: describe(error) });
  }
}

const MAX_LOGGED_EVENTS = 500;

let wasm: Promise<unknown> | undefined;
let nextSeq = 0;

function wasmReady(): Promise<unknown> {
  wasm ??= init();
  return wasm;
}

function record(event: AetherClientEvent): void {
  const seq = nextSeq++;
  useAether.setState((state) => {
    const events = [
      ...state.events.slice(1 - MAX_LOGGED_EVENTS),
      { seq, at: new Date(), event },
    ];
    switch (event.type) {
      case "conversation_changed":
        return {
          events,
          conversations: {
            ...state.conversations,
            [event.sessionId]: event.conversation,
          },
        };
      case "elicitation_request":
        return {
          events,
          elicitations: [
            ...state.elicitations,
            { id: seq, request: event.request, respond: event.respond },
          ],
        };
      case "connection_closed":
        return {
          events,
          status: { kind: "disconnected", close: event.close },
          client: null,
          elicitations: [],
        };
      default:
        return { events };
    }
  });
}

async function openNewSession(
  client: AetherClient,
  cwd: string,
): Promise<void> {
  const { sessionId } = await client.newSession({ cwd });
  const conversation = client.conversation(sessionId);
  useAether.setState((state) => ({
    sessionId,
    conversations: conversation
      ? { ...state.conversations, [sessionId]: conversation }
      : state.conversations,
  }));
}

async function attempt(action: () => Promise<unknown>): Promise<void> {
  useAether.setState({ error: null });
  try {
    await action();
  } catch (error) {
    useAether.setState({ error: describe(error) });
  }
}

function remoteServerInfo(
  response: InitializeResponse,
): RemoteServerInfo | null {
  const aether = response._meta?.["contextbridge/aether"] as
    { remote?: RemoteServerInfo } | undefined;
  return aether?.remote ?? null;
}

function closeOf(error: unknown): WebSocketClose | null {
  return isClientError(error) ? (error.close ?? null) : null;
}

function describe(error: unknown): string {
  if (isClientError(error)) return `${error.code}: ${error.message}`;
  return error instanceof Error ? error.message : String(error);
}

function isClientError(error: unknown): error is AetherClientError {
  return error instanceof Error && "code" in error;
}

// Editing this module re-runs it; release the server's single client slot the old instance holds.
import.meta.hot?.dispose(() => {
  void disconnect();
});
