// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { GitFile } from "@/generated/bindings";
import { ChangedFilesTree } from "./changed-files-tree";

const files: GitFile[] = [
  {
    path: "README.md",
    oldPath: null,
    status: "modified",
    stageState: "unstaged",
    additions: 2,
    deletions: 1,
    binary: false,
  },
  {
    path: "src/components/button.tsx",
    oldPath: null,
    status: "added",
    stageState: "partiallyStaged",
    additions: 12,
    deletions: 3,
    binary: false,
  },
  {
    path: "assets/logo.png",
    oldPath: null,
    status: "untracked",
    stageState: "unstaged",
    additions: null,
    deletions: null,
    binary: true,
  },
];

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  );
  Element.prototype.scrollTo = vi.fn();
});

afterEach(() => {
  act(() => root.unmount());
  vi.unstubAllGlobals();
  document.body.innerHTML = "";
});

const treeRows = (): ShadowRoot => {
  const host = container.querySelector("file-tree-container");
  expect(host?.shadowRoot).not.toBeNull();
  return host!.shadowRoot!;
};

describe("ChangedFilesTree", () => {
  it("renders repository paths as a Git-aware hierarchy and selects files", async () => {
    const onSelectFile = vi.fn();

    await act(async () => {
      root.render(
        <ChangedFilesTree
          files={files}
          onSelectFile={onSelectFile}
          onStageFile={vi.fn()}
          onDiscardFile={vi.fn()}
        />,
      );
    });

    const shadowRoot = treeRows();
    expect(
      Array.from(shadowRoot.querySelectorAll("[data-item-path]")).map(
        (row) => (row as HTMLElement).dataset.itemPath,
      ),
    ).toEqual(expect.arrayContaining(["src/", "src/components/"]));
    const button = shadowRoot.querySelector<HTMLButtonElement>(
      '[data-item-path="src/components/button.tsx"]',
    );
    expect(button?.dataset.itemGitStatus).toBe("added");
    expect(button?.textContent).toContain("+12");
    expect(button?.textContent).toContain("-3");
    expect(button?.textContent).toContain("Partial");

    await act(async () => button?.click());

    expect(onSelectFile).toHaveBeenCalledWith("src/components/button.tsx");
  });

  it("exposes stage and discard actions for a file", async () => {
    const onStageFile = vi.fn();
    const onDiscardFile = vi.fn();

    await act(async () => {
      root.render(
        <ChangedFilesTree
          files={files}
          onSelectFile={vi.fn()}
          onStageFile={onStageFile}
          onDiscardFile={onDiscardFile}
        />,
      );
    });

    const row = treeRows().querySelector<HTMLElement>(
      '[data-item-path="src/components/button.tsx"]',
    );
    await act(async () => {
      row?.dispatchEvent(
        new MouseEvent("contextmenu", {
          bubbles: true,
          cancelable: true,
          clientX: 10,
          clientY: 10,
        }),
      );
    });

    const stage = Array.from(container.querySelectorAll("button")).find(
      (button) => button.textContent === "Stage rest",
    );
    const discard = Array.from(container.querySelectorAll("button")).find(
      (button) => button.textContent === "Discard changes…",
    );
    expect(stage).toBeDefined();
    expect(discard).toBeDefined();

    await act(async () => stage?.click());
    expect(onStageFile).toHaveBeenCalledWith(files[1]);

    await act(async () => {
      row?.dispatchEvent(
        new MouseEvent("contextmenu", {
          bubbles: true,
          cancelable: true,
          clientX: 10,
          clientY: 10,
        }),
      );
    });
    const reopenedDiscard = Array.from(
      container.querySelectorAll("button"),
    ).find((button) => button.textContent === "Discard changes…");
    await act(async () => reopenedDiscard?.click());
    expect(onDiscardFile).toHaveBeenCalledWith(files[1]);
  });

  it("updates paths, Git status, and decorations after a snapshot refresh", async () => {
    const props = {
      onSelectFile: vi.fn(),
      onStageFile: vi.fn(),
      onDiscardFile: vi.fn(),
    };

    await act(async () => {
      root.render(<ChangedFilesTree files={files} {...props} />);
    });
    await act(async () => {
      root.render(
        <ChangedFilesTree
          files={[
            {
              ...files[0],
              status: "renamed",
              stageState: "staged",
              additions: 5,
              deletions: 0,
            },
          ]}
          {...props}
        />,
      );
    });

    const shadowRoot = treeRows();
    expect(
      shadowRoot.querySelector('[data-item-path="src/components/button.tsx"]'),
    ).toBeNull();
    const readme = shadowRoot.querySelector<HTMLElement>(
      '[data-item-path="README.md"]',
    );
    expect(readme?.dataset.itemGitStatus).toBe("renamed");
    expect(readme?.textContent).toContain("Staged");
    expect(readme?.textContent).toContain("+5");
  });
});
