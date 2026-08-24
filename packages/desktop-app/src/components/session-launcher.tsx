import { useEffect, useState } from "react";
import { Button } from "./ui/button";
import { Input } from "./ui/input";
import { Notice } from "./ui/notice";
import { Surface } from "./ui/surface";
import { useAppActions, useChatStore } from "../app-provider";

export function SessionLauncher() {
  const [cwd, setCwd] = useState(".");
  const connection = useChatStore((state) => state.connection);
  const error = useChatStore((state) => state.error);
  const { start, close } = useAppActions();

  useEffect(
    () => () => {
      void close();
    },
    [close],
  );

  if (connection.status === "connected") {
    return (
      <header className="flex min-h-14 items-center justify-between border-b bg-surface px-4">
        <div className="flex items-baseline gap-3">
          <strong>{connection.agentName}</strong>
          <span className="text-sm text-muted-foreground">{cwd}</span>
        </div>
        <div className="flex gap-2">
          <Button variant="secondary" onClick={() => void start(cwd)}>
            New session
          </Button>
          <Button variant="outline" onClick={() => void close()}>
            End session
          </Button>
        </div>
      </header>
    );
  }

  return (
    <Surface
      variant="elevated"
      padding="lg"
      className="m-auto w-[min(34rem,calc(100%-3rem))]"
    >
      <div>
        <p className="m-0 text-xs font-bold uppercase tracking-[0.12em] text-primary">
          Wisp desktop
        </p>
        <h1 className="mt-1 text-left text-2xl font-semibold tracking-tight">
          Start an ACP session
        </h1>
        <p className="mt-2 text-muted-foreground">
          Connect to the local Aether ACP agent and start a threaded
          conversation.
        </p>
      </div>
      <form
        className="mt-6 flex flex-col gap-2"
        onSubmit={(event) => {
          event.preventDefault();
          void start(cwd);
        }}
      >
        <label className="text-sm font-medium" htmlFor="cwd">
          Working directory
        </label>
        <Input
          id="cwd"
          value={cwd}
          onChange={(event) => setCwd(event.target.value)}
        />
        <Button
          className="mt-2"
          type="submit"
          disabled={connection.status === "connecting"}
        >
          {connection.status === "connecting" ? "Connecting…" : "Start Aether"}
        </Button>
      </form>
      {error && (
        <Notice className="mt-4" tone="danger">
          {error}
        </Notice>
      )}
    </Surface>
  );
}
