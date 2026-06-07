// Memory subsystem tested against the REAL backends (DESIGN §13): node:sqlite FTS + a real tmp
// git repo. No mocks here — this is the high-fidelity tier. Each test gets a fresh MEMORY_DIR.

import { strict as assert } from "node:assert";
import { existsSync, mkdtempSync, readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, it } from "node:test";

import { openMemFs, MemoryError, type MemFs } from "../src/memory/memfs.js";
import { commitCount } from "../src/memory/git.js";
import { parseBlock, serializeBlock } from "../src/memory/block.js";

const open: MemFs[] = [];
function freshMem(now?: () => Date): { mem: MemFs; dir: string } {
  const dir = mkdtempSync(join(tmpdir(), "mem-"));
  const mem = openMemFs({ dir, now });
  open.push(mem);
  return { mem, dir };
}

afterEach(() => {
  while (open.length) open.pop()!.close();
});

describe("block model", () => {
  it("round-trips frontmatter + body", () => {
    const raw = "---\ndescription: a fact\nlimit: 500\n---\nhello world\n";
    const b = parseBlock(raw);
    assert.equal(b.description, "a fact");
    assert.equal(b.limit, 500);
    assert.equal(b.body, "hello world\n");
    assert.match(serializeBlock(b), /description: a fact/);
  });

  it("treats a file with no frontmatter as pure body", () => {
    const b = parseBlock("just text\n");
    assert.equal(b.description, undefined);
    assert.equal(b.body, "just text\n");
    assert.equal(serializeBlock(b), "just text\n"); // no frontmatter emitted
  });
});

describe("MemFs commands (real git + sqlite)", () => {
  it("create writes a file, commits, and indexes it for search", () => {
    const { mem, dir } = freshMem();
    const before = commitCount(dir);
    const res = mem.create({
      command: "create",
      path: "/memories/archival/facts/db.md",
      file_text: "---\ndescription: database choice\n---\nWe picked PostgreSQL for the API.\n",
    });
    assert.match(res, /Created/);
    assert.ok(existsSync(join(dir, "archival/facts/db.md")));
    assert.equal(commitCount(dir), before + 1, "create makes exactly one commit");

    const hits = mem.search("PostgreSQL");
    assert.equal(hits.length, 1);
    assert.equal(hits[0]!.path, "/memories/archival/facts/db.md");
    assert.match(hits[0]!.snippet, /PostgreSQL/);
  });

  it("str_replace edits in place, commits, and re-indexes", () => {
    const { mem, dir } = freshMem();
    mem.create({ command: "create", path: "/memories/archival/x.md", file_text: "alpha beta\n" });
    const c0 = commitCount(dir);
    mem.str_replace({
      command: "str_replace",
      path: "/memories/archival/x.md",
      old_str: "beta",
      new_str: "gamma",
    });
    assert.equal(commitCount(dir), c0 + 1);
    assert.equal(readFileSync(join(dir, "archival/x.md"), "utf8"), "alpha gamma\n");
    assert.equal(mem.search("beta").length, 0, "old term no longer indexed");
    assert.equal(mem.search("gamma").length, 1, "new term indexed");
  });

  it("str_replace refuses a non-unique or missing match", () => {
    const { mem } = freshMem();
    mem.create({ command: "create", path: "/memories/archival/d.md", file_text: "a a a\n" });
    assert.throws(
      () =>
        mem.str_replace({ command: "str_replace", path: "/memories/archival/d.md", old_str: "a", new_str: "b" }),
      MemoryError,
    );
    assert.throws(
      () =>
        mem.str_replace({ command: "str_replace", path: "/memories/nope.md", old_str: "a", new_str: "b" }),
      MemoryError,
    );
  });

  it("insert places text at the requested line", () => {
    const { mem, dir } = freshMem();
    mem.create({ command: "create", path: "/memories/archival/list.md", file_text: "one\ntwo\n" });
    mem.insert({ command: "insert", path: "/memories/archival/list.md", insert_line: 1, insert_text: "ONE.5" });
    assert.equal(readFileSync(join(dir, "archival/list.md"), "utf8"), "one\nONE.5\ntwo\n");
  });

  it("delete removes the file, drops it from search, and commits", () => {
    const { mem, dir } = freshMem();
    mem.create({ command: "create", path: "/memories/archival/gone.md", file_text: "ephemeral data\n" });
    assert.equal(mem.search("ephemeral").length, 1);
    const c0 = commitCount(dir);
    mem.delete({ command: "delete", path: "/memories/archival/gone.md" });
    assert.equal(commitCount(dir), c0 + 1);
    assert.ok(!existsSync(join(dir, "archival/gone.md")));
    assert.equal(mem.search("ephemeral").length, 0);
  });

  it("rename moves the file and updates its indexed path", () => {
    const { mem, dir } = freshMem();
    mem.create({ command: "create", path: "/memories/archival/old.md", file_text: "movable content\n" });
    mem.rename({ command: "rename", old_path: "/memories/archival/old.md", new_path: "/memories/archival/new.md" });
    assert.ok(existsSync(join(dir, "archival/new.md")));
    assert.ok(!existsSync(join(dir, "archival/old.md")));
    const hits = mem.search("movable");
    assert.equal(hits[0]!.path, "/memories/archival/new.md");
  });

  it("view returns file contents and directory listings", () => {
    const { mem } = freshMem();
    mem.create({ command: "create", path: "/memories/archival/a.md", file_text: "line1\nline2\nline3\n" });
    assert.match(mem.view({ command: "view", path: "/memories/archival/a.md" }), /line1\nline2/);
    assert.equal(mem.view({ command: "view", path: "/memories/archival/a.md", view_range: [2, 2] }), "line2");
    assert.match(mem.view({ command: "view", path: "/memories/archival" }), /a\.md/);
  });

  it("execute() dispatches raw tool input", () => {
    const { mem } = freshMem();
    const out = mem.execute({ command: "create", path: "/memories/archival/e.md", file_text: "via execute\n" });
    assert.match(out, /Created/);
    assert.equal(mem.search("execute").length, 1);
  });

  it("rejects path traversal outside /memories", () => {
    const { mem } = freshMem();
    assert.throws(
      () => mem.create({ command: "create", path: "/memories/../escape.md", file_text: "x" }),
      MemoryError,
    );
  });
});

