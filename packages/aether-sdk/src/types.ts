import type * as acp from "@agentclientprotocol/sdk";
import type {
  CallToolResult,
  ToolAnnotations,
} from "@modelcontextprotocol/sdk/types.js";
import type { z } from "zod";
import type { ReasoningEffort } from "./generated/aether-settings.js";

export type AgentSelection =
  | { agent: string; model?: never; reasoningEffort?: never }
  | { agent?: never; model: string; reasoningEffort?: ReasoningEffort }
  | { agent?: never; model?: never; reasoningEffort?: never };

export interface AetherElicitationRequest {
  method: "_aether/elicitation";
  params: Record<string, unknown>;
}

export interface AetherElicitationResponse {
  action: "accept" | "decline" | "cancel";
  content?: Record<string, unknown>;
}

export interface SdkMcpToolDefinition<Schema extends z.ZodRawShape> {
  name: string;
  description: string;
  inputSchema: Schema;
  handler: (args: z.infer<z.ZodObject<Schema>>) => Promise<CallToolResult>;
  annotations?: ToolAnnotations;
}

export type AetherMessage =
  | {
      type: "session_update";
      sessionId: string;
      update: acp.SessionUpdate;
      raw: acp.SessionNotification;
    }
  | {
      type: "ext_notification";
      method: string;
      params: Record<string, unknown>;
    }
  | { type: "result"; sessionId: string; stopReason: acp.StopReason }
  | { type: "error"; error: unknown };
