import { strict as assert } from "node:assert";
import { createServer, type Server } from "node:http";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, before, describe, it } from "node:test";
import { createSpriteHold } from "../src/runtime/hold.js";

interface Recorded {
  method: string;
  url: string;
  body: string;
}

describe("createSpriteHold (off-Sprite)", () => {
  it("degrades to a no-op when the Tasks API socket is absent", async () => {
    const hold = createSpriteHold({ socketPath: "/nonexistent/.sprite/api.sock" });
    // Must not throw even though there is no socket to talk to.
    await hold.acquire();
    await hold.release();
  });
});

describe("createSpriteHold (against a fake Tasks API)", () => {
  let server: Server;
  let socketPath: string;
  let dir: string;
  const calls: Recorded[] = [];

  before(async () => {
    dir = mkdtempSync(join(tmpdir(), "sprite-sock-"));
    socketPath = join(dir, "api.sock");
    server = createServer((req, res) => {
      let body = "";
      req.on("data", (c: Buffer) => (body += c.toString()));
      req.on("end", () => {
        calls.push({ method: req.method ?? "", url: req.url ?? "", body });
        res.writeHead(200, { "content-type": "application/json" });
        res.end("{}");
      });
    });
    await new Promise<void>((resolve) => server.listen(socketPath, resolve));
  });

  after(async () => {
    await new Promise<void>((resolve) => server.close(() => resolve()));
    rmSync(dir, { recursive: true, force: true });
  });

  it("refcounts: one hold across nested acquires, deleted only on final release", async () => {
    // Large heartbeat so only the initial PUT happens during the test (deterministic).
    const hold = createSpriteHold({ socketPath, heartbeatMs: 600_000 });

    await hold.acquire();
    await hold.acquire(); // nested: should NOT issue a second create
    assert.equal(calls.filter((c) => c.method === "PUT").length, 1, "exactly one create PUT");
    assert.equal(calls[0]!.url, "/v1/tasks/codex-turn");
    assert.deepEqual(JSON.parse(calls[0]!.body), { expire: "5m" });

    await hold.release(); // still one holder left
    assert.equal(calls.filter((c) => c.method === "DELETE").length, 0, "not released yet");

    await hold.release(); // last holder -> delete
    const del = calls.filter((c) => c.method === "DELETE");
    assert.equal(del.length, 1, "released on final unref");
    assert.equal(del[0]!.url, "/v1/tasks/codex-turn");
  });
});
