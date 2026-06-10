// Worker⇄manager protocol: the prompt preamble we hand workers, and the reader that keeps only the
// worker's summary block (so the manager's context carries the conclusion, not a byte-clipped head).

import { strict as assert } from "node:assert";
import { describe, it } from "node:test";

import {
  MANAGER_SUMMARY_MARKER,
  WORKER_PROTOCOL,
  withProtocol,
  extractManagerSummary,
  managerSummarizer,
} from "../src/workers/protocol.js";

describe("worker protocol", () => {
  it("prepends the summary-block + checkpoint guidance to every prompt, then the task", () => {
    const out = withProtocol("do the thing");
    assert.ok(out.includes(MANAGER_SUMMARY_MARKER), "tells the worker the exact marker to use");
    assert.match(out, /only thing it\s+receives/i, "explains the manager only sees the summary");
    assert.match(out, /git status --short/i, "tells workers to inspect the worktree");
    assert.match(out, /Commit your own finished edits/i, "tells workers to checkpoint their work");
    assert.ok(out.endsWith("do the thing"), "the task comes last, after the protocol");
    assert.ok(out.startsWith(WORKER_PROTOCOL), "protocol is the preamble");
  });

  it("extracts ONLY the summary block when the marker is present", () => {
    const output = [
      "lots of setup chatter…",
      "ran the build, fixed a route, restarted the service",
      `${MANAGER_SUMMARY_MARKER}`,
      "Added orders#export + route; GET /orders.csv -> 200; commit abc123.",
    ].join("\n");
    const s = extractManagerSummary(output);
    assert.equal(s, "Added orders#export + route; GET /orders.csv -> 200; commit abc123.");
    assert.ok(!s.includes("setup chatter"), "the transcript before the block is dropped");
  });

  it("uses the LAST marker if a worker writes the phrase more than once", () => {
    const output = `intro ${MANAGER_SUMMARY_MARKER} draft\nmore work\n${MANAGER_SUMMARY_MARKER}\nfinal block`;
    assert.equal(extractManagerSummary(output), "final block");
  });

  it("falls back to the TAIL (not the head) when there is no summary block", () => {
    const long = "HEAD-MARKER" + "x".repeat(4000) + "TAIL-VERIFICATION: GET / -> 200";
    const s = extractManagerSummary(long);
    assert.ok(s.includes("TAIL-VERIFICATION"), "keeps the conclusion at the end");
    assert.ok(!s.includes("HEAD-MARKER"), "drops the setup at the start");
    assert.match(s, /no summary block/i);
  });

  it("returns short no-marker output verbatim", () => {
    assert.equal(extractManagerSummary("  all good, 200 OK  "), "all good, 200 OK");
  });

  it("managerSummarizer is the async Summarize form of the extractor", async () => {
    const sum = managerSummarizer();
    assert.equal(await sum(`x\n${MANAGER_SUMMARY_MARKER}\nthe block`), "the block");
  });
});
