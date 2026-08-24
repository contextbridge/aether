import { Channel } from "@tauri-apps/api/core";
import type { SessionUpdate } from "@agentclientprotocol/sdk";
import {
  applySessionUpdate,
  extractMentionedFilePaths,
  finalizeOpenMessage,
} from "./acp-message-reducer";
import {
  chatSessionFromState,
  chatStateFromSession,
  type AppEvent,
  type ChatSession,
  type ChatStore,
  type ConnectedSession,
} from "./acp-store";
import {
  createGitReviewState,
  formatReviewPrompt,
  type ReviewComment,
} from "./git-review-state";
import {
  commands,
  type DiffScope,
  type FileStatus,
} from "./generated/bindings";

export class AppActions {
  private messageNumber = 0;
  private gitRequestNumber = 0;

  constructor(private readonly store: ChatStore) {}

  readonly start = async (cwd: string): Promise<void> => {
    const previousState = this.store.getState();
    const hasActiveSession = previousState.connection.status === "connected";
    if (!hasActiveSession) {
      this.store.setState({
        connection: { status: "connecting" },
        cwd,
        configOptions: [],
        availableCommands: [],
        workspaceFiles: [],
        messages: [],
        openMessageId: null,
        openMessageSeeded: false,
        isRunning: false,
        error: null,
      });
    }
    const events = new Channel<AppEvent>(this.handleEvent);

    try {
      const info = await commands.startSession(
        { program: "aether", args: ["acp"], cwd },
        events,
      );
      const { configOptions, ...connectionInfo } = info;
      const connection: ConnectedSession = {
        status: "connected",
        ...connectionInfo,
      };
      const workspaceFiles =
        (await commands.indexWorkspaceFiles(cwd).catch(() => [])) ?? [];
      const availableCommands = this.store.getState().availableCommands;
      const session: ChatSession = {
        connection,
        cwd,
        configOptions,
        availableCommands,
        workspaceFiles,
        messages: [],
        isRunning: false,
        error: null,
        openMessageId: null,
        openMessageSeeded: false,
        title: `${connection.agentName} · ${cwd}`,
        gitReview: createGitReviewState(),
      };
      this.setActiveSession(session);
    } catch (error) {
      const message = errorMessage(error);
      const state = this.store.getState();
      const previousSession = state.activeSessionId
        ? state.sessions[state.activeSessionId]
        : undefined;
      if (previousSession) {
        this.setActiveSession({ ...previousSession, error: message });
      } else {
        this.store.setState({
          connection: { status: "failed", error: message },
          error: message,
        });
      }
    }
  };

  readonly send = async (text: string): Promise<void> => {
    const trimmed = text.trim();
    const state = this.store.getState();
    const connection = state.connection;
    if (!trimmed || connection.status !== "connected") return;

    const filePaths = extractMentionedFilePaths(trimmed);
    this.updateSession(connection.sessionId, (session) => ({
      messages: [
        ...session.messages,
        { id: this.nextMessageId("user"), role: "user", content: trimmed },
      ],
      isRunning: true,
      error: null,
      lastMessageAt: new Date(),
    }));

    try {
      await commands.sendPrompt(
        connection.sessionId,
        trimmed,
        filePaths.length > 0 ? filePaths : null,
      );
    } catch (error) {
      this.updateSession(connection.sessionId, {
        isRunning: false,
        error: errorMessage(error),
      });
    }
  };

  readonly cancel = async (): Promise<void> => {
    const connection = this.store.getState().connection;
    if (connection.status !== "connected") return;
    try {
      await commands.cancelPrompt(connection.sessionId);
    } catch (error) {
      this.updateSession(connection.sessionId, { error: errorMessage(error) });
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
      this.updateSession(connection.sessionId, { error: errorMessage(error) });
    }
  };

  readonly close = async (): Promise<void> => {
    const connection = this.store.getState().connection;
    if (connection.status !== "connected") return;
    await this.closeSession(connection.sessionId);
  };

  readonly closeSession = async (sessionId: string): Promise<void> => {
    try {
      await commands.closeSession(sessionId);
    } finally {
      this.removeSession(sessionId);
    }
  };

  readonly renameSession = (sessionId: string, title: string): void => {
    this.updateSession(sessionId, { title });
  };

  readonly closeAll = async (): Promise<void> => {
    const sessionIds = Object.keys(this.store.getState().sessions);
    await Promise.allSettled(
      sessionIds.map((sessionId) => commands.closeSession(sessionId)),
    );
    this.store.setState({
      ...this.emptyActiveState(),
      sessions: {},
      activeSessionId: null,
    });
  };

