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

export type AetherToolGroups = Record<string, SdkMcpToolDefinition<any>[]>;

export type ExternalMcpServerConfig =
  | StdioMcpServerConfig
  | HttpMcpServerConfig
  | SseMcpServerConfig;

export interface StdioMcpServerConfig {
  type: "stdio";
  command: string;
  args?: string[];
  env?: Record<string, string | undefined>;
}

export interface HttpMcpServerConfig {
  type: "http";
  url: string;
  headers?: Record<string, string>;
}

export interface SseMcpServerConfig {
  type: "sse";
  url: string;
  headers?: Record<string, string>;
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
