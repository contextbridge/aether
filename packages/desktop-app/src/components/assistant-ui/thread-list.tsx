"use client";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { cn } from "@/lib/utils";
import { useAppActions, useChatStore } from "@/app-provider";
import {
  AuiIf,
  ThreadListItemMorePrimitive,
  ThreadListItemPrimitive,
  ThreadListPrimitive,
  useAui,
  useAuiState,
} from "@assistant-ui/react";
import {
  ArchiveIcon,
  ChevronDownIcon,
  ChevronRightIcon,
  FolderOpenIcon,
  Loader2Icon,
  MoreHorizontalIcon,
  PencilIcon,
  PlusIcon,
  SearchIcon,
  TrashIcon,
} from "lucide-react";
import { useShallow } from "zustand/react/shallow";
import {
  forwardRef,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ComponentPropsWithoutRef,
  type FC,
} from "react";

export const ThreadList: FC = () => {
  const [search, setSearch] = useState("");
  const hasThreads = useAuiState((s) => s.threads.threadIds.length > 0);

  return (
    <ThreadListRoot>
      <OpenWorkspace />
      {hasThreads && (
        <ThreadListSearch value={search} onValueChange={setSearch} />
      )}
      <ThreadListItems searchQuery={hasThreads ? search : ""} />
    </ThreadListRoot>
  );
};

export const ThreadListSearch = forwardRef<
  HTMLInputElement,
  Omit<ComponentPropsWithoutRef<typeof Input>, "value" | "onChange"> & {
    value: string;
    onValueChange: (value: string) => void;
  }
>(({ className, value, onValueChange, ...props }, ref) => {
  return (
    <div data-slot="aui_thread-list-search" className="relative px-0.5 py-1">
      <SearchIcon
        data-slot="aui_thread-list-search-icon"
        className="text-muted-foreground pointer-events-none absolute start-3 top-1/2 size-4 -translate-y-1/2"
      />
      <Input
        ref={ref}
        type="search"
        value={value}
        onChange={(event) => onValueChange(event.target.value)}
        aria-label="Search threads"
        placeholder="Search threads"
        className={cn("h-8 ps-8 text-sm", className)}
        {...props}
      />
    </div>
  );
});

ThreadListSearch.displayName = "ThreadListSearch";

export const ThreadListRoot: FC<
  ComponentPropsWithoutRef<typeof ThreadListPrimitive.Root>
> = ({ className, ...props }) => {
  return (
    <ThreadListPrimitive.Root
      data-slot="aui_thread-list-root"
      className={cn("flex flex-col gap-0.5", className)}
      {...props}
    />
  );
};

export const ThreadListItems: FC<
  ComponentPropsWithoutRef<"div"> & { searchQuery?: string }
> = ({ className, searchQuery = "", ...props }) => {
  return (
    <div
      data-slot="aui_thread-list-items"
      className={cn("flex flex-col gap-0.5", className)}
      {...props}
    >
      <AuiIf
        condition={(s) =>
          s.threads.isLoading && s.threads.threadIds.length === 0
        }
      >
        <ThreadListSkeleton />
      </AuiIf>
      <AuiIf
        condition={(s) =>
          !s.threads.isLoading || s.threads.threadIds.length > 0
        }
      >
        <ThreadListItemGroups searchQuery={searchQuery} />
      </AuiIf>
    </div>
  );
};

const ThreadListItemGroups: FC<{ searchQuery?: string }> = ({
  searchQuery = "",
}) => {
  const threadIds = useAuiState((s) => s.threads.threadIds);
  const threadItems = useAuiState((s) => s.threads.threadItems);
  const { workspaces, threads, selectedWorkspaceId } = useChatStore(
    useShallow((state) => ({
      workspaces: state.workspaces,
      threads: state.threads,
      selectedWorkspaceId: state.selectedWorkspaceId,
    })),
  );
  const { start, selectWorkspace, toggleWorkspace } = useAppActions();
  const query = searchQuery.trim().toLowerCase();
  const itemsById = useMemo(
    () => new Map(threadItems.map((item) => [item.id, item])),
    [threadItems],
  );
  const indexById = useMemo(
    () => new Map(threadIds.map((id, index) => [id, index])),
    [threadIds],
  );

  const groups = Object.values(workspaces).map((workspace) => {
    const ids = threadIds
      .filter((id) => threads[id]?.cwd === workspace.path)
      .filter(
        (id) =>
          !query ||
          (itemsById.get(id)?.title || "New Chat")
            .toLowerCase()
            .includes(query),
      )
      .sort((a, b) => {
        const aTime = itemsById.get(a)?.lastMessageAt?.getTime() ?? 0;
        const bTime = itemsById.get(b)?.lastMessageAt?.getTime() ?? 0;
        return bTime - aTime;
      });
    return { workspace, ids };
  });

  if (query && groups.every((group) => group.ids.length === 0)) {
    return (
      <div className="text-muted-foreground px-2.5 py-4 text-sm">
        No threads found
      </div>
    );
  }

  return groups.map(({ workspace, ids }) => (
    <div key={workspace.id} className="pt-1">
      <div
        className={cn(
          "group/workspace flex h-8 items-center rounded-md",
          selectedWorkspaceId === workspace.id && "bg-muted/60",
        )}
      >
        <button
          type="button"
          className="flex min-w-0 flex-1 items-center gap-1.5 px-2 text-left text-sm font-medium"
          onClick={() => {
            selectWorkspace(workspace.id);
            toggleWorkspace(workspace.id);
          }}
          title={workspace.path}
        >
          {workspace.collapsed ? (
            <ChevronRightIcon className="size-3.5 shrink-0" />
          ) : (
            <ChevronDownIcon className="size-3.5 shrink-0" />
          )}
          <span className="truncate">{workspace.name}</span>
        </button>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="mr-1 size-7"
          aria-label={`New thread in ${workspace.name}`}
          onClick={() => {
            selectWorkspace(workspace.id);
            void start(workspace.path);
          }}
        >
          <PlusIcon className="size-3.5" />
        </Button>
      </div>
      {!workspace.collapsed &&
        ids.map((id) => {
          const index = indexById.get(id);
          return index === undefined ? null : (
            <div key={id} className="pl-3">
              <ThreadListPrimitive.ItemByIndex
                index={index}
                components={{ ThreadListItem }}
              />
            </div>
          );
        })}
      {!workspace.collapsed && ids.length === 0 && !query && (
        <p className="text-muted-foreground px-6 py-1 text-xs">No threads yet</p>
      )}
    </div>
  ));
};

