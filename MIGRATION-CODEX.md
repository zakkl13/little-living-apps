# Migration: manager brain Opus → Codex/GPT (v0.3)

Status: **IMPLEMENTED** (the Phase 0 spike was skipped — wiring proven directly in the real code +
`test/mcp.test.ts`).

> **Superseded in part (2026-06): purely ephemeral workers.** After this migration shipped, the
> worker tier was cut to single-shot: `subagent_start` is the only orchestration tool (`_send`,
> `_steer`, `_cancel`, `_list` deleted along with the registry and the `system/workers.md` roster
> mirror), and the snapshot is **v4** `{managerThreadId, queue, usage}` — no worker records.
> References below to those tools, the roster mirror, and snapshot v3 are historical. This supersedes the Anthropic-backed manager described in DESIGN.md §4. Working
rules from the project still apply: **rip out prior code (no compat shims), commit straight to
`main`, the host is the security boundary, and the bot refuses to start if
`OPENAI_API_KEY`/`CODEX_API_KEY` is set.**

What shipped: `src/manager/managerCodex.ts` (locked-down Codex thread factory), `src/manager/driver.ts`
(`ManagerDriver` — one streamed turn), `src/manager/mcp/` (the loopback Lila MCP server: memory +
subagent tools), `src/manager/backend.ts` (assembles AGENTS.md + MCP + driver), `prompt.ts` split into
static `AGENTS.md` + per-turn context header, snapshot **v3** (`{managerThreadId, queue, workers}`),
owner-photo intake (`view_image` → `local_image`), and the config/env diff. Deleted: `manager/anthropic.ts`,
`manager/manager.ts`, `manager/tools/*`, the `@anthropic-ai/sdk` dependency, and all Anthropic env keys.
The Inspector (off by default) now reads a reconstructed conversation log from the Codex item stream
instead of the deleted `ModelMessage[]` transcript.

## 1. Goal & the one constraint

Cut the manager 100% to Codex so there is **no metered plane left** — manager *and* workers ride
the single ChatGPT subscription. The cost story collapses to "the subscription."

The constraint that shapes everything: **ChatGPT-subscription billing is only reachable through the
Codex agent harness (CLI/SDK)** — there is no raw GPT completions endpoint that bills against the
subscription. So we cannot keep the `createMessage` tool-loop and swap the model id. **The manager
itself becomes a long-lived Codex thread.** The worker side already proves this path.

Manager intelligence target: the strongest Codex model with `model_reasoning_effort = "xhigh"`.

## 2. Research conclusion — the intended Codex sub-agent pattern

Codex has **no native sub-agent / agent-as-tool primitive**; `Thread` is single-agent. The
supported composition blocks are:

1. **Host-driven Threads** — your process owns multiple `Thread`s and coordinates them. This *is*
   the SDK's multi-agent story and is exactly what lilapps already does (the host process owns
   worker Threads via `createCodexRunner`). **Unchanged.**
2. **MCP custom tools** — to let an *agent* trigger that orchestration from inside its reasoning,
   expose tools over MCP; the handler (in our host process) does the Thread work. Both stdio and
   streamable-HTTP MCP are first-class (`codex mcp add --url … --bearer-token-env-var …`).
3. **Codex-as-MCP-server** (`codex mcp-server`) — Codex-calls-Codex, but the subordinate is a
   *generic* Codex with none of our registry/steer/summarize/snapshot semantics. **Rejected** for
   the worker layer; it throws away the Orchestrator.

**Decision:** keep the Orchestrator in the host process and bridge only the *trigger* to the
manager through a thin **in-process streamable-HTTP MCP server** whose handlers call the live
`Orchestrator` and `MemFs`. HTTP (not stdio) because the handlers must reach in-process state a
stdio child couldn't without extra IPC.

## 3. Architecture

```
Telegram long-poll ─► event queue ─► serialized loop ─► Manager Codex thread (one, long-lived)
   (owner msgs,                         (one turn at a    model=<best>, reasoning=xhigh
    worker events)                       time, unchanged)  sandbox=read-only, network=off
                                                           shell_tool=off, web_search=off
                                                           view_image=ON
                                                           AGENTS.md = static persona/rules
                                                           tools = Lila MCP (loopback HTTP) ─┐
                                                                                             │
                       ┌─────────────────────────────────────────────────────────────────────┘
                       ▼
            Lila MCP server (in-process, 127.0.0.1, bearer token)
              ├─ memory_view / _create / _str_replace / _insert / _delete / _rename → MemFs
              ├─ memory_search / recall_search                                       → MemFs FTS
              └─ subagent_start / _send / _steer / _cancel / _poll / _list           → Orchestrator
                                                                                          │
                                                                                          ▼
                                                              Worker Codex threads (UNCHANGED)
```

What **does not change**: `runtime/loop.ts`, `runtime/eventQueue.ts`, `memory/*`,
`workers/*` (runner, orchestrator, registry, summarize, protocol), `transport/*`. The migration
touches only the manager's brain and its tool envelope.

## 4. The manager Codex thread

