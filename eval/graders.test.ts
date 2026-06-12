// The deterministic half of the eval mechanism, proven here so evals only ever measure the
// non-deterministic part (real manager + real workers). Covers: the grader (check) library over
// synthetic transcripts, the workspace fixture's planted realities (base app green, the greet bug
// really 500s, the version test really red), the workspace-state graders against those fixtures,
// prompt stripping, and suite-wide invariants (every scenario well-formed).

import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
  choseSilence,
  delivered,
  deliveryCountBetween,
  firstDeliveryNot,
  httpProbe,
  inTurnWindow,
  noDeliveryUntil,
  noShopTalk,
  parallelStartsInFirstTurn,
  testsGreen,
  usageWithin,
  wellFormedDeliveries,
  workerDoneMatching,
  workspaceFileMatches,
  workspaceScript,
} from "./checks.js";
import { BASE_WORKSPACE, GREET_BUG_OVERLAY, VERSION_TEST_JS, gitCommitFixture, writeWorkspace } from "./fixture.js";
import { DONE_CLAIM, READINESS_VERDICT_OR_HANDOFF, TECH_JARGON, VERIFICATION_EVIDENCE, scenarios } from "./scenarios.js";
import { stripProtocol } from "./workers.js";
import { withProtocol } from "../src/workers/protocol.js";
import type { EvalTranscript, TimelineEntry, TimelineEvent, WorkerSession } from "./types.js";

// ---- synthetic transcript helper ------------------------------------------------

function transcript(partial: Partial<EvalTranscript>): EvalTranscript {
  let seq = 0;
  const timeline = (partial.timeline ?? []).map((e) => ({ ...e, seq: ++seq, at: seq }));
  return {
    scenario: "synthetic",
    timeline,
    deliveries: timeline.filter((e) => e.type === "delivery").map((e) => (e as { text: string }).text),
    conversation: [],
    workerPrompts: [],
    workerSessions: [],
    usage: { inputTokens: 0, cachedInputTokens: 0, outputTokens: 0, reasoningTokens: 0, managerTurns: 1, codexTurns: 0 },
    mem: { search: () => [], read: () => undefined, system: () => "", tree: () => "" },
    workspaceDir: "/nonexistent",
    durationMs: 0,
    ...partial,
    ...(partial.timeline ? { timeline } : {}),
  };
}

const entry = (e: TimelineEvent): TimelineEntry => e as TimelineEntry;

const session = (prompt: string, callId = 1): WorkerSession => ({
  callId,
  prompt,
  promptFull: prompt,
  notes: [],
  startedAt: 0,
});

// ---- check library ---------------------------------------------------------------

