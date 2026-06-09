// Telemetry recorder (Inspector backend): per-turn token capture, cost arithmetic, the worker-prompt
// log with turn stamping, ring-buffer eviction, and the durable cost snapshot round-trip.

import { strict as assert } from "node:assert";
import { describe, it } from "node:test";

import { createTelemetry } from "../src/runtime/telemetry.js";

const opts = { priceInPerMTok: 15, priceOutPerMTok: 75 };

describe("telemetry", () => {
  it("accumulates per-turn usage and computes cost from the price table", () => {
    const t = createTelemetry(opts);
    t.beginTurn(1, "owner_message", "build a thing", 7);
    t.recordUsage(1, { inputTokens: 1_000_000, outputTokens: 200_000 });
    t.recordUsage(1, { inputTokens: 500_000, outputTokens: 100_000 });
    t.endTurn(1);

    const [turn] = t.turns();
    assert.equal(turn!.turnId, 1);
    assert.equal(turn!.iterations, 2);
    assert.equal(turn!.inputTokens, 1_500_000);
    assert.equal(turn!.outputTokens, 300_000);
    // 1.5 Mtok * $15 + 0.3 Mtok * $75 = 22.5 + 22.5
    assert.equal(turn!.costUsd, 45);
    assert.ok(turn!.endedAt && turn!.endedAt >= turn!.startedAt);

    const m = t.meter();
    assert.equal(m.inputTokens, 1_500_000);
    assert.equal(m.costUsd, 45);
    assert.equal(m.managerTurns, 1);
    // contextTokens = the most recent call's input size, not the sum.
    assert.equal(t.contextTokens(), 500_000);
  });

  it("logs worker prompts stamped with the originating turn and counts codex turns", () => {
    const t = createTelemetry(opts);
    t.recordPrompt({ turnId: 4, workerId: "w1", kind: "start", prompt: "scope: src/api/**" });
    t.recordPrompt({ turnId: 4, workerId: "w2", kind: "start", prompt: "scope: test/**" });
    t.recordPrompt({ turnId: 9, workerId: "w1", kind: "steer", prompt: "redirect" });
    t.recordPrompt({ turnId: 9, workerId: "w1", kind: "cancel", prompt: "" });

    assert.equal(t.prompts({ turnId: 4 }).length, 2, "two prompts traced to turn 4");
    assert.equal(t.prompts({ workerId: "w1" }).length, 3);
    assert.equal(t.prompts({ workerId: "w1" })[0]!.prompt, "scope: src/api/**");
    // cancel does not count as a codex turn (no run launched).
    assert.equal(t.meter().codexTurns, 3);
  });

  it("evicts the oldest turns past the ring cap", () => {
    const t = createTelemetry({ ...opts, maxTurns: 2 });
    for (const id of [1, 2, 3]) t.beginTurn(id, "owner_message", `r${id}`, 1);
    const ids = t.turns().map((x) => x.turnId);
    assert.deepEqual(ids, [2, 3], "turn 1 evicted; meter total still counts all 3");
    assert.equal(t.meter().managerTurns, 3);
  });

  it("round-trips the cost snapshot across a restart", () => {
    const t = createTelemetry(opts);
    t.beginTurn(1, "owner_message", "x", 1);
    t.recordUsage(1, { inputTokens: 2_000_000, outputTokens: 0 });
    t.recordPrompt({ turnId: 1, workerId: "w1", kind: "start", prompt: "go" });

    const snap = t.costSnapshot();
    const restored = createTelemetry(opts);
    restored.loadCost(snap);

    const m = restored.meter();
    assert.equal(m.inputTokens, 2_000_000);
    assert.equal(m.costUsd, 30); // 2 Mtok * $15
    assert.equal(m.managerTurns, 1);
    assert.equal(m.codexTurns, 1);
  });
});
