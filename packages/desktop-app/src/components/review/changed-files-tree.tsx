import { resolveTheme } from "@pierre/diffs";
import { themeToTreeStyles } from "@pierre/trees";
import type { ContextMenuOpenContext } from "@pierre/trees";
import { FileTree, useFileTree } from "@pierre/trees/react";
import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
} from "react";
import type { GitFile } from "@/generated/bindings";

export type ChangedFilesTreeProps = {
  files: GitFile[];
  onSelectFile: (path: string) => void;
  onStageFile: (file: GitFile) => void;
  onDiscardFile: (file: GitFile) => void;
};

export function ChangedFilesTree({
  files,
  onSelectFile,
  onStageFile,
  onDiscardFile,
}: ChangedFilesTreeProps) {
  const [treeStyle, setTreeStyle] = useState<CSSProperties>(defaultTreeStyle);
  const filesByPath = useRef(new Map(files.map((file) => [file.path, file])));
  const callbacks = useRef({ onDiscardFile, onSelectFile, onStageFile });
  filesByPath.current = new Map(files.map((file) => [file.path, file]));
  callbacks.current = { onDiscardFile, onSelectFile, onStageFile };

  const paths = useMemo(() => files.map((file) => file.path), [files]);
  const gitStatus = useMemo(
    () => files.map((file) => ({ path: file.path, status: file.status })),
    [files],
  );
  const { model } = useFileTree({
    paths,
    gitStatus,
    initialExpansion: "open",
    flattenEmptyDirectories: false,
    search: true,
    density: "compact",
    composition: {
      contextMenu: {
        enabled: true,
        triggerMode: "both",
        buttonVisibility: "when-needed",
      },
    },
    onSelectionChange: (selectedPaths) => {
      for (let index = selectedPaths.length - 1; index >= 0; index -= 1) {
        const path = selectedPaths[index];
        if (filesByPath.current.has(path)) {
          callbacks.current.onSelectFile(path);
          return;
        }
      }
    },
    renderRowDecoration: ({ item }) => {
      const file = filesByPath.current.get(item.path);
      return file ? fileDecoration(file) : null;
    },
  });

  useEffect(() => {
    let active = true;
    void resolveTheme("ayu-dark")
      .then((theme) => {
        if (active)
          setTreeStyle({ ...defaultTreeStyle, ...themeToTreeStyles(theme) });
      })
      .catch(() => undefined);
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    const selectedPaths = model.getSelectedPaths();
    model.resetPaths(paths);
    model.setGitStatus(gitStatus);
    for (const path of selectedPaths) {
      model.getItem(path)?.select();
    }
  }, [gitStatus, model, paths]);

  return (
    <FileTree
      model={model}
      aria-label="Changed files"
      className="h-full"
      style={treeStyle}
      renderContextMenu={(item, context) => {
        const file = filesByPath.current.get(item.path);
        return file ? (
          <FileActions
            file={file}
            context={context}
            onStageFile={callbacks.current.onStageFile}
            onDiscardFile={callbacks.current.onDiscardFile}
          />
        ) : null;
      }}
    />
  );
}

function FileActions({
  file,
  context,
  onStageFile,
  onDiscardFile,
}: {
  file: GitFile;
  context: ContextMenuOpenContext;
  onStageFile: (file: GitFile) => void;
  onDiscardFile: (file: GitFile) => void;
}) {
  const run = (operation: (file: GitFile) => void) => {
    context.close();
    operation(file);
  };

  return (
    <div
      role="menu"
      aria-label={`Actions for ${file.path}`}
      className="z-50 min-w-40 rounded-md border bg-popover p-1 text-popover-foreground shadow-md"
    >
      <button
        type="button"
        role="menuitem"
        className="flex w-full rounded-sm px-2 py-1.5 text-left text-xs hover:bg-accent"
        onClick={() => run(onStageFile)}
      >
        {stageActionLabel(file)}
      </button>
      <button
        type="button"
        role="menuitem"
        className="flex w-full rounded-sm px-2 py-1.5 text-left text-xs text-destructive hover:bg-destructive/10"
        onClick={() => run(onDiscardFile)}
      >
        Discard changes…
      </button>
    </div>
  );
}

function fileDecoration(file: GitFile) {
  if (file.binary) {
    return { text: "Binary", title: "Binary file" };
  }

  const stage =
    file.stageState === "staged"
      ? "Staged"
      : file.stageState === "partiallyStaged"
        ? "Partial"
        : "";
  const additions = file.additions === null ? "" : `+${file.additions}`;
  const deletions = file.deletions === null ? "" : `-${file.deletions}`;
  const text = [stage, additions, deletions].filter(Boolean).join(" ");

  return {
    text,
    title: [
      stage,
      file.additions === null ? "" : `${file.additions} additions`,
      file.deletions === null ? "" : `${file.deletions} deletions`,
    ]
      .filter(Boolean)
      .join(", "),
    parts: [
      ...(stage ? [{ text: `${stage} ` }] : []),
      ...(additions
        ? [{ text: `${additions} `, color: "var(--trees-git-added-color)" }]
        : []),
      ...(deletions
        ? [{ text: deletions, color: "var(--trees-git-deleted-color)" }]
        : []),
    ],
  };
}

function stageActionLabel(file: GitFile): string {
  if (file.stageState === "staged") return "Unstage";
  if (file.stageState === "partiallyStaged") return "Stage rest";
  return "Stage";
}

const defaultTreeStyle: CSSProperties = {
  colorScheme: "dark",
};

