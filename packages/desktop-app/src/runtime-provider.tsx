import {
  AssistantRuntimeProvider,
  useExternalStoreRuntime,
  type ThreadMessageLike,
} from "@assistant-ui/react";
import { useShallow } from "zustand/react/shallow";
import type { PropsWithChildren } from "react";
import { useAppActions, useChatStore } from "./app-provider";

export function RuntimeProvider({ children }: PropsWithChildren) {
  const { messages, isRunning } = useChatStore(
    useShallow((state) => ({
      messages: state.messages,
      isRunning: state.isRunning,
    })),
  );
  const { send, cancel } = useAppActions();

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
  });

  return (
    <AssistantRuntimeProvider runtime={runtime}>
      {children}
    </AssistantRuntimeProvider>
  );
}
