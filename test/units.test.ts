import { strict as assert } from "node:assert";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, it } from "node:test";
import { openSessionStore } from "../src/sessions.js";
import { chunkText } from "../src/telegram.js";

describe("sessions store", () => {
  it("persists across reopen and supports delete", () => {
    const path = join(mkdtempSync(join(tmpdir(), "sess-")), ".sessions.json");
    const a = openSessionStore(path);
    assert.equal(a.get(7), undefined);
    a.set(7, "sid-7");
    a.set(8, "sid-8");

    const b = openSessionStore(path); // simulates a process restart
    assert.equal(b.get(7), "sid-7");
    assert.equal(b.get(8), "sid-8");

    b.delete(7);
    const c = openSessionStore(path);
    assert.equal(c.get(7), undefined);
    assert.equal(c.get(8), "sid-8");
  });

  it("tolerates a missing file by starting empty", () => {
    const path = join(mkdtempSync(join(tmpdir(), "sess-")), "nope.json");
    assert.equal(openSessionStore(path).get(1), undefined);
  });
});

describe("chunkText", () => {
  it("returns a single chunk when under the limit", () => {
    assert.deepEqual(chunkText("hello"), ["hello"]);
    assert.deepEqual(chunkText(""), []);
  });

  it("splits long text into <=4096 pieces with no loss", () => {
    const text = "X".repeat(9000);
    const chunks = chunkText(text);
    assert.equal(chunks.length, 3);
    assert.ok(chunks.every((c) => c.length <= 4096));
    assert.equal(chunks.join("").length, 9000);
  });

  it("prefers to break on newline boundaries", () => {
    const block = "a".repeat(4000) + "\n" + "b".repeat(4000);
    const chunks = chunkText(block);
    assert.equal(chunks.length, 2);
    assert.equal(chunks[0], "a".repeat(4000));
    assert.equal(chunks[1], "b".repeat(4000));
  });
});