  readonly switchToThread = (sessionId: string): void => {
    const session = this.store.getState().sessions[sessionId];
    if (session) this.setActiveSession(session);
  };

  readonly openGitReview = async (): Promise<void> => {
    const connection = this.store.getState().connection;
    if (connection.status !== "connected") return;
    this.updateSession(connection.sessionId, (session) => ({
      gitReview: { ...session.gitReview, view: "gitReview" },
    }));
    await this.loadGitReview();
  };

  readonly closeGitReview = (): void => {
    const connection = this.store.getState().connection;
    if (connection.status !== "connected") return;
    this.updateSession(connection.sessionId, (session) => ({
      gitReview: {
        ...session.gitReview,
        view: "conversation",
        pendingMutation: null,
      },
    }));
  };

  readonly loadGitReview = async (scope?: DiffScope): Promise<void> => {
    const state = this.store.getState();
    const connection = state.connection;
    if (connection.status !== "connected") return;
    const nextScope = scope ?? state.gitReview.scope;
    const requestNumber = ++this.gitRequestNumber;
    this.updateSession(connection.sessionId, (session) => ({
      gitReview: {
        ...session.gitReview,
        scope: nextScope,
        status: "loading",
        error: null,
      },
    }));
    try {
      const snapshot = await commands.loadGitSnapshot(
        connection.sessionId,
        nextScope,
      );
      if (
        requestNumber !== this.gitRequestNumber ||
        this.store.getState().connection.status !== "connected"
      )
        return;
      this.updateSession(connection.sessionId, (session) => ({
        gitReview: {
          ...session.gitReview,
          scope: nextScope,
          status: "ready",
          snapshot,
          error: null,
        },
      }));
    } catch (error) {
      if (requestNumber === this.gitRequestNumber) {
        this.updateGitReviewError(connection.sessionId, error);
      }
    }
  };

  readonly stageGitPaths = async (
    paths: string[],
    staged: boolean,
  ): Promise<void> => {
    await this.runGitMutation((sessionId) =>
      staged
        ? commands.unstageGitPaths(sessionId, paths)
        : commands.stageGitPaths(sessionId, paths),
    );
  };

  readonly stageAllGitChanges = async (staged: boolean): Promise<void> => {
    await this.runGitMutation((sessionId) =>
      staged
        ? commands.unstageAllGitChanges(sessionId)
        : commands.stageAllGitChanges(sessionId),
    );
  };

  readonly commitGitChanges = async (message: string): Promise<void> => {
    await this.runGitMutation((sessionId) =>
      commands.commitGitChanges(sessionId, message),
    );
  };

  readonly discardGitPath = async (
    path: string,
    oldPath: string | null,
    status: FileStatus,
  ): Promise<void> => {
    await this.runGitMutation((sessionId) =>
      commands.discardGitPath(sessionId, path, oldPath, status),
    );
  };

  readonly addReviewComment = (comment: ReviewComment): void => {
    const connection = this.store.getState().connection;
    if (connection.status !== "connected") return;
    this.updateSession(connection.sessionId, (session) => ({
      gitReview: {
        ...session.gitReview,
        comments: [...session.gitReview.comments, comment],
      },
    }));
  };

  readonly removeReviewComment = (commentId: string): void => {
    const connection = this.store.getState().connection;
    if (connection.status !== "connected") return;
    this.updateSession(connection.sessionId, (session) => ({
      gitReview: {
        ...session.gitReview,
        comments: session.gitReview.comments.filter(
          (comment) => comment.id !== commentId,
        ),
      },
    }));
  };

  readonly clearReviewComments = (): void => {
    const connection = this.store.getState().connection;
    if (connection.status !== "connected") return;
    this.updateSession(connection.sessionId, (session) => ({
      gitReview: { ...session.gitReview, comments: [], pendingMutation: null },
    }));
  };

  readonly submitGitReview = async (): Promise<void> => {
    const comments = this.store.getState().gitReview.comments;
    if (comments.length === 0) return;
    this.closeGitReview();
    this.clearReviewComments();
    await this.send(formatReviewPrompt(comments));
  };

