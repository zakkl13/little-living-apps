# Eval framework — Rust port design

Status: **largely implemented (2026-06-19).** Built: the production token-stat split (manager vs
worker, per backend) surfaced in `lila status` + the snapshot; the env-gated eval trace
(`LILA_EVAL_TRACE`); the deterministic grading core (`src/eval/`: transcript types, trace reader,
Node fixture, grader library) proven by `tests/eval_graders.rs`; and the binary-driven harness +
11-scenario suite + `lila-eval` CLI with the `scenario@backend` token-rich baseline, smoke-tested
hermetically by `tests/eval_harness.rs`. **Remaining:** the LLM judge (`--judge`, §6.2 cross-backend)
and worker progress notes (`worker_note`; the trace record exists, the runner just doesn't stream
them yet — forensic only, no grader needs it). The long-horizon Pocketbook scenario is also not yet
ported. A live `lila-eval --smoke` run (real subscription) is the user's to execute. Source of truth
for the original: `../sprite-codex-bot/eval/` (~2,570 LoC of TS across 10 files).

## 1. What the eval suite is (and isn't)

The deterministic tests (`cargo test`, 56 green) cover the runtime, memory, MCP, orchestrator, and the
binary-driven e2e flows. The **eval** suite measures the one thing tests can't: how the *real* manager
and *real* workers behave on ambiguous asks — quality, persona adherence, token/turn efficiency. A
trial boots the full production system; the **only** substitution is Telegram (deliveries captured for
grading instead of sent). Manager at prod effort (xhigh), real Codex/Claude workers doing real
shell/git work in a throwaway per-trial workspace, real git+sqlite memory, real Lila MCP, all on the
subscription (the billing guard applies). No `--effort/--model/--mock` knobs: an eval of a tuned-down
system measures a system you don't ship.

Three grading layers, in order of authority:
1. **Real end state** — functional graders: `node --test` green in the final workspace, HTTP probes,
   file-content matches. The work runs or it doesn't.
2. **Behavior over the timeline** — ordering/discipline: "no done-claim before a validator passed,"
   parallel starts in the first turn, chose-silence, memory writes. Outcomes and order, never "call #3
   must be tool X."
3. **Efficiency** — manager turns / worker runs / tokens. Soft budgets (`required:false`): bloat shaves
   the score, never gates.

Plus an optional **LLM judge** for soft qualities behind a per-scenario rubric (tone, scope quality),
reported separately, never gating by default.

## 2. The core decision: binary-driven, not in-process

The TS harness is **in-process**: it calls `createManagerApp({ config, runner, deliver, summarize })`,
injects a capturing `deliver` + an instrumented `runner`, then reaches into `app.mem`,
`app.telemetry.conversation()/prompts()/turns()/meter()`, `app.loop.whenIdle()`,
`app.orchestrator.whenQuiet()/running()`, `app.queue.size()` for quiescence and grading.

That shape does **not** transfer cleanly to the Rust crate as built:

| TS seam the harness uses | Rust today |
|---|---|
| injectable `deliver` fn | App hardcodes `deliver(&self.telegram, …)` — no sink seam |
| `telemetry.conversation()` (ConvMessage log) | **absent** — driver streams events, accumulates nothing |
| `telemetry.prompts()` (PromptRecord: turnId, kind, prompt) | **absent** — only a `worker_turns` count |
| runner `onProgress(note)` | **absent** — `Runner::run(RunArgs)->RunOutcome`, no callback |
| `loop.whenIdle()` / `orchestrator.whenQuiet()` | **absent** — only `Orchestrator::running()` count |

So a faithful in-process port forces invasive refactors into a *finished, tested* core: an injectable
delivery path on `App`, telemetry enrichment, a `Runner` progress callback, and a quiescence API.

**Decision: drive the compiled `lila` binary, and have it emit a structured eval trace.** This:
- Honors the project's load-bearing principle ("integration tests invokable against the compiled
  binary"; evals are the deepest such test).
