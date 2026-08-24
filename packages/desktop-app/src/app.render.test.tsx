// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import type { AppEvent } from "./acp-store";
import {
  AppProvider,
  createAppServices,
  type AppServices,
} from "./app-provider";
import App from "./App";

let channelCallback: ((event: AppEvent) => void) | null = null;
let services: AppServices;

vi.mock("@tauri-apps/api/core", () => ({
  Channel: class {
    constructor(callback: (event: AppEvent) => void) {
      channelCallback = callback;
    }
  },
  invoke: vi.fn(async (command: string) => {
    if (command === "load_git_snapshot") {
      return {
        id: "snapshot-1",
        repoRoot: "/workspace",
        patch: "",
        files: [],
        scope: "both",
      };
    }
    if (command === "start_session") {
      return {
        connectionId: "conn-1",
        sessionId: "s1",
        agentName: "Aether",
        configOptions: [
          {
            type: "select",
            id: "mode",
            name: "Mode",
            category: "mode",
            currentValue: "Coder",
            options: [
              { value: "Coder", name: "Coder" },
              { value: "Planner", name: "Planner" },
            ],
          },
          {
            type: "select",
            id: "model",
            name: "Model",
            category: "model",
            currentValue: "anthropic:sonnet",
            options: [
              { value: "anthropic:sonnet", name: "Claude Sonnet" },
              { value: "openai:gpt", name: "GPT" },
            ],
          },
          {
            type: "select",
            id: "reasoning_effort",
            name: "Reasoning Effort",
            category: "thought_level",
            currentValue: "high",
            options: [
              { value: "none", name: "None" },
              { value: "high", name: "High" },
            ],
          },
        ],
      };
    }
    return undefined;
  }),
}));

// jsdom lacks these browser APIs used by the thread viewport and reasoning UI.
beforeEach(() => {
  services = createAppServices();
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  );
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) =>
    setTimeout(() => callback(performance.now()), 1),
  );
  Element.prototype.scrollIntoView = vi.fn();
  Element.prototype.scrollTo = vi.fn();
});

afterEach(() => {
  vi.unstubAllGlobals();
  channelCallback = null;
  document.body.innerHTML = "";
});

const feed = (update: Record<string, unknown>) => {
  act(() => {
    channelCallback?.({
      kind: "sessionUpdate",
      connectionId: "conn-1",
      sessionId: "s1",
      update,
    } as AppEvent);
  });
};

describe("end-to-end rendering", () => {
  it("renders the streamed assistant response", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    let root: Root | null = null;

    await act(async () => {
      root = createRoot(container);
      root.render(
        <AppProvider services={services}>
          <App />
        </AppProvider>,
      );
    });

    await act(async () => {
      await services.actions.start(".");
    });

    feed({
      sessionUpdate: "agent_thought_chunk",
      content: { type: "text", text: "thinking" },
      messageId: "m1",
    });
    feed({
      sessionUpdate: "agent_message_chunk",
      content: { type: "text", text: "Hello there" },
      messageId: "m1",
    });
    services.actions.handleEvent({
      kind: "promptDone",
      connectionId: "conn-1",
      sessionId: "s1",
      stopReason: "endTurn",
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 80));
    });

    const text = container.textContent ?? "";
    expect(text).toContain("Hello there");
    expect(text).toContain("Reasoning");
    expect(text).toContain("Coder");
    expect(text).toContain("Claude Sonnet");
    expect(text).toContain("High");

    (root as Root | null)?.unmount();
  });

  it("opens the Git review mode for the active session", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    let root: Root | null = null;

    await act(async () => {
      root = createRoot(container);
      root.render(
        <AppProvider services={services}>
          <App />
        </AppProvider>,
      );
      await services.actions.start(".");
      await services.actions.openGitReview();
    });

    expect(container.textContent).toContain("Conversation");
    expect(container.textContent).toContain("No changes in the working tree");

    (root as Root | null)?.unmount();
  });

  it("renders tool call activity with its title", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    let root: Root | null = null;

    await act(async () => {
      root = createRoot(container);
      root.render(
        <AppProvider services={services}>
          <App />
        </AppProvider>,
      );
    });
    await act(async () => {
      await services.actions.start(".");
    });

    feed({
      sessionUpdate: "tool_call",
      toolCallId: "t1",
      title: "Read file",
      status: "in_progress",
      rawInput: { path: "src/lib.rs" },
    });
    feed({
      sessionUpdate: "tool_call_update",
      toolCallId: "t1",
      status: "completed",
      content: [
        { type: "content", content: { type: "text", text: "42 lines" } },
      ],
    });
    feed({
      sessionUpdate: "agent_message_chunk",
      content: { type: "text", text: "Done!" },
      messageId: "m1",
    });
    services.actions.handleEvent({
      kind: "promptDone",
      connectionId: "conn-1",
      sessionId: "s1",
      stopReason: "endTurn",
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 80));
    });

    const text = container.textContent ?? "";
    expect(text).toContain("Done!");

    (root as Root | null)?.unmount();
  });
});
