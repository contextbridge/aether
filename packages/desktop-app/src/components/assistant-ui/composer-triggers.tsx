"use client";

import { ComposerTriggerPopover } from "@/components/assistant-ui/composer-trigger-popover";
import { useChatStore } from "@/app-provider";
import {
  ComposerPrimitive,
  unstable_defaultDirectiveFormatter,
  unstable_useMentionAdapter,
  unstable_useSlashCommandAdapter,
  type Unstable_DirectiveFormatter,
  type Unstable_TriggerItem,
} from "@assistant-ui/react";
import { FileCode2Icon, SlashIcon } from "lucide-react";
import { useShallow } from "zustand/react/shallow";
import type { ReactNode } from "react";

const fileMentionFormatter: Unstable_DirectiveFormatter = {
  serialize: (item: Unstable_TriggerItem) =>
    `:${item.type}[${item.label}]{name=${item.id}}`,
  parse: unstable_defaultDirectiveFormatter.parse,
};

const acpSlashFormatter: Unstable_DirectiveFormatter = {
  serialize: (item: Unstable_TriggerItem) => `/${item.id}`,
  parse: unstable_defaultDirectiveFormatter.parse,
};

export function ComposerTriggers() {
  const { availableCommands, workspaceFiles } = useChatStore(
    useShallow((state) => ({
      availableCommands: state.availableCommands,
      workspaceFiles: state.workspaceFiles,
    })),
  );
  const mention = unstable_useMentionAdapter({
    formatter: fileMentionFormatter,
    items: workspaceFiles.map((file) => ({
      id: file.path,
      type: "file",
      label: file.displayName,
      description: file.path,
      icon: "file",
    })),
    includeModelContextTools: false,
  });
  const slash = unstable_useSlashCommandAdapter({
    commands: availableCommands.map((command) => ({
      id: command.name,
      label: `/${command.name}`,
      description: command.input?.hint
        ? `${command.description} · ${command.input.hint}`
        : command.description,
      execute: () => undefined,
    })),
  });

  return (
    <>
      <ComposerTriggerPopover
        char="@"
        {...mention}
        fallbackIcon={FileCode2Icon}
        emptyItemsLabel="No matching files"
        aria-label="Files"
      />
      <ComposerTriggerPopover
        char="/"
        {...slash}
        action={{
          ...slash.action,
          formatter: acpSlashFormatter,
        }}
        fallbackIcon={SlashIcon}
        emptyItemsLabel="No matching commands"
        aria-label="Commands"
      />
    </>
  );
}

export function ComposerTriggerRoot({
  children,
}: Readonly<{ children: ReactNode }>) {
  return (
    <ComposerPrimitive.Unstable_TriggerPopoverRoot>
      {children}
    </ComposerPrimitive.Unstable_TriggerPopoverRoot>
  );
}
