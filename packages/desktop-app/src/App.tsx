import { Thread } from "./components/assistant-ui/thread";
import { PlanDataUI } from "./components/plan";
import { SessionLauncher } from "./components/session-launcher";
import { Notice } from "./components/ui/notice";
import { RuntimeProvider } from "./runtime-provider";
import { useChatStore } from "./app-provider";
import "./App.css";

function App() {
  const connection = useChatStore((state) => state.connection);
  const error = useChatStore((state) => state.error);

  return (
    <RuntimeProvider>
      <main className="dark flex h-full min-h-0 flex-col bg-background text-foreground">
        <SessionLauncher />
        {connection.status === "connected" ? (
          <div className="relative min-h-0 flex-1">
            <PlanDataUI />
            <Thread />
            {error && (
              <Notice
                tone="danger"
                className="absolute right-4 bottom-4 max-w-[min(32rem,calc(100%-2rem))]"
              >
                {error}
              </Notice>
            )}
          </div>
        ) : (
          <div className="min-h-0 flex-1" />
        )}
      </main>
    </RuntimeProvider>
  );
}

export default App;
