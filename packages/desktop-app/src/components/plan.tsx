"use client";

import { makeAssistantDataUI } from "@assistant-ui/react";
import type { DataMessagePartComponent } from "@assistant-ui/react";
import type { PlanEntry, PlanEntryStatus } from "@agentclientprotocol/sdk";
import type { LucideIcon } from "lucide-react";
import { CheckIcon, CircleIcon, LoaderIcon } from "lucide-react";
import { cn } from "@/lib/utils";

const statusIcon: Record<PlanEntryStatus, LucideIcon> = {
  pending: CircleIcon,
  in_progress: LoaderIcon,
  completed: CheckIcon,
};

const statusClass: Record<PlanEntryStatus, string> = {
  pending: "text-muted-foreground",
  in_progress: "animate-spin text-primary",
  completed: "text-emerald-500",
};

const priorityLabel: Record<PlanEntry["priority"], string> = {
  high: "High",
  medium: "Medium",
  low: "Low",
};

const PlanRenderer: DataMessagePartComponent<PlanEntry[]> = ({
  data,
}: {
  data: PlanEntry[];
}) => {
  return (
    <div className="aui-plan-root mb-4 w-full rounded-lg border px-3 py-2">
      <p className="text-muted-foreground text-sm font-medium">Plan</p>
      <ol className="mt-2 flex flex-col gap-1.5">
        {data.map((entry, index) => {
          const Icon = statusIcon[entry.status];
          return (
            <li key={index} className="flex items-start gap-2 text-sm">
              <Icon
                className={cn(
                  "mt-0.5 size-3.5 shrink-0",
                  statusClass[entry.status],
                )}
              />
              <span className="min-w-0 flex-1">{entry.content}</span>
              <span className="text-muted-foreground shrink-0 text-xs uppercase tracking-wide">
                {priorityLabel[entry.priority]}
              </span>
            </li>
          );
        })}
      </ol>
    </div>
  );
};

export const PlanDataUI = makeAssistantDataUI({
  name: "plan",
  render: PlanRenderer,
});
