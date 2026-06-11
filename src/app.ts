// Composition root for the v0.3 manager runtime (MIGRATION-CODEX.md §3 topology). The manager brain
// is now a long-lived Codex thread driven by a ManagerDriver and fed by an in-process Lila MCP server
// (memory + subagent tools); the Anthropic createMessage loop and its transcript are gone. This file
// wires memory, the worker orchestrator, the manager backend, the serialized loop, and crash
// snapshots into one object. The manager backend is injected (deps.backendFactory) so the whole
// runtime runs against fakes in tests and the real Codex SDK + MCP server in production.

import type { Config } from "./config.js";
import { logger } from "./logger.js";

import { openMemFs, type MemFs } from "./memory/memfs.js";
import { buildContextHeader } from "./manager/prompt.js";
import type { DeliverFn, TurnInput } from "./manager/driver.js";
import { startManagerBackend, type ManagerBackend, type ManagerBackendFactory } from "./manager/backend.js";

import { createEventQueue, type EventQueue, type ManagerEvent } from "./runtime/eventQueue.js";
import { createLoop, type ManagerLoop } from "./runtime/loop.js";
import { openSnapshotStore } from "./runtime/snapshot.js";
import { createTelemetry, type Telemetry } from "./runtime/telemetry.js";
import type { TelegramUpdate } from "./transport/telegram.js";

import type { CodexRunner } from "./workers/runner.js";
import { createOrchestrator } from "./workers/orchestrator.js";
import type { Orchestrator } from "./workers/types.js";
import type { Summarize } from "./workers/summarize.js";

export interface ManagerAppDeps {
  config: Config;
  runner: CodexRunner;
  /** Delivery channel to the owner (wraps Telegram sendMessage). The manager's reply channel, and
   *  the sink for deterministic system replies (commands, auth refusals). */
  deliver: DeliverFn;
  /** Optional over-long-output condenser (default: clip). */
  summarize?: Summarize;
  /** Download an owner-sent photo to a local path (view_image intake); omitted → photos ignored. */
  downloadPhoto?: (fileId: string) => Promise<string | undefined>;
  /** Builds the manager backend (MCP server + Codex driver). Defaults to the real one; tests inject
   *  a fake so the loop/persistence/orchestration paths run without a real Codex thread. */
  backendFactory?: ManagerBackendFactory;
}

export interface ManagerApp {
  queue: EventQueue;
  loop: ManagerLoop;
  mem: MemFs;
  orchestrator: Orchestrator;
  /** Passive observability recorder (read by the Inspector; fed by the loop + driver). */
  telemetry: Telemetry;
  /** Enqueue an owner message (the poller calls this after authorizing). */
  enqueueOwner(chatId: number, text: string): void;
  /** Authorize + handle commands + enqueue, from a raw Telegram update (the poller sink). */
  ingestTelegramUpdate(update: TelegramUpdate): Promise<void>;
  /** Persist thread id + queue + usage (run after each turn). */
  persist(): void;
  /** Rehydrate from the last snapshot (cold-wake recovery). */
  restore(): void;
  start(): void;
  close(): Promise<void>;
}

