# Evals — measuring the non-deterministic part

`npm test` covers everything **deterministic**: the runtime (loop, memory, MCP handlers,
orchestrator) *and the eval machinery itself* — graders, the workspace fixture's planted realities,
suite invariants (`eval/graders.test.ts`). Evals exist solely to measure what tests can't: how
the **real manager and real workers behave** — does the agent perform well on ambiguous asks, does
it exhibit the persona's desired behaviors, and is it token/turn efficient?

So a trial boots the **full production system** — no fakes, no knobs:

```
owner turn ──▶ REAL queue/loop ──▶ REAL Codex manager thread ──▶ REAL Lila MCP (memory_*, subagent_*)
                                        │                              │
   captured deliveries ◀────────────────┘              REAL Codex workers (production runner,
                                                       instrumented for the timeline only) doing
                                                       real shell/file/git work in a real per-trial
                                                       workspace (eval/fixture.ts)
```

The single substitution is Telegram: deliveries are captured for grading instead of sent.
Everything else is exactly what's deployed — manager at prod effort (xhigh) and SDK-default model,
workers via the production `createCodexRunner`, real git+sqlite memory, prod worker sandbox.
Everything rides the ChatGPT subscription (manager, workers, optional judge). No API key, no
metered plane — the repo's billing guard applies here too.

**There are deliberately no `--effort`/`--model`/`--mock` flags.** An eval run at non-prod settings
measures an agent you don't ship.

## ⚠️ Real workers, real shell

Workers run real shell commands. The sandbox defaults to **prod parity**
(`danger-full-access` — in production the disposable host IS the security boundary). The runner
warns about this at startup. On a non-disposable laptop:

```bash
CODEX_SANDBOX_MODE=workspace-write npm run eval -- --smoke
```

That's the existing production knob passed through, not an eval-only invention — workers are then
confined to the throwaway temp workspace.

## Running

```bash
npm run eval -- --smoke                  # representative subset (cheapest live pulse)
npm run eval                             # full live suite (real workers — budget an hour+)
npm run eval -- --filter remember        # one scenario (remember-fact is the cheapest: no workers)
npm run eval -- --axis validation --trials 3   # focus one behavior, 3 trials each (pass^k)
npm run eval -- --judge                  # add rubric scoring by a Codex judge (soft qualities)
npm run eval -- --keep-tmp               # keep trial workspaces on disk for forensics
npm run eval -- --list                   # show the suite
```

Live runs need a Codex login (`codex login`; auth read from `~/.codex` or `$CODEX_HOME`).
Each run writes `eval/results/<run-id>/` (gitignored): `report.md`, `report.json`, and one
**trial report** JSON per trial. **Read the trial reports of failed runs**; that's where the
insight is.

## The trial report (`<scenario>.t<n>.json`)

Each trial persists ONE self-contained, versioned record (`schema: "lila-eval-trial@1"`, the
`TrialReport` type in `eval/types.ts`) designed so a review UI can render a trial from the file
alone:

| section | contents |
|---|---|
| `scenario` | name, axis, description, the owner turns sent, rubric, planted overlay/memory seeds |
| `settings` | model / effort / sandbox the trial ran at (always prod parity, recorded anyway) |
| `pass` / `score` / `checks` | grading: every check with required/weight/pass + evidence detail |
| `judge` | the LLM judge's `{score, reasoning}`, `{error}` if it failed, `null` if not judged |
| `ownerMessages` / `deliveries` | both ends of the owner channel, in order |
| `timeline` | the seq/at-stamped master event stream; worker events carry a `callId` for lane-splitting |
| `conversation` | the manager's reconstructed thread: text (incl. NO_REPLY), thinking, tool_use/tool_result |
| `managerTurns` | per-turn envelopes: what opened the turn, iterations, the four token counters |
| `workerPrompts` | dispatches as the manager wrote them (raw objective, turn- and worker-stamped) |
| `workerSessions` | one per worker run: stripped + full prompt (protocol incl.), live notes (timestamped), final report, ok, Codex thread id, timing |
| `usage` | cumulative manager tokens + turn/worker counts (known gap: the SDK reports no per-worker tokens) |
| `memory` / `workspace` | end state: memory tree + system files, workspace file listing + dir |

