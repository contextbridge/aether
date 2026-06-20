import { randomBytes, timingSafeEqual } from "node:crypto";
import { once } from "node:events";
import { createServer, type Server } from "node:http";
import { type AddressInfo } from "node:net";

import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { createMcpExpressApp } from "@modelcontextprotocol/sdk/server/express.js";
import { StreamableHTTPServerTransport } from "@modelcontextprotocol/sdk/server/streamableHttp.js";
import express, { type Request, type Response } from "express";

import { AetherSdkError } from "../errors.js";
import type { SdkMcpToolDefinition } from "../types.js";

export interface LocalMcpServerConfig {
  name: string;
  tools: SdkMcpToolDefinition<any>[];
}

export interface LocalMcpServerInfo {
  url: string;
  authToken: string;
}

export class LocalMcpServerHost {
  private readonly config: LocalMcpServerConfig;
  private httpServer: Server | null = null;
  private authToken: string | null = null;

  constructor(config: LocalMcpServerConfig) {
    this.config = config;
  }

  async start(): Promise<LocalMcpServerInfo> {
    if (this.httpServer) {
      throw new AetherSdkError(
        "mcp_server_invalid_config",
        `MCP server "${this.config.name}" already started`,
      );
    }

    const app = createMcpExpressApp().use("/mcp", this.buildMcpRouter());
    this.httpServer = createServer(app);
    this.httpServer.listen(0, "127.0.0.1");
    this.authToken = randomBytes(32).toString("base64url");

    try {
      await once(this.httpServer, "listening");
    } catch (err) {
      throw new AetherSdkError(
        "mcp_server_start_failed",
        "Failed to start local MCP server",
        err,
      );
    }
    const { port } = this.httpServer.address() as AddressInfo;

    return {
      url: `http://127.0.0.1:${port}/mcp`,
      authToken: this.authToken,
    };
  }

  async stop(): Promise<void> {
    const httpServer = this.httpServer;
    this.httpServer = null;
    this.authToken = null;
    if (!httpServer) return;

    await new Promise<void>((resolve, reject) => {
      httpServer.close((err?: Error) => {
        if (err) reject(err);
        else resolve();
      });
      httpServer.closeAllConnections();
    });
  }

  private buildMcpRouter(): express.Router {
    const router = express.Router();
    router.use((req, res, next) => {
      if (this.checkAuth(req.headers.authorization)) {
        next();
        return;
      }

      res.status(401).json({
        jsonrpc: "2.0",
        error: { code: -32000, message: "Unauthorized" },
        id: null,
      });
    });

    router.all("/", async (req: Request, res: Response) => {
      const transport = new StreamableHTTPServerTransport({
        sessionIdGenerator: undefined,
      });

      const server = this.buildServer();

      res.on("close", () => {
        void transport.close();
        void server.close();
      });

      await server.connect(transport);
      await transport.handleRequest(req, res, req.body);
    });

    return router;
  }

  private buildServer(): McpServer {
    const server = new McpServer({ name: this.config.name, version: "0.1.1" });
    for (const def of this.config.tools) {
      server.registerTool(
        def.name,
        {
          description: def.description,
          inputSchema: def.inputSchema,
          annotations: def.annotations,
        },
        async (args: unknown) => def.handler(args as never),
      );
    }
    return server;
  }

  private checkAuth(header: string | undefined): boolean {
    if (!this.authToken || !header?.startsWith("Bearer ")) return false;
    const presented = Buffer.from(header.slice("Bearer ".length));
    const expected = Buffer.from(this.authToken);
    return (
      presented.length === expected.length &&
      timingSafeEqual(presented, expected)
    );
  }
}
