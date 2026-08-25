import { Button } from "./ui/button";
import { Notice } from "./ui/notice";
import { Surface } from "./ui/surface";
import { useAppActions, useChatStore } from "../app-provider";

export function SessionLauncher() {
  const connection = useChatStore((state) => state.connection);
  const workspaceCount = useChatStore(
    (state) => Object.keys(state.workspaces).length,
  );
  const error = useChatStore((state) => state.error);
  const { pickAndOpenWorkspace } = useAppActions();

  if (connection.status === "connected" || workspaceCount > 0) return null;

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
          Open a workspace
        </h1>
        <p className="mt-2 text-muted-foreground">
          Choose a project directory. You can then create and restore threads
          inside that workspace.
        </p>
      </div>
      <Button
        className="mt-6 w-full"
        type="button"
        disabled={connection.status === "connecting"}
        onClick={() => void pickAndOpenWorkspace()}
      >
        Choose workspace…
      </Button>
      {error && (
        <Notice className="mt-4" tone="danger">
          {error}
        </Notice>
      )}
    </Surface>
  );
}
