import type {
  SubAgent,
  ToolCallState,
  ToolStatus,
} from "@aether-agent/browser";
import type {
  ContentBlock,
  ToolCallContent,
} from "@agentclientprotocol/sdk/experimental/v2";
import type {
  ToolCallMessagePartComponent,
  ToolCallMessagePartStatus,
} from "@assistant-ui/react";
import { CheckIcon, LoaderIcon, XCircleIcon } from "lucide-react";
import {
  formatUnknownValue,
  ToolFallbackArgs,
  ToolFallbackContent,
  ToolFallbackResult,
  ToolFallbackRoot,
  ToolFallbackTrigger,
} from "@/components/assistant-ui/elements/tool-fallback.aui";
import { hasType } from "@/aether/acp";
import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";

/**
 * Renders every tool call. Aether's tools are dynamic (MCP servers, sub-agents), so one card
 * reads the call's full `ToolCallState` from the part's `artifact` instead of a UI per tool name.
 */
export const AetherToolCard: ToolCallMessagePartComponent = ({
  toolName,
  argsText,
  status,
  artifact,
}) => {
  const tool = artifact as ToolCallState | undefined;
  const { toolCall } = tool ?? {};
  const failed = tool && typeof tool.status === "object" ? tool.status : null;
  // assistant-ui reports a finished call as complete even when it failed.
  const triggerStatus: ToolCallMessagePartStatus = failed
    ? { type: "incomplete", reason: "error", error: failed.error }
    : status;
  return (
    <ToolFallbackRoot>
      <ToolFallbackTrigger
        toolName={toolCall?.title ?? toolName}
        status={triggerStatus}
      />
      <ToolFallbackContent>
        <div className="flex flex-wrap gap-1.5">
          <Badge variant="outline">{toolName}</Badge>
          {toolCall?.kind && <Badge variant="secondary">{toolCall.kind}</Badge>}
        </div>
        {failed && <p className="text-destructive text-xs">{failed.error}</p>}
        <ToolFallbackArgs argsText={argsText} />
        {toolCall?.content?.map((content, index) => (
          <ToolContent key={index} content={content} />
        ))}
        {tool?.subAgents.map((agent) => (
          <SubAgentView key={agent.taskId} agent={agent} />
        ))}
        {toolCall?.rawOutput !== undefined && (
          <ToolFallbackResult result={toolCall.rawOutput} />
        )}
      </ToolFallbackContent>
    </ToolFallbackRoot>
  );
};

function ToolContent({ content }: { content: ToolCallContent }) {
  if (hasType(content, "content")) return <Block block={content.content} />;
  if (hasType(content, "diff")) return <DiffView diff={content} />;
  if (hasType(content, "terminal")) {
    return (
      <p className="text-muted-foreground text-xs">
        Terminal {content.terminalId}
      </p>
    );
  }
  return <Pre>{formatUnknownValue(content, 2)}</Pre>;
}

function Block({ block }: { block: ContentBlock }) {
  if (hasType(block, "text")) return <Pre>{block.text}</Pre>;
  if (hasType(block, "image")) {
    return (
      <img
        className="max-h-64 rounded-md"
        src={`data:${block.mimeType};base64,${block.data}`}
        alt=""
      />
    );
  }
  return <Pre>{formatUnknownValue(block, 2)}</Pre>;
}

function DiffView({
  diff,
}: {
  diff: Extract<ToolCallContent, { type: "diff" }>;
}) {
  return (
    <div className="flex flex-col gap-1">
      <ul className="text-muted-foreground text-xs">
        {diff.changes.map((change, index) => (
          <li key={index}>
            <span className="font-medium">{change.operation}</span>{" "}
            {"oldPath" in change ? `${String(change.oldPath)} → ` : ""}
            {"path" in change ? String(change.path) : ""}
          </li>
        ))}
      </ul>
      {diff.patch && (
        <pre className="bg-muted/50 overflow-x-auto rounded-md p-2.5 text-xs">
          {diff.patch.text.split("\n").map((line, index) => (
            <div
              key={index}
              className={cn(
                line.startsWith("+") &&
                  !line.startsWith("+++") &&
                  "text-green-600 dark:text-green-400",
                line.startsWith("-") &&
                  !line.startsWith("---") &&
                  "text-red-600 dark:text-red-400",
                line.startsWith("@@") && "text-muted-foreground",
              )}
            >
              {line || " "}
            </div>
          ))}
        </pre>
      )}
    </div>
  );
}

function SubAgentView({ agent }: { agent: SubAgent }) {
  return (
    <div className="border-l-2 pl-3">
      <p className="flex items-center gap-1.5 text-xs font-medium">
        <StatusIcon status={agent.done ? "success" : "running"} />
        {agent.agentName}
      </p>
      <ul className="mt-1 flex flex-col gap-0.5">
        {agent.toolCalls.map((call) => (
          <li
            key={call.id}
            className="text-muted-foreground flex items-center gap-1.5 text-xs"
          >
            <StatusIcon status={call.status} />
            <span className="font-mono">{call.name}</span>
            {call.displayValue && (
              <span className="truncate">{call.displayValue}</span>
            )}
          </li>
        ))}
      </ul>
    </div>
  );
}

function StatusIcon({ status }: { status: ToolStatus }) {
  if (status === "running")
    return <LoaderIcon className="size-3 animate-spin" />;
  if (status === "success") return <CheckIcon className="size-3" />;
  return <XCircleIcon className="text-destructive size-3" />;
}

function Pre({ children }: { children: string }) {
  return (
    <pre className="bg-muted/50 text-foreground/90 max-h-80 overflow-auto rounded-md p-2.5 text-xs whitespace-pre-wrap">
      {children}
    </pre>
  );
}