const OpenWorkspace: FC = () => {
  const { pickAndOpenWorkspace } = useAppActions();

  return (
    <Button
      type="button"
      variant="ghost"
      className="h-8 justify-start gap-2 px-2.5 text-sm font-normal"
      onClick={() => void pickAndOpenWorkspace()}
    >
      <FolderOpenIcon className="size-4" />
      Open workspace…
    </Button>
  );
};

export const ThreadListNew = forwardRef<
  HTMLButtonElement,
  ComponentPropsWithoutRef<typeof Button> & { labelClassName?: string }
>(({ className, labelClassName, children, ...props }, ref) => {
  return (
    <ThreadListPrimitive.New
      render={
        <Button
          ref={ref}
          variant="ghost"
          data-slot="aui_thread-list-new"
          className={cn(
            "hover:bg-muted data-active:bg-muted h-8 justify-start gap-2 rounded-md px-2.5 text-sm font-normal",
            className,
          )}
          {...props}
        />
      }
    >
      {children ?? (
        <>
          <PlusIcon
            data-slot="aui_thread-list-new-icon"
            className="size-4 shrink-0"
          />
          <span
            data-slot="aui_thread-list-new-label"
            className={cn("whitespace-nowrap", labelClassName)}
          >
            New Thread
          </span>
        </>
      )}
    </ThreadListPrimitive.New>
  );
});

ThreadListNew.displayName = "ThreadListNew";

const ThreadListSkeleton: FC = () => {
  return (
    <div className="flex flex-col gap-0.5">
      {Array.from({ length: 5 }, (_, i) => (
        <div
          key={i}
          role="status"
          aria-label="Loading threads"
          data-slot="aui_thread-list-skeleton-wrapper"
          className="flex h-8 items-center px-2.5"
        >
          <Skeleton
            data-slot="aui_thread-list-skeleton"
            className="h-3.5 w-full"
          />
        </div>
      ))}
    </div>
  );
};

export const ThreadListItem: FC = () => {
  const isRunning = useAuiState((s) => s.threadListItem.isRunning);
  const [isRenaming, setIsRenaming] = useState(false);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const restoreFocusRef = useRef(false);

  useEffect(() => {
    if (isRenaming || !restoreFocusRef.current) return;
    restoreFocusRef.current = false;
    triggerRef.current?.focus();
  }, [isRenaming]);

  return (
    <ThreadListItemPrimitive.Root
      data-slot="aui_thread-list-item"
      className="group hover:bg-muted focus-visible:bg-muted data-active:bg-muted has-focus-visible:bg-muted has-data-[state=open]:bg-muted relative flex h-8 items-center rounded-md transition-colors focus-visible:outline-none"
    >
      {isRenaming ? (
        <ThreadListItemRename
          onDone={(restoreFocus) => {
            restoreFocusRef.current = restoreFocus;
            setIsRenaming(false);
          }}
        />
      ) : (
        <ThreadListItemPrimitive.Trigger
          ref={triggerRef}
          data-slot="aui_thread-list-item-trigger"
          className="focus-visible:ring-ring/50 flex h-full min-w-0 flex-1 items-center rounded-md px-2.5 text-start text-sm outline-none group-hover:pe-9 group-has-focus-visible:pe-9 group-has-data-[state=open]:pe-9 group-data-active:pe-9 focus-visible:ring-1"
        >
          {isRunning && (
            <Loader2Icon
              aria-hidden
              data-slot="aui_thread-list-item-running"
              className="text-muted-foreground me-1.5 size-3.5 shrink-0 animate-spin"
            />
          )}
          <span
            data-slot="aui_thread-list-item-title"
            className="min-w-0 flex-1 truncate"
          >
            <ThreadListItemPrimitive.Title fallback="New Chat" />
          </span>
          {isRunning && <span className="sr-only">Running</span>}
        </ThreadListItemPrimitive.Trigger>
      )}
      <ThreadListItemMore onRename={() => setIsRenaming(true)} />
    </ThreadListItemPrimitive.Root>
  );
};

