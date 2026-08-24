import { Channel } from "@tauri-apps/api/core";
import type { SessionUpdate } from "@agentclientprotocol/sdk";
import {
  applySessionUpdate,
  extractMentionedFilePaths,
  finalizeOpenMessage,
} from "./acp-message-reducer";
import type { AppEvent, ChatStore } from "./acp-store";
import { commands } from "./generated/bindings";

export class AppActions {
  private messageNumber = 0;

  constructor(private readonly store: ChatStore) {}

  readonly start = async (cwd: string): Promise<void> => {
    this.store.setState({
      connection: { status: "connecting" },
      configOptions: [],
      availableCommands: [],
      workspaceFiles: [],
      messages: [],
      openMessageId: null,
      openMessageSeeded: false,
      error: null,
    });
    const events = new Channel<AppEvent>(this.handleEvent);

    try {
      const info = await commands.startSession(
        { program: "aether", args: ["acp"], cwd },
        events,
      );
      const { configOptions, ...connectionInfo } = info;
      const workspaceFiles =
        (await commands.indexWorkspaceFiles(cwd).catch(() => [])) ?? [];
      this.store.setState({
        connection: { status: "connected", ...connectionInfo },
        configOptions,
        workspaceFiles,
        error: null,
      });
    } catch (error) {
      const message = errorMessage(error);
      this.store.setState({
        connection: { status: "failed", error: message },
        error: message,
      });
    }
  };

  readonly send = async (text: string): Promise<void> => {
    const trimmed = text.trim();
    const connection = this.store.getState().connection;
    if (!trimmed || connection.status !== "connected") return;

    const filePaths = extractMentionedFilePaths(trimmed);
    this.store.setState((state) => ({
      messages: [
        ...state.messages,
        { id: this.nextMessageId("user"), role: "user", content: trimmed },
      ],
      isRunning: true,
      error: null,
    }));

    try {
      await commands.sendPrompt(
        connection.sessionId,
        trimmed,
        filePaths.length > 0 ? filePaths : null,
      );
    } catch (error) {
      this.store.setState({ isRunning: false, error: errorMessage(error) });
    }
  };

  readonly cancel = async (): Promise<void> => {
    const connection = this.store.getState().connection;
    if (connection.status !== "connected") return;
    try {
      await commands.cancelPrompt(connection.sessionId);
    } catch (error) {
      this.store.setState({ error: errorMessage(error) });
    }
  };

  readonly setConfigOption = async (
    configId: string,
    value: string,
  ): Promise<void> => {
    const connection = this.store.getState().connection;
    if (connection.status !== "connected") return;

    try {
      await commands.setSessionConfigOption(
        connection.sessionId,
        configId,
        value,
      );
    } catch (error) {
      this.store.setState({ error: errorMessage(error) });
    }
  };

  readonly close = async (): Promise<void> => {
    const connection = this.store.getState().connection;
    try {
      if (connection.status === "connected") {
        await commands.closeSession(connection.sessionId);
      }
    } finally {
      this.store.setState({
        connection: { status: "disconnected" },
        configOptions: [],
        availableCommands: [],
        workspaceFiles: [],
        openMessageId: null,
        openMessageSeeded: false,
        isRunning: false,
      });
    }
  };

  readonly handleEvent = (event: AppEvent): void => {
    const state = this.store.getState();
    if (state.connection.status === "connecting") {
      if (
        event.kind === "sessionUpdate" &&
        event.update.sessionUpdate === "available_commands_update"
      ) {
        this.store.setState({
          availableCommands: event.update.availableCommands,
        });
      }
      return;
    }
    if (
      state.connection.status !== "connected" ||
      state.connection.connectionId !== event.connectionId
    ) {
      return;
    }

    switch (event.kind) {
      case "sessionUpdate":
        this.handleSessionUpdate(event.update);
        break;
      case "promptDone":
        this.store.setState((current) => ({
          messages: finalizeOpenMessage(
            current.messages,
            current.openMessageId,
          ),
          openMessageId: null,
          openMessageSeeded: false,
          isRunning: false,
        }));
        break;
      case "promptError":
        this.store.setState((current) => ({
          messages: finalizeOpenMessage(
            current.messages,
            current.openMessageId,
            {
              type: "incomplete",
              reason: "error",
              error: event.error,
            },
          ),
          openMessageId: null,
          openMessageSeeded: false,
          isRunning: false,
          error: event.error,
        }));
        break;
      case "connectionClosed":
        this.store.setState({
          connection: { status: "disconnected" },
          configOptions: [],
          availableCommands: [],
          workspaceFiles: [],
          openMessageId: null,
          openMessageSeeded: false,
          isRunning: false,
          error: event.error,
        });
        break;
    }
  };

  private handleSessionUpdate(update: SessionUpdate): void {
    if (update.sessionUpdate === "config_option_update") {
      this.store.setState({ configOptions: update.configOptions });
      return;
    }
    if (update.sessionUpdate === "available_commands_update") {
      this.store.setState({ availableCommands: update.availableCommands });
      return;
    }

    const state = this.store.getState();
    const result = applySessionUpdate(
      state.messages,
      {
        openMessageId: state.openMessageId,
        seeded: state.openMessageSeeded,
      },
      update,
      () => this.nextMessageId("assistant"),
    );
    this.store.setState({
      messages: result.messages,
      openMessageId: result.cursor.openMessageId,
      openMessageSeeded: result.cursor.seeded,
    });
  }

  private nextMessageId(prefix: string): string {
    return `${prefix}-${++this.messageNumber}`;
  }
}

const errorMessage = (error: unknown): string =>
  error instanceof Error ? error.message : String(error);
