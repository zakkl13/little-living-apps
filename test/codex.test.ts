import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import type { ThreadItem } from "@openai/codex-sdk";
import { formatItem, friendlyError } from "../src/workers/runner.js";

// Minimal ThreadItem builders — we only populate the fields formatItem reads.
const item = (o: Record<string, unknown>): ThreadItem => o as unknown as ThreadItem;

describe("formatItem", () => {
  it("renders a command execution as a single $-prefixed line", () => {
    assert.equal(
      formatItem(item({ id: "1", type: "command_execution", command: "npm   test\n--watch" })),
      "$ npm test --watch",
    );
  });

  it("summarizes file changes and pluralizes correctly", () => {
    assert.equal(
      formatItem(item({ id: "2", type: "file_change", changes: [{}, {}, {}] })),
      "✏️ 3 files changed",
    );
    assert.equal(
      formatItem(item({ id: "3", type: "file_change", changes: [{}] })),
      "✏️ 1 file changed",
    );
  });

  it("renders web search and mcp tool calls", () => {
    assert.equal(formatItem(item({ id: "4", type: "web_search", query: "how to fly" })), "🔍 how to fly");
    assert.equal(
      formatItem(item({ id: "5", type: "mcp_tool_call", server: "fs", tool: "read" })),
      "🔧 fs.read",
    );
  });

  it("skips agent_message and reasoning (they are not progress lines)", () => {
    assert.equal(formatItem(item({ id: "6", type: "agent_message", text: "hi" })), undefined);
    assert.equal(formatItem(item({ id: "7", type: "reasoning", text: "thinking" })), undefined);
  });
});

describe("friendlyError", () => {
  it("adds a re-auth hint when the error looks auth-related", () => {
    const msg = friendlyError("request failed: 401 unauthorized");
    assert.match(msg, /auth problem/i);
    assert.match(msg, /codex login/);
  });

  it("returns a generic error otherwise", () => {
    const msg = friendlyError("disk full");
    assert.match(msg, /Codex error/);
    assert.doesNotMatch(msg, /auth problem/i);
  });
});
