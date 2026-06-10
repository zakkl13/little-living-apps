// The Lila MCP server (MIGRATION-CODEX.md §5). Two layers:
//   1. The tool handlers (lilaTools) against a REAL MemFs + a fake Orchestrator — memory ops land on
//      disk and are searchable, errors surface as is_error, and subagent_* calls reach the
//      orchestrator with their prompts traced to the active turn.
//   2. The HTTP envelope (startLilaMcpServer): bearer-token gating on the loopback transport.

import { strict as assert } from "node:assert";
import { afterEach, describe, it } from "node:test";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { openMemFs, type MemFs } from "../src/memory/memfs.js";
import { lilaTools, type LilaTool } from "../src/manager/mcp/tools.js";
import { startLilaMcpServer, type LilaMcpServer } from "../src/manager/mcp/server.js";
import type { Orchestrator, PromptRecorder, WorkerInfo } from "../src/workers/types.js";

const cleanups: Array<() => void | Promise<void>> = [];
afterEach(async () => {
  while (cleanups.length) await cleanups.pop()!();
});

function freshMem(): MemFs {
  const mem = openMemFs({ dir: mkdtempSync(join(tmpdir(), "mcp-")) });
  cleanups.push(() => mem.close());
  return mem;
}

function fakeOrchestrator(): { orch: Orchestrator; started: Array<{ objective: string; project?: string }> } {
  const started: Array<{ objective: string; project?: string }> = [];
  let n = 0;
  const info = (id: string, purpose: string, project = "/srv/app"): WorkerInfo => ({
    id,
    purpose,
    status: "running",
    project,
  });
  const orch: Orchestrator = {
    start: (objective, project) => {
      started.push({ objective, ...(project ? { project } : {}) });
      return info(`w${++n}`, objective, project);
    },
    send: (id) => info(id, "follow-up", "/srv/app"),
    steer: (id) => info(id, "steered", "/srv/app"),
    cancel: (id) => ({ ...info(id, "x"), status: "canceled" }),
    poll: (id) => (id === "w1" ? { info: info("w1", "scope"), latest: "halfway done" } : undefined),
    list: () => [],
  };
  return { orch, started };
}

function captureRecorder(): { rec: PromptRecorder; prompts: Array<{ turnId: number; workerId: string; kind: string; prompt: string }> } {
  const prompts: Array<{ turnId: number; workerId: string; kind: string; prompt: string }> = [];
  return { rec: { recordPrompt: (r) => prompts.push(r) }, prompts };
}

function toolMap(deps: Parameters<typeof lilaTools>[0]): Map<string, LilaTool> {
  return new Map(lilaTools(deps).map((t) => [t.name, t]));
}
const textOf = (r: { content: Array<{ text: string }> }): string => r.content.map((c) => c.text).join("\n");

describe("Lila MCP tools — memory", () => {
  it("creates a file that lands on disk and is searchable", async () => {
    const mem = freshMem();
    const tools = toolMap({ mem, orchestrator: fakeOrchestrator().orch, currentTurnId: () => 0 });
    const res = await tools.get("memory_create")!.handler({
      path: "/memories/archival/facts/stack.md",
      file_text: "the API uses Fastify\n",
    });
    assert.equal(res.isError, undefined);
    assert.equal(mem.search("Fastify").length, 1);

    const view = await tools.get("memory_view")!.handler({ path: "/memories/archival/facts/stack.md" });
    assert.match(textOf(view), /Fastify/);

    const hits = await tools.get("memory_search")!.handler({ query: "Fastify" });
    assert.match(textOf(hits), /stack\.md/);
  });

  it("surfaces a bad edit as is_error (recoverable)", async () => {
    const mem = freshMem();
    const tools = toolMap({ mem, orchestrator: fakeOrchestrator().orch, currentTurnId: () => 0 });
    const res = await tools.get("memory_str_replace")!.handler({
      path: "/memories/nope.md",
      old_str: "a",
      new_str: "b",
    });
    assert.equal(res.isError, true);
    assert.match(textOf(res), /error:/);
  });
});

describe("Lila MCP tools — orchestration", () => {
  it("subagent_start reaches the orchestrator and traces the prompt to the active turn", async () => {
    const mem = freshMem();
    const { orch, started } = fakeOrchestrator();
    const { rec, prompts } = captureRecorder();
    let turn = 0;
    const tools = toolMap({ mem, orchestrator: orch, telemetry: rec, currentTurnId: () => turn });

    turn = 42;
    const res = await tools.get("subagent_start")!.handler({ objective: "work only within src/api/**", project: "proj" });
    assert.match(textOf(res), /started worker w1/);
    assert.deepEqual(started, [{ objective: "work only within src/api/**", project: "proj" }]);
    assert.deepEqual(prompts, [
      { turnId: 42, workerId: "w1", kind: "start", prompt: "work only within src/api/**" },
    ]);
  });

  it("subagent_poll on an unknown worker is an is_error", async () => {
    const mem = freshMem();
    const tools = toolMap({ mem, orchestrator: fakeOrchestrator().orch, currentTurnId: () => 0 });
    const res = await tools.get("subagent_poll")!.handler({ id: "w999" });
    assert.equal(res.isError, true);
    assert.match(textOf(res), /no such worker/);
  });
});

describe("Lila MCP server — HTTP bearer gating", () => {
  async function boot(): Promise<LilaMcpServer> {
    const mem = freshMem();
    const server = await startLilaMcpServer({
      mem,
      orchestrator: fakeOrchestrator().orch,
      token: "secret-mcp",
    });
    cleanups.push(() => server.close());
    return server;
  }

  it("rejects a request without the bearer token", async () => {
    const server = await boot();
    const res = await fetch(server.url, { method: "POST", headers: { "content-type": "application/json" }, body: "{}" });
    assert.equal(res.status, 401);
    await res.body?.cancel();
  });

  it("accepts the MCP handshake with the bearer token", async () => {
    const server = await boot();
    const res = await fetch(server.url, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        accept: "application/json, text/event-stream",
        authorization: "Bearer secret-mcp",
      },
      body: JSON.stringify({
        jsonrpc: "2.0",
        id: 1,
        method: "initialize",
        params: { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "t", version: "0" } },
      }),
    });
    assert.equal(res.status, 200, "authorized handshake succeeds");
    const body = (await res.json()) as { result?: { serverInfo?: { name?: string } } };
    assert.equal(body.result?.serverInfo?.name, "lila");
  });
});
