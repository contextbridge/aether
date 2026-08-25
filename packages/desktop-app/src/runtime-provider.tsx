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
  const {
    messages,
    isRunning,
    activeSessionId,
    sessions,
    threads,
    threadsLoading,
    loadingThreadId,
    selectedWorkspaceId,
  } = useChatStore(
    useShallow((state) => ({
      messages: state.messages,
      isRunning: state.isRunning,
      activeSessionId: state.activeSessionId,
      sessions: state.sessions,
      threads: state.threads,
      threadsLoading: state.threadsLoading,
      loadingThreadId: state.loadingThreadId,
      selectedWorkspaceId: state.selectedWorkspaceId,
    })),
  );
  const { send, cancel, start, switchToThread, deleteThread, renameSession } =
    useAppActions();

  const threadList = useMemo(
    () => ({
      threadId: activeSessionId ?? undefined,
      isLoading: threadsLoading,
      threads: Object.values(threads).map((thread) =>
        threadData(
          thread,
          sessions[thread.id]?.isRunning ?? loadingThreadId === thread.id,
        ),
      ),
      archivedThreads: [] as ExternalStoreThreadData<"archived">[],
      onSwitchToNewThread: () => {
        if (selectedWorkspaceId) return start(selectedWorkspaceId);
      },
      onSwitchToThread: (threadId: string) => switchToThread(threadId),
      onRename: (threadId: string, title: string) =>
        renameSession(threadId, title),
      onArchive: (threadId: string) => deleteThread(threadId),
      onDelete: (threadId: string) => deleteThread(threadId),
    }),
    [
      activeSessionId,
      deleteThread,
      renameSession,
      sessions,
      selectedWorkspaceId,
      start,
      threads,
      threadsLoading,
      loadingThreadId,
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

const threadData = (
  thread: { id: string; title: string; updatedAt?: Date },
  isRunning: boolean,
): ExternalStoreThreadData<"regular"> =>
  ({
    id: thread.id,
    status: "regular",
    title: thread.title,
    lastMessageAt: thread.updatedAt,
    isRunning,
  }) as ExternalStoreThreadData<"regular"> & { lastMessageAt?: Date };
