// The ManagerDriver over a *real* fake Codex thread (scripted ThreadEvent stream) — the same
// seam-injection discipline the old fake-Anthropic tests used. Asserts the turn contract: an
// final agent_message is delivered (honoring NO_REPLY), reasoning stays private, usage is reported, the
// volatile context header is prepended, an owner image becomes a local_image input, the thread id is
// captured, resume/reset choose start vs resume, and a turn.failed surfaces a friendly error.

import { strict as assert } from "node:assert";
import { describe, it } from "node:test";

import type { Input, ThreadEvent, Usage } from "@openai/codex-sdk";
import { createManagerDriver, type ConvMessage, type ManagerUsage, type RunTurnOpts } from "../src/manager/driver.js";
import type { ManagerThread, ManagerThreadFactory } from "../src/manager/managerCodex.js";

// ---- event + factory builders ----------------------------------------------

const agentMessage = (text: string): ThreadEvent => ({
  type: "item.completed",
  item: { id: "a", type: "agent_message", text },
});
const reasoning = (text: string): ThreadEvent => ({
  type: "item.completed",
  item: { id: "r", type: "reasoning", text },
});
const mcpCall = (server: string, tool: string): ThreadEvent => ({
  type: "item.completed",
  item: { id: "m", type: "mcp_tool_call", server, tool, arguments: {}, status: "completed" },
});
const failedMcpCall = (message: string): ThreadEvent => ({
  type: "item.completed",
  item: {
    id: "m",
    type: "mcp_tool_call",
    server: "lila",
    tool: "memory_view",
    arguments: { path: "/memories/project_status.md" },
    status: "failed",
    error: { message },
  },
});
const usage = (input = 100, output = 20): Usage => ({
  input_tokens: input,
  cached_input_tokens: 10,
  output_tokens: output,
  reasoning_output_tokens: 5,
});
const turnCompleted = (u: Usage = usage()): ThreadEvent => ({ type: "turn.completed", usage: u });
const turnFailed = (message: string): ThreadEvent => ({ type: "turn.failed", error: { message } });

interface FactoryState {
  inputs: Input[];
  started: number;
  resumed: string[];
}

function makeFakeFactory(turns: Array<(input: Input) => ThreadEvent[]>): {
  factory: ManagerThreadFactory;
  state: FactoryState;
} {
  const state: FactoryState = { inputs: [], started: 0, resumed: [] };
  let idx = 0;
  const tid = "thread-1";
  const makeThread = (initialId: string | null): ManagerThread => {
    let id = initialId;
    return {
      get id() {
        return id;
      },
      async runStreamed(input) {
        state.inputs.push(input);
        id = tid; // a turn started → the thread id is now known
        const script = turns[idx++] ?? (() => []);
        const events = script(input);
        return {
          events: (async function* () {
            for (const e of events) yield e;
          })(),
        };
      },
    };
  };
  return {
    state,
    factory: {
      start: () => {
        state.started += 1;
        return makeThread(null);
      },
      resume: (rid) => {
        state.resumed.push(rid);
        return makeThread(rid);
      },
    },
  };
}

interface Harness {
  sent: Array<{ chatId: number; text: string }>;
  usages: ManagerUsage[];
  conversation: ConvMessage[];
}

function driverWith(turns: Array<(input: Input) => ThreadEvent[]>, header = "HEADER") {
  const { factory, state } = makeFakeFactory(turns);
  const h: Harness = { sent: [], usages: [], conversation: [] };
  const driver = createManagerDriver({
    factory,
    deliver: async (chatId, text) => {
      h.sent.push({ chatId, text });
    },
    buildContextHeader: () => header,
  });
  const run = (input: { text: string; imagePath?: string }, chatId = 7, extraOpts: Partial<RunTurnOpts> = {}) =>
    driver.runTurn(input, chatId, {
      ...extraOpts,
      onUsage: (u) => h.usages.push(u),
      onConversation: (m) => h.conversation.push(m),
    });
  return { driver, state, h, run };
}

const firstText = (input: Input): string =>
  typeof input === "string" ? input : (input[0] as { text: string }).text;

