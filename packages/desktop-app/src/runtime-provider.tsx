import {
  AssistantRuntimeProvider,
  useExternalStoreRuntime,
  type ExternalStoreThreadData,
  type ThreadMessageLike,
} from "@assistant-ui/react";
import { useShallow } from "zustand/react/shallow";
import { useMemo, type PropsWithChildren } from "react";
import { useAppActions, useChatStore } from "./app-provider";

export function RuntimeProvider({ children }: PropsWithChildren) {
  const { messages, isRunning, cwd, activeSessionId, sessions } = useChatStore(
    useShallow((state) => ({
      messages: state.messages,
      isRunning: state.isRunning,
      cwd: state.cwd,
      activeSessionId: state.activeSessionId,
      sessions: state.sessions,
    })),
  );
  const { send, cancel, start, switchToThread, closeSession, renameSession } =
    useAppActions();

  const threadList = useMemo(
    () => ({
      threadId: activeSessionId ?? undefined,
      isLoading: false,
      threads: Object.values(sessions).map(threadData),
      archivedThreads: [] as ExternalStoreThreadData<"archived">[],
      onSwitchToNewThread: () => start(cwd),
      onSwitchToThread: (threadId: string) => switchToThread(threadId),
      onRename: (threadId: string, title: string) =>
        renameSession(threadId, title),
      onArchive: (threadId: string) => closeSession(threadId),
      onDelete: (threadId: string) => closeSession(threadId),
    }),
    [
      activeSessionId,
      closeSession,
      cwd,
      renameSession,
      sessions,
      start,
      switchToThread,
    ],
  );

  const runtime = useExternalStoreRuntime({
    messages,
    isRunning,
    convertMessage: (message: ThreadMessageLike) => message,
    onNew: async (message) => {
      const text = message.content
        .filter(
          (part): part is { type: "text"; text: string } =>
            part.type === "text",
        )
        .map((part) => part.text)
        .join("");
      await send(text);
    },
    onCancel: cancel,
    adapters: { threadList },
  });

  return (
    <AssistantRuntimeProvider runtime={runtime}>
      {children}
    </AssistantRuntimeProvider>
  );
}

const threadData = (session: {
  connection: { sessionId: string };
  title: string;
}): ExternalStoreThreadData<"regular"> => ({
  id: session.connection.sessionId,
  status: "regular",
  title: session.title,
});
