// Inspector — a read-only HTTP plane over the manager's live state. Bound to 127.0.0.1 and fronted
// by Caddy at /_inspect (handle_path strips the prefix, so this server sees plain /, /api/*). It is
// deliberately NOT a model tool (the manager's "no hands" boundary stays airtight); it only observes.
//
// It reads four sources, all cheap: the passive Telemetry recorder (turns, prompts, cost), the live
// transcript (snapshot copy), MemFs (memory files on disk), and the workspace memory-bank reader.
// Nothing here mutates runtime state.

import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import type { AddressInfo } from "node:net";

import type { ConvMessage } from "../manager/driver.js";
import type { Telemetry } from "../runtime/telemetry.js";
import type { AppFiles } from "./appfiles.js";
import { logger } from "../logger.js";
import { INSPECTOR_HTML } from "./html.js";

export interface InspectorDeps {
  port: number;
  /** Required secret; when set, every request must carry ?t= or x-inspector-token. */
  token?: string;
  managerModel: string;
  workspaceDir: string;
  appPublicUrl: string;
  telemetry: Telemetry;
  /** Reconstructed conversation log (Telemetry.conversation), from the Codex item stream. */
  conversation: () => ConvMessage[];
  /** All memory files on disk (MemFs.listAll). */
  memories: () => Array<{ path: string; body: string }>;
  appFiles: AppFiles;
}

export interface InspectorServer {
  /** Actual bound port (resolved once `ready` settles; matters when port 0 is requested). */
  readonly port: number;
  /** Resolves with the bound port once the server is listening. */
  ready: Promise<number>;
  close(): Promise<void>;
}

export function startInspector(deps: InspectorDeps): InspectorServer {
  const server = createServer((req, res) => handle(req, res, deps));
  server.on("error", (err) => logger.error("Inspector server error", { error: err.message }));

  let resolvedPort = deps.port;
  // Localhost only: the disposable VM + Caddy (token / basic_auth) are the boundary; we never expose
  // this directly. Binding to the loopback interface keeps it off every other interface entirely.
  const ready = new Promise<number>((resolve) => {
    server.listen(deps.port, "127.0.0.1", () => {
      resolvedPort = (server.address() as AddressInfo).port;
      logger.info("Inspector listening (read-only)", {
        url: `http://127.0.0.1:${resolvedPort}`,
        caddyPath: "/_inspect",
        tokenRequired: Boolean(deps.token),
      });
      resolve(resolvedPort);
    });
  });

  return {
    get port() {
      return resolvedPort;
    },
    ready,
    close: () =>
      new Promise((resolve) => {
        server.close(() => resolve());
      }),
  };
}

function handle(req: IncomingMessage, res: ServerResponse, deps: InspectorDeps): void {
  const url = new URL(req.url ?? "/", "http://localhost");
  const path = url.pathname.replace(/\/+$/, "") || "/";

  // Auth: a single shared secret, accepted as ?t= or the x-inspector-token header. Skipped only when
  // no token is configured (Caddy basic_auth is then expected to be the guard).
  if (deps.token) {
    const supplied = url.searchParams.get("t") ?? req.headers["x-inspector-token"];
    if (supplied !== deps.token) {
      res.writeHead(401, { "content-type": "text/plain" });
      res.end("unauthorized");
      return;
    }
  }

  try {
    if (path === "/") return sendHtml(res, INSPECTOR_HTML);
    if (path === "/api/overview") return sendJson(res, overview(deps));
    if (path === "/api/conversation") return sendJson(res, conversation(deps));
    if (path === "/api/usage") return sendJson(res, usage(deps));
    if (path === "/api/workers") return sendJson(res, workersView(deps));
    if (path === "/api/memories") return sendJson(res, { files: deps.memories() });
    if (path === "/api/trace") return sendJson(res, trace(deps, url.searchParams.get("turn")));
    if (path === "/api/appfiles") return appfiles(res, deps, url.searchParams.get("path"));
  } catch (err) {
    res.writeHead(500, { "content-type": "application/json" });
    res.end(JSON.stringify({ error: (err as Error).message }));
    return;
  }

  res.writeHead(404, { "content-type": "text/plain" });
  res.end("not found");
}

// ---- views -----------------------------------------------------------------

function overview(deps: InspectorDeps): unknown {
  const turns = deps.telemetry.turns();
  return {
    managerModel: deps.managerModel,
    workspaceDir: deps.workspaceDir,
    appPublicUrl: deps.appPublicUrl || null,
    contextTokens: deps.telemetry.contextTokens(),
    usage: deps.telemetry.meter(),
    counts: {
      turns: turns.length,
      workers: new Set(deps.telemetry.prompts().map((p) => p.workerId)).size,
      memories: deps.memories().length,
    },
    lastTurn: turns.length ? turns[turns.length - 1] : null,
  };
}

function conversation(deps: InspectorDeps): unknown {
  const messages = deps.conversation();
  return {
    contextTokens: deps.telemetry.contextTokens(),
    messageCount: messages.length,
    // Reconstructed from the Codex item stream: text (agent_message), thinking (reasoning), and
    // tool_use/tool_result (mcp_tool_call) blocks. The client renders by block.type.
    messages,
  };
}

function usage(deps: InspectorDeps): unknown {
  return {
    meter: deps.telemetry.meter(),
    note: "Everything rides the ChatGPT subscription — no metered $. These are token counts only.",
    turns: deps.telemetry.turns(),
  };
}

// Workers are ephemeral (single-shot) — there is no registry. This view is the dispatch history:
// every worker launch the telemetry traced, with the exact prompt it received, newest first.
function workersView(deps: InspectorDeps): unknown {
  const prompts = deps.telemetry.prompts();
  const ids = [...new Set(prompts.map((p) => p.workerId))];
  return {
    workers: ids
      .map((id) => ({ id, prompts: prompts.filter((p) => p.workerId === id) }))
      .reverse(),
  };
}

function trace(deps: InspectorDeps, turnRaw: string | null): unknown {
  const turnId = Number(turnRaw);
  if (turnRaw === null || turnRaw === "" || !Number.isFinite(turnId)) {
    // Index mode: every turn with the worker prompts it spawned, newest first.
    const turns = deps.telemetry.turns();
    return {
      turns: turns
        .map((t) => ({ ...t, prompts: deps.telemetry.prompts({ turnId: t.turnId }) }))
        .reverse(),
    };
  }
  const turn = deps.telemetry.turns().find((t) => t.turnId === turnId);
  return { turn: turn ?? null, prompts: deps.telemetry.prompts({ turnId }) };
}

function appfiles(res: ServerResponse, deps: InspectorDeps, pathRaw: string | null): void {
  if (!pathRaw) return sendJson(res, { workspaceDir: deps.workspaceDir, files: deps.appFiles.list() });
  const body = deps.appFiles.read(pathRaw);
  if (body === undefined) {
    res.writeHead(404, { "content-type": "application/json" });
    res.end(JSON.stringify({ error: `not a readable memory-bank file: ${pathRaw}` }));
    return;
  }
  sendJson(res, { path: pathRaw, body });
}

// ---- helpers ---------------------------------------------------------------

function sendJson(res: ServerResponse, body: unknown): void {
  res.writeHead(200, { "content-type": "application/json", "cache-control": "no-store" });
  res.end(JSON.stringify(body));
}

function sendHtml(res: ServerResponse, html: string): void {
  res.writeHead(200, { "content-type": "text/html; charset=utf-8", "cache-control": "no-store" });
  res.end(html);
}
