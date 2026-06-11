// Telemetry recorder (Inspector backend): per-turn token capture (input / cached / output /
// reasoning), the worker-prompt log with turn stamping, ring-buffer eviction, and the durable usage
// snapshot round-trip. No dollars — everything rides the subscription, so we track token counts only.

import { strict as assert } from "node:assert";
import { describe, it } from "node:test";

import { createTelemetry } from "../src/runtime/telemetry.js";

describe("telemetry", () => {
  it("accumulates per-turn token usage across all four counters", () => {
    const t = createTelemetry();
    t.beginTurn(1, "owner_message", "build a thing", 7);
    t.recordUsage(1, { inputTokens: 1_000_000, outputTokens: 200_000, cachedInputTokens: 800_000, reasoningTokens: 50_000 });
    t.recordUsage(1, { inputTokens: 500_000, outputTokens: 100_000, cachedInputTokens: 400_000, reasoningTokens: 25_000 });
    t.endTurn(1);

    const [turn] = t.turns();
    assert.equal(turn!.turnId, 1);
    assert.equal(turn!.iterations, 2);
    assert.equal(turn!.inputTokens, 1_500_000);
    assert.equal(turn!.outputTokens, 300_000);
    assert.equal(turn!.cachedInputTokens, 1_200_000);
    assert.equal(turn!.reasoningTokens, 75_000);
    assert.ok(turn!.endedAt && turn!.endedAt >= turn!.startedAt);

    const m = t.meter();
    assert.equal(m.inputTokens, 1_500_000);
    assert.equal(m.cachedInputTokens, 1_200_000);
    assert.equal(m.outputTokens, 300_000);
    assert.equal(m.reasoningTokens, 75_000);
    assert.equal(m.managerTurns, 1);
    // contextTokens = the most recent call's input size, not the sum.
    assert.equal(t.contextTokens(), 500_000);
  });

  it("treats cached/reasoning as optional in a partial usage", () => {
    const t = createTelemetry();
    t.beginTurn(1, "owner_message", "x", 1);
    t.recordUsage(1, { inputTokens: 100, outputTokens: 20 });
    const m = t.meter();
    assert.equal(m.inputTokens, 100);
    assert.equal(m.cachedInputTokens, 0);
    assert.equal(m.reasoningTokens, 0);
  });

  it("logs worker prompts stamped with the originating turn and counts codex turns", () => {
    const t = createTelemetry();
    t.recordPrompt({ turnId: 4, workerId: "w1", kind: "start", prompt: "scope: src/api/**" });
    t.recordPrompt({ turnId: 4, workerId: "w2", kind: "start", prompt: "scope: test/**" });
    t.recordPrompt({ turnId: 9, workerId: "w3", kind: "start", prompt: "validate the change" });

    assert.equal(t.prompts({ turnId: 4 }).length, 2, "two prompts traced to turn 4");
    assert.equal(t.prompts({ workerId: "w1" }).length, 1, "single-shot: one prompt per worker, ever");
    assert.equal(t.prompts({ workerId: "w1" })[0]!.prompt, "scope: src/api/**");
    assert.equal(t.meter().codexTurns, 3, "every launch is one codex turn");
  });

  it("evicts the oldest turns past the ring cap", () => {
    const t = createTelemetry({ maxTurns: 2 });
    for (const id of [1, 2, 3]) t.beginTurn(id, "owner_message", `r${id}`, 1);
    const ids = t.turns().map((x) => x.turnId);
    assert.deepEqual(ids, [2, 3], "turn 1 evicted; meter total still counts all 3");
    assert.equal(t.meter().managerTurns, 3);
  });

  it("round-trips the usage snapshot across a restart", () => {
    const t = createTelemetry();
    t.beginTurn(1, "owner_message", "x", 1);
    t.recordUsage(1, { inputTokens: 2_000_000, outputTokens: 0, cachedInputTokens: 1_900_000, reasoningTokens: 10 });
    t.recordPrompt({ turnId: 1, workerId: "w1", kind: "start", prompt: "go" });

    const snap = t.usageSnapshot();
    const restored = createTelemetry();
    restored.loadUsage(snap);

    const m = restored.meter();
    assert.equal(m.inputTokens, 2_000_000);
    assert.equal(m.cachedInputTokens, 1_900_000);
    assert.equal(m.reasoningTokens, 10);
    assert.equal(m.managerTurns, 1);
    assert.equal(m.codexTurns, 1);
  });
});
