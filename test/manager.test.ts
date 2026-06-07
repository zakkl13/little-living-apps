// Phase 2: the manager loop driven by a scripted model (fakeAnthropic) over a REAL MemFs. Asserts
// the turn contract, tool dispatch, memory writes landing on disk, the compaction round-trip, and
// serialized draining. No real Anthropic, no workers yet.

import { strict as assert } from "node:assert";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, it } from "node:test";

import { openMemFs, type MemFs } from "../src/memory/memfs.js";
import { buildRegistry } from "../src/manager/tools/registry.js";
import { memoryToolModule } from "../src/manager/tools/memory.js";
import { orchestrationToolModule } from "../src/manager/tools/orchestration.js";
import { createTranscript, runManagerTurn } from "../src/manager/manager.js";
import { buildSystemPrompt } from "../src/manager/prompt.js";
import { createEventQueue } from "../src/runtime/eventQueue.js";
import { createLoop, type ManagerLoop } from "../src/runtime/loop.js";
import { noopHold } from "../src/runtime/hold.js";
import { makeFakeAnthropic, resp, text, toolUse, compaction, type FakeAnthropic } from "./fakes/fakeAnthropic.js";

const OWNER = 11111111;

interface Harness {
  mem: MemFs;
  fake: FakeAnthropic;
  loop: ManagerLoop;
  sent: Array<{ chatId: number; text: string }>;
  send(text: string, chatId?: number): Promise<void>;
}

const cleanups: Array<() => void> = [];
afterEach(() => {
  while (cleanups.length) cleanups.pop()!();
});

function makeHarness(): Harness {
  const dir = mkdtempSync(join(tmpdir(), "mgr-"));
  const mem = openMemFs({ dir });
  cleanups.push(() => mem.close());

  const fake = makeFakeAnthropic();
  const sent: Array<{ chatId: number; text: string }> = [];
  const deliver = async (chatId: number, t: string): Promise<void> => {
    sent.push({ chatId, text: t });
  };

  const registry = buildRegistry([
    memoryToolModule(mem),
    orchestrationToolModule(), // no orchestrator in Phase 2
  ]);
  const transcript = createTranscript();
  const queue = createEventQueue();

  const loop = createLoop({
    queue,
    hold: noopHold,
    ownerChatId: OWNER,
    runTurn: (event, chatId) =>
      runManagerTurn(event, chatId, {
        model: fake,
        modelName: "test-opus",
        registry,
        transcript,
        deliver,
        buildSystem: () => buildSystemPrompt({ mem }),
      }),
  });
  loop.start();

  return {
    mem,
    fake,
    loop,
    sent,
    async send(t, chatId = OWNER) {
      queue.enqueue({ kind: "owner_message", chatId, text: t });
      await loop.whenIdle();
    },
  };
}

describe("manager turn", () => {
  it("delivers the manager's plain text to the owner", async () => {
    const h = makeHarness();
    h.fake.push(resp([text("on it 👍")]));
    await h.send("build me a thing");
    assert.equal(h.sent.length, 1);
    assert.deepEqual(h.sent[0], { chatId: OWNER, text: "on it 👍" });
  });

  it("stays silent when the turn emits only NO_REPLY", async () => {
    const h = makeHarness();
    h.fake.push(resp([text("NO_REPLY")]));
    await h.send("hi");
    assert.equal(h.sent.length, 0, "the NO_REPLY sentinel suppresses delivery");
  });

  it("delivers an acknowledgement alongside a tool call, then the result", async () => {
    const h = makeHarness();
    h.fake.push(
      // First message: ack + a tool call. The ack ships immediately; the turn continues.
      resp([text("on it"), toolUse("memory", { command: "view", path: "/memories" })]),
      resp([text("done")]),
    );
    await h.send("look something up");
    assert.deepEqual(
      h.sent.map((m) => m.text),
      ["on it", "done"],
    );
  });

  it("executes a memory write that lands on disk and is searchable", async () => {
    const h = makeHarness();
    h.fake.push(
      resp([
        toolUse("memory", {
          command: "create",
          path: "/memories/archival/facts/milk.md",
          file_text: "remember the milk\n",
        }),
      ]),
      resp([text("noted")]),
    );
    await h.send("remember to buy milk");
    assert.equal(h.mem.search("milk").length, 1, "memory write hit MemFs");
    assert.ok(h.sent.some((m) => m.text === "noted"));
  });

  it("runs memory_search and returns hits to the model", async () => {
    const h = makeHarness();
    h.mem.create({
      command: "create",
      path: "/memories/archival/facts/stack.md",
      file_text: "the API uses Fastify\n",
    });
    h.fake.push(
      resp([toolUse("memory_search", { query: "Fastify" })]),
      // The search result is appended as a tool_result; the model then replies.
      (req) => {
        const last = req.messages.at(-1)!;
        const tr = last.content.find((b) => b.type === "tool_result") as unknown as { content: string };
        return resp([text(`found: ${tr.content.replace(/\n/g, " ")}`)]);
      },
    );
    await h.send("what web framework are we using?");
    assert.ok(h.sent.some((m) => /Fastify/.test(m.text)), "search hit fed back to the model");
  });

  it("surfaces a tool error as is_error so the model can recover", async () => {
    const h = makeHarness();
    h.fake.push(
      resp([toolUse("memory", { command: "str_replace", path: "/memories/nope.md", old_str: "a", new_str: "b" })]),
      resp([text("that file doesn't exist; moving on")]),
    );
    await h.send("edit the missing file");
    // The 2nd request must carry a tool_result flagged is_error.
    const second = h.fake.requests[1]!;
    const toolResult = second.messages.at(-1)!.content.find((b) => b.type === "tool_result") as {
      is_error?: boolean;
    };
    assert.equal(toolResult.is_error, true);
    assert.ok(h.sent.some((m) => /doesn't exist/.test(m.text)));
  });
});

describe("compaction round-trip (DESIGN §4)", () => {
  it("appends the full assistant content (incl. compaction blocks) back into the next request", async () => {
    const h = makeHarness();
    // Turn 1: model emits a compaction block + text. Manager must keep it verbatim.
    h.fake.push(resp([compaction("cmp_A"), text("acknowledged")]));
    await h.send("first message");

    // Turn 2: a fresh request — assert the compaction block from turn 1 round-tripped.
    h.fake.push(resp([text("second reply")]));
    await h.send("second message");

    const secondRequest = h.fake.requests[1]!;
    const allBlocks = secondRequest.messages.flatMap((m) => m.content);
    const compactionBlock = allBlocks.find((b) => b.type === "compaction");
    assert.ok(compactionBlock, "compaction block must survive into the next request");
    assert.equal((compactionBlock as unknown as { id: string }).id, "cmp_A", "verbatim, same id");
  });
});

describe("serialized loop", () => {
  it("drains owner messages in order, one turn at a time", async () => {
    const h = makeHarness();
    h.fake.push(
      resp([text("first")]),
      resp([text("second")]),
    );
    await h.send("a");
    await h.send("b");
    assert.deepEqual(
      h.sent.map((m) => m.text),
      ["first", "second"],
    );
  });
});
