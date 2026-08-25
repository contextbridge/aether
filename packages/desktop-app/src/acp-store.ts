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

export type Workspace = {
  id: string;
  path: string;
  name: string;
  collapsed: boolean;
};

export type ThreadSummary = {
  id: string;
  cwd: string;
  title: string;
  updatedAt?: Date;
};

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
  workspaces: Record<string, Workspace>;
  selectedWorkspaceId: string | null;
  threads: Record<string, ThreadSummary>;
  threadsLoading: boolean;
  loadingThreadId: string | null;
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
  workspaces: {},
  selectedWorkspaceId: null,
  threads: {},
  threadsLoading: false,
  loadingThreadId: null,
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
): Omit<
  ChatState,
  | "sessions"
  | "activeSessionId"
  | "workspaces"
  | "selectedWorkspaceId"
  | "threads"
  | "threadsLoading"
  | "loadingThreadId"
> => ({
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
  createStore<ChatState>()(() => {
    const workspaces = readSavedWorkspaces();
    return {
      ...initialChatState,
      workspaces,
      selectedWorkspaceId: Object.keys(workspaces)[0] ?? null,
    };
  });

const WORKSPACES_KEY = "aether.desktop.workspaces.v1";

const readSavedWorkspaces = (): Record<string, Workspace> => {
  if (typeof localStorage === "undefined") return {};
  try {
    const value = JSON.parse(localStorage.getItem(WORKSPACES_KEY) ?? "{}");
    return value && typeof value === "object" ? value : {};
  } catch {
    return {};
  }
};

export const saveWorkspaces = (workspaces: Record<string, Workspace>): void => {
  if (typeof localStorage === "undefined") return;
  localStorage.setItem(WORKSPACES_KEY, JSON.stringify(workspaces));
};

export type ChatStore = ReturnType<typeof createChatStore>;
