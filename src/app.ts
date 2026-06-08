// Composition root for the v0.2 manager runtime (DESIGN §2 topology). Wires the memory subsystem,
// the worker orchestrator, the tool registry, the serialized loop, and crash snapshots into one
// object. Boundaries (model, runner, deliver) are injected so the whole runtime runs against
// fakes in tests and real SDKs in production — the same seam discipline as v0.1.

import type { Config } from "./config.js";
import { logger } from "./logger.js";

import { openMemFs, type MemFs } from "./memory/memfs.js";
import type { ManagerModel } from "./manager/anthropic.js";
import { buildRegistry } from "./manager/tools/registry.js";
import { memoryToolModule } from "./manager/tools/memory.js";
import { orchestrationToolModule, type WorkerInfo } from "./manager/tools/orchestration.js";
import { buildSystemPrompt } from "./manager/prompt.js";
import { createTranscript, runManagerTurn, type DeliverFn, type Transcript } from "./manager/manager.js";

import { createEventQueue, type EventQueue } from "./runtime/eventQueue.js";
import { createLoop, type ManagerLoop } from "./runtime/loop.js";
import { openSnapshotStore } from "./runtime/snapshot.js";
import type { TelegramUpdate } from "./transport/telegram.js";

import type { CodexRunner } from "./workers/runner.js";
import { createOrchestrator, type WorkerOrchestrator } from "./workers/orchestrator.js";
import type { Summarize } from "./workers/summarize.js";

export interface ManagerAppDeps {
  config: Config;
  model: ManagerModel;
  runner: CodexRunner;
  /** Delivery channel to the owner (wraps Telegram sendMessage). The manager's reply channel, and
   *  the sink for deterministic system replies (commands, auth refusals). */
  deliver: DeliverFn;
  /** Optional over-long-output condenser (default: clip). */
  summarize?: Summarize;
}

export interface ManagerApp {
  queue: EventQueue;
  loop: ManagerLoop;
  mem: MemFs;
  orchestrator: WorkerOrchestrator;
  transcript: Transcript;
  /** Enqueue an owner message (the poller calls this after authorizing). */
  enqueueOwner(chatId: number, text: string): void;
  /** Authorize + handle commands + enqueue, from a raw Telegram update (the poller sink). */
  ingestTelegramUpdate(update: TelegramUpdate): void;
  /** Persist transcript + queue + workers (run after each turn). */
  persist(): void;
  /** Rehydrate from the last snapshot (cold-wake recovery). */
  restore(): void;
  start(): void;
  close(): Promise<void>;
}

export function createManagerApp(deps: ManagerAppDeps): ManagerApp {
  const { config, model, runner, deliver } = deps;

  const mem = openMemFs({ dir: config.memoryDir, ftsPath: `${config.memoryDir}.fts.sqlite` });
  const transcript = createTranscript();
  const queue = createEventQueue();
  const snapshots = openSnapshotStore(config.managerStateDir);
  const ownerChatId = config.allowedUserIds[0]!;

  const orchestrator = createOrchestrator({
    runner,
    workspaceDir: config.workspaceDir,
    emitEvent: (e) =>
      queue.enqueue({ kind: "worker_event", workerId: e.workerId, status: e.status, summary: e.summary }),
    ...(deps.summarize ? { summarize: deps.summarize } : {}),
    onWorkersChanged: mirrorWorkers,
  });

  const registry = buildRegistry([
    memoryToolModule(mem),
    orchestrationToolModule(orchestrator),
  ]);

  const fmtWorker = (w: WorkerInfo): string => `- ${w.id} [${w.status}] ${w.purpose} @ ${w.project}`;

  function workersLine(): string | undefined {
    const ws = orchestrator.list();
    return ws.length ? ws.map(fmtWorker).join("\n") : undefined;
  }

  function mirrorWorkers(ws: WorkerInfo[]): void {
    const body = ws.length ? ws.map(fmtWorker).join("\n") : "(no active workers)";
    // create overwrites; commit-per-write dedups identical content, so redundant mirrors are free.
    mem.create({
      command: "create",
      path: "/memories/system/workers.md",
      file_text: `---\ndescription: active workers (mirrors the registry)\n---\n${body}\n`,
    });
  }

  function handleCommand(text: string, chatId: number): void {
    const command = text.split(/\s+/)[0]?.toLowerCase().replace(/@.*$/, "") ?? "";
    switch (command) {
      case "/start":
      case "/help":
        void deliver(
          chatId,
          "🤖 Manager ready. Tell me what to build and I'll delegate to Codex workers, " +
            "remember what matters, and report back.\n\nCommands:\n/status — workers + state\n" +
            "/new — clear the working transcript (memory is kept)",
        );
        return;
      case "/status": {
        const ws = orchestrator.list();
        const lines = [
          `Workers: ${ws.length}`,
          ...ws.map((w) => `  ${w.id} [${w.status}] ${w.purpose}`),
          `Pending events: ${queue.size()}`,
          `Memory: ${config.memoryDir}`,
        ];
        void deliver(chatId, lines.join("\n"));
        return;
      }
      case "/new":
        transcript.load([]);
        persist();
        void deliver(chatId, "🆕 Cleared the working transcript. Long-term memory is untouched.");
        return;
      default:
        void deliver(chatId, `Unknown command: ${command}. Try /help.`);
    }
  }

  function persist(): void {
    snapshots.save({
      version: 1,
      transcript: transcript.snapshot(),
      queue: queue.snapshot(),
      workers: orchestrator.registry.snapshot(),
    });
  }

  const loop = createLoop({
    queue,
    ownerChatId,
    runTurn: (event, chatId) =>
      runManagerTurn(event, chatId, {
        model,
        modelName: config.managerModel,
        registry,
        transcript,
        deliver,
        buildSystem: () => {
          const runtime = {
            appPublicUrl: config.appPublicUrl,
            workspaceDir: config.workspaceDir,
          };
          const line = workersLine();
          return buildSystemPrompt(line ? { mem, runtime, workersLine: line } : { mem, runtime });
        },
      }),
    onTurnComplete: persist,
  });

  return {
    queue,
    loop,
    mem,
    orchestrator,
    transcript,
    enqueueOwner: (chatId, text) => queue.enqueue({ kind: "owner_message", chatId, text }),
    ingestTelegramUpdate(update) {
      const msg = update.message ?? update.edited_message;
      if (!msg || typeof msg.text !== "string") return;
      const chatId = msg.chat.id;
      const fromId = msg.from?.id;
      const text = msg.text.trim();

      // Authorize (DESIGN: single owner). Unauthorized senders get a refusal, never a turn.
      if (fromId === undefined || !config.allowedUserIds.includes(fromId)) {
        logger.warn("Rejected unauthorized update", { fromId, chatId });
        void deliver(chatId, "⛔ You are not authorized to use this bot.");
        return;
      }

      if (text.startsWith("/")) {
        handleCommand(text, chatId);
        return;
      }
      queue.enqueue({ kind: "owner_message", chatId, text });
    },
    persist,
    restore() {
      const snap = snapshots.load();
      if (!snap) return;
      transcript.load(snap.transcript);
      queue.load(snap.queue);
      orchestrator.registry.rehydrate(snap.workers);
      logger.info("Restored manager state from snapshot", {
        messages: snap.transcript.length,
        pending: snap.queue.length,
        workers: snap.workers.length,
      });
    },
    start: () => loop.start(),
    async close() {
      await loop.stop();
      mem.close();
    },
  };
}
