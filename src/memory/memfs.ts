// MemFS — the storage backend behind Anthropic's native memory tool (`memory_20250818`),
// implementing the fixed command set (view/create/str_replace/insert/delete/rename) over a
// `/memories` directory (DESIGN §5). Source of truth is markdown on disk in a git repo; the FTS
// index is written through on every change. Two behaviors are *our* additions on top of the
// standard tool: `system/` is auto-injected into the prompt in full, and we expose search.

import {
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  renameSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { dirname, join, normalize, posix, relative } from "node:path";

import { indexableText, parseBlock, serializeBlock } from "./block.js";
import { commitAll, ensureRepo } from "./git.js";
import { openFts, type FtsIndex, type SearchHit } from "./fts.js";

export const MEMORY_MOUNT = "/memories";

/** Raised on bad tool input (path traversal, missing file, etc.); surfaced as a tool_result. */
export class MemoryError extends Error {}

// The six memory-tool commands (shapes mirror the SDK's BetaMemoryTool20250818Command union).
export interface ViewCommand {
  command: "view";
  path: string;
  view_range?: number[];
}
export interface CreateCommand {
  command: "create";
  path: string;
  file_text: string;
}
export interface StrReplaceCommand {
  command: "str_replace";
  path: string;
  old_str: string;
  new_str: string;
}
export interface InsertCommand {
  command: "insert";
  path: string;
  insert_line: number;
  insert_text: string;
}
export interface DeleteCommand {
  command: "delete";
  path: string;
}
export interface RenameCommand {
  command: "rename";
  old_path: string;
  new_path: string;
}
export type MemoryCommand =
  | ViewCommand
  | CreateCommand
  | StrReplaceCommand
  | InsertCommand
  | DeleteCommand
  | RenameCommand;

export interface MemFs {
  // --- memory tool command handlers ---
  view(cmd: ViewCommand): string;
  create(cmd: CreateCommand): string;
  str_replace(cmd: StrReplaceCommand): string;
  insert(cmd: InsertCommand): string;
  delete(cmd: DeleteCommand): string;
  rename(cmd: RenameCommand): string;
  /** Dispatch a raw tool_use input (from the model) to the right handler. */
  execute(input: MemoryCommand): string;

  // --- our additions (DESIGN §5) ---
  /** Concatenated `system/` bodies, injected into the system prompt in full every turn. */
  loadSystem(): string;
  /** Tree listing of non-system files with their frontmatter descriptions. */
  treeListing(): string;
  /** FTS over all memory files. */
  search(query: string, limit?: number): SearchHit[];
  /** FTS restricted to `recall/`. */
  recallSearch(query: string, limit?: number): SearchHit[];
  /** Write a summarized-conversation file under recall/<month>/ (used by the recall tier). */
  writeRecall(name: string, body: string, monthKey?: string): string;
  /** Read a file by repo-relative path (e.g. "system/persona.md"); undefined if absent. */
  readRelative(rel: string): string | undefined;
  /** Rebuild the FTS index from disk (cold-wake / corruption recovery). */
  reindex(): void;
  close(): void;
}

export interface MemFsOptions {
  dir: string;
  /** Injected FTS index; one is created (in-memory unless ftsPath given) when omitted. */
  fts?: FtsIndex;
  ftsPath?: string;
  /** Clock for deterministic recall month folders in tests. */
  now?: () => Date;
}

export function openMemFs(opts: MemFsOptions): MemFs {
  const dir = opts.dir;
  const fts = opts.fts ?? openFts(opts.ftsPath ?? ":memory:");
  const now = opts.now ?? (() => new Date());

  ensureRepo(dir);
  ensureGitignore(dir);
  seedScaffold(dir);

  // ---- path handling -------------------------------------------------------
  /** Map a tool path ("/memories/..." from the model, or a bare repo-relative path) to a safe
   * repo-relative path. */
  function toRel(toolPath: string): string {
    let p = String(toolPath ?? "").trim().replace(/\\/g, "/");
    if (p === MEMORY_MOUNT || p === MEMORY_MOUNT + "/") return "";
    if (p.startsWith(MEMORY_MOUNT + "/")) p = p.slice(MEMORY_MOUNT.length + 1);
    p = p.replace(/^\/+/, "");
    const norm = normalize(p);
    if (norm === ".." || norm.startsWith("../") || norm.startsWith("/")) {
      throw new MemoryError(`path escapes ${MEMORY_MOUNT}: ${toolPath}`);
    }
    return norm === "." ? "" : norm.split(/[\\/]/).join("/");
  }
  const abs = (rel: string): string => join(dir, rel);
  const toolPathOf = (rel: string): string => posix.join(MEMORY_MOUNT, rel);

  function reindexInto(): void {
    fts.clear();
    for (const rel of walkFiles(dir)) {
      const raw = readFileSync(abs(rel), "utf8");
      fts.upsert(toolPathOf(rel), indexableText(parseBlock(raw)));
    }
  }
  function indexFile(rel: string): void {
    const raw = readFileSync(abs(rel), "utf8");
    fts.upsert(toolPathOf(rel), indexableText(parseBlock(raw)));
  }

  // ---- handlers ------------------------------------------------------------
  const api: MemFs = {
    view(cmd) {
      const rel = toRel(cmd.path);
      const target = abs(rel);
      if (!existsSync(target)) throw new MemoryError(`no such path: ${cmd.path}`);
      if (statSync(target).isDirectory()) {
        const entries = listDir(target).map((e) => (e.dir ? `${e.name}/` : e.name));
        return entries.length ? entries.join("\n") : "(empty directory)";
      }
      let content = readFileSync(target, "utf8");
      if (cmd.view_range && cmd.view_range.length === 2) {
        const [start, end] = cmd.view_range as [number, number];
        const lines = content.split("\n");
        content = lines.slice(Math.max(0, start - 1), end === -1 ? undefined : end).join("\n");
      }
      return content;
    },

    create(cmd) {
      const rel = toRel(cmd.path);
      if (rel === "") throw new MemoryError("cannot create the root");
      const target = abs(rel);
      mkdirSync(dirname(target), { recursive: true });
      writeFileSync(target, cmd.file_text);
      indexFile(rel);
      commit(`create ${rel} — ${summary(cmd.file_text)}`);
      return `Created ${toolPathOf(rel)} (${cmd.file_text.length} chars).`;
    },

    str_replace(cmd) {
      const rel = toRel(cmd.path);
      const target = abs(rel);
      if (!existsSync(target)) throw new MemoryError(`no such file: ${cmd.path}`);
      const before = readFileSync(target, "utf8");
      const idx = before.indexOf(cmd.old_str);
      if (idx === -1) throw new MemoryError(`old_str not found in ${cmd.path}`);
      if (before.indexOf(cmd.old_str, idx + cmd.old_str.length) !== -1) {
        throw new MemoryError(`old_str is not unique in ${cmd.path}; add more context`);
      }
      const after = before.slice(0, idx) + cmd.new_str + before.slice(idx + cmd.old_str.length);
      writeFileSync(target, after);
      indexFile(rel);
      commit(`str_replace ${rel} — ${summary(cmd.new_str)}`);
      return `Edited ${toolPathOf(rel)}.`;
    },

    insert(cmd) {
      const rel = toRel(cmd.path);
      const target = abs(rel);
      if (!existsSync(target)) throw new MemoryError(`no such file: ${cmd.path}`);
      const lines = readFileSync(target, "utf8").split("\n");
      const at = Math.max(0, Math.min(cmd.insert_line, lines.length));
      lines.splice(at, 0, cmd.insert_text);
      writeFileSync(target, lines.join("\n"));
      indexFile(rel);
      commit(`insert ${rel}:${cmd.insert_line} — ${summary(cmd.insert_text)}`);
      return `Inserted into ${toolPathOf(rel)} at line ${cmd.insert_line}.`;
    },

    delete(cmd) {
      const rel = toRel(cmd.path);
      if (rel === "") throw new MemoryError("cannot delete the root");
      const target = abs(rel);
      if (!existsSync(target)) throw new MemoryError(`no such path: ${cmd.path}`);
      const isDir = statSync(target).isDirectory();
      const removed = isDir ? walkFiles(target).map((r) => join(rel, r)) : [rel];
      rmSync(target, { recursive: true, force: true });
      for (const r of removed) fts.remove(toolPathOf(r.split(/[\\/]/).join("/")));
      commit(`delete ${rel}`);
      return `Deleted ${toolPathOf(rel)}.`;
    },

    rename(cmd) {
      const fromRel = toRel(cmd.old_path);
      const toRelPath = toRel(cmd.new_path);
      const from = abs(fromRel);
      const to = abs(toRelPath);
      if (!existsSync(from)) throw new MemoryError(`no such path: ${cmd.old_path}`);
      mkdirSync(dirname(to), { recursive: true });
      renameSync(from, to);
      fts.rename(toolPathOf(fromRel), toolPathOf(toRelPath));
      commit(`rename ${fromRel} -> ${toRelPath}`);
      return `Renamed ${toolPathOf(fromRel)} → ${toolPathOf(toRelPath)}.`;
    },

    execute(input) {
      switch (input.command) {
        case "view":
          return api.view(input);
        case "create":
          return api.create(input);
        case "str_replace":
          return api.str_replace(input);
        case "insert":
          return api.insert(input);
        case "delete":
          return api.delete(input);
        case "rename":
          return api.rename(input);
        default:
          throw new MemoryError(`unknown memory command: ${(input as { command: string }).command}`);
      }
    },

    loadSystem() {
      const sysDir = join(dir, "system");
      if (!existsSync(sysDir)) return "";
      const sections: string[] = [];
      for (const rel of walkFiles(sysDir).sort()) {
        const block = parseBlock(readFileSync(join(sysDir, rel), "utf8"));
        const body = block.body.trim();
        if (!body && !block.description) continue;
        sections.push(`### system/${rel}\n${body}`);
      }
      return sections.join("\n\n");
    },

    treeListing() {
      const lines: string[] = [];
      for (const rel of walkFiles(dir).sort()) {
        if (rel.startsWith("system/")) continue; // shown in full already
        const block = parseBlock(readFileSync(abs(rel), "utf8"));
        const desc = block.description ? ` — ${block.description}` : "";
        lines.push(`${toolPathOf(rel)}${desc}`);
      }
      return lines.length ? lines.join("\n") : "(no archival/recall files yet)";
    },

    search(query, limit = 10) {
      return fts.search(query, { limit });
    },
    recallSearch(query, limit = 10) {
      return fts.search(query, { limit, prefix: toolPathOf("recall/") });
    },

    writeRecall(name, body, monthKey) {
      const month = monthKey ?? monthFolder(now());
      const safe = name.replace(/[^\w.-]/g, "_");
      const rel = `recall/${month}/${safe.endsWith(".md") ? safe : safe + ".md"}`;
      return api.create({ command: "create", path: toolPathOf(rel), file_text: body });
    },

    readRelative(rel) {
      const target = abs(toRel(rel));
      return existsSync(target) && statSync(target).isFile()
        ? readFileSync(target, "utf8")
        : undefined;
    },

    reindex() {
      reindexInto();
    },
    close() {
      fts.close();
    },
  };

  function commit(message: string): void {
    commitAll(dir, message);
  }

  // Build the initial index from whatever scaffold/seed exists on disk.
  reindexInto();
  return api;
}

// ---- helpers ---------------------------------------------------------------

function summary(text: string): string {
  const firstLine = text.split("\n").find((l) => l.trim().length > 0) ?? "";
  const clipped = firstLine.trim().slice(0, 72);
  return clipped || "(empty)";
}

function monthFolder(d: Date): string {
  return `${d.getUTCFullYear()}-${String(d.getUTCMonth() + 1).padStart(2, "0")}`;
}

function listDir(absDir: string): Array<{ name: string; dir: boolean }> {
  return readdirSync(absDir, { withFileTypes: true })
    .filter((e) => e.name !== ".git")
    .map((e) => ({ name: e.name, dir: e.isDirectory() }))
    .sort((a, b) => a.name.localeCompare(b.name));
}

/** All files (recursively) under `root`, as POSIX-relative paths, excluding .git. */
function walkFiles(root: string): string[] {
  const out: string[] = [];
  const stack = [root];
  while (stack.length) {
    const current = stack.pop()!;
    if (!existsSync(current)) continue;
    for (const entry of readdirSync(current, { withFileTypes: true })) {
      if (entry.name.startsWith(".")) continue; // skip .git, .gitignore, dotfiles
      const full = join(current, entry.name);
      if (entry.isDirectory()) stack.push(full);
      else out.push(relative(root, full).split(/[\\/]/).join("/"));
    }
  }
  return out;
}

function ensureGitignore(dir: string): void {
  const gi = join(dir, ".gitignore");
  if (!existsSync(gi)) writeFileSync(gi, "# derived FTS index lives outside the repo by default\n");
}

/** Lay down the empty core-memory scaffold (DESIGN §5 layout) on first boot. */
function seedScaffold(dir: string): void {
  for (const sub of ["system", "archival", "recall"]) {
    mkdirSync(join(dir, sub), { recursive: true });
  }
  const persona = join(dir, "system", "persona.md");
  if (!existsSync(persona)) {
    writeFileSync(
      persona,
      serializeBlock({
        description: "who the manager is and how it works",
        body:
          "The manager plans, remembers, and delegates to Codex workers. It has no shell/file/net\n" +
          "tools of its own — its only hands are the worker and memory tools. It speaks to the owner\n" +
          "simply by writing an ordinary message.\n",
      }),
    );
  }
}