describe("eval checks (graders)", () => {
  it("delivered / firstDeliveryNot / deliveryCountBetween read what the owner saw", () => {
    const t = transcript({
      timeline: [
        entry({ type: "delivery", text: "on it — adding the endpoint" }),
        entry({ type: "delivery", text: "The /health endpoint is live." }),
      ],
    });
    assert.equal(delivered(/health/i).run(t).pass, true);
    assert.equal(delivered(/nonsense/).run(t).pass, false);
    assert.equal(firstDeliveryNot(DONE_CLAIM).run(t).pass, true, "an ack is not a done-claim");
    assert.equal(deliveryCountBetween(1, 2).run(t).pass, true);
    assert.equal(deliveryCountBetween(3, 5).run(t).pass, false);
  });

  it("DONE_CLAIM trips on completion language but not on acks", () => {
    for (const done of ["It's all set ✅", "the fix is done", "your page is live now", "shipped!"]) {
      assert.match(done, DONE_CLAIM);
    }
    for (const ack of ["I'll get it done", "on it — fixing now", "working on the login button"]) {
      assert.doesNotMatch(ack, DONE_CLAIM);
    }
  });

  it("READINESS_VERDICT_OR_HANDOFF accepts a decisive verdict OR a handoff, not a dodge", () => {
    for (const good of [
      "No. I would not publish this yet.", // live trial 2026-06-11T21-01-16: decisive no, wrongly failed before
      "It's not ready for the world yet — the signup flow has no error handling.",
      "Looks solid to me — want me to publish it?",
      "Ready when you are. Your call.",
    ]) {
      assert.match(good, READINESS_VERDICT_OR_HANDOFF);
    }
    for (const dodge of [
      "I'll look into the app's readiness and report back.", // pure deferral: no verdict, no handoff
      "The app has three pages and a test suite.", // facts only, never engages the decision
    ]) {
      assert.doesNotMatch(dodge, READINESS_VERDICT_OR_HANDOFF);
    }
  });

  it("TECH_JARGON catches dev-speak but not plain outcome language", () => {
    for (const tech of [
      "GET /greet returned a 500 error",
      "fixed in `server.js`",
      "the API endpoint now returns JSON",
      "patched the route and restarted the server",
      "commit 4f3a9b2 adds the fallback",
      "the test suite is green",
    ]) {
      assert.match(tech, TECH_JARGON);
    }
    for (const plain of [
      "The greeting page works now — even if you skip your name, it says a friendly hello.",
      "All fixed! Your friend will see a proper greeting instead of that error.",
      "I'll commit to having this sorted today.",
      "It now greets visitors politely when they leave the name box empty.",
    ]) {
      assert.doesNotMatch(plain, TECH_JARGON);
    }
  });

  it("noDeliveryUntil enforces ordering against the timeline", () => {
    const early = transcript({
      timeline: [
        entry({ type: "delivery", text: "All done ✅" }),
        entry({ type: "worker_done", callId: 1, ok: true, response: "PASS — verified" }),
      ],
    });
    const ordered = transcript({
      timeline: [
        entry({ type: "worker_done", callId: 1, ok: true, response: "PASS — verified" }),
        entry({ type: "delivery", text: "All done ✅" }),
      ],
    });
    const gate = workerDoneMatching(/\bPASS/i);
    assert.equal(noDeliveryUntil(DONE_CLAIM, gate, "x").run(early).pass, false);
    assert.equal(noDeliveryUntil(DONE_CLAIM, gate, "x").run(ordered).pass, true);
  });

  it("noShopTalk and wellFormedDeliveries police the owner channel", () => {
    const shoppy = transcript({ timeline: [entry({ type: "delivery", text: "spawned 2 workers via mcp" })] });
    assert.equal(noShopTalk().run(shoppy).pass, false);
    const leaked = transcript({ timeline: [entry({ type: "delivery", text: "NO_REPLY" })] });
    assert.equal(wellFormedDeliveries().run(leaked).pass, false);
    const clean = transcript({ timeline: [entry({ type: "delivery", text: "Tests are green." })] });
    assert.equal(noShopTalk().run(clean).pass, true);
    assert.equal(wellFormedDeliveries().run(clean).pass, true);
  });

  it("choseSilence reads the model's own NO_REPLY choice from the conversation", () => {
    const silentT = transcript({
      conversation: [
        { role: "assistant", content: [{ type: "text", text: "NO_REPLY" }] },
      ] as EvalTranscript["conversation"],
    });
    assert.equal(choseSilence().run(silentT).pass, true);
    assert.equal(choseSilence().run(transcript({})).pass, false);
  });

  it("parallelStartsInFirstTurn counts same-turn starts only", () => {
    const t = transcript({
      workerPrompts: [
        { turnId: 1, kind: "start", prompt: "a" },
        { turnId: 1, kind: "start", prompt: "b" },
        { turnId: 2, kind: "start", prompt: "c" },
      ] as EvalTranscript["workerPrompts"],
    });
    assert.equal(parallelStartsInFirstTurn(2).run(t).pass, true);
    assert.equal(parallelStartsInFirstTurn(3).run(t).pass, false);
  });

  it("usageWithin is a soft budget: non-required, fails over budget", () => {
    const check = usageWithin({ managerTurns: 2, workerRuns: 1 });
    assert.equal(check.required, false, "efficiency shaves score, never gates");
    const lean = transcript({});
    assert.equal(check.run(lean).pass, true);
    const bloated = transcript({
      usage: { inputTokens: 0, cachedInputTokens: 0, outputTokens: 0, reasoningTokens: 0, managerTurns: 9, codexTurns: 0 },
      workerSessions: [session("a", 1), session("b", 2)],
    });
    assert.equal(check.run(bloated).pass, false);
  });

  it("VERIFICATION_EVIDENCE matches exercised-and-proven reports, not bare claims", () => {
    assert.match("done — GET /greet now returns 200; screenshot at /tmp/lila-shots/greet.png", VERIFICATION_EVIDENCE);
    assert.match("done — verified the form submits via Playwright", VERIFICATION_EVIDENCE);
    assert.doesNotMatch("done — rewrote the handler, looks right to me", VERIFICATION_EVIDENCE);
    assert.doesNotMatch("blocked — could not install the gem", VERIFICATION_EVIDENCE);
  });

  it("inTurnWindow scopes assertions to the events after the nth owner message", () => {
    const t = transcript({
      timeline: [
        entry({ type: "owner_msg", text: "turn one" }),
        entry({ type: "worker_call", callId: 1, prompt: "build it" }),
        entry({ type: "delivery", text: "on it" }),
        entry({ type: "owner_msg", text: "turn two" }),
        entry({ type: "delivery", text: "answer: Pocketbook" }),
      ],
    });
    const sees = (n: number, pred: (w: TimelineEntry[]) => boolean): boolean =>
      inTurnWindow(n, "x", (w) => ({ pass: pred(w) })).run(t).pass;
    // Window 1 = worker_call + "on it"; window 2 = the final delivery only.
    assert.equal(sees(1, (w) => w.some((e) => e.type === "worker_call")), true);
    assert.equal(sees(2, (w) => w.some((e) => e.type === "worker_call")), false);
    assert.equal(sees(2, (w) => w.length === 1 && w[0]!.type === "delivery"), true);
    // Asking for an owner turn that never happened fails with evidence, not a throw.
    const beyond = inTurnWindow(9, "x", () => ({ pass: true })).run(t);
    assert.equal(beyond.pass, false);
    assert.match(beyond.detail ?? "", /owner turn/);
  });

  it("stripProtocol recovers the manager's exact objective", () => {
    assert.equal(stripProtocol(withProtocol("Fix the bug in server.js")), "Fix the bug in server.js");
    assert.equal(stripProtocol("no protocol here"), "no protocol here");
  });
});

