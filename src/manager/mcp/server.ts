// The Lila MCP server (MIGRATION-CODEX.md §5): an in-process streamable-HTTP MCP server bound to
// 127.0.0.1, exposing the manager's memory + orchestration tools. The manager Codex thread reaches
// it via `mcp_servers.lila.url` and a per-boot bearer token. HTTP (not stdio) because the handlers
// must touch live in-process state (MemFs, the Orchestrator) a stdio child couldn't reach without
// extra IPC.
//
// Defense in depth: it binds the loopback interface only AND requires Authorization: Bearer <token>
// (random per boot, mirroring the Inspector token). On a disposable single-tenant host that is belt
// and suspenders, but it costs nothing.

import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import type { AddressInfo } from "node:net";
import { randomUUID } from "node:crypto";

import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StreamableHTTPServerTransport } from "@modelcontextprotocol/sdk/server/streamableHttp.js";

import { lilaTools, type LilaToolDeps } from "./tools.js";
import { logger } from "../../logger.js";

export interface LilaMcpDeps extends Omit<LilaToolDeps, "currentTurnId"> {
  /** Port to bind on 127.0.0.1; 0 picks a free one (read back via `.port`). */
  port?: number;
  /** Bearer token required on every request. */
  token: string;
}

export interface LilaMcpServer {
  /** The URL the Codex `mcp_servers.lila.url` config points at (loopback, /mcp path). */
  readonly url: string;
  /** Actual bound port (meaningful when port 0 was requested). */
  readonly port: number;
  /** Stamp subsequent tool calls with this turn id (worker-prompt tracing). Set per manager turn. */
  setTurn(turnId: number): void;
  close(): Promise<void>;
}

export async function startLilaMcpServer(deps: LilaMcpDeps): Promise<LilaMcpServer> {
  let turnId = 0;

  const mcp = new McpServer({ name: "lila", version: "0.3.0" });
  for (const tool of lilaTools({ ...deps, currentTurnId: () => turnId })) {
    mcp.registerTool(
      tool.name,
      { description: tool.description, inputSchema: tool.inputSchema },
      // The MCP SDK passes parsed args; our handlers take a plain record and return {content,isError}.
      (args: Record<string, unknown>) => tool.handler(args) as never,
    );
  }

  // Stateful session mode: initialize issues a session id Codex echoes on later requests, so the
  // handshake's separate HTTP requests share the server's initialized state (proven in the spike).
  const transport = new StreamableHTTPServerTransport({
    sessionIdGenerator: () => randomUUID(),
    enableJsonResponse: true,
  });
  await mcp.connect(transport);

  const server: Server = createServer((req, res) => {
    if (!authorized(req, deps.token)) {
      res.writeHead(401, { "content-type": "text/plain" }).end("unauthorized");
      return;
    }
    if (!req.url?.startsWith("/mcp")) {
      res.writeHead(404).end();
      return;
    }
    void readBody(req).then((body) =>
      transport.handleRequest(req, res, body).catch((err) => {
        logger.error("Lila MCP request failed", { error: (err as Error).message });
        if (!res.headersSent) res.writeHead(500).end();
      }),
    );
  });

  const port = await new Promise<number>((resolve) => {
    server.listen(deps.port ?? 0, "127.0.0.1", () => {
      resolve((server.address() as AddressInfo).port);
    });
  });
  const url = `http://127.0.0.1:${port}/mcp`;
  logger.info("Lila MCP server listening (loopback, bearer-guarded)", { url });

  return {
    url,
    port,
    setTurn: (id) => {
      turnId = id;
    },
    close: () =>
      new Promise((resolve) => {
        void transport.close();
        server.close(() => resolve());
      }),
  };
}

function authorized(req: IncomingMessage, token: string): boolean {
  return req.headers["authorization"] === `Bearer ${token}`;
}

async function readBody(req: IncomingMessage): Promise<unknown> {
  const chunks: Buffer[] = [];
  for await (const c of req) chunks.push(c as Buffer);
  if (chunks.length === 0) return undefined;
  try {
    return JSON.parse(Buffer.concat(chunks).toString("utf8"));
  } catch {
    return undefined;
  }
}