describe("MemFs core-memory behaviors (DESIGN §5 additions)", () => {
  it("loadSystem injects all system/ bodies in full", () => {
    const { mem } = freshMem();
    mem.create({
      command: "create",
      path: "/memories/system/owner.md",
      file_text: "---\ndescription: owner profile\n---\nOwner prefers terse replies.\n",
    });
    const sys = mem.loadSystem();
    assert.match(sys, /system\/persona\.md/); // seeded on boot
    assert.match(sys, /system\/owner\.md/);
    assert.match(sys, /Owner prefers terse replies\./);
  });

  it("treeListing shows archival files with descriptions and hides system/", () => {
    const { mem } = freshMem();
    mem.create({
      command: "create",
      path: "/memories/archival/decisions/d1.md",
      file_text: "---\ndescription: chose webhook over polling\n---\nbody\n",
    });
    const tree = mem.treeListing();
    assert.match(tree, /archival\/decisions\/d1\.md — chose webhook over polling/);
    assert.doesNotMatch(tree, /system\/persona/);
  });

  it("recallSearch is restricted to recall/ and writeRecall files by month", () => {
    const { mem, dir } = freshMem(() => new Date("2026-06-15T00:00:00Z"));
    mem.create({ command: "create", path: "/memories/archival/k.md", file_text: "keyword in archival\n" });
    mem.writeRecall("conv1", "discussed the keyword at length\n");

    assert.ok(existsSync(join(dir, "recall/2026-06/conv1.md")), "recall file lands in month folder");

    const all = mem.search("keyword");
    assert.equal(all.length, 2, "global search sees both archival and recall");

    const recalls = mem.recallSearch("keyword");
    assert.equal(recalls.length, 1);
    assert.ok(recalls[0]!.path.startsWith("/memories/recall/"));
  });

  it("reindex() rebuilds the FTS index from disk (cold-wake recovery)", () => {
    const { mem, dir } = freshMem();
    // Write a file directly on disk, bypassing the tool (simulates files restored from git).
    mkdirSync(join(dir, "archival"), { recursive: true });
    writeFileSync(join(dir, "archival/restored.md"), "phoenix rises from disk\n");
    assert.equal(mem.search("phoenix").length, 0, "not yet indexed");
    mem.reindex();
    assert.equal(mem.search("phoenix").length, 1, "indexed after reindex");
  });
});
