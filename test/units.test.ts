import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { chunkText } from "../src/transport/telegram.js";

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