- **Reuses the existing `tests/common` `FakeTelegram`** (raw-TCP HTTP/1.1) — it already scripts owner
  turns via `getUpdates` long-poll and captures deliveries as `sendMessage` POST bodies. That IS the
  "single substitution is Telegram" the eval requires, for free.
- Reads end state from disk: the real per-trial **workspace** (git repo) and **memory** (`memory_dir`
  + its FTS sqlite). Functional graders run against the real tree exactly as TS does.
- Makes the trace emitter do double duty as the **Inspector (M9) data plane** — the one deferred piece.
  Conversation, worker prompts/notes, usage, and idle markers are exactly what the Inspector wants.

The cost is the one new production surface: an **eval/inspector trace**. It's additive (gated by env,
inert otherwise), not a refactor of the delivery path. See §4.

> Considered and rejected: a pure in-process harness (more core refactors, exercises *less* of the
> real system — the model never sees the Telegram client, so binary-driving costs the measurement
> nothing) and a "drive binary but grade only from FakeTelegram + disk" harness (can't see the
> conversation log, worker prompts, worker notes, or quiescence — loses `choseSilence`,
> `parallelStartsInFirstTurn`, the judge's internal view, and reliable drain detection).

## 3. Architecture

```
eval/                              (a second [[bin]] "lila-eval" + a library module, in the same crate)
  DESIGN.md                        (this file)
  fixture.rs        // BASE_WORKSPACE (tiny dep-free Node app) + overlays + git commit
  scenarios.rs      // the scenario suite (data) + behavior regexes + selectScenarios
  checks.rs         // the grader library (deterministic, over the captured transcript + disk)
  transcript.rs     // EvalTranscript / TimelineEntry / WorkerSession / TrialReport (serde)
  trace.rs          // reader for the binary's JSONL eval trace → timeline/conversation/prompts
  harness.rs        // one trial: spawn lila run vs FakeTelegram, send turns, drain, grade
  judge.rs          // LLM-as-judge (a tool-less Codex/Claude thread, behind a rubric)
  run.rs            // the CLI: select, run N trials, aggregate, baseline diff, write reports
  baseline.json     // scenario → mean score (committed regression reference)
```

A trial (`harness::run_scenario_trial`):
1. `mkdtemp` → `workspace/`, `memory/`, `state/`, `trace.jsonl`.
2. `fixture::write_workspace(ws, overlay)` + `scenario.setup(ws)` + `git commit` (base app + planted
   bug/red test). Seed `scenario.memory` files into `memory/` (write + one commit, or via
   `lila memory create`).
3. Start `FakeTelegram` (bind :0). It serves `getUpdates` from a scripted queue and records every
   `sendMessage` body into `deliveries`.
4. Spawn the **real binary**: `lila run` with env = prod-parity config pointed at the temp dirs,
   `TELEGRAM_API_BASE_URL=http://127.0.0.1:<fake>`, `LILA_EVAL_TRACE=<trace.jsonl>`,
   `CODEX_SANDBOX_MODE` defaulted to `workspace-write` (see §6.13). No model/effort overrides.
5. For each `scenario.turns[i]`: enqueue it as a FakeTelegram update; **drain to quiescence** by
   tailing `trace.jsonl` for the `idle` marker emitted when the loop blocks with an empty queue and
   zero workers in flight (timeout at the per-trial deadline; long-horizon scenarios raise the floor).
6. On settle: `SIGTERM` the binary, wait for clean exit (this also exercises the snapshot path).
7. Build `EvalTranscript` from: FakeTelegram `deliveries`, the parsed `trace.jsonl` (timeline,
   conversation, worker prompts, worker sessions+notes, usage), and the on-disk `workspace/` +
   `memory/`. Run `checks`; optionally `judge`. Persist one `TrialReport` JSON.

Quiescence is the one subtlety binary-driving adds. The trace's `idle` marker (emitted exactly where
`drive_loop` calls `block_for_input` with `queue.is_empty() && orch.running()==0`) makes it
deterministic — strictly better than the TS sleep-and-recheck poll.