export async function createManagerApp(deps: ManagerAppDeps): Promise<ManagerApp> {
  const { config, runner, deliver } = deps;

  const mem = openMemFs({ dir: config.memoryDir, ftsPath: `${config.memoryDir}.fts.sqlite` });
  const queue = createEventQueue();
  const snapshots = openSnapshotStore(config.managerStateDir);
  const telemetry = createTelemetry();
  const ownerChatId = config.allowedUserIds[0]!;

  const orchestrator = createOrchestrator({
    runner,
    workspaceDir: config.workspaceDir,
    emitEvent: (e) =>
      queue.enqueue({
        kind: "worker_event",
        workerId: e.workerId,
        objective: e.objective,
        status: e.status,
        summary: e.summary,
      }),
    ...(deps.summarize ? { summarize: deps.summarize } : {}),
  });

  // One-time hygiene: pre-ephemeral versions mirrored a worker roster into this always-loaded
  // system file. Workers are single-shot now, so a lingering roster is stale context — drop it.
  try {
    mem.delete({ command: "delete", path: "/memories/system/workers.md" });
  } catch {
    // not present — nothing to clean
  }

  // The manager backend: the Lila MCP server + the Codex-thread driver. Memory + subagent tool
  // handlers run in-process against the live MemFs and Orchestrator above.
  const backend: ManagerBackend = await (deps.backendFactory ?? startManagerBackend)({
    config,
    mem,
    orchestrator,
    telemetry,
    deliver,
  });

  // Compact one-liner for an objective: many paragraphs in, one glanceable label out. Used to make
  // worker events self-describing in the manager's context (workers have no roster to consult).
  const oneLine = (s: string, max = 80): string => {
    const first = (s.split("\n").find((l) => l.trim()) ?? "").trim();
    return first.length > max ? first.slice(0, max - 1) + "…" : first;
  };

  function handleCommand(text: string, chatId: number): void {
    const command = text.split(/\s+/)[0]?.toLowerCase().replace(/@.*$/, "") ?? "";
    switch (command) {
      case "/start":
      case "/help":
        void deliver(
          chatId,
          "🤖 Manager ready. Tell me what to build and I'll delegate to Codex workers, " +
            "remember what matters, and report back.\n\nCommands:\n/status — workers + state\n" +
            "/new — start a fresh manager thread (memory is kept)",
        );
        return;
      case "/status": {
        const lines = [
          `Workers running: ${orchestrator.running()}`,
          `Pending events: ${queue.size()}`,
          `Memory: ${config.memoryDir}`,
        ];
        void deliver(chatId, lines.join("\n"));
        return;
      }
      case "/new":
        // Drop the manager thread: working context cleared, long-term memory untouched (§7).
        backend.driver.reset();
        persist();
        void deliver(chatId, "🆕 Started a fresh manager thread. Long-term memory is untouched.");
        return;
      default:
        void deliver(chatId, `Unknown command: ${command}. Try /help.`);
    }
  }

  function persist(): void {
    const threadId = backend.driver.threadId();
    snapshots.save({
      version: 4,
      ...(threadId ? { managerThreadId: threadId } : {}),
      queue: queue.snapshot(),
      usage: telemetry.usageSnapshot(),
    });
  }

  // A worker event leads with the objective's first line: the worker is gone, so the event itself
  // must say what the work was — the manager has no roster to resolve an id against.
  const requestText = (event: ManagerEvent): string =>
    event.kind === "owner_message"
      ? event.text
      : `[subagent ${event.status}: ${oneLine(event.objective)}]\n${event.summary}`;

  const toTurnInput = (event: ManagerEvent): TurnInput =>
    event.kind === "owner_message"
      ? { text: event.text, ...(event.imagePath ? { imagePath: event.imagePath } : {}) }
      : { text: requestText(event) };

  const loop = createLoop({
    queue,
    ownerChatId,
    runTurn: async (event, chatId, turnId) => {
      telemetry.beginTurn(turnId, event.kind, requestText(event), chatId);
      backend.setActiveTurn(turnId);
      const allowReply = (): boolean => {
        if (event.kind === "owner_message") return true;
        const workerEventsQueued = queue.snapshot().some((e) => e.kind === "worker_event");
        return orchestrator.running() === 0 && !workerEventsQueued;
      };
      try {
        await backend.driver.runTurn(toTurnInput(event), chatId, {
          onUsage: (u) => telemetry.recordUsage(turnId, u),
          onConversation: (m) => telemetry.recordConversation(m),
          allowReply,
        });
      } finally {
        telemetry.endTurn(turnId);
      }
    },
    onTurnComplete: persist,
  });

  async function ingestTelegramUpdate(update: TelegramUpdate): Promise<void> {
    const msg = update.message ?? update.edited_message;
    if (!msg) return;
    const chatId = msg.chat.id;
    const fromId = msg.from?.id;

    // Authorize (single owner). Unauthorized senders get a refusal, never a turn.
    if (fromId === undefined || !config.allowedUserIds.includes(fromId)) {
      logger.warn("Rejected unauthorized update", { fromId, chatId });
      void deliver(chatId, "⛔ You are not authorized to use this bot.");
      return;
    }

    // Photo intake (view_image is on): download the largest size, open the turn with it. The caption
    // (or a default) is the accompanying text.
    if (msg.photo && msg.photo.length > 0 && deps.downloadPhoto) {
      const largest = msg.photo[msg.photo.length - 1]!;
      const imagePath = await deps.downloadPhoto(largest.file_id).catch((err) => {
        logger.warn("Photo download failed; treating as text", { error: (err as Error).message });
        return undefined;
      });
      const text = (msg.caption ?? "").trim() || "(the owner sent an image)";
      queue.enqueue({ kind: "owner_message", chatId, text, ...(imagePath ? { imagePath } : {}) });
      return;
    }

    if (typeof msg.text !== "string") return;
    const text = msg.text.trim();
    if (text.startsWith("/")) {
      handleCommand(text, chatId);
      return;
    }
    queue.enqueue({ kind: "owner_message", chatId, text });
  }

  return {
    queue,
    loop,
    mem,
    orchestrator,
    telemetry,
    enqueueOwner: (chatId, text) => queue.enqueue({ kind: "owner_message", chatId, text }),
    ingestTelegramUpdate,
    persist,
    restore() {
      const snap = snapshots.load();
      if (!snap) return;
      backend.driver.adoptThreadId(snap.managerThreadId);
      queue.load(snap.queue);
      if (snap.usage) telemetry.loadUsage(snap.usage);
      logger.info("Restored manager state from snapshot", {
        managerThread: snap.managerThreadId ?? "(fresh)",
        pending: snap.queue.length,
      });
    },
    start: () => loop.start(),
    async close() {
      await loop.stop();
      await backend.close();
      mem.close();
    },
  };
}

// Re-export so callers can keep `buildContextHeader` available alongside the app (used in tests).
export { buildContextHeader };
