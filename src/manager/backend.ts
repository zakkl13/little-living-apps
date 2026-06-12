// The manager backend (MIGRATION-CODEX.md §4, §5): assembles the three pieces that make the manager
// brain — the static AGENTS.md, the in-process Lila MCP server (memory + subagent tools), and the
// Codex-thread driver — and hands the app a ManagerDriver plus a turn-id stamp and a close hook.
// Injected behind ManagerBackendFactory so tests can swap a fake driver in (no real Codex / MCP).

import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { randomUUID } from "node:crypto";

import type { Config } from "../config.js";
import type { MemFs } from "../memory/memfs.js";
import type { Orchestrator, PromptRecorder } from "../workers/types.js";

import { startLilaMcpServer } from "./mcp/server.js";
import { createManagerThreadFactory } from "./managerCodex.js";
import { createManagerDriver, type DeliverFn, type ManagerDriver } from "./driver.js";
import { buildAgentsMd, buildContextHeader, type RuntimeFacts } from "./prompt.js";

export interface ManagerBackend {
  driver: ManagerDriver;
  /** Stamp the active turn id so the MCP server traces worker prompts to their originating turn. */
  setActiveTurn(turnId: number): void;
  close(): Promise<void>;
}

export interface ManagerBackendCtx {
  config: Config;
  mem: MemFs;
  orchestrator: Orchestrator;
  telemetry: PromptRecorder;
  deliver: DeliverFn;
}

export type ManagerBackendFactory = (ctx: ManagerBackendCtx) => Promise<ManagerBackend>;

export const startManagerBackend: ManagerBackendFactory = async (ctx) => {
  const { config, mem, orchestrator, telemetry, deliver } = ctx;

  // 1. Static instructions → AGENTS.md in the manager working directory (Codex reads it per session).
  mkdirSync(config.managerDir, { recursive: true });
  const runtime: RuntimeFacts = {
    appPublicUrl: config.appPublicUrl,
    workspaceDir: config.workspaceDir,
    appServiceName: config.appServiceName,
  };
  writeFileSync(join(config.managerDir, "AGENTS.md"), buildAgentsMd(runtime) + "\n");

  // 2. Lila MCP server (loopback, bearer-guarded) — the manager's only hands.
  const token = config.lilaMcpToken ?? `lila-${randomUUID()}`;
  const mcp = await startLilaMcpServer({
    mem,
    orchestrator,
    telemetry,
    token,
    ...(config.lilaMcpPort !== undefined ? { port: config.lilaMcpPort } : {}),
  });

  // 3. The locked-down Codex thread factory + the driver that turns events into turns.
  const factory = createManagerThreadFactory({
    ...(config.managerModel ? { model: config.managerModel } : {}),
    reasoningEffort: config.managerReasoningEffort,
    managerDir: config.managerDir,
    mcpUrl: mcp.url,
    mcpToken: token,
    ...(config.codexPathOverride ? { codexPathOverride: config.codexPathOverride } : {}),
  });
  const driver = createManagerDriver({
    factory,
    deliver,
    buildContextHeader: () => buildContextHeader(mem),
  });

  return {
    driver,
    setActiveTurn: (turnId) => mcp.setTurn(turnId),
    close: () => mcp.close(),
  };
};
