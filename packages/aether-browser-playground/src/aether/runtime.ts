import type { ContentBlock } from "@agentclientprotocol/sdk/experimental/v2";
import {
  type AppendMessage,
  ExportedMessageRepository,
  type ThreadMessage,
  useExternalStoreRuntime,
} from "@assistant-ui/react";
import { useMemo } from "react";
import { cancelTurn, sendPrompt, useAether, useConversation } from "./store";
import { toThreadMessages } from "./thread-messages";

/**
 * An assistant-ui runtime over the active session's conversation. The wasm client already
 * reduces the session, echoes prompts and tracks the turn, so this only projects its snapshots.
 *
 * Snapshots go in as a whole repository rather than `messages`, which assistant-ui only ever adds
 * to: a conversation that is cleared or replayed must also drop the messages it no longer has.
 */
export function useAetherRuntime() {
  const conversation = useConversation();
  const canSend = useAether(
    (state) => state.client !== null && state.sessionId !== null,
  );
  const messageRepository = useMemo(
    () =>
      ExportedMessageRepository.fromArray(
        conversation ? toThreadMessages(conversation) : [],
      ),
    [conversation],
  );
  return useExternalStoreRuntime<ThreadMessage>({
    messageRepository,
    isRunning: conversation !== null && conversation.turn !== "idle",
    isDisabled: !canSend,
    onNew,
    onCancel: cancelTurn,
  });
}

async function onNew(message: AppendMessage): Promise<void> {
  const prompt = message.content.flatMap((part): ContentBlock[] =>
    part.type === "text" ? [{ type: "text", text: part.text }] : [],
  );
  await sendPrompt(prompt);
}
