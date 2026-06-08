# DESIGN.md — Agent-Manager over Codex Workers (v0.2)

> Companion to `SPEC.md`. `SPEC.md` describes the v0.1 "thin proof" (a Telegram bot that relays
> directly to one Codex session). This document supersedes that model: the owner now talks to a
> **Claude manager** that orchestrates many **Codex workers**. The lower half of v0.1 (Codex SDK
> integration, Sprite keep-alive, Telegram transport) is reused; the relaying handler is replaced
> by a manager runtime + memory + coordination layer.

## 1. Goals & non-goals

**Goals**
- A single **manager** (Claude Opus) is the only agent the owner talks to. It plans, remembers,
  and delegates. It has **no filesystem / shell / network tools** — a capability restriction
  enforced by its tool surface, not the process (the process needs net for Anthropic and disk for
  memory).
- **Workers** are Codex sessions (via `@openai/codex-sdk`) with full access, doing all concrete
  work in the workspace under standing rules (`AGENTS.md`).
- **Parallel workers on a single codebase** coordinate via **manager-assigned non-overlapping
  scopes on a shared tree** — a prompt convention in v0.2 (no git worktrees, no branches, no PRs —
  the code is not for human review; formal lease enforcement is deferred, see §7).
- **Memory** is the manager's only durable state, **tool-mediated** via Anthropic's native
  **memory tool** (`memory_20250818`), with a backend modeled on Letta's MemFS: git-tracked
  markdown files on the Sprite, zero external deps for storage.
- Everything runs on a **hibernating Sprite** and survives cold wake.
- **Minimal dependencies**: `@openai/codex-sdk` (workers) + `@anthropic-ai/sdk` (manager loop);
  otherwise Node 22 built-ins + `node:sqlite`. The hardest context-management pieces — compaction,
  memory, stale-result pruning — are **Anthropic Messages API betas**, not hand-rolled (see §4–§5).

**Non-goals (v0.2)**: multi-owner, human review/PR flows, streaming the manager's thinking token
by token, a web UI, peer-to-peer worker coordination (designed-for, not built — see §7).

## 2. Topology

```
                ┌──────────────────── Sprite Service (bot process) ─────────────────────┐
 Telegram ─────▶│ webhook ─┐                                                             │
  (owner)       │          ├─▶ EVENT QUEUE ─▶ manager turn (SERIALIZED) ──┐             │
 reply  text ◀──│ worker  ─┘   owner_msg | worker_event | tick            │             │
                │ events ◀─────────────────────────┐                      ▼             │
                │                                   │              Anthropic Messages    │
                │                                   │              (Opus) tool loop      │
                │                                   │                      │             │
                │   ┌── tools (the ONLY hands) ─────┼──────────────────────┘             │
                │   │ memory_*        → MemFS (markdown + git) + node:sqlite FTS          │
                │   │ subagent_*      → Codex workers (async, parallel; start/send/steer)  │
                │   │ (no comms tool) → the manager's plain text IS the reply (§4)         │
                │   └────────────────────────────────────────────────────────────────── │
                │   workers run in the SHARED tree, within prompt-assigned scopes (§7)    │
                │   keep-alive hold held while: queue non-empty ∨ worker active ∨ turn    │
                └────────────────────────────────────────────────────────────────────────┘
```

## 3. Runtime model

**One serialized manager, many parallel workers.** Two input sources — owner messages and
worker-completion events — feed **one durable queue** drained by **one consumer**. Serializing
manager turns is the core invariant: it's how memory + transcript stay coherent without locks.
Workers run *outside* the turn, in parallel, and re-enter as events.

**A manager turn**
1. Pop next event; append to the working transcript.
2. Build the Anthropic request (system = persona + rules + core memory; messages = recall summary
   block + working transcript; tools = the registry).