Driven through a new `ManagerDriver` (replaces `manager/manager.ts` + `manager/anthropic.ts`).

**Construction** (`CodexOptions` + `ThreadOptions`, via the SDK's `config` passthrough):

```
new Codex({
  env: sanitizedEnv(),                       // strips OPENAI_API_KEY/CODEX_API_KEY (reused from runner)
  config: {
    model_reasoning_effort: "xhigh",
    features: { shell_tool: false },         // no shell/exec
    tools: { web_search: false, view_image: true },
    web_search: "disabled",
    mcp_servers: { lila: {
      url: "http://127.0.0.1:<port>/mcp",
      bearer_token_env_var: "LILA_MCP_TOKEN",
      default_tools_approval_mode: "approve",
    } },
  },
})

thread = codex.startThread({
  model: config.managerModel,                // strongest Codex model
  workingDirectory: <managerDir with AGENTS.md>,
  skipGitRepoCheck: true,
  sandboxMode: "read-only",
  networkAccessEnabled: false,
  webSearchEnabled: false,
  approvalPolicy: "never",
})
```

**Capability boundary (user decision: strip to the best of our ability; residual read-only is OK).**
`shell_tool=off` + `web_search=off` + read-only sandbox + network off leaves the manager operating
*only* through its MCP tools — a near-perfect reconstruction of DESIGN §4's "the tool list is the
boundary," now configured rather than structural. `view_image` stays **on** so the manager can see
owner-sent screenshots (see §8). Any tool that can't be toggled off (e.g. `apply_patch`) is
neutralized by the read-only sandbox.

**One turn** (replaces `runManagerTurn`): resume the persisted manager thread, `runStreamed(input)`,
and stream events:

| Codex event/item            | Action                                                            |
|-----------------------------|-------------------------------------------------------------------|
| `agent_message` item        | deliver to Telegram, honoring the `NO_REPLY` sentinel (reused)    |
| `mcp_tool_call` item        | internal (manager calling memory/orchestration) — log only        |
| `reasoning` item            | private — never delivered                                         |
| `turn.completed.usage`      | `telemetry.recordUsage` (input/output/cached/reasoning tokens)    |
| `turn.failed` / `error`     | `friendlyError` (reused from runner)                              |

The async worker model is preserved verbatim: `subagent_start` (an MCP tool) returns immediately,
the manager's turn ends with an ack, the worker runs in the host Orchestrator, and its completion
enqueues a `worker_event` that drives the next manager turn. No new concurrency.

## 5. Lila MCP server (`src/manager/mcp/`)

In-process streamable-HTTP server (add `@modelcontextprotocol/sdk` as a direct dep — already a
transitive dep of codex-sdk). Binds `127.0.0.1:<port>`, path `/mcp`, requires
`Authorization: Bearer <LILA_MCP_TOKEN>` (random per boot; defense-in-depth on loopback, mirrors the
Inspector token).

- **Memory tools** map onto `MemFs` — discrete tools (`memory_view`, `memory_create`,
  `memory_str_replace`, `memory_insert`, `memory_delete`, `memory_rename`) adapting their args to the
  existing `MemoryCommand` union and calling `mem.execute(...)`, plus `memory_search` /
  `recall_search` over `mem.search` / `mem.recallSearch`. (Discrete tools read better for Codex than
  one polymorphic command tool; the adapter is thin. Single-tool fallback available if needed.)
- **Orchestration tools** (`subagent_start/_send/_steer/_cancel/_poll/_list`) reuse the existing
  handler bodies from `orchestration.ts` verbatim against the live `Orchestrator`. Prompt telemetry
  is stamped with the active turn id tracked by the `ManagerDriver`.

The existing Anthropic `ToolSpec`/`buildRegistry`/`dispatch` envelope is deleted; the handler
*logic* is preserved.

## 6. Instructions: static AGENTS.md + per-turn context header

`buildSystemPrompt` splits in two:

- **Static → `AGENTS.md`** (written to the manager working dir at startup): persona, "how you work"
  (including what to expect from self-validating workers), runtime facts, and a new short "your
  tools" section telling it to use the memory + subagent MCP tools and that an ordinary message goes
  straight to the owner / `NO_REPLY` for silence. Codex reads `AGENTS.md` from the working directory
  per session.
- **Volatile → per-turn prompt prefix**: the always-loaded core memory (`system/` bodies, which
  include the `system/workers.md` roster mirror) + the archival index. Kept compact; prepended to
  each event's input so the manager never operates without its standing context. `mirrorWorkers`
  stays as-is. (If long-thread bloat shows up, optimize to inject-on-change; rely on Codex
  compaction for v1.)

## 7. Durability — big simplification (`runtime/snapshot.ts`)

Codex persists each thread's rollout on disk (`CODEX_HOME/sessions`) and runs its own context
compaction, so we stop snapshotting the `ModelMessage[]` transcript and compaction blocks entirely.

Snapshot v3 = `{ version: 3, managerThreadId, queue, workers, cost? }`. Cold restart =
`resumeThread(managerThreadId)`. The `Transcript` abstraction and the transcript/compaction logic in
`snapshot.ts` are deleted. `/new` becomes "start a fresh manager thread" (drop `managerThreadId`) —
working context cleared, memory kept. **Cutover note:** the first start after deploy has no
`managerThreadId`, so it begins a fresh thread seeded by persisted memory; the prior Opus transcript
is discarded (acceptable — memory is the semantic truth).

## 8. Owner images (view_image on)

`view_image` is enabled, so make it useful: extend `ingestTelegramUpdate` to accept `photo`
messages — download the largest size via Telegram `getFile`, save to a temp path, and open the turn
with `[{type:"text", text:caption}, {type:"local_image", path}]` instead of plain text. The config
flag is harmless on its own; this intake is what gives the manager eyes on owner-sent screenshots.

## 9. Config / env diff (`config.ts`, `.env.example`)

Remove: `ANTHROPIC_API_KEY` (the last required key — gone), `ANTHROPIC_BASE_URL`, the
`claude-opus-4-8` default, `INSPECTOR_PRICE_IN/OUT` semantics (now flat).

Add: `MANAGER_MODEL` (strongest Codex model — **verify the exact id on the host**),
`MANAGER_REASONING_EFFORT` (default `xhigh`), `MANAGER_DIR` (working dir holding AGENTS.md, default
under `MANAGER_STATE_DIR`), `LILA_MCP_PORT` (loopback), `LILA_MCP_TOKEN` (auto-generated if unset).
Optionally split `WORKER_MODEL`/`WORKER_REASONING_EFFORT` from the manager's.

**Keep** the `OPENAI_API_KEY`/`CODEX_API_KEY` start-guard — now the *only* billing protection.

## 10. Deletions (no-compat rule)

`src/manager/anthropic.ts`; the `createMessage` loop + `Transcript` in `src/manager/manager.ts`;
`src/manager/tools/{registry,memory,orchestration,types}.ts` as Anthropic tool modules (logic
re-homed into the MCP server); transcript/compaction snapshot fields; `@anthropic-ai/sdk` dependency.
Rewrite the manager sections of DESIGN/SPEC/README.

## 11. Inspector

Off by default, so a follow-up. Token usage now comes from `turn.completed.usage`; cost becomes flat
(subscription) rather than per-token. The transcript view reads the streamed item log (or the Codex
rollout) instead of the `ModelMessage[]` snapshot.

## 12. Risks & the Phase 0 spike (the gate)

Open unknowns, all answered by one throwaway spike before touching the real manager:

1. HTTP-MCP wiring through `CodexOptions.config` (server discovered + tool called).
2. `features.shell_tool=false` honored by the **SDK-resolved binary** (host runs the SDK's vendored
   codex; local CLI is 0.125 vs SDK 0.137 — check the live version).
3. `agent_message` → reply + `NO_REPLY` round-trip; MCP tool calls not blocked by read-only sandbox
   / network-off.
4. A long-lived resumed thread compacting acceptably over the app's lifetime (else periodic
   re-seed from memory).