describe("ManagerDriver turn", () => {
  it("delivers the agent_message to the owner", async () => {
    const { h, run } = driverWith([() => [agentMessage("on it 👍"), turnCompleted()]]);
    await run({ text: "build me a thing" });
    assert.deepEqual(h.sent, [{ chatId: 7, text: "on it 👍" }]);
  });

  it("delivers only the final agent_message from a streamed turn", async () => {
    const { h, run } = driverWith([
      () => [
        agentMessage("starting"),
        mcpCall("lila", "subagent_start"),
        agentMessage("still working"),
        mcpCall("lila", "subagent_list"),
        agentMessage("done"),
        turnCompleted(),
      ],
    ]);
    await run({ text: "build me a thing" });
    assert.deepEqual(h.sent, [{ chatId: 7, text: "done" }]);
  });

  it("records but does not deliver when the host delivery gate is closed", async () => {
    const { h, run } = driverWith([() => [agentMessage("internal progress"), turnCompleted()]]);
    await run({ text: "worker finished" }, 7, { allowReply: () => false });
    assert.deepEqual(h.sent, []);
    assert.ok(
      h.conversation.some((m) =>
        m.content.some((b) => b.type === "text" && b.text === "internal progress"),
      ),
      "suppressed replies still appear in Inspector conversation",
    );
  });

  it("stays silent on a bare NO_REPLY, and when reasoning precedes it", async () => {
    const { h, run } = driverWith([
      () => [agentMessage("NO_REPLY"), turnCompleted()],
      () => [agentMessage("Worker still running; no need to message yet.\n\nNO_REPLY"), turnCompleted()],
      () => [agentMessage("intermediate status"), agentMessage("NO_REPLY"), turnCompleted()],
    ]);
    await run({ text: "hi" });
    await run({ text: "kick it off" });
    await run({ text: "worker event" });
    assert.equal(h.sent.length, 0, "NO_REPLY suppresses the whole message");
  });

  it("never delivers reasoning, only the agent_message alongside it", async () => {
    const { h, run } = driverWith([
      () => [reasoning("the owner wants X; I'll do Y"), mcpCall("lila", "memory_view"), agentMessage("done"), turnCompleted()],
    ]);
    await run({ text: "do the thing" });
    assert.deepEqual(h.sent.map((m) => m.text), ["done"]);
  });

  it("records failed MCP tool-call details for observability", async () => {
    const { h, run } = driverWith([() => [failedMcpCall("user cancelled MCP tool call"), agentMessage("blocked"), turnCompleted()]]);
    await run({ text: "read memory" });
    const blocks = h.conversation.flatMap((m) => m.content);
    assert.ok(
      blocks.some(
        (b) =>
          b.type === "tool_use" &&
          b.name === "lila.memory_view" &&
          b.status === "failed" &&
          b.error === "user cancelled MCP tool call",
      ),
    );
    assert.ok(blocks.some((b) => b.type === "tool_result" && b.content === "error: user cancelled MCP tool call"));
  });

  it("reports token usage from turn.completed", async () => {
    const { h, run } = driverWith([() => [agentMessage("ok"), turnCompleted(usage(123, 45))]]);
    await run({ text: "x" });
    assert.equal(h.usages.length, 1);
    assert.equal(h.usages[0]!.inputTokens, 123);
    assert.equal(h.usages[0]!.outputTokens, 45);
    assert.equal(h.usages[0]!.cachedInputTokens, 10);
    assert.equal(h.usages[0]!.reasoningTokens, 5);
  });

  it("prepends the volatile context header to the input", async () => {
    const { state, run } = driverWith([() => [agentMessage("ok"), turnCompleted()]], "CORE-MEMORY");
    await run({ text: "remember the milk" });
    const text = firstText(state.inputs[0]!);
    assert.match(text, /^CORE-MEMORY\n\n---\n\n/);
    assert.match(text, /remember the milk/);
  });

  it("opens the turn with a local_image input when the owner sent a photo", async () => {
    const { state, run } = driverWith([() => [agentMessage("nice shot"), turnCompleted()]]);
    await run({ text: "what's wrong here?", imagePath: "/tmp/shot.png" });
    const input = state.inputs[0]!;
    assert.ok(Array.isArray(input));
    assert.deepEqual(input[1], { type: "local_image", path: "/tmp/shot.png" });
  });

  it("captures the thread id for snapshotting", async () => {
    const { driver, run } = driverWith([() => [agentMessage("ok"), turnCompleted()]]);
    assert.equal(driver.threadId(), undefined);
    await run({ text: "x" });
    assert.equal(driver.threadId(), "thread-1");
  });

  it("resumes an adopted thread id, and reset() forces a fresh start", async () => {
    const { driver, state, run } = driverWith([
      () => [agentMessage("a"), turnCompleted()],
      () => [agentMessage("b"), turnCompleted()],
    ]);
    driver.adoptThreadId("thread-prev");
    await run({ text: "first" });
    assert.deepEqual(state.resumed, ["thread-prev"], "adopted id → resume()");
    assert.equal(state.started, 0);

    driver.reset();
    await run({ text: "second" });
    assert.equal(state.started, 1, "reset() → next turn starts fresh");
  });

  it("delivers a friendly error when the turn fails", async () => {
    const { h, run } = driverWith([() => [turnFailed("401 unauthorized — please login again")]]);
    await run({ text: "x" });
    assert.equal(h.sent.length, 1);
    assert.match(h.sent[0]!.text, /auth problem/i);
  });

  // Regression: the real Codex SDK yields turn.failed THEN throws "Codex Exec exited with code 1:
  // <stderr>" because codex exec exits non-zero on a failed turn, leaving only the
  // "Reading prompt from stdin..." banner on stderr. The captured turn.failed reason must win over
  // that generic throw, so the owner sees the real cause (e.g. a usage limit) — not the banner.
  it("keeps the streamed turn.failed reason when the SDK then throws on exit code 1", async () => {
    const sent: Array<{ chatId: number; text: string }> = [];
    const reason = "You've hit your usage limit. Try again at 9:33 PM.";
    const thread: ManagerThread = {
      id: "thread-1",
      async runStreamed() {
        return {
          events: (async function* () {
            yield turnFailed(reason);
            throw new Error("Codex Exec exited with code 1: Reading prompt from stdin...\n");
          })(),
        };
      },
    };
    const factory: ManagerThreadFactory = { start: () => thread, resume: () => thread };
    const driver = createManagerDriver({
      factory,
      deliver: async (chatId, text) => {
        sent.push({ chatId, text });
      },
      buildContextHeader: () => "",
    });

    await driver.runTurn({ text: "x" }, 7);
    assert.equal(sent.length, 1);
    assert.match(sent[0]!.text, /usage limit/i, "real reason surfaces");
    assert.doesNotMatch(sent[0]!.text, /Reading prompt from stdin/, "stderr banner is not surfaced");
  });
});
