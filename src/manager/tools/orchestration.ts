// Worker orchestration tools (DESIGN §6, §9). All calls are ASYNC: subagent_start/send/steer
// return a handle immediately and finish the turn; the worker runs in the background and re-enters
// as a worker_event. The Orchestrator interface is implemented for real in Phase 3
// (workers/orchestrator.ts over CodexRunner); this module just maps tool calls onto it.

import type { ToolModule } from "./types.js";

export type WorkerStatus = "running" | "idle" | "completed" | "failed" | "canceled";

export interface WorkerInfo {
  id: string;
  purpose: string;
  status: WorkerStatus;
  project: string;
}

export interface Orchestrator {
  /** Spawn a worker for `objective` in `project` (returns immediately). */
  start(objective: string, project?: string): WorkerInfo;
  /** Send a follow-up to an idle worker (async). */
  send(id: string, message: string): WorkerInfo;
  /** Redirect a busy worker: abort the in-flight run, then resume with `guidance`. */
  steer(id: string, guidance: string): WorkerInfo;
  /** Abort a worker's run without resuming. */
  cancel(id: string): WorkerInfo;
  /** Status + latest condensed output for one worker. */
  poll(id: string): { info: WorkerInfo; latest?: string } | undefined;
  /** All known workers (mirrors system/workers.md). */
  list(): WorkerInfo[];
}

const SUBAGENT_SPECS: ToolModule["specs"] = [
  spec("subagent_start", "Spawn a Codex worker on an objective in a project. Returns a worker id; " +
    "the worker runs in the background and reports back as an event. Give it an explicit, " +
    "non-overlapping file scope.", {
    objective: { type: "string" },
    project: { type: "string", description: "project dir under the workspace (optional)" },
  }, ["objective"]),
  spec("subagent_send", "Send a follow-up message to an idle worker.", {
    id: { type: "string" },
    message: { type: "string" },
  }, ["id", "message"]),
  spec("subagent_steer", "Redirect a busy worker (aborts its current run, resumes with guidance).", {
    id: { type: "string" },
    guidance: { type: "string" },
  }, ["id", "guidance"]),
  spec("subagent_poll", "Get a worker's status and latest condensed output.", {
    id: { type: "string" },
  }, ["id"]),
  spec("subagent_list", "List active workers.", {}, []),
  spec("subagent_cancel", "Cancel a worker's run without resuming.", {
    id: { type: "string" },
  }, ["id"]),
];

export function orchestrationToolModule(orchestrator?: Orchestrator): ToolModule {
  const need = (): Orchestrator => {
    if (!orchestrator) throw new Error("worker orchestration is not available");
    return orchestrator;
  };
  return {
    specs: SUBAGENT_SPECS,
    handlers: {
      subagent_start: (input) => {
        const w = need().start(String(input.objective ?? ""), optStr(input.project));
        return { content: `started worker ${w.id} (${w.status}) — ${w.purpose}` };
      },
      subagent_send: (input) => {
        const w = need().send(String(input.id), String(input.message ?? ""));
        return { content: `sent to ${w.id} (${w.status})` };
      },
      subagent_steer: (input) => {
        const w = need().steer(String(input.id), String(input.guidance ?? ""));
        return { content: `steering ${w.id} (${w.status})` };
      },
      subagent_cancel: (input) => {
        const w = need().cancel(String(input.id));
        return { content: `canceled ${w.id} (${w.status})` };
      },
      subagent_poll: (input) => {
        const r = need().poll(String(input.id));
        if (!r) return { content: `no such worker: ${String(input.id)}`, isError: true };
        return { content: `${r.info.id} [${r.info.status}] ${r.info.purpose}\n${r.latest ?? "(no output yet)"}` };
      },
      subagent_list: () => {
        const ws = need().list();
        if (ws.length === 0) return { content: "(no active workers)" };
        return { content: ws.map((w) => `${w.id} [${w.status}] ${w.purpose} @ ${w.project}`).join("\n") };
      },
    },
  };
}

function spec(
  name: string,
  description: string,
  properties: Record<string, unknown>,
  required: string[],
): ToolModule["specs"][number] {
  return { kind: "custom", name, description, input_schema: { type: "object", properties, required } };
}

function optStr(v: unknown): string | undefined {
  return typeof v === "string" && v.length > 0 ? v : undefined;
}
