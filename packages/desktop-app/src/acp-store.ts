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

export type ChatState = {
  connection: ConnectionState;
  configOptions: SessionConfigOption[];
  availableCommands: AvailableCommand[];
  workspaceFiles: WorkspaceFile[];
  messages: ThreadMessageLike[];
  isRunning: boolean;
  error: string | null;
  openMessageId: string | null;
  openMessageSeeded: boolean;
};

export const initialChatState: ChatState = {
  connection: { status: "disconnected" },
  configOptions: [],
  availableCommands: [],
  workspaceFiles: [],
  messages: [],
  isRunning: false,
  error: null,
  openMessageId: null,
  openMessageSeeded: false,
};

export const createChatStore = () =>
  createStore<ChatState>()(() => ({ ...initialChatState }));

export type ChatStore = ReturnType<typeof createChatStore>;
