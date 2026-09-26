import { type SubmitEvent, useState } from "react";
import {
  connect,
  type ConnectionStatus,
  disconnect,
  newSession,
  useAether,
  useConversation,
} from "@/aether/store";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

const DEFAULT_URL = "ws://127.0.0.1:8765";

export function ConnectionBar({
  onToggleEvents,
}: {
  onToggleEvents: () => void;
}) {
  const [url, setUrl] = useState(
    () => new URLSearchParams(location.search).get("url") ?? DEFAULT_URL,
  );
  const status = useAether((state) => state.status);
  const connected = status.kind === "connected";
  const submit = (event: SubmitEvent) => {
    event.preventDefault();
    void (connected ? disconnect() : connect(url));
  };

  return (
    <header className="flex flex-col gap-2 border-b px-4 py-3">
      <form onSubmit={submit} className="flex items-center gap-2">
        <Input
          aria-label="aether server URL"
          className="max-w-sm font-mono"
          value={url}
          disabled={status.kind !== "disconnected"}
          onChange={(event) => setUrl(event.target.value)}
        />
        <Button type="submit" disabled={status.kind === "connecting"}>
          {connected ? "Disconnect" : "Connect"}
        </Button>
        <Button
          type="button"
          variant="outline"
          disabled={!connected}
          onClick={() => void newSession()}
        >
          New session
        </Button>
        <Button
          type="button"
          variant="ghost"
          className="ml-auto"
          onClick={onToggleEvents}
        >
          Events
        </Button>
      </form>
      <SessionSummary status={status} />
    </header>
  );
}

function SessionSummary({ status }: { status: ConnectionStatus }) {
  const agentName = useAether((state) => state.agentName);
  const cwd = useAether((state) => state.remote?.cwd);
  const sessionId = useAether((state) => state.sessionId);
  const conversation = useConversation();
  const usage = conversation?.contextUsage;

  return (
    <div className="text-muted-foreground flex flex-wrap items-center gap-x-4 gap-y-1 text-xs">
      <StatusBadge status={status} />
      {agentName && <span>agent: {agentName}</span>}
      {cwd && <span className="font-mono">{cwd}</span>}
      {sessionId && <span className="font-mono">session: {sessionId}</span>}
      {conversation && (
        <span>
          turn: {conversation.turn}
          {conversation.activity.phase !== "idle" &&
            ` · ${conversation.activity.phase}`}
          {conversation.compacting && " · compacting"}
        </span>
      )}
      {usage && (
        <span>
          context: {usage.used.toLocaleString()} / {usage.size.toLocaleString()}{" "}
          ({Math.round((usage.used / usage.size) * 100)}%)
        </span>
      )}
    </div>
  );
}

function StatusBadge({ status }: { status: ConnectionStatus }) {
  switch (status.kind) {
    case "connected":
      return <Badge>connected</Badge>;
    case "connecting":
      return <Badge variant="secondary">connecting…</Badge>;
    case "disconnected":
      return (
        <Badge variant="outline">
          disconnected
          {status.close &&
            ` (${status.close.code}${status.close.reason ? `: ${status.close.reason}` : ""})`}
        </Badge>
      );
  }
}