`timeline` is the master ordering; `workerSessions[].callId` ↔ `timeline` worker events, and
`workerSessions[].prompt` ↔ `workerPrompts[].prompt` are the join keys (so a UI can attach a
session to the manager turn that launched it via `workerPrompts[].turnId`).

## What gets graded

Three layers, in order of authority:

1. **Real end state** — the honest functional graders: `testsGreen` (the fixture app's own
   `node --test` suite in the final workspace), `httpProbe` (boot `server.js`, assert an HTTP
   status), `workspaceFileMatches`, fs checks. The work either runs or it doesn't.
2. **Behavior over the timeline** — ordering and discipline: `noDeliveryUntil(DONE_CLAIM, …)`
   ("don't tell the owner it's done before a validator PASSed"), `parallelStartsInFirstTurn`,
   `choseSilence`, `noShopTalk`, memory checks. Outcomes and order, never "call #3 must be tool X".
3. **Efficiency** — every trial reports manager turns / worker runs / tokens (in the console
   summary as `Xmt/Yw/Ztok`, and in `report.md`). Most scenarios carry a soft `usageWithin` budget:
   over-budget **shaves the score but never gates** (`required: false`). Same outcome in fewer
   turns is better; bloat shows up as a score dip, not a red ✗.

The Codex judge (`--judge`) only scores scenarios carrying a `rubric` — soft qualities like scope
quality and outcome-language — reported separately, never gating. Judges drift and reward
verbosity; calibrate rubrics against transcripts you've scored yourself.

## The workspace fixture

Each trial seeds a real git repo containing a deliberately tiny, dependency-free Node HTTP app
(`eval/fixture.ts`): `server.js`, a `node --test` suite, README. Scenarios overlay files to plant
**real** bugs (`GREET_BUG_OVERLAY` makes `GET /greet` genuinely 500) or red tests
(`VERSION_TEST_JS`), and may mutate the tree imperatively via `setup()` (e.g. backdating mtimes).
The planted realities are themselves proven by unit tests — the bug really 500s, the red test is
really red, the base suite is really green — so a failed eval is always about the agent.

Why Node and not Rails: production's substrate (Rails 8 + systemd) needs a provisioned Linux host
and minutes of scaffolding per trial. The behaviors this suite grades — delegation, validation,
reply discipline, memory, autonomy, honesty, efficiency — are substrate-agnostic, and real workers
orient off the actual workspace (the worker protocol makes them look before acting). Known
mismatch, accepted: the manager persona mentions Rails/systemd/port 3000; transcripts show the
workers correcting course off the real tree.

## The behavior axes

Scenarios are tagged with the behavior they exercise — these mirror the persona in
`src/manager/prompt.ts`, which is what you're tuning:

| axis | desired behavior |
|---|---|
| `delegation` | hands real work to subagents, scopes them, acks and lets go |
| `validation` | independently verifies user-visible work (separate validator, PASS/FAIL) before claiming done |
| `reply-discipline` | NO_REPLY on noise events, no narration, one outcome report, no shop talk, matches the owner's technical register |
| `memory` | writes durable facts down; answers from memory instead of guessing or re-delegating |

The memory axis includes **`long-horizon-build`**, the suite's one marathon: 10 owner turns
building a real notes app (Pocketbook) in a single ongoing conversation. Standing conventions are
stated once in turn 1 and probed indirectly for the next 9 ("you know where it should live"), a
mid-series recall question must be answered from memory without re-delegating, and a late-arriving
reply preference must stick. It's graded with the same end-state checks (the app must actually
work end to end) plus per-turn-window assertions (`inTurnWindow`). Budget **60–90 min** per trial;
it declares its own `timeoutMs` floor and is deliberately not in `--smoke`.
| `autonomy` | acts on inferable requests; escalates only genuinely owner-only calls (e.g. publishing) |
| `honesty` | never fabricates state it can't know; grounds answers in worker reports |

## The optimization loop

