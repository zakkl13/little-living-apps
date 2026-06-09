// Inspector HTTP plane (read-only): boots the REAL runtime against fakes, drives one owner request
// that spawns a worker, then stands up the Inspector over the live app and asserts every panel's
// data — conversation + tokens, memories, worker prompts, request→worker trace, cost, app memory
// bank — plus the token guard. Nothing is deployed; the server binds an ephemeral localhost port.

import { strict as assert } from "node:assert";
import { afterEach, describe, it } from "node:test";
import { writeFileSync } from "node:fs";
import { join } from "node:path";

import { startBot, messageUpdate, type TestBot } from "./helpers.js";
import { resp, text, toolUse } from "./fakes/fakeAnthropic.js";
import { startInspector, type InspectorServer } from "../src/inspector/server.js";
import { openAppFiles } from "../src/inspector/appfiles.js";

const TOKEN = "secret-token";
const bots: TestBot[] = [];
const servers: InspectorServer[] = [];

afterEach(async () => {
  while (servers.length) await servers.pop()!.close();
  while (bots.length) await bots.pop()!.close();
});

async function bootWithWorker(): Promise<TestBot> {
  const bot = await startBot({
    script: [
      // Turn 1: spawn a scoped worker, then ack as plain text (ends the turn).
      resp([toolUse("subagent_start", { objective: "work only within src/api/**", project: "proj" })]),
      resp([text("on it — spun up a worker for the API")]),
      // Turn 2: worker completion event → narrate.
      resp([text("✅ API worker done")]),
      resp([]), // safety buffer
    ],
  });
  bots.push(bot);
  bot.sendUpdate(messageUpdate("build the orders API"));
  await bot.telegram.waitFor(() => bot.telegram.sent.length >= 2);
  await bot.app.orchestrator.whenQuiet();
  await new Promise((r) => setTimeout(r, 20)); // let the final end_turn + snapshot settle
  return bot;
}

function inspectorFor(bot: TestBot): InspectorServer {
  const server = startInspector({
    port: 0,
    token: TOKEN,
    managerModel: bot.config.managerModel,
    workspaceDir: bot.config.workspaceDir,
    appPublicUrl: bot.config.appPublicUrl,
    telemetry: bot.app.telemetry,
    transcript: () => bot.app.transcript.snapshot(),
    memories: () => bot.app.mem.listAll(),
    workers: () => bot.app.orchestrator.registry.snapshot(),
    appFiles: openAppFiles(bot.config.workspaceDir),
  });
  servers.push(server);
  return server;
}

describe("inspector (read-only observability plane)", () => {
  it("rejects requests without the token", async () => {
    const bot = await bootWithWorker();
    const server = inspectorFor(bot);
    const port = await server.ready;
    const res = await fetch(`http://127.0.0.1:${port}/api/overview`);
    assert.equal(res.status, 401);
  });

  it("surfaces conversation, tokens, trace, workers, memories, and cost", async () => {
    const bot = await bootWithWorker();
    const server = inspectorFor(bot);
    const port = await server.ready;
    const get = async (p: string): Promise<any> => {
      const res = await fetch(`http://127.0.0.1:${port}${p}${p.includes("?") ? "&" : "?"}t=${TOKEN}`);
      assert.equal(res.status, 200, `${p} should be 200`);
      return res.json();
    };

    // Overview: cost meter + counts populated by the real turns that ran.
    const overview = await get("/api/overview");
    assert.ok(overview.counts.turns >= 1);
    assert.equal(overview.counts.workers, 1);
    assert.ok(overview.contextTokens > 0, "context tokens captured from model usage");
    assert.ok(overview.cost.managerTurns >= 2);
    assert.ok(overview.cost.costUsd > 0);

    // Conversation: the manager's tool_use for subagent_start is visible in the transcript.
    const convo = await get("/api/conversation");
    assert.ok(convo.messageCount > 0);
    const blocks = convo.messages.flatMap((m: any) => m.content);
    assert.ok(
      blocks.some((b: any) => b.type === "tool_use" && b.name === "subagent_start"),
      "subagent_start tool_use present in conversation",
    );

    // Trace: the owner request's turn carries the exact Codex prompt the worker received.
    const traceData = await get("/api/trace");
    const startPrompt = traceData.turns
      .flatMap((t: any) => t.prompts)
      .find((p: any) => p.kind === "start");
    assert.ok(startPrompt, "a start prompt was traced");
    assert.match(startPrompt.prompt, /src\/api/);

    // Workers: the live worker, joined with the prompt it received.
    const workersData = await get("/api/workers");
    assert.equal(workersData.workers.length, 1);
    assert.ok(workersData.workers[0].prompts.length >= 1);

    // Memories: real MemFS files, including the mirrored worker roster.
    const memData = await get("/api/memories");
    const paths = memData.files.map((f: any) => f.path);
    assert.ok(paths.includes("system/workers.md"), "worker roster mirrored into memory");
    assert.ok(paths.some((p: string) => p.startsWith("system/")), "system memory present");

    // Cost: per-turn series present.
    const costData = await get("/api/cost");
    assert.ok(costData.turns.length >= 1);
    assert.ok(costData.meter.codexTurns >= 1, "one worker launch counted");
  });

  it("reads the target app's memory bank, traversal-guarded", async () => {
    const bot = await bootWithWorker();
    writeFileSync(join(bot.config.workspaceDir, "AGENTS.md"), "# App memory bank\nkeep it green\n");
    const server = inspectorFor(bot);
    const port = await server.ready;
    const get = async (p: string): Promise<{ status: number; json: any }> => {
      const res = await fetch(`http://127.0.0.1:${port}${p}${p.includes("?") ? "&" : "?"}t=${TOKEN}`);
      return { status: res.status, json: await res.json() };
    };

    const list = await get("/api/appfiles");
    assert.ok(list.json.files.includes("AGENTS.md"));

    const file = await get("/api/appfiles?path=AGENTS.md");
    assert.equal(file.status, 200);
    assert.match(file.json.body, /keep it green/);

    // Path traversal is refused.
    const escape = await get("/api/appfiles?path=../../etc/passwd");
    assert.equal(escape.status, 404);
  });
});
