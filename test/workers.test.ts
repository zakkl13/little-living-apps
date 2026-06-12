// The async worker orchestrator over the fake CodexRunner — purely ephemeral lifecycle: start
// returns immediately, the single-shot run settles into exactly ONE worker_event (objective
// attached, so the event is self-describing), failures surface as failed events, over-long output
// is condensed. No registry, no resume, no steer. Plus the runner-util tests carried over from v0.1.

import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import type { ThreadItem } from "@openai/codex-sdk";

import { formatItem, friendlyError } from "../src/workers/runner.js";
import { createOrchestrator } from "../src/workers/orchestrator.js";
import { clipSummarizer } from "../src/workers/summarize.js";
import { makeFakeCodex } from "./fakes/fakeCodex.js";

interface EmittedEvent {
  workerId: string;
  objective: string;
  status: "completed" | "failed";
  summary: string;
}

function harness(opts: { summarizeLimit?: number } = {}) {
  const runner = makeFakeCodex();
  const events: EmittedEvent[] = [];
  const orch = createOrchestrator({
    runner,
    workspaceDir: "/workspace",
    emitEvent: (e) => events.push(e),
    // Comfortably above WORKER_PROTOCOL's length: the fake runner echoes the whole prompt back,
    // and these tests assert the objective (which rides after the preamble) survives the clip.
    summarize: clipSummarizer(opts.summarizeLimit ?? 4000),
  });
  return { runner, events, orch };
}

describe("worker orchestrator (ephemeral single-shot lifecycle)", () => {
  it("start returns immediately, then emits exactly one completed worker_event", async () => {
    const { orch, events, runner } = harness();
    const { id } = orch.start("work only within src/telegram/**", "proj");
    assert.equal(orch.running(), 1, "run is in flight, the turn is not blocked");
    assert.equal(events.length, 0, "no event yet");

    await orch.whenQuiet();
    assert.equal(events.length, 1, "one run, one event — then the worker is gone");
    assert.equal(events[0]!.workerId, id);
    assert.equal(events[0]!.objective, "work only within src/telegram/**");
    assert.equal(events[0]!.status, "completed");
    assert.match(events[0]!.summary, /work only within src\/telegram/);
    assert.equal(orch.running(), 0, "nothing tracked after settle");
    assert.equal(runner.calls.length, 1);
    assert.match(runner.calls[0]!.prompt, /work only within src\/telegram/, "objective dispatched");
  });

  it("a failed worker run emits a failed worker_event with the objective attached", async () => {
    const { orch, events } = harness();
    orch.start("WORKER_FAIL — this build breaks");
    await orch.whenQuiet();
    assert.equal(events.length, 1);
    assert.equal(events[0]!.status, "failed");
    assert.equal(events[0]!.objective, "WORKER_FAIL — this build breaks");
    assert.match(events[0]!.summary, /build error/);
  });

  it("hard-clips over-long worker output at the limit", async () => {
    const { orch, events } = harness({ summarizeLimit: 100 });
    orch.start("LONG_OUTPUT — produces a wall of text");
    await orch.whenQuiet();
    assert.ok(events[0]!.summary.length < 200, "long output was condensed");
    assert.match(events[0]!.summary, /truncated/);
  });

  it("parallel starts get distinct ids and settle independently", async () => {
    const { orch, events, runner } = harness();
    const a = orch.start("alpha — scope src/api/**");
    const b = orch.start("beta — scope test/**");
    assert.notEqual(a.id, b.id);
    assert.equal(orch.running(), 2);

    await orch.whenQuiet();
    assert.equal(orch.running(), 0);
    assert.equal(events.length, 2, "one event per run, nothing lingers");
    assert.equal(runner.calls.length, 2);
    assert.deepEqual(new Set(events.map((e) => e.workerId)), new Set([a.id, b.id]));
  });
});

// ---- runner utilities carried over from v0.1 (formatItem / friendlyError) ----

describe("formatItem", () => {
  it("renders a command execution as a single $-prefixed line", () => {
    const item = { type: "command_execution", command: "echo  hi\n " } as unknown as ThreadItem;
    assert.equal(formatItem(item), "$ echo hi");
  });
  it("summarizes file changes and pluralizes", () => {
    const one = { type: "file_change", changes: [{}] } as unknown as ThreadItem;
    const two = { type: "file_change", changes: [{}, {}] } as unknown as ThreadItem;
    assert.equal(formatItem(one), "✏️ 1 file changed");
    assert.equal(formatItem(two), "✏️ 2 files changed");
  });
  it("skips agent_message (handled separately)", () => {
    assert.equal(formatItem({ type: "agent_message", text: "hi" } as unknown as ThreadItem), undefined);
  });
});

describe("friendlyError", () => {
  it("adds a re-auth hint when the error looks auth-related", () => {
    assert.match(friendlyError("401 unauthorized"), /codex login/);
  });
  it("flags a usage/rate limit with actionable advice", () => {
    const msg = friendlyError("You've hit your usage limit. Purchase more credits.");
    assert.match(msg, /usage limit/i);
    assert.match(msg, /credits|upgrade|reset/i);
  });
  it("returns a generic error otherwise", () => {
    assert.match(friendlyError("disk full"), /Codex error/);
  });
});