const ThreadListItemRename: FC<{
  onDone: (restoreFocus: boolean) => void;
}> = ({ onDone }) => {
  const aui = useAui();
  const title = useAuiState((s) => s.threadListItem.title) ?? "";
  const [value, setValue] = useState(title);
  const inputRef = useRef<HTMLInputElement>(null);
  const settledRef = useRef(false);

  useEffect(() => {
    inputRef.current?.select();
  }, []);

  const commit = (restoreFocus: boolean) => {
    if (settledRef.current) return;
    settledRef.current = true;

    const next = value.trim();
    if (!next || next === title) {
      onDone(restoreFocus);
      return;
    }

    // Deferred so a synchronous throw lands on the rejection path too.
    Promise.resolve()
      .then(() => aui.threadListItem.rename(next))
      .then(
        () => onDone(restoreFocus),
        () => {
          settledRef.current = false;
          if (restoreFocus) inputRef.current?.focus();
        },
      );
  };

  const cancel = () => {
    if (settledRef.current) return;
    settledRef.current = true;
    onDone(true);
  };

  return (
    <Input
      ref={inputRef}
      autoFocus
      data-slot="aui_thread-list-item-rename"
      aria-label="Rename thread"
      value={value}
      className="h-7 min-w-0 flex-1 ps-2.5 pe-9 text-sm"
      onChange={(event) => setValue(event.target.value)}
      onBlur={() => commit(false)}
      onKeyDown={(event) => {
        if (event.key === "Enter") {
          event.preventDefault();
          commit(true);
        } else if (event.key === "Escape") {
          event.preventDefault();
          cancel();
        }
      }}
    />
  );
};

const ThreadListItemMore: FC<{ onRename: () => void }> = ({ onRename }) => {
  return (
    <ThreadListItemMorePrimitive.Root sharedFocusGroup>
      <ThreadListItemMorePrimitive.Trigger
        render={
          <Button
            variant="ghost"
            size="icon"
            data-slot="aui_thread-list-item-more"
            className="data-[state=open]:bg-accent absolute end-1.5 top-1/2 size-6 -translate-y-1/2 p-0 opacity-0 group-hover:opacity-100 group-has-focus-visible:opacity-100 group-data-active:opacity-100 data-[state=open]:opacity-100"
          />
        }
      >
        <MoreHorizontalIcon className="size-3.5" />
        <span className="sr-only">More options</span>
      </ThreadListItemMorePrimitive.Trigger>
      <ThreadListItemMorePrimitive.Content
        side="right"
        align="start"
        sideOffset={6}
        data-slot="aui_thread-list-item-more-content"
        className="bg-popover text-popover-foreground data-[state=open]:fade-in-0 data-[state=open]:zoom-in-95 data-[state=open]:animate-in data-[state=closed]:fade-out-0 data-[state=closed]:zoom-out-95 data-[state=closed]:animate-out data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2 data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2 z-50 min-w-32 overflow-hidden rounded-xl border p-1.5"
      >
        <ThreadListItemMorePrimitive.Item
          data-slot="aui_thread-list-item-more-item"
          className="hover:bg-accent hover:text-accent-foreground focus:bg-accent focus:text-accent-foreground flex cursor-pointer items-center gap-2 rounded-lg px-2.5 py-1.5 text-sm outline-none select-none"
          onSelect={onRename}
        >
          <PencilIcon className="size-4" />
          Rename
        </ThreadListItemMorePrimitive.Item>
        <ThreadListItemPrimitive.Archive
          render={
            <ThreadListItemMorePrimitive.Item
              data-slot="aui_thread-list-item-more-item"
              className="hover:bg-accent hover:text-accent-foreground focus:bg-accent focus:text-accent-foreground flex cursor-pointer items-center gap-2 rounded-lg px-2.5 py-1.5 text-sm outline-none select-none"
            />
          }
        >
          <ArchiveIcon className="size-4" />
          Archive
        </ThreadListItemPrimitive.Archive>
        <ThreadListItemPrimitive.Delete
          render={
            <ThreadListItemMorePrimitive.Item
              data-slot="aui_thread-list-item-more-item"
              className="text-destructive hover:bg-destructive/10 hover:text-destructive focus:bg-destructive/10 focus:text-destructive flex cursor-pointer items-center gap-2 rounded-lg px-2.5 py-1.5 text-sm outline-none select-none"
            />
          }
        >
          <TrashIcon className="size-4" />
          Delete
        </ThreadListItemPrimitive.Delete>
      </ThreadListItemMorePrimitive.Content>
    </ThreadListItemMorePrimitive.Root>
  );
};