## 4. Production-crate changes required (additive, env-gated)

All gated by `LILA_EVAL_TRACE` being set; **inert and zero-cost in production** (one `Option` check),
same discipline as the existing `LILA_FAKE_BACKEND` fakes.

1. **Eval trace emitter** (`runtime/trace.rs`, new). A thin sink the loop/driver/orchestrator write
   structured records to as JSONL. Records (superset of TS `TimelineEvent` + the telemetry views):
   - `owner_msg{text}`, `delivery{text}` (mirrors FakeTelegram; reconciled on read)
   - `manager_msg{role, blocks:[text|thinking|tool_use{name,input}|tool_result{content}]}` — the
     `conversation` log (`choseSilence`, the judge)
   - `worker_prompt{turn_id, kind, prompt}` — the `workerPrompts` (`parallelStartsInFirstTurn`, judge)
   - `worker_call{call_id, prompt}` / `worker_note{call_id, note}` / `worker_done{call_id, ok, response}`
   - `usage{input,output,cached,reasoning}` per turn; `idle` quiescence marker
2. **Driver accumulates conversation.** `ManagerDriver::run_turn` already streams `BackendEvent`s; when
   tracing, also emit `manager_msg` per assistant/tool block. This is the only change touching hot
   logic; it's a write-through, no behavior change.
3. **`Runner` progress callback.** Add an optional `on_progress: Option<&ProgressSink>` to `RunArgs`
   (or a second trait method). The Codex/Claude runners already see streamed item events; forward
   them as notes. Drives `worker_note`. Also lets the **Inspector** show live worker progress later.
4. **Orchestrator/loop trace hooks.** `Orchestrator::start` emits `worker_prompt` + `worker_call`;
   the spawned task emits `worker_done`. `drive_loop` emits `idle`. All behind the trace `Option`.

These four are exactly the Inspector's needs, so none is throwaway. Telemetry's `UsageMeter` and
`TurnRecord` already exist and are read via `lila status` / the trace `usage` records.

> If we ever want the lighter path, items 2–3 (conversation + progress recording) are also what an
> in-process harness would need — so building them first keeps both doors open.

## 5. The grader library (`checks.rs`)

Port 1:1; these are the suite's value. Each `Check` is `{name, weight=1, required=true, run(&Transcript)->Outcome}`.
`baseline_checks()` (wellFormedDeliveries + noShopTalk) prepends to every scenario. Notes:

- **Regexes**: all TS patterns are `regex`-crate compatible (alternation, `\b`, `(?:…)`, `i` flag; no
  lookahead/backreference). Port `DONE_CLAIM`, `VERIFICATION_EVIDENCE`, `TECH_JARGON`,
  `READINESS_VERDICT_OR_HANDOFF` verbatim. Compile once (`once_cell`-free: build in the check ctor).
- **Functional graders** (`testsGreen`, `httpProbe`, `workspaceScript`) shell out to `node` in the
  workspace via `std::process::Command` (exit 0 = pass). **This makes `node` a host dependency of the
  eval** (not of the product) — document it; it's the same dependency TS has. The `NODE_TEST_CONTEXT`
  un-set guard is Node-test-specific and carries over.
- **Memory checks** (`memoryContains`) query the real FTS index on disk (open read-only, or
  `lila memory search`).
- **Ordering checks** (`noDeliveryUntil`, `inTurnWindow`) operate on the reconciled timeline (trace +
  FakeTelegram deliveries, merged by seq).
- **`choseSilence`** reuses the driver's `apply_no_reply` (already public) over `manager_msg` text.
- **The deterministic half is proven in `cargo test`**, not the eval — port `graders.test.ts` (311
  LoC) as a Rust test module: synthetic transcripts exercise every grader, and the fixture's planted
  realities are proven real (base app green, greet bug really 500s, version test really red) so a
  failed eval is always about the agent.

## 6. Scenario review + improvement opportunities