3. Call Opus. While the response contains `tool_use`: execute each tool, append `tool_result`,
   call again. The manager's plain `text` blocks ARE the reply: deliver them to the owner each
   iteration (so an ack can land before the work finishes). `thinking` blocks stay private; a turn
   stays silent by emitting only the `NO_REPLY` sentinel.
4. Snapshot transcript + queue to disk. Update the keep-alive hold. Context-window growth is
   handled by **server-side compaction** (no manual truncation); an optional idle memory-hygiene
   tick (§5) is separate and lighter.

**`subagent_start` does not block the turn.** The manager spawns a worker (handle returned
immediately), records it, and finishes its turn ("on it…"). The worker runs in the background; on
completion its (condensed) result is pushed as a `worker_event`, which triggers a later manager
turn that narrates the outcome. This is what keeps long builds from freezing the conversation.

**Keep-alive (already built, lifecycle expanded).** A paused Sprite drops open TCP connections
even on warm pause, which would kill a worker's streaming connection. We hold a Tasks-API lease
(`PUT /v1/tasks/{name}` heartbeated every 60s, `DELETE` to release) while **the queue is
non-empty, OR any worker is active, OR a turn is running**. Only when fully idle do we release and
let the Sprite hibernate.

## 4. Manager agent loop

We use the **`@anthropic-ai/sdk` Messages API** (not the CLI-based Claude *Agent* SDK — evaluated
and rejected: it's a coding-agent framework whose built-in "hands" resist removal, whose subagents
are Claude not Codex, and which bundles the `claude` binary). We own the loop; the SDK gives typed
messages, retries, and streaming. Three betas carry the hard parts so we don't hand-roll them:

- **Compaction** — `compact-2026-01-12` (`context_management.edits: [{type:"compact_20260112"}]`).
  The API summarizes earlier context server-side as the window fills. **We must append the full
  `response.content` (including `compaction` blocks) back each turn** — the API uses them to replace
  compacted history. This *replaces* the hand-rolled compaction module entirely.
- **Context editing** — `clear_tool_uses_20250919` (`context-management-2025-06-27`) prunes stale
  worker `tool_result`s; paired with the memory tool it gives Claude a "save before clear" warning.
- **Memory tool** — `memory_20250818` is the model's memory interface; we own the backend (§5).

Loop: build request (system + core memory + tools) → call Opus → while `tool_use`, execute tools,
append `tool_result`, call again. The manager's plain `text` blocks are delivered to the owner each
iteration (Hermes/Letta-v1 style: there is no `notify_user` tool — staying in-distribution with how
the models emit assistant text). `thinking` blocks are private; the `NO_REPLY` sentinel buys silence.

- **Models:** `MANAGER_MODEL` = `claude-opus-4-8` (the only model — there is no utility model; the
  worker-output bound is a deterministic clip, not a model call). The manager request sets
  `thinking:{type:"adaptive",display:"summarized"}` (the only on-mode for opus-4-8; auto-enables
  interleaved thinking, no beta header) + `output_config:{effort:"high"}`. Thinking blocks are the
  manager's private reasoning channel — never delivered (the loop delivers only `text`), round-tripped
  verbatim. Manager leanness is the cost-control lever.