  readonly handleEvent = (event: AppEvent): void => {
    const state = this.store.getState();
    let session = Object.values(state.sessions).find(
      (candidate) => candidate.connection.connectionId === event.connectionId,
    );
    if (
      !session &&
      state.connection.status === "connected" &&
      state.connection.connectionId === event.connectionId
    ) {
      session = chatSessionFromState(state, state.connection);
      this.store.setState({
        sessions: {
          ...state.sessions,
          [session.connection.sessionId]: session,
        },
        activeSessionId: session.connection.sessionId,
      });
    }

    if (!session) {
      if (
        event.kind === "sessionUpdate" &&
        event.update.sessionUpdate === "available_commands_update" &&
        state.connection.status === "connecting"
      ) {
        this.store.setState({
          availableCommands: event.update.availableCommands,
        });
      }
      return;
    }

    switch (event.kind) {
      case "sessionUpdate":
        this.handleSessionUpdate(session, event.update);
        break;
      case "promptDone":
        this.updateSession(session.connection.sessionId, (current) => ({
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
        this.updateSession(session.connection.sessionId, (current) => ({
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
        this.removeSession(session.connection.sessionId, event.error);
        break;
    }
  };

  private handleSessionUpdate(
    session: ChatSession,
    update: SessionUpdate,
  ): void {
    if (update.sessionUpdate === "config_option_update") {
      this.updateSession(session.connection.sessionId, {
        configOptions: update.configOptions,
      });
      return;
    }
    if (update.sessionUpdate === "available_commands_update") {
      this.updateSession(session.connection.sessionId, {
        availableCommands: update.availableCommands,
      });
      return;
    }

    const result = applySessionUpdate(
      session.messages,
      {
        openMessageId: session.openMessageId,
        seeded: session.openMessageSeeded,
      },
      update,
      () => this.nextMessageId("assistant"),
    );
    this.updateSession(session.connection.sessionId, {
      messages: result.messages,
      openMessageId: result.cursor.openMessageId,
      openMessageSeeded: result.cursor.seeded,
      lastMessageAt: new Date(),
    });
  }

  private setActiveSession(session: ChatSession): void {
    const state = this.store.getState();
    this.store.setState({
      ...chatStateFromSession(session),
      sessions: { ...state.sessions, [session.connection.sessionId]: session },
      activeSessionId: session.connection.sessionId,
    });
  }

  private updateSession(
    sessionId: string,
    changes:
      Partial<ChatSession> | ((session: ChatSession) => Partial<ChatSession>),
  ): void {
    this.store.setState((state) => {
      const current = state.sessions[sessionId];
      if (!current) return state;
      const patch = typeof changes === "function" ? changes(current) : changes;
      const session = { ...current, ...patch };
      return state.activeSessionId === sessionId
        ? {
            ...state,
            ...chatStateFromSession(session),
            sessions: { ...state.sessions, [sessionId]: session },
          }
        : { sessions: { ...state.sessions, [sessionId]: session } };
    });
  }

  private removeSession(sessionId: string, error: string | null = null): void {
    const state = this.store.getState();
    const sessions = Object.fromEntries(
      Object.entries(state.sessions).filter(([id]) => id !== sessionId),
    );
    if (state.activeSessionId !== sessionId) {
      this.store.setState({ sessions });
      return;
    }

    const nextSession = Object.values(sessions).at(-1);
    this.store.setState({
      ...this.emptyActiveState(),
      ...(nextSession ? chatStateFromSession(nextSession) : { error }),
      sessions,
      activeSessionId: nextSession?.connection.sessionId ?? null,
    });
  }

  private emptyActiveState() {
    return {
      connection: { status: "disconnected" as const },
      configOptions: [],
      availableCommands: [],
      workspaceFiles: [],
      messages: [],
      isRunning: false,
      openMessageId: null,
      openMessageSeeded: false,
      error: null,
      gitReview: createGitReviewState(),
    };
  }

  private async runGitMutation(
    operation: (sessionId: string) => Promise<unknown>,
  ): Promise<void> {
    const connection = this.store.getState().connection;
    if (connection.status !== "connected") return;
    try {
      await operation(connection.sessionId);
      await this.loadGitReview();
    } catch (error) {
      this.updateGitReviewError(connection.sessionId, error);
    }
  }

  private updateGitReviewError(sessionId: string, error: unknown): void {
    this.updateSession(sessionId, (session) => ({
      gitReview: {
        ...session.gitReview,
        status: "error",
        error: errorMessage(error),
      },
    }));
  }

  private nextMessageId(prefix: string): string {
    return `${prefix}-${++this.messageNumber}`;
  }
}

const errorMessage = (error: unknown): string =>
  error instanceof Error ? error.message : String(error);
