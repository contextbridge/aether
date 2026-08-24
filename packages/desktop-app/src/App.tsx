import { Thread } from "./components/assistant-ui/thread";
import { ThreadListSidebar } from "./components/assistant-ui/threadlist-sidebar";
import { PlanDataUI } from "./components/plan";
import { GitReviewView } from "./components/review/git-review-view";
import { DiffWorkerPoolProvider } from "./components/review/worker-pool-provider";
import { SessionLauncher } from "./components/session-launcher";
import { Notice } from "./components/ui/notice";
import {
  SidebarInset,
  SidebarProvider,
  SidebarTrigger,
} from "./components/ui/sidebar";
import { RuntimeProvider } from "./runtime-provider";
import { useEffect } from "react";
import { useAppActions, useChatStore } from "./app-provider";
import "./App.css";

function App() {
  const connection = useChatStore((state) => state.connection);
  const error = useChatStore((state) => state.error);
  const gitReview = useChatStore((state) => state.gitReview);
  const actions = useAppActions();

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (
        connection.status === "connected" &&
        gitReview.view === "conversation" &&
        (event.ctrlKey || event.metaKey) &&
        event.key.toLowerCase() === "g"
      ) {
        event.preventDefault();
        void actions.openGitReview();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [actions, connection.status, gitReview.view]);

  return (
    <DiffWorkerPoolProvider>
      <RuntimeProvider>
        <main className="dark flex h-full min-h-0 flex-col bg-background text-foreground">
          <SessionLauncher />
          {connection.status === "connected" ? (
            <SidebarProvider className="h-full min-h-0 flex-1">
              <ThreadListSidebar collapsible="icon" />
              <SidebarInset className="min-h-0">
                <SidebarTrigger className="absolute top-4 right-4 z-10" />
                <div className="relative min-h-0 flex-1">
                  {gitReview.view === "gitReview" ? (
                    <GitReviewView />
                  ) : (
                    <>
                      <PlanDataUI />
                      <Thread />
                    </>
                  )}
                  {error && (
                    <Notice
                      tone="danger"
                      className="absolute right-4 bottom-4 max-w-[min(32rem,calc(100%-2rem))]"
                    >
                      {error}
                    </Notice>
                  )}
                </div>
              </SidebarInset>
            </SidebarProvider>
          ) : (
            <div className="min-h-0 flex-1" />
          )}
        </main>
      </RuntimeProvider>
    </DiffWorkerPoolProvider>
  );
}

export default App;
