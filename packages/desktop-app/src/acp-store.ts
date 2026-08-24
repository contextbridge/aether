import type {
  AvailableCommand,
  SessionConfigOption,
} from "@agentclientprotocol/sdk";
import type { ThreadMessageLike } from "@assistant-ui/react";
import { createStore } from "zustand/vanilla";
import type {
  AppEvent as GeneratedAppEvent,
  SessionInfo,
} from "./generated/bindings";
import { createGitReviewState, type GitReviewState } from "./git-review-state";

export type AppEvent = GeneratedAppEvent;

export type WorkspaceFile = {
  path: string;
  displayName: string;
};

export type ConnectionState =
  | { status: "disconnected" }
  | { status: "connecting" }
  | ({ status: "connected" } & Omit<SessionInfo, "configOptions">)
  | { status: "failed"; error: string };

export type ConnectedSession = Extract<
  ConnectionState,
  { status: "connected" }
>;

export type ChatSession = {
  connection: ConnectedSession;
  cwd: string;
  configOptions: SessionConfigOption[];
  availableCommands: AvailableCommand[];
  workspaceFiles: WorkspaceFile[];
  messages: ThreadMessageLike[];
  isRunning: boolean;
  error: string | null;
  openMessageId: string | null;
  openMessageSeeded: boolean;
  title: string;
  lastMessageAt?: Date;
  gitReview: GitReviewState;
};

export type ChatState = {
  connection: ConnectionState;
  cwd: string;
  configOptions: SessionConfigOption[];
  availableCommands: AvailableCommand[];
  workspaceFiles: WorkspaceFile[];
  messages: ThreadMessageLike[];
  isRunning: boolean;
  error: string | null;
  openMessageId: string | null;
  openMessageSeeded: boolean;
  activeSessionId: string | null;
  sessions: Record<string, ChatSession>;
  gitReview: GitReviewState;
};

export const initialChatState: ChatState = {
  connection: { status: "disconnected" },
  cwd: ".",
  configOptions: [],
  availableCommands: [],
  workspaceFiles: [],
  messages: [],
  isRunning: false,
  error: null,
  openMessageId: null,
  openMessageSeeded: false,
  activeSessionId: null,
  sessions: {},
  gitReview: createGitReviewState(),
};

export const chatSessionFromState = (
  state: ChatState,
  connection: ConnectedSession,
): ChatSession => ({
  connection,
  cwd: state.cwd,
  configOptions: state.configOptions,
  availableCommands: state.availableCommands,
  workspaceFiles: state.workspaceFiles,
  messages: state.messages,
  isRunning: state.isRunning,
  error: state.error,
  openMessageId: state.openMessageId,
  openMessageSeeded: state.openMessageSeeded,
  title: `${connection.agentName} · ${state.cwd}`,
  gitReview: state.gitReview,
});

export const chatStateFromSession = (
  session: ChatSession,
): Omit<ChatState, "sessions" | "activeSessionId"> => ({
  connection: session.connection,
  cwd: session.cwd,
  configOptions: session.configOptions,
  availableCommands: session.availableCommands,
  workspaceFiles: session.workspaceFiles,
  messages: session.messages,
  isRunning: session.isRunning,
  error: session.error,
  openMessageId: session.openMessageId,
  openMessageSeeded: session.openMessageSeeded,
  gitReview: session.gitReview,
});

export const createChatStore = () =>
  createStore<ChatState>()(() => ({ ...initialChatState }));

export type ChatStore = ReturnType<typeof createChatStore>;