**Spike:** a standalone script that runs a one-tool echo HTTP-MCP server, starts a Codex thread with
the §4 config, and confirms (a) the thread calls the MCP tool, (b) the manager can't shell out,
(c) the final `agent_message` is delivered. Pass = green light.

## 13. Phased implementation plan

- **Phase 0 — spike** (above). The gate; converts the 4 risks into facts.
- **Phase 1 — Lila MCP server** (`src/manager/mcp/`): memory + subagent tools over HTTP, bearer
  auth, handlers calling live `MemFs`/`Orchestrator`. Unit-test each tool against fakes.
- **Phase 2 — manager-as-Codex** (`ManagerDriver`): resumable thread, stream→deliver, `NO_REPLY`,
  AGENTS.md (static) + per-turn header (volatile). Delete `anthropic.ts` + the Anthropic loop.
  Rewire the `runTurn` seam in `app.ts`.
- **Phase 3 — durability + config**: snapshot v3 (`managerThreadId`); `/new` → fresh thread;
  `config.ts`/`.env.example` diff; keep the API-key guard; drop `@anthropic-ai/sdk`.
- **Phase 4 — owner images**: Telegram photo intake → `local_image` inputs.
- **Phase 5 — Inspector + docs**: usage from `turn.completed`, flat cost; rewrite DESIGN/SPEC/README.

## 14. Test plan

`npm run typecheck` + `npm test` stay green throughout. New/updated tests: Lila MCP tool handlers
(memory ops + subagent dispatch against fakes); `ManagerDriver` against a **fake Codex** (scripted
event stream) asserting delivery + `NO_REPLY` + usage recording — the same seam-injection discipline
the current fake-Anthropic tests use; snapshot v3 round-trip; config validation (guard intact,
Anthropic keys gone). Then the Phase 0 spike re-run against the **live host's** codex binary before
deploy (`dogfooding/`), since `features.shell_tool` honoring is binary-version-sensitive.
</content>
</invoke>