1. `npm run eval -- --update-baseline` once to bless current scores into `eval/baseline.json`
   (committed — it's the regression reference).
2. Edit the thing you're tuning — usually `src/manager/prompt.ts` (persona / how-you-work /
   validation discipline), sometimes the tool descriptions in `src/manager/mcp/tools.ts`.
3. `npm run eval -- --axis <the-behavior> --trials 3` while iterating; full run before blessing.
4. The summary diffs every scenario against the baseline (📉 flags drops > 0.05; `--strict` makes
   regressions exit non-zero). Improve the axis you're after **without** regressing the others —
   and watch the efficiency columns: a prompt change that keeps scores flat but doubles tokens is
   a regression too. Then `--update-baseline` again.

Because trials are nondeterministic, never conclude from 1 trial; `--trials 3` and pass^k (the
`pass` column requires *all* trials green) is the honest signal.

## Adding a scenario

In `eval/scenarios.ts`:

```ts
{
  name: "my-behavior",
  axis: "reply-discipline",
  description: "…(shown to the judge; say what desired behavior this exercises)",
  memory: { "/memories/archival/notes/x.md": "---\ndescription: …\n---\n…" }, // optional seed
  workspace: { "server.js": SOME_OVERLAY },   // optional: plant a real bug / red test
  setup: (dir) => { /* optional imperative tree mutation before the fixture commit */ },
  turns: ["the owner message"],               // harness drains the full cascade after each
  timeoutMs: 5_400_000,                       // optional: per-trial floor for long-horizon scenarios
  checks: [...baselineChecks(), delivered(/…/i), httpProbe("/x", 200), usageWithin({ managerTurns: 4 })],
  rubric: "…",                                // optional, for --judge
  smoke: true,                                // optional: include in --smoke
}
```

Then prove the deterministic half **before** burning a live run: add the overlay's reality to
`eval/graders.test.ts` (does the planted bug really reproduce? does the fix-check start red?)
and run `npm test`. Keep regexes tight but ack-safe (see `DONE_CLAIM` — "I'll get it done" must not
trip a completion-claim check; that's unit-tested).

## Design choices (and the practices behind them)

- **Evals measure only what tests can't.** Anything checkable deterministically belongs in
  `npm test` — including the graders and fixture themselves. A live trial spends real model time
  only on the live question: behavior, judgment, efficiency.
- **Full production parity, no fidelity knobs.** Real manager thread, real workers via the
  production runner (instrumented only to record the timeline), prod effort/model/sandbox. The
  manager⇄worker interplay is the thing under test — faking either side voids the measurement.
- **Small, sharp suite** (12 scenarios) drawn from persona-mandated behaviors, not hundreds of
  synthetic cases. When the live bot misbehaves on the host, distill that transcript into a new
  scenario — real failures are the best eval cases.
- **Grade outcomes and state, not tool paths.** What the owner saw, what landed in memory, what the
  workspace actually does now, in what order things happened. Agents that find a smarter route
  still pass.
- **Code graders first, judge second; efficiency soft.** Deterministic checks gate; the judge and
  the usage budgets only shade the picture.
- **Isolation per trial**: fresh temp memory/state/workspace and a fresh manager thread every
  trial; nothing leaks between trials or into the real deployment.
- **Zero new dependencies**: the harness reuses the app's own seams (`runner`, `deliver`,
  `createManagerApp`), node:util `parseArgs`, and the already-present Codex SDK for the judge.

## Caveats

- A live trial with real workers takes **minutes** (default per-trial budget 1800s). The cheapest
  scenarios are the memory ones (no workers); `--smoke` is the cheapest representative set.
- The baseline is only meaningful across runs of the same suite at the same (prod) settings.
- The host-side `allowReply` gate suppresses some model chattiness before it reaches "Telegram", so
  delivery-count checks measure the *system*; `choseSilence()` reads the conversation log and
  measures the *model*. Both matter; know which one a check is asserting.
- `looksLikeValidation` is a heuristic over the manager's protocol-stripped prompts. If the manager
  phrases a builder objective like a validation one, a dispatch-classification check can misread it
  — check the transcript before blaming the model.
