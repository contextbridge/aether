// @vitest-environment jsdom
import { act, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AppProvider, createAppServices } from "@/app-provider";
import { createGitReviewState } from "@/git-review-state";
import { GitReviewView } from "./git-review-view";

vi.mock("./changed-files-tree", () => ({
  ChangedFilesTree: () => null,
}));

vi.mock("@pierre/diffs/react", async () => {
  const { forwardRef, useState } = await import("react");
  return {
    CodeView: forwardRef(function FakeCodeView(
      props: {
        options?: {
          onGutterUtilityClick?: (...args: unknown[]) => unknown;
        };
        renderAnnotation?: (...args: unknown[]) => ReactNode;
      },
      _ref,
    ) {
      const [showAnnotation, setShowAnnotation] = useState(false);
      return (
        <>
          <button
            type="button"
            aria-label="Add line comment"
            onClick={() => {
              props.options?.onGutterUtilityClick?.(
                { start: 2, end: 2, side: "additions" },
                { type: "diff", item: { id: "diff:src/app.ts" } },
              );
              setShowAnnotation(true);
            }}
          >
            +
          </button>
          {showAnnotation &&
            props.renderAnnotation?.(
              { side: "additions", lineNumber: 2, metadata: { draft: true } },
              { id: "diff:src/app.ts", type: "diff" },
            )}
        </>
      );
    }),
  };
});

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  vi.unstubAllGlobals();
  document.body.innerHTML = "";
});

describe("GitReviewView", () => {
  it("opens the review comment composer from the gutter plus button", async () => {
    const services = createAppServices();
    const review = {
      ...createGitReviewState(),
      view: "gitReview" as const,
      status: "ready" as const,
      snapshot: {
        id: "snapshot-1",
        repoRoot: "/workspace",
        scope: "both" as const,
        patch: [
          "diff --git a/src/app.ts b/src/app.ts",
          "--- a/src/app.ts",
          "+++ b/src/app.ts",
          "@@ -1,2 +1,2 @@",
          " const first = true;",
          "-const value = 1;",
          "+const value = 2;",
        ].join("\n"),
        files: [
          {
            path: "src/app.ts",
            oldPath: null,
            status: "modified" as const,
            stageState: "unstaged" as const,
            additions: 1,
            deletions: 1,
            binary: false,
          },
        ],
      },
    };
    const addReviewComment = vi.spyOn(services.actions, "addReviewComment");
    services.store.setState({
      connection: {
        status: "connected",
        connectionId: "connection-1",
        sessionId: "session-1",
        agentName: "Aether",
      },
      gitReview: review,
    });

    await act(async () => {
      root.render(
        <AppProvider services={services}>
          <GitReviewView />
        </AppProvider>,
      );
    });

    const addComment = container.querySelector<HTMLButtonElement>(
      'button[aria-label="Add line comment"]',
    );
    expect(addComment).not.toBeNull();

    await act(async () => addComment?.click());

    expect(container.textContent).toContain("Comment on src/app.ts lines 2–2");
    const textarea = container.querySelector<HTMLTextAreaElement>("textarea");
    expect(textarea).not.toBeNull();

    await act(async () => {
      const setValue = Object.getOwnPropertyDescriptor(
        HTMLTextAreaElement.prototype,
        "value",
      )?.set;
      setValue?.call(textarea, "Please explain this change.");
      textarea?.dispatchEvent(new Event("input", { bubbles: true }));
    });
    const saveComment = Array.from(container.querySelectorAll("button")).find(
      (button) => button.textContent === "Add comment",
    );
    expect(saveComment?.hasAttribute("disabled")).toBe(false);

    await act(async () => saveComment?.click());

    expect(addReviewComment).toHaveBeenCalledWith(
      expect.objectContaining({
        path: "src/app.ts",
        side: "additions",
        startLine: 2,
        endLine: 2,
        body: "Please explain this change.",
      }),
    );
  });
});
