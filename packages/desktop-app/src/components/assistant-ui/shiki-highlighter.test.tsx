// @vitest-environment jsdom
import { flushSync } from "react-dom";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { SyntaxHighlighterProps } from "@assistant-ui/react-markdown";

const { useShikiHighlighter } = vi.hoisted(() => ({
  useShikiHighlighter: vi.fn(),
}));

vi.mock("react-shiki", () => ({ useShikiHighlighter }));

import { SyntaxHighlighter } from "./shiki-highlighter";

const components: SyntaxHighlighterProps["components"] = {
  Pre: ({ children, ...props }) => <pre {...props}>{children}</pre>,
  Code: ({ children, ...props }) => <code {...props}>{children}</code>,
};

describe("SyntaxHighlighter", () => {
  let root: Root | null = null;

  afterEach(() => {
    root?.unmount();
    root = null;
    document.body.innerHTML = "";
    vi.clearAllMocks();
  });

  it("renders Shiki's highlighted output with the markdown block styling", () => {
    useShikiHighlighter.mockReturnValue(
      <pre>
        <code>
          <span data-token="keyword">const</span> value = 1;
        </code>
      </pre>,
    );

    const container = document.createElement("div");
    document.body.appendChild(container);

    flushSync(() => {
      root = createRoot(container);
      root.render(
        <SyntaxHighlighter
          components={components}
          language="typescript"
          code="const value = 1;"
        />,
      );
    });

    expect(container.querySelector("[data-token=keyword]")?.textContent).toBe(
      "const",
    );
    expect(container.querySelector(".aui-md-pre")).not.toBeNull();
    expect(useShikiHighlighter).toHaveBeenCalledWith(
      "const value = 1;",
      "typescript",
      "vitesse-dark",
      { delay: 150 },
    );
  });
});