The 13 scenarios are well-constructed — sharp, drawn from persona-mandated behaviors, each grading
real end state plus behavior. Port them faithfully. Observations, roughly prioritized:

**Keep as-is (port verbatim):**
- `delegate-and-report`, `verify-before-done`, `absorb-noise`, `remember-fact`, `recall-fact` (the
  smoke set) — tight, cheap, each isolates one axis. `remember-fact`/`recall-fact` spawn no workers
  (cheapest live pulse). Good.
- `scope-separation`, `decompose-unprompted` — the parallel-decomposition pair; the `custom` "two
  objectives distinct" + `parallelStartsInFirstTurn` combo is the right signal.
- `make-suite-green` (don't water down the test), `match-owner-register` (non-technical register),
  `act-dont-ask` / `ask-before-publishing` (the autonomy boundary), `grounded-answers` (no
  fabrication). All sound.
- `long-horizon-build` — the 10-turn Pocketbook marathon; the suite's crown jewel and best signal.

**Improvements to the existing suite (carry into the Rust port):**

1. **Enforce a token budget, not just turns/workers.** `usageWithin` only checks `managerTurns` +
   `workerRuns` though tokens are reported and the README explicitly says doubling tokens at flat
   score is a regression. Add an optional `tokens` field to `usageWithin` (soft, `required:false`).
   The Rust rewrite can also **close the per-worker-token gap**: the codex-client-sdk surfaces
   `TurnCompleted{usage}` on *worker* runs too (the manager SDK doesn't aggregate them), so the
   instrumented runner can record per-worker tokens — `meanTokens` becomes whole-system, not
   manager-only. Concrete win the rewrite enables.

2. **Reduce judge self-preference bias.** Today a Codex judge scores a Codex manager. Allow the judge
   backend/model to differ (e.g. judge with Claude when the manager is Codex) — a flag, defaulting to
   cross-backend when both CLIs are present. Cheap, meaningfully less biased.

3. **Default trials > 1 for `--smoke`.** The README stresses "never conclude from 1 trial; pass^k is
   the honest signal," yet `--trials` defaults to 1. Default smoke to 2–3, or warn loudly at 1.

4. **Enrich the baseline.** `baseline.json` is a flat `scenario→meanScore`. Also record `passRate`,
   `meanTokens`, and `meanManagerTurns` so efficiency regressions and flakiness are visible, not just
   mean score. Keep score as the gate; surface the rest in the diff.

**Coverage gaps worth a new scenario (additive; do after parity):**

5. **Honest failure** (honesty axis): plant a bug the worker genuinely *can't* fix in one shot (or a
   flapping test). Grade that the manager reports the failure honestly and does **not** emit a
   `DONE_CLAIM` — the inverse of `verify-before-done`. The current suite only tests honesty when work
   succeeds.

6. **Memory conflict / update** (memory axis): seed a remembered fact, then have the owner contradict
   it ("actually, we moved to Postgres"). Grade that memory is *updated* (old fact gone/superseded,
   new fact recalled), not merely appended. Tests the write-discipline tests can't.

7. **Vision / image intake** (the Rust app supports photo turns; FakeTelegram can serve a photo
   update): owner sends a screenshot of a broken page; grade the manager acts on what the image shows.
   Exercises a real production path no scenario covers.

8. **Interruption / concurrency** (reply-discipline): owner sends a second ask while a worker is still
   running. Grade serialized-loop correctness *and* that the manager doesn't drop or conflate them.

**Substrate note (no change recommended):** the Node fixture vs Rails-production mismatch is documented
and accepted — the graded behaviors are substrate-agnostic and Node is the cheapest dep-free HTTP app.
Keep it; it means the eval host needs `node`, which is fine. Porting the fixture to Rust/Python would
add per-trial cost for no measurement gain.

## 7. Safety changes for the Rust eval

- **Invert the sandbox footgun.** TS defaults workers to `danger-full-access` (prod parity) and asks
  you to remember `CODEX_SANDBOX_MODE=workspace-write` on a laptop. The Rust eval should **default to
  `workspace-write`** and require an explicit `--prod-sandbox` (or a disposable-host marker file) to
  opt into full access. Same prod knob, safer default — evals usually run on dev boxes.
- **Billing guard already applies** (the harness spawns the real binary, which refuses to start with a
  flip-key set and strips it from worker env). Keep the preflight warning for `OPENAI_API_KEY` etc.

## 7.5 Live validation findings (2026-06-20, smoke)

First live runs (codex + claude, smoke) surfaced three things:

- **codex-client-sdk 0.107 vs codex CLI 0.141 mismatch** — the CLI emits `status: "in_progress"` for
  in-flight file patches; the SDK's `PatchApplyStatus` enum (Completed/Failed only) can't parse it, so
  **every codex worker run failed** (`unknown variant 'in_progress'`) — in production too, not just
  eval. No CLI-matched SDK exists on crates.io, so the SDK is vendored at `vendor/codex-client-sdk`
  with the variant added (via `[patch.crates-io]`). Fixed.
- **Persona-vs-fixture confound** — the worker standing rules ([agents.rs:40](../src/workers/agents.rs))
  and manager prompt assert "this app is a **Rails 8** app" (production-correct), but the original eval
  fixture was **Node**. On `delegate-and-report`, **codex took the objective literally and scaffolded a
  whole Rails 8.1.3 app** (~1M worker tokens) → `/health` 404 on the Node grader; **claude detected the
  mismatch and adapted** to the real `server.js` → `/health` 200, in ~12k tokens total.
  **RESOLVED (E5, 2026-06-20):** the fixture is now a real pre-scaffolded **Rails 8 app** (`Substrate::Rails`,
  `eval/fixtures/rails-app`, built once by `setup-rails.sh`, copied per trial via APFS clone — instant),
  with Rails graders (`bin/rails test`, boot-puma-and-probe) and the planted bugs ported to Rails
  (proven in `tests/eval_rails.rs`). Worker scenarios now run on the substrate the persona asserts;
  the worker-free memory scenarios stay on the cheap Node track. Toolchain was already present
  (Homebrew Ruby 4.0.5 + Rails 8.1.3).
- **Backend differences (same tasks):** see the table below. codex ≈ 7× claude's manager tokens on
  trivial memory tasks (xhigh reasoning default) and ~87× total on the worker task (Rails scaffold);
  claude is far cheaper and adapted correctly, but leaked shop talk to the owner ("the worker
  reported back…") and over-narrated internals — codex kept replies clean. Both reached score 0.86 on
  `delegate-and-report` but for OPPOSITE reasons (codex failed the functional probe, claude failed the
  no-shop-talk discipline check) — the per-check breakdown is what tells them apart.

## 8. Milestones

- **E1 — Trace + seams** (production crate, §4): trace emitter, driver conversation write-through,
  `Runner` progress, orchestrator/loop hooks. Prove with a unit test that a fake-backend `lila run`
  emits a well-formed trace. *Gates everything; also unblocks Inspector M9.*
- **E2 — Fixture + graders + graders test**: port `fixture.rs`, `checks.rs`, `transcript.rs`, and the
  `graders.test.ts` → Rust `cargo test`. Fully deterministic, no subscription. Proves the planted
  realities and every grader before a single live token is spent.
- **E3 — Harness + scenarios + run CLI**: `harness.rs` (spawn binary vs FakeTelegram, drain via trace
  `idle`), `scenarios.rs` (all 13), `run.rs` (select/trials/aggregate/baseline). First live `--smoke`.
- **E4 — Judge + baseline**: `judge.rs`, `--judge`, bless `baseline.json`, wire `--strict` regression
  diff. Cross-backend judge default (§6.2).
- **E5 — New scenarios + token budgets** (§6.1, §6.5–6.8): the additive improvements, once parity holds.

E1–E4 reach parity with the TS suite; E5 is the upside the rewrite unlocks.
