import { AssistantRuntimeProvider } from "@assistant-ui/react";
import { useState } from "react";
import { useAetherRuntime } from "@/aether/runtime";
import { useAether } from "@/aether/store";
import { ConnectionBar } from "@/components/aether/connection-bar";
import { ElicitationDialog } from "@/components/aether/elicitation-dialog";
import { EventLog } from "@/components/aether/event-log";
import { PlanPanel } from "@/components/aether/plan-panel";
import { AetherToolCard } from "@/components/aether/tool-card";
import { Thread } from "@/components/assistant-ui/elements/thread.aui";
import { TooltipProvider } from "@/components/ui/tooltip";

export function App() {
  const runtime = useAetherRuntime();
  const [showEvents, setShowEvents] = useState(false);

  return (
    <TooltipProvider>
      <AssistantRuntimeProvider runtime={runtime}>
        <div className="flex h-dvh flex-col">
          <ConnectionBar
            onToggleEvents={() => setShowEvents((shown) => !shown)}
          />
          <ErrorBanner />
          <div className="flex min-h-0 flex-1">
            <main className="min-w-0 flex-1">
              <Thread components={{ ToolFallback: AetherToolCard, Welcome }} />
            </main>
            <aside className="flex w-96 shrink-0 flex-col border-l empty:hidden">
              <PlanPanel />
              {showEvents && <EventLog />}
            </aside>
          </div>
        </div>
        <ElicitationDialog />
      </AssistantRuntimeProvider>
    </TooltipProvider>
  );
}

function ErrorBanner() {
  const error = useAether((state) => state.error);
  if (!error) return null;
  return (
    <p className="border-destructive/30 bg-destructive/10 text-destructive border-b px-4 py-2 text-sm">
      {error}
    </p>
  );
}

function Welcome() {
  const connected = useAether((state) => state.status.kind === "connected");
  const hasSession = useAether((state) => state.sessionId !== null);
  if (connected) {
    return (
      <p className="mb-6 px-2 text-2xl font-medium tracking-tight">
        {hasSession ? "Send a prompt to start." : "No session is open."}
      </p>
    );
  }
  return (
    <div className="mb-6 flex flex-col gap-2 px-2">
      <p className="text-2xl font-medium tracking-tight">
        Connect to an aether server.
      </p>
      <p className="text-muted-foreground text-sm">
        Start one with <code>aether server --cwd &lt;project&gt;</code>, then
        press Connect. The server accepts one client at a time.
      </p>
    </div>
  );
}
