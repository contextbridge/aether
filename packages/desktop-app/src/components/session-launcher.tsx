import { useEffect, useState } from "react";
import { Button } from "./ui/button";
import { Input } from "./ui/input";
import { Notice } from "./ui/notice";
import { Surface } from "./ui/surface";
import { useAppActions, useChatStore } from "../app-provider";

export function SessionLauncher() {
  const [cwd, setCwd] = useState(".");
  const connection = useChatStore((state) => state.connection);
  const activeCwd = useChatStore((state) => state.cwd);
  const error = useChatStore((state) => state.error);
  const { start, closeAll } = useAppActions();

  useEffect(
    () => () => {
      void closeAll();
    },
    [closeAll],
  );

  useEffect(() => {
    if (connection.status === "connected") setCwd(activeCwd);
  }, [activeCwd, connection.status]);

  if (connection.status === "connected") return null;

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
