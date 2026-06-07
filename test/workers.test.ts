// Phase 3: the async worker orchestrator over the fake CodexRunner. Asserts the async lifecycle
// (handle now, event later), steer = abort+resume, cancel, failure events, the summarize fallback,
// and the keep-alive hold lifecycle. Plus the runner-util tests carried over from v0.1.

import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import type { ThreadItem } from "@openai/codex-sdk";

import { formatItem, friendlyError } from "../src/workers/runner.js";
import { createOrchestrator } from "../src/workers/orchestrator.js";
import { clipSummarizer } from "../src/workers/summarize.js";
import { makeFakeCodex } from "./fakes/fakeCodex.js";
import { makeCountingHold } from "./helpers.js";

interface EmittedEvent {
  workerId: string;
  status: "completed" | "failed";
  summary: string;
}

function harness(opts: { summarizeLimit?: number } = {}) {
  const runner = makeFakeCodex();
  const hold = makeCountingHold();
  const events: EmittedEvent[] = [];
  const mirrored: number[] = [];
  const orch = createOrchestrator({
    runner,
    hold,
    workspaceDir: "/workspace",
    emitEvent: (e) => events.push(e),
    summarize: clipSummarizer(opts.summarizeLimit ?? 2000),
    onWorkersChanged: (ws) => mirrored.push(ws.length),
  });
  return { runner, hold, events, mirrored, orch };
}

describe("worker orchestrator (async lifecycle)", () => {
  it("start returns a handle immediately, then emits a completed worker_event", async () => {
    const { orch, events, runner } = harness();
    const info = orch.start("work only within src/telegram/**", "proj");
    assert.equal(info.status, "running", "handle returned before the run finishes");
    assert.equal(events.length, 0, "no event yet — the turn is not blocked");

    await orch.whenQuiet();
    assert.equal(events.length, 1);
    assert.equal(events[0]!.workerId, info.id);
    assert.equal(events[0]!.status, "completed");
    assert.match(events[0]!.summary, /work only within src\/telegram/);
    assert.equal(orch.registry.get(info.id)!.status, "idle", "ready for more work");
    assert.equal(runner.calls.length, 1);
    assert.equal(runner.calls[0]!.hadSignal, true, "run was given an AbortSignal");
  });

  it("steer aborts the in-flight run and resumes the SAME thread (single completed event)", async () => {
    const { orch, events, runner } = harness();
    const info = orch.start("WAIT_FOR_ABORT — long build in scope A");
    const threadId = orch.registry.get(info.id)!.threadId;
    assert.ok(threadId, "thread id known as soon as the run starts");

    orch.steer(info.id, "actually, finish scope A quickly");
    await orch.whenQuiet();

    assert.equal(events.length, 1, "the aborted-for-steer run emits NO event; only the resume does");
    assert.equal(events[0]!.status, "completed");
    assert.match(events[0]!.summary, new RegExp(`resumed ${threadId}`));
    assert.equal(runner.calls.length, 2, "two runs: original + resume");
    assert.equal(runner.calls[1]!.resumeThreadId, threadId, "resume targets the same thread");
  });

  it("cancel aborts without resuming and emits no event", async () => {
    const { orch, events, hold } = harness();
    const info = orch.start("WAIT_FOR_ABORT — runaway worker");
    orch.cancel(info.id);
    await orch.whenQuiet();

    assert.equal(events.length, 0);
    assert.equal(orch.registry.get(info.id)!.status, "canceled");
    assert.equal(hold.held, 0, "cancel releases the keep-alive hold");
  });

  it("a failed worker run emits a failed worker_event", async () => {
    const { orch, events } = harness();
    const info = orch.start("WORKER_FAIL — this build breaks");
    await orch.whenQuiet();
    assert.equal(events.length, 1);
    assert.equal(events[0]!.status, "failed");
    assert.match(events[0]!.summary, /build error/);
    assert.equal(orch.registry.get(info.id)!.status, "failed");
  });

  it("condenses over-long worker output via the summarize fallback", async () => {
    const { orch, events } = harness({ summarizeLimit: 100 });
    orch.start("LONG_OUTPUT — produces a wall of text");
    await orch.whenQuiet();
    assert.ok(events[0]!.summary.length < 200, "long output was condensed");
    assert.match(events[0]!.summary, /truncated/);
  });

  it("holds the Sprite awake while a worker runs and releases it on completion", async () => {
    const { orch, hold } = harness();
    orch.start("quick task");
    assert.equal(hold.held, 1, "held while the worker is in flight");
    await orch.whenQuiet();
    assert.equal(hold.held, 0, "released when the worker settles");
    assert.equal(hold.acquired, 1);
    assert.equal(hold.released, 1);
  });

  it("send resumes an idle worker for a follow-up", async () => {
    const { orch, events, runner } = harness();
    const info = orch.start("first objective");
    await orch.whenQuiet();
    orch.send(info.id, "second objective");
    await orch.whenQuiet();
    assert.equal(events.length, 2);
    assert.equal(runner.calls.length, 2);
    assert.equal(runner.calls[1]!.resumeThreadId, orch.registry.get(info.id)!.threadId);
  });

  it("poll and list reflect worker state", async () => {
    const { orch } = harness();
    const a = orch.start("alpha", "p1");
    await orch.whenQuiet();
    const polled = orch.poll(a.id);
    assert.equal(polled!.info.status, "idle");
    assert.match(polled!.latest!, /alpha/);
    assert.equal(orch.list().length, 1);
    assert.equal(orch.poll("nope"), undefined);
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
  it("returns a generic error otherwise", () => {
    assert.match(friendlyError("disk full"), /Codex error/);
  });
});