- **"No hands" is the tool list.** The request includes only our tool groups (§9) + the `memory`
  tool. There is no bash/read/write/web tool on the raw Messages API unless we add one — so the
  capability boundary is airtight by construction (the Agent SDK couldn't guarantee this).
- **Token accounting** comes from `usage.input_tokens`; compaction triggers automatically server-
  side, so we don't gate on a manual threshold.

## 5. Memory subsystem (native memory tool + MemFS backend)

The **model-facing interface is Anthropic's memory tool** (`memory_20250818`): the SDK helper
`betaMemoryTool(handlers)` lets us implement the storage backend behind the tool's fixed command
set (`view`/`create`/`str_replace`/`insert`/`delete`/`rename`) over a `/memories` directory. **We
implement that backend as MemFS** — so we get a well-tuned, Anthropic-designed editing contract
(and the context-editing "save before clear" integration) while keeping full control of storage.

**Source of truth = a git repo of markdown files** (`MEMORY_DIR`, exposed to the tool as
`/memories`); **`node:sqlite` FTS5 is a derived index** for search (the "git=truth, sqlite=query"
split used by Letta and mcp_agent_mail).

**Layout**
```
<MEMORY_DIR>/                 # the manager's own git repo (NOT a project repo)
  system/                     # ALWAYS loaded in full, every turn  (= core memory)
    persona.md                # who the manager is, how it works
    owner.md                  # owner profile, preferences, comms style
    projects.md               # active projects + status (pointers, not contents)
    workers.md                # active worker ids + purpose (mirrors the registry)
    pinned.md                 # must-not-forget items
  archival/                   # visible as file-tree + description only; loaded on demand
    decisions/*.md
    facts/*.md
    outcomes/*.md
  recall/                     # summarized past conversation (searchable history)
    YYYY-MM/*.md
```

**Block model.** Every file has YAML frontmatter: `description` (always visible even when the body
isn't) and optional `limit` (character budget). Core blocks are small and budgeted; the manager
sees `system/` bodies in full plus a *tree listing* of `archival/` with descriptions.

**Editing — via the native `memory` tool.** The model edits memory through `memory_20250818`'s
commands (`create`/`str_replace`/`insert`/`delete`/`rename`/`view`), handled by our MemFS backend.
This replaces the hand-rolled `memory_replace`/`insert`/`rethink` tools. Two backend behaviors are
*our* additions on top of the standard tool:
- **`system/` is auto-injected** in full into the system prompt every turn (the tool doesn't do
  this — our prompt builder reads `MEMORY_DIR/system/*` directly); everything else is shown as a
  tree + frontmatter `description`, pulled on demand via the tool's `view`.
- **Search tools** the memory tool lacks: `memory_search(query)` (FTS over all files) and
  `recall_search(query)` (over `recall/`), exposed alongside the `memory` tool.

Every backend write is a **git commit** (message = command + one-line summary) → a literal
changelog of what the manager has learned; the FTS index is upserted on the same write.

**Context-window management is server-side (§4):** the `compact-2026-01-12` beta summarizes old
turns automatically, and `clear_tool_uses_20250919` prunes stale worker results — so there is **no
hand-rolled compaction**. What remains is a lighter, optional **idle memory-hygiene tick**: when the
queue drains, a Haiku pass may consolidate/defragment memory files (split large, merge dupes, file
salient facts into `archival/`). This is housekeeping, not context-truncation, and is never on the
hot path. `system/` is never cleared.

## 6. Worker tier

A worker = a Codex thread driven by our existing `CodexRunner` (`@openai/codex-sdk`,
`startThread`/`resumeThread`/`runStreamed`). The `subagent_*` tools are a thin async layer:

| Tool | Implementation |
|---|---|
| `subagent_start(objective, project)` | `startThread({ workingDirectory: <project>, sandboxMode: danger-full-access })`; run async; register w/ an `AbortController`; return worker id (= thread id) |
| `subagent_send(id, message)` | `resumeThread(id).run(...)` async (used when the worker is idle) |
| `subagent_steer(id, guidance)` | **abort the in-flight run, then `resumeThread(id).run(guidance)`** — redirect a busy worker without losing thread context (see below) |
| `subagent_poll(id)` | status + latest condensed output (for long-running workers) |
| `subagent_list()` | active workers (also mirrored in `system/workers.md`) |
| `subagent_cancel(id)` | abort the run, do not resume |

- **Steering semantics (`subagent_steer`).** The Codex *app-server protocol* has true mid-turn
  `turn/steer`, but the **TypeScript SDK does not expose it** (only `run`/`runStreamed` + an
  `AbortSignal` via `TurnOptions.signal`; open req [#12329](https://github.com/openai/codex/issues/12329)).
  So we implement steer as **abort + resume**: each active run holds an `AbortController`; steering
  aborts it and immediately calls `resumeThread(id).run(guidance)`. Because the thread persists its
  context and the workspace is on disk, the worker continues toward the revised objective — it
  restarts the *turn*, it does not inject live mid-turn. An aborted-for-steer run is treated as a
  transition (not a failure event). If we ever need true in-flight injection, the upgrade path is
  to drive workers via `codex app-server` JSON-RPC instead of the SDK (larger change, noted in §17).
- **Summaries-and-pointers contract.** Workers return concise results (paths/ids, not file dumps);
  enforced by the objective wording + `AGENTS.md`. A Haiku `summarize` pass is the fallback if a
  worker over-returns.
- **Threads survive our process dying.** Codex persists threads server-side. On cold wake/crash
  we rehydrate worker ids from `system/workers.md` and re-poll/resume — durability for free.
- **Worker standing rules (`AGENTS.md`)** add two disciplines beyond v0.1:
  - **Memory Bank** ([Cline pattern](https://docs.cline.bot/best-practices/memory-bank)): every
    project repo has a `memory-bank/` (`projectbrief.md`, `productContext.md`, `systemPatterns.md`,
    `techContext.md`, `activeContext.md`, `progress.md`). Workers **read all of it at the start of
    every objective** (cold-start resilient) and update `activeContext.md`/`progress.md` when done.
    This is the per-codebase analog of the manager's memory.
  - **Scope discipline**: edit only files inside the manager-assigned scope; commit small units.
    Advisory in v0.2 (no commit guard yet — see §7).

## 7. Coordination (prompt-only convention for v0.2)

No worktrees, and — for v0.2 — **no formal lease tools or commit guard.** The manager partitions
work so parallel writers never overlap; coordination is expressed as **prose discipline in the
manager system prompt and worker `AGENTS.md`**, not enforced machinery. This keeps the first build
small; the enforcement layer is a documented future addition (below).

```
manager decomposes goal → N non-overlapping file scopes (by module/dir/feature)
  ├─ worker A objective: "work only within src/codex/**"    ┐ parallel · same tree · no overlap
  ├─ worker B objective: "work only within src/telegram/**" ┤ → no conflict → nothing to merge
  └─ worker C objective: "work only within test/**"         ┘
workers commit small units to the single branch (linear history = rollback/audit)
overlap that can't be partitioned → manager SERIALIZES (run one, then the next)
reads need no scoping (parallel exploration always safe)
```

- **The scope contract lives in the prompt.** The manager is instructed to assign each worker an
  explicit, non-overlapping file scope in its objective, to serialize when scopes can't be
  separated, and to track "who's working on what" in `system/workers.md`. Worker `AGENTS.md` is
  instructed to stay within the assigned scope and commit small units.
- **No enforcement teeth yet.** We accept that this is advisory in v0.2. Correctness rests on good
  decomposition + serialization, plus git-as-checkpoint for rollback if a worker strays.
- **Half-finished edits are live** (Services run from the shared tree). That's build/deploy
  discipline (AGENTS.md restart-tolerance + commit-as-checkpoint), not a coordination concern.

**Deferred (designed-for, not built):** formal `lease_*` tools backed by a per-project lease store
(`{ worker, globs[], exclusive, ttl }`) + a **git pre-commit guard** that rejects writes outside a
worker's lease, and later peer-to-peer lease negotiation between workers. The prompt convention is
forward-compatible with all of it.

## 8. Transport (reused from v0.1)

`telegram.ts` (sendMessage chunked ≤4096, editMessageText, setWebhook) and `webhook.ts`
(`node:http`, secret-path + `X-Telegram-Bot-Api-Secret-Token` verify). The only change: the webhook
no longer runs Codex — it **enqueues an `owner_message` event** and 200s immediately.

## 9. Tool surface (the manager's entire capability)

| Group | Tools |
|---|---|
| **Worker orchestration** | `subagent_start`, `subagent_send`, `subagent_steer`, `subagent_poll`, `subagent_list`, `subagent_cancel` |
| **Memory** | `memory` (native `memory_20250818` — CRUD over `/memories`, MemFS-backed) + our `memory_search`, `recall_search` |
| **Owner comms** | *(none — the manager's plain `text` is its reply; `NO_REPLY` stays silent. §4)* |

> Coordination is **prompt-only** in v0.2 (no `lease_*` tools). See §7.

All orchestration calls are **async** (return a handle; results arrive as events). No tool returns
raw file contents or full logs into the manager's context — summaries and pointers only.

## 10. Config & billing

- **Required**: `TELEGRAM_BOT_TOKEN`, `ALLOWED_USER_IDS`, `TELEGRAM_WEBHOOK_SECRET`,
  `ANTHROPIC_API_KEY`.
- **Forbidden** (billing-flip guard, unchanged from v0.1): `OPENAI_API_KEY`, `CODEX_API_KEY` —
  workers must ride the ChatGPT subscription via `CODEX_HOME`.
- **Models**: `MANAGER_MODEL` = `claude-opus-4-8`, `UTILITY_MODEL` = `claude-haiku-4-5`.
- **Anthropic betas** (set on the manager's Messages calls): `compact-2026-01-12` (compaction),
  `context-management-2025-06-27` (memory tool + context editing).
- **Paths**: `WORKSPACE_DIR` (holds project repos), `MEMORY_DIR` (manager memory repo, exposed to
  the memory tool as `/memories`), `MANAGER_STATE_DIR` (transcript/queue snapshots), `CODEX_HOME`.
- **Two billing planes**: Anthropic (metered, manager) + ChatGPT subscription (free-tier-ish,
  workers). Server-side compaction + context editing keep the metered plane cheap.

## 11. Persistence & cold-wake recovery

| State | Where | On boot |
|---|---|---|
| Core/archival/recall memory | `MEMORY_DIR` (git markdown) + FTS db | load `system/`; FTS reindex if stale |
| Working transcript (incl. server `compaction` blocks) | `MANAGER_STATE_DIR/transcript.json` | rehydrate verbatim — compaction blocks must be preserved |
| Event queue (pending) | `MANAGER_STATE_DIR/queue.json` | resume draining |
| Worker registry (ids, purpose, status) | `system/workers.md` (+ snapshot) | reconcile via `subagent_poll` (threads persist) |

Snapshots are written **after every turn** (cheap, small), so a hibernate mid-conversation loses
nothing. Memory is the *semantic* truth; snapshots are crash recovery.

## 12. Failure modes

- **Manager turn crashes** → event stays/redrives; snapshot is pre-turn; owner gets an error reply.
- **Reasoning leaks as a reply** → since plain text now reaches the owner, sloppy "think out loud"
  text would be delivered. Mitigation: adaptive `thinking` gives the model a private channel, the
  prompt forbids narration, and `NO_REPLY` is the explicit silence path. No dedup needed — there is
  only one channel, so a turn cannot both tool-send and end-turn-send the same line.
- **Worker crashes / non-zero** → `worker_event(failed, detail)`; manager decides (retry, re-scope,
  ask owner). Auth-flavored failures surface the re-login hint (as today).
- **Cold wake mid-build** → hold should prevent it; if force-killed, threads persist → re-poll.
- **Scope overlap** (no enforcement in v0.2) → manager re-scopes or serializes; git-as-checkpoint
  allows rollback if a worker strayed.
- **Anthropic rate limit / 5xx** → SDK backoff; turn re-enqueued. Dropped `compaction` blocks would
  silently lose history — the snapshot persists full `response.content` to prevent this.

## 13. Test strategy (fake everything — no real Sprite/Telegram/Codex/Claude)

Every boundary stays injectable, so the **real** runtime loop runs against fakes:
- **`fakeAnthropic`** — a scripted Messages client returning predetermined `tool_use` / text (and
  optional `compaction` blocks) so we drive deterministic manager behavior ("owner says build X →
  emit `subagent_start` → … → plain-text reply") and assert compaction-block round-tripping.
- **`fakeCodex`** — the in-process `CodexRunner` fake we already have.
- **`fakeTelegram`** — records `sendMessage`/`editMessageText` (already have).
- **Memory** — the memory-tool handlers + FTS exercised against the **real** `node:sqlite` + a
  **real tmp git repo** (fast, dependency-free, high-fidelity).
- **Sprite** — a local process; the keep-alive hold no-ops off-Sprite (already built).

Headline e2e: owner message → manager turn → `subagent_start` (parallel ×2, prompt-scoped) →
workers complete → `worker_event`s → manager narrates as plain text → Telegram; assert memory-tool writes
land in MemFS, compaction blocks round-trip, and snapshot/restore survives a simulated cold wake.

## 14. Proposed file tree / modules

> Rough proposal — names will shift in implementation. Grouped by subsystem.

```
src/
  index.ts                  # entrypoint: load config, wire deps, boot, start webhook, run loop
  config.ts                 # env + validation (require ANTHROPIC_API_KEY; forbid OPENAI/CODEX_API_KEY)
  logger.ts

  transport/
    telegram.ts             # Bot API client (sendMessage chunked, editMessageText, setWebhook)
    webhook.ts              # node:http; secret verify; enqueues owner_message; 200 immediately

  runtime/
    eventQueue.ts           # durable serialized queue (owner_message | worker_event | tick)
    loop.ts                 # drain queue → run one manager turn at a time
    hold.ts                 # Sprite keep-alive (Tasks API); lifecycle = queue ∨ worker ∨ turn
    snapshot.ts             # transcript + queue persistence; cold-wake rehydrate

  manager/
    manager.ts              # one turn: build request → @anthropic-ai/sdk tool loop → deliver
    anthropic.ts            # thin wrapper over @anthropic-ai/sdk: betas (compact-2026-01-12,
                            #   context-management-2025-06-27), preserve compaction blocks, usage
    prompt.ts               # persona + rules + system/ core-memory assembly into the system block
    hygiene.ts              # OPTIONAL idle memory consolidation/defrag (Haiku) — not compaction
    tools/
      registry.ts           # tool schemas + dispatch table (the manager's only capability)
      orchestration.ts      # subagent_start/send/steer/poll/list/cancel
      memory.ts             # native `memory` tool (betaMemoryTool→MemFS) + memory_search/recall_search

  memory/
    memfs.ts                # MemFS backend for the memory tool: /memories ↔ files + frontmatter;
                            #   system/ load; tree listing; commit-per-write
    block.ts                # block model (label/description/value/limit) + parse/serialize
    recall.ts               # summarized conversation history tier
    fts.ts                  # node:sqlite FTS5 derived index (write-through, search)

  workers/
    runner.ts               # CodexRunner over @openai/codex-sdk (moved from v0.1 codex.ts);
                            #   holds per-run AbortController for steer/cancel
    registry.ts             # active workers (id, purpose, status, abort handle); mirrors system/workers.md
    summarize.ts            # Haiku condense fallback for over-long worker output

provision/
  AGENTS.md                 # worker standing rules (Sprite rules + memory-bank + scope discipline)
  memory-bank/              # templates dropped into each new project repo
    projectbrief.md  productContext.md  systemPatterns.md
    techContext.md   activeContext.md   progress.md
  bootstrap.sh  provision.sh

test/
  fakes/
    fakeAnthropic.ts        # scripted manager tool-call sequences
    fakeCodex.ts            # in-process CodexRunner fake (have)
    fakeTelegram.ts         # records send/edit (have)
  manager.test.ts  memory.test.ts  workers.test.ts
  runtime.test.ts  e2e.test.ts  config.test.ts

DESIGN.md  SPEC.md  README.md  AGENTS.md  .env.example
```

## 15. What carries over from v0.1

| v0.1 module | v0.2 fate |
|---|---|
| `codex.ts` (`CodexRunner`) | **move** → `workers/runner.ts`; the engine behind `subagent_*` |
| `sprite.ts` (keep-alive hold) | **move** → `runtime/hold.ts`; lifecycle expanded |
| `telegram.ts` / `webhook.ts` | **keep** → `transport/`; webhook now enqueues instead of running Codex |
| `config.ts` (billing guard) | **extend** → require `ANTHROPIC_API_KEY` |
| `sessions.ts` (chatId→threadId) | **replace** → worker registry in memory + `workers/registry.ts` |
| `handler.ts` (relay loop) | **replace** → `runtime/loop.ts` + `manager/` |
| fake-injection e2e harness | **extend** → add `fakeAnthropic`, real `node:sqlite`/git |

## 16. Build sequencing (incremental, each step runnable)

1. **Memory subsystem** — MemFS backend for the native `memory` tool + `node:sqlite` FTS +
   git changelog + `memory_search`/`recall_search`, tested standalone against real sqlite/tmp-git.
2. **Manager loop skeleton** — `anthropic.ts` (`@anthropic-ai/sdk` + compaction/context-editing
   betas) + `loop.ts` + `eventQueue.ts` with a stub worker; owner message → manager →
   plain-text reply; assert compaction blocks round-trip. No real Codex yet.
3. **Worker orchestration** — wire `subagent_*` to the existing `CodexRunner`; single worker,
   async completion events, plus `subagent_steer`/`subagent_cancel` (abort + resume).
4. **Parallel workers** — multiple async workers with prompt-assigned, non-overlapping scopes
   (no formal leases yet); serialize on overlap.
5. **Durability + hygiene** — snapshots (incl. compaction blocks), cold-wake rehydrate, optional
   idle memory consolidation.
6. **Provisioning** — `AGENTS.md` + memory-bank templates on the Sprite.

## 17. Open questions / risks

- **Beta dependency** — compaction (`compact-2026-01-12`), the memory tool (`memory_20250818`),
  and context editing (`clear_tool_uses_20250919`) are Anthropic *betas*. Risk shifts from "hard to
  build" to "API surface may change"; mitigations: pin beta headers, keep `anthropic.ts` the single
  choke point, and the snapshot persists full `response.content` so a beta hiccup can't silently
  drop compacted history. (This *replaces* the former "compaction is the riskiest hand-rolled
  module" risk — we no longer hand-roll it.)
- **Decomposition quality** — partitioning work into non-overlapping scopes is a manager skill;
  bad partitions force serialization (lower throughput). Acceptable; correctness first.
- **Single owner** assumed; multi-user means per-owner memory + queues (deferred).
- **Anthropic cost** is real and ongoing; server-side compaction + context editing bound the
  metered plane, but watch token growth and `effort` per route.
- **Managed Agents (CMA) deferred** — spiritually aligned (host-side custom tools = our `subagent_*`,
  memory stores, webhooks-to-wake) but relocates the loop and memory *off* the Sprite, contradicting
  "manager runs on the box / memory is the box's durable state." Revisit only if that constraint
  relaxes.
- **`node:sqlite`** is recent (Node 22) — confirm it's available in the Sprite's Node build, else
  fall back to grep-over-markdown for search (files remain the source of truth either way).
- **`subagent_steer` is abort+resume, not true in-flight steer** — the worker loses its current
  partial turn (thread context is preserved). If live mid-turn steering becomes important, the
  upgrade is to drive workers over `codex app-server` JSON-RPC (`turn/steer`) instead of the SDK —
  a meaningful change to the worker tier; tracked against SDK issue #12329.
- **No coordination enforcement in v0.2** — scope discipline is advisory (prompt-only). A
  mis-decomposed goal can let two workers touch the same files; mitigations are good decomposition,
  serialization, and git-as-checkpoint. Formal leases + commit guard are the planned hardening.
```
