import type { DiffScope, FileStatus, GitSnapshot } from "./generated/bindings";

export type ReviewSide = "additions" | "deletions";

export type ReviewComment = {
  id: string;
  path: string;
  side: ReviewSide;
  startLine: number;
  endLine: number;
  lineText: string;
  body: string;
};

export type GitReviewState = {
  view: "conversation" | "gitReview";
  scope: DiffScope;
  status: "idle" | "loading" | "ready" | "error";
  snapshot: GitSnapshot | null;
  comments: ReviewComment[];
  error: string | null;
  pendingMutation: string | null;
};

export const createGitReviewState = (): GitReviewState => ({
  view: "conversation",
  scope: "both",
  status: "idle",
  snapshot: null,
  comments: [],
  error: null,
  pendingMutation: null,
});

export const shouldConfirmReviewMutation = (
  comments: ReviewComment[],
): boolean => comments.length > 0;

export const formatReviewPrompt = (comments: ReviewComment[]): string => {
  const sorted = [...comments].sort(
    (left, right) =>
      left.path.localeCompare(right.path) || left.startLine - right.startLine,
  );
  let prompt = "I'm reviewing the working tree diff. Here are my comments:\n";
  let currentPath: string | null = null;

  for (const comment of sorted) {
    if (comment.path !== currentPath) {
      currentPath = comment.path;
      prompt += `\n## \`${comment.path}\`\n`;
    }
    const kind = comment.side === "additions" ? "added" : "removed";
    const line =
      comment.startLine === comment.endLine
        ? `Line ${comment.startLine}`
        : `Lines ${comment.startLine}-${comment.endLine}`;
    prompt += `\n**${line} (${kind}):** \`${comment.lineText}\`\n> ${comment.body}\n`;
  }

  return prompt;
};

export const statusLabel = (status: FileStatus): string => {
  switch (status) {
    case "added":
      return "A";
    case "deleted":
      return "D";
    case "renamed":
      return "R";
    case "untracked":
      return "?";
    case "modified":
      return "M";
  }
};
