import { describe, expect, it } from "vitest";
import {
  createGitReviewState,
  formatReviewPrompt,
  shouldConfirmReviewMutation,
  type ReviewComment,
} from "./git-review-state";

const comment = (overrides: Partial<ReviewComment> = {}): ReviewComment => ({
  id: "comment-1",
  path: "src/lib.rs",
  side: "additions",
  startLine: 12,
  endLine: 12,
  lineText: "let value = load();",
  body: "Handle this error.",
  ...overrides,
});

describe("git review state", () => {
  it("starts in the combined working-tree scope", () => {
    expect(createGitReviewState()).toMatchObject({
      view: "conversation",
      scope: "both",
      status: "idle",
      comments: [],
      snapshot: null,
    });
  });

  it("guards mutations while comments are queued", () => {
    expect(shouldConfirmReviewMutation([comment()])).toBe(true);
    expect(shouldConfirmReviewMutation([])).toBe(false);
  });

  it("formats comments in file and line order for the agent", () => {
    const prompt = formatReviewPrompt([
      comment({
        id: "second",
        startLine: 20,
        endLine: 22,
        body: "Extract this block.",
      }),
      comment({
        id: "first",
        startLine: 4,
        endLine: 4,
        side: "deletions",
        lineText: "unwrap()",
        body: "Keep the error context.",
      }),
      comment({
        id: "other",
        path: "README.md",
        startLine: 2,
        endLine: 2,
        lineText: "Install",
        body: "Mention pnpm.",
      }),
    ]);

    expect(prompt).toContain("I'm reviewing the working tree diff");
    expect(prompt.indexOf("Line 4 (removed)")).toBeLessThan(
      prompt.indexOf("Lines 20-22 (added)"),
    );
    expect(prompt).toContain("## `README.md`");
    expect(prompt).toContain("> Mention pnpm.");
  });
});
