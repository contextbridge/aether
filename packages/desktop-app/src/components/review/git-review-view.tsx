import {
  parsePatchFiles,
  type CodeViewItem,
  type CodeViewLineSelection,
  type DiffLineAnnotation,
  type FileDiffContentsLoader,
} from "@pierre/diffs";
import {
  CodeView,
  type CodeViewHandle,
  type CodeViewReactOptions,
} from "@pierre/diffs/react";
import {
  ArrowLeft,
  Check,
  GitCommitHorizontal,
  MessageSquarePlus,
  RefreshCw,
  RotateCcw,
  Trash2,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { useAppActions, useChatStore } from "@/app-provider";
import type { ReviewComment } from "@/git-review-state";
import { commands, type DiffScope, type GitFile } from "@/generated/bindings";
import { useIsMobile } from "@/hooks/use-mobile";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { ChangedFilesTree } from "./changed-files-tree";

interface CommentAnnotation {
  commentId?: string;
  draft?: boolean;
}

const itemVersion = (
  snapshotId: string,
  stageState: string,
  comments: ReviewComment[],
  draft: string,
): number => {
  const value = `${snapshotId}:${stageState}:${draft}:${comments.map((comment) => `${comment.id}:${comment.body}`).join(":")}`;
  let hash = 0;
  for (let index = 0; index < value.length; index += 1) {
    hash = (hash * 31 + value.charCodeAt(index)) | 0;
  }
  return hash;
};

const scopes: { value: DiffScope; label: string }[] = [
  { value: "both", label: "Both" },
  { value: "unstaged", label: "Unstaged" },
  { value: "staged", label: "Staged" },
];

export function GitReviewView() {
  const sessionId = useChatStore((state) =>
    state.connection.status === "connected" ? state.connection.sessionId : null,
  );
  const review = useChatStore((state) => state.gitReview);
  const actions = useAppActions();
  const isMobile = useIsMobile();
  const viewerRef = useRef<CodeViewHandle<CommentAnnotation> | null>(null);
  const [selection, setSelection] = useState<CodeViewLineSelection | null>(
    null,
  );
  const [commentBody, setCommentBody] = useState("");
  const [commitMessage, setCommitMessage] = useState("");

  const parsedFiles = useMemo(() => {
    if (!review.snapshot?.patch) return [];
    try {
      return parsePatchFiles(
        review.snapshot.patch,
        review.snapshot.id,
        false,
      ).flatMap((patch) => patch.files);
    } catch {
      return [];
    }
  }, [review.snapshot]);

  const metadataByPath = useMemo(
    () => new Map(parsedFiles.map((file) => [file.name, file])),
    [parsedFiles],
  );
  const gitFilesByPath = useMemo(
    () =>
      new Map(review.snapshot?.files.map((file) => [file.path, file]) ?? []),
    [review.snapshot],
  );
  const items = useMemo<CodeViewItem<CommentAnnotation>[]>(
    () =>
      parsedFiles.map((fileDiff) => {
        const comments = review.comments.filter(
          (comment) => comment.path === fileDiff.name,
        );
        const stageState = gitFilesByPath.get(fileDiff.name)?.stageState ?? "";
        const id = `diff:${fileDiff.name}`;
        const draftSelection = selection?.id === id ? selection : null;
        const annotations = comments.map<DiffLineAnnotation<CommentAnnotation>>(
          (comment) => ({
            side: comment.side,
            lineNumber: comment.endLine,
            metadata: { commentId: comment.id },
          }),
        );
        if (draftSelection) {
          annotations.push({
            side:
              draftSelection.range.endSide ??
              draftSelection.range.side ??
              "additions",
            lineNumber: draftSelection.range.end,
            metadata: { draft: true },
          });
        }
        const draft = draftSelection
          ? `${draftSelection.range.start}:${draftSelection.range.end}:${draftSelection.range.side ?? ""}:${draftSelection.range.endSide ?? ""}`
          : "";
        return {
          id,
          type: "diff",
          fileDiff,
          annotations,
          version: itemVersion(
            review.snapshot?.id ?? "",
            stageState,
            comments,
            draft,
          ),
        };
      }),
    [
      gitFilesByPath,
      parsedFiles,
      review.comments,
      review.snapshot?.id,
      selection,
    ],
  );

  const loadDiffFiles = useMemo<FileDiffContentsLoader | undefined>(() => {
    if (!sessionId || !review.snapshot) return undefined;
    const snapshot = review.snapshot;
    return async (fileDiff) => {
      const contents = await commands.loadDiffFiles(
        sessionId,
        fileDiff.name,
        fileDiff.prevName ?? null,
        snapshot.scope,
      );
      return {
        oldFile: {
          name: fileDiff.prevName ?? fileDiff.name,
          contents: contents.oldContents ?? "",
          cacheKey: `${snapshot.id}:${fileDiff.name}:old`,
        },
        newFile: {
          name: fileDiff.name,
          contents: contents.newContents ?? "",
          cacheKey: `${snapshot.id}:${fileDiff.name}:new`,
        },
      };
    };
  }, [review.snapshot, sessionId]);

  const options = useMemo<CodeViewReactOptions<CommentAnnotation>>(
    () => ({
      theme: "ayu-dark",
      themeType: "dark",
      diffStyle: isMobile ? "unified" : "split",
      diffIndicators: "bars",
      overflow: "scroll",
      stickyHeaders: true,
      hunkSeparators: "line-info",
      enableLineSelection: true,
      enableGutterUtility: true,
      onGutterUtilityClick: (range, context) =>
        setSelection({ id: context.item.id, range }),
      lineHoverHighlight: "number",
      loadDiffFiles,
      expansionLineCount: 20,
    }),
    [isMobile, loadDiffFiles],
  );

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "g") {
        event.preventDefault();
        actions.closeGitReview();
      } else if (event.key === "Escape" && selection === null) {
        actions.closeGitReview();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [actions, selection]);

  const confirmMutation = (): boolean =>
    review.comments.length === 0 ||
    window.confirm(
      "This operation can invalidate queued review comments. Continue and clear them?",
    );

  const runMutation = async (operation: () => Promise<void>) => {
    if (!confirmMutation()) return;
    if (review.comments.length > 0) actions.clearReviewComments();
    setSelection(null);
    await operation();
  };

  const selectScope = async (scope: DiffScope) => {
    if (scope === review.scope || !confirmMutation()) return;
    if (review.comments.length > 0) actions.clearReviewComments();
    setSelection(null);
    await actions.loadGitReview(scope);
  };

  const saveComment = async () => {
    if (!selection || !commentBody.trim()) return;
    const path = selection.id.replace(/^diff:/, "");
    const side = selection.range.endSide ?? selection.range.side ?? "additions";
    const startLine = Math.min(selection.range.start, selection.range.end);
    const endLine = Math.max(selection.range.start, selection.range.end);
    const metadata = metadataByPath.get(path);
    let lines =
      side === "additions" ? metadata?.additionLines : metadata?.deletionLines;
    if (metadata?.isPartial && sessionId && review.snapshot) {
      try {
        const contents = await commands.loadDiffFiles(
          sessionId,
          path,
          metadata.prevName ?? null,
          review.snapshot.scope,
        );
        const text =
          side === "additions" ? contents.newContents : contents.oldContents;
        lines = text?.split(/\r?\n/);
      } catch {
        lines = undefined;
      }
    }
    const comment: ReviewComment = {
      id: crypto.randomUUID(),
      path,
      side,
      startLine,
      endLine,
      lineText: lines?.[Math.max(0, endLine - 1)] ?? "selected diff range",
      body: commentBody.trim(),
    };
    actions.addReviewComment(comment);
    setCommentBody("");
    setSelection(null);
  };

  const stageFile = (file: GitFile) =>
    runMutation(() =>
      actions.stageGitPaths([file.path], file.stageState === "staged"),
    );

  const discardFile = (file: GitFile) => {
    if (
      !window.confirm(
        `Discard all changes to ${file.path}? This cannot be undone.`,
      )
    )
      return;
    return runMutation(() =>
      actions.discardGitPath(file.path, file.oldPath, file.status),
    );
  };

  const commit = async () => {
    if (!commitMessage.trim()) return;
    await runMutation(() => actions.commitGitChanges(commitMessage));
    setCommitMessage("");
  };

  return (
    <section
      className="flex h-full min-h-0 flex-col bg-background"
      aria-label="Git review"
    >
      <header className="flex flex-wrap items-center gap-2 border-b px-3 py-2">
        <Button variant="ghost" size="sm" onClick={actions.closeGitReview}>
          <ArrowLeft /> Conversation
        </Button>
        <div className="flex rounded-lg border p-0.5" aria-label="Diff scope">
          {scopes.map((scope) => (
            <Button
              key={scope.value}
              size="xs"
              variant={review.scope === scope.value ? "secondary" : "ghost"}
              onClick={() => selectScope(scope.value)}
            >
              {scope.label}
            </Button>
          ))}
        </div>
        <Button
          size="sm"
          variant="outline"
          onClick={() => runMutation(() => actions.loadGitReview())}
          disabled={review.status === "loading"}
        >
          <RefreshCw
            className={review.status === "loading" ? "animate-spin" : ""}
          />{" "}
          Refresh
        </Button>
        <Button
          size="sm"
          variant="outline"
          onClick={() => runMutation(() => actions.stageAllGitChanges(false))}
        >
          <Check /> Stage all
        </Button>
        <Button
          size="sm"
          variant="outline"
          onClick={() => runMutation(() => actions.stageAllGitChanges(true))}
        >
          <RotateCcw /> Unstage all
        </Button>
        <div className="ml-auto flex items-center gap-2">
          <Input
            className="h-8 w-48"
            value={commitMessage}
            onChange={(event) => setCommitMessage(event.target.value)}
            onKeyDown={(event) => event.key === "Enter" && commit()}
            placeholder="Commit message"
            aria-label="Commit message"
          />
          <Button size="sm" onClick={commit} disabled={!commitMessage.trim()}>
            <GitCommitHorizontal /> Commit
          </Button>
          <Button
            size="sm"
            onClick={actions.submitGitReview}
            disabled={review.comments.length === 0}
          >
            Submit review ({review.comments.length})
          </Button>
        </div>
      </header>

      {review.status === "error" ? (
        <div className="m-auto max-w-lg rounded-lg border border-destructive/40 bg-destructive/10 p-6 text-center">
          <h2 className="font-semibold">Git diff unavailable</h2>
          <p className="mt-2 text-sm text-muted-foreground">{review.error}</p>
        </div>
      ) : review.status === "loading" && !review.snapshot ? (
        <div className="m-auto text-sm text-muted-foreground">
          Loading working tree diff…
        </div>
      ) : review.snapshot?.files.length === 0 ? (
        <div className="m-auto text-sm text-muted-foreground">
          No changes in the working tree for this scope.
        </div>
      ) : (
        <div className="flex min-h-0 flex-1">
          <aside className="hidden w-72 shrink-0 border-r md:block">
            <ChangedFilesTree
              files={review.snapshot?.files ?? []}
              onSelectFile={(path) =>
                viewerRef.current?.scrollTo({
                  type: "item",
                  id: `diff:${path}`,
                  align: "start",
                })
              }
              onStageFile={stageFile}
              onDiscardFile={discardFile}
            />
          </aside>

          <div className="relative min-h-0 min-w-0 flex-1">
            {items.length > 0 ? (
              <CodeView<CommentAnnotation>
                ref={viewerRef}
                items={items}
                options={options}
                className="git-review-code-view h-full overflow-auto [--diffs-font-size:12px]"
                selectedLines={selection}
                onSelectedLinesChange={setSelection}
                renderHeaderPrefix={(item) => {
                  if (item.type !== "diff") return null;
                  const file = gitFilesByPath.get(item.fileDiff.name);
                  if (!file) return null;
                  return (
                    <button
                      className="mr-2 rounded border px-1.5 py-0.5 text-[10px]"
                      onClick={(event) => {
                        event.stopPropagation();
                        stageFile(file);
                      }}
                    >
                      {file.stageState === "staged"
                        ? "Staged"
                        : file.stageState === "partiallyStaged"
                          ? "Partial"
                          : "Stage"}
                    </button>
                  );
                }}
                renderHeaderMetadata={(item) => {
                  if (item.type !== "diff") return null;
                  const count = review.comments.filter(
                    (comment) => comment.path === item.fileDiff.name,
                  ).length;
                  return count > 0 ? (
                    <span className="text-xs">
                      {count} comment{count === 1 ? "" : "s"}
                    </span>
                  ) : null;
                }}
                renderAnnotation={(annotation) => {
                  const comment = review.comments.find(
                    (candidate) =>
                      candidate.id === annotation.metadata?.commentId,
                  );
                  if (annotation.metadata?.draft) {
                    return (
                      <div className="m-2 rounded-lg border bg-popover p-3 text-sm shadow-xl">
                        <p className="mb-2 text-xs text-muted-foreground">
                          Comment on {selection?.id.replace(/^diff:/, "")} lines{" "}
                          {selection?.range.start}–{selection?.range.end}
                        </p>
                        <Textarea
                          autoFocus
                          value={commentBody}
                          onChange={(event) => setCommentBody(event.target.value)}
                          placeholder="Leave a review comment…"
                        />
                        <div className="mt-2 flex justify-end gap-2">
                          <Button
                            size="sm"
                            variant="ghost"
                            onClick={() => setSelection(null)}
                          >
                            Cancel
                          </Button>
                          <Button
                            size="sm"
                            onClick={saveComment}
                            disabled={!commentBody.trim()}
                          >
                            Add comment
                          </Button>
                        </div>
                      </div>
                    );
                  }
                  if (!comment) return null;
                  return (
                    <div className="m-2 rounded-md border bg-card p-3 text-sm shadow-sm">
                      <div className="flex items-start gap-2">
                        <MessageSquarePlus className="mt-0.5 size-4 text-muted-foreground" />
                        <p className="flex-1 whitespace-pre-wrap">
                          {comment.body}
                        </p>
                        <Button
                          size="icon-xs"
                          variant="ghost"
                          aria-label="Remove comment"
                          onClick={() =>
                            actions.removeReviewComment(comment.id)
                          }
                        >
                          <Trash2 />
                        </Button>
                      </div>
                    </div>
                  );
                }}
              />
            ) : (
              <div className="m-auto p-8 text-center text-sm text-muted-foreground">
                This scope only contains binary changes.
              </div>
            )}

          </div>
        </div>
      )}
    </section>
  );
}