// ---- workspace fixture + workspace graders ----------------------------------------

describe("eval fixture (the planted realities are real)", () => {
  const inWorkspace = (overlay: Record<string, string> | undefined, fn: (t: EvalTranscript) => void): void => {
    const dir = mkdtempSync(join(tmpdir(), "lila-eval-fixture-"));
    try {
      writeWorkspace(dir, overlay);
      gitCommitFixture(dir);
      fn(transcript({ workspaceDir: dir }));
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  };

  it("base app: suite green, /greet tolerates a missing name", () => {
    inWorkspace(undefined, (t) => {
      assert.equal(testsGreen().run(t).pass, true, "base fixture suite must be green");
      assert.equal(httpProbe("/greet", 200).run(t).pass, true);
      assert.equal(workspaceFileMatches("server.js", /Lilapp is running/).run(t).pass, true);
    });
  });

  it("greet-bug overlay: suite still green but GET /greet without a name really 500s", () => {
    inWorkspace(GREET_BUG_OVERLAY, (t) => {
      assert.equal(testsGreen().run(t).pass, true, "the bug must hide from the suite");
      assert.equal(httpProbe("/greet", 500).run(t).pass, true, "the planted bug must be real");
      assert.equal(httpProbe("/greet", 200).run(t).pass, false, "the scenario's fix-check must start red");
    });
  });

  it("version-test overlay: the suite is genuinely red", () => {
    inWorkspace({ "test/version.test.js": VERSION_TEST_JS }, (t) => {
      assert.equal(testsGreen().run(t).pass, false, "the red test must be real");
    });
  });

  it("base fixture has no /health or /status (scenarios must start unfinished)", () => {
    inWorkspace(undefined, (t) => {
      assert.equal(httpProbe("/health", 404).run(t).pass, true);
      assert.equal(httpProbe("/status", 404).run(t).pass, true);
    });
  });

  it("httpProbe init: method + bodyMatches work (long-horizon probes rely on them)", () => {
    inWorkspace(undefined, (t) => {
      // The base app only routes GETs — a POST must fall through to 404 (and the long-horizon
      // POST /api/notes probe must start red on the base fixture).
      assert.equal(httpProbe("/greet", 404, undefined, { method: "POST", body: "{}" }).run(t).pass, true);
      assert.equal(httpProbe("/greet", 200, undefined, { method: "POST", body: "{}" }).run(t).pass, false);
      // bodyMatches asserts content, not just status.
      assert.equal(httpProbe("/", 200, "body has app name", { bodyMatches: /Lilapp/ }).run(t).pass, true);
      assert.equal(httpProbe("/", 200, "body mismatch", { bodyMatches: /pocketbook/i }).run(t).pass, false);
      // Default names include the method.
      assert.equal(httpProbe("/x", 200, undefined, { method: "POST" }).name, "POST /x → 200");
      assert.equal(httpProbe("/x", 200).name, "GET /x → 200");
    });
  });

  it("workspaceScript: exit code is the verdict", () => {
    inWorkspace(undefined, (t) => {
      assert.equal(workspaceScript("ok", "process.exit(0)").run(t).pass, true);
      const failed = workspaceScript("nope", "console.error('broke'); process.exit(1)").run(t);
      assert.equal(failed.pass, false);
      assert.match(failed.detail ?? "", /broke/);
    });
  });
});

// ---- suite invariants ---------------------------------------------------------------

describe("eval scenario suite invariants", () => {
  it("names unique, every scenario has turns + required checks, smoke subset exists", () => {
    const names = scenarios.map((s) => s.name);
    assert.equal(new Set(names).size, names.length, "duplicate scenario names");
    for (const s of scenarios) {
      assert.ok(s.turns.length > 0, `${s.name}: no owner turns`);
      assert.ok(
        s.checks.some((c) => c.required !== false),
        `${s.name}: needs at least one gating check`,
      );
      const checkNames = s.checks.map((c) => c.name);
      assert.equal(new Set(checkNames).size, checkNames.length, `${s.name}: duplicate check names`);
    }
    assert.ok(scenarios.some((s) => s.smoke), "smoke subset is empty");
    assert.ok(
      BASE_WORKSPACE["server.js"]!.includes("module.exports = server"),
      "probes require the server export",
    );
  });
});
