// Block model for memory files (DESIGN §5 "Block model").
//
// Every memory file may carry YAML-ish frontmatter with `description` (always visible to the
// manager even when the body isn't) and an optional `limit` (character budget). We hand-roll a
// tiny parser rather than pull in a YAML dependency — the design's "zero external deps for
// storage" rule. Only flat `key: value` scalars are supported; that is all the block model needs.

export interface MemoryBlock {
  /** One-line summary shown in the archival tree even when the body is not loaded. */
  description?: string;
  /** Optional character budget for the body (advisory; surfaced to the manager). */
  limit?: number;
  /** Everything after the frontmatter (the actual content). */
  body: string;
}

const FENCE = "---";

/** Parse a raw file into { description, limit, body }. No frontmatter → body is the whole file. */
export function parseBlock(raw: string): MemoryBlock {
  const normalized = raw.replace(/\r\n/g, "\n");
  if (!normalized.startsWith(FENCE + "\n")) {
    return { body: normalized };
  }
  const end = normalized.indexOf("\n" + FENCE, FENCE.length);
  if (end === -1) {
    // Unterminated frontmatter — treat the whole thing as body rather than throwing.
    return { body: normalized };
  }
  const frontmatter = normalized.slice(FENCE.length + 1, end);
  // Body starts after the closing fence line.
  const afterFence = normalized.indexOf("\n", end + 1);
  const body = afterFence === -1 ? "" : normalized.slice(afterFence + 1);

  const block: MemoryBlock = { body };
  for (const line of frontmatter.split("\n")) {
    const m = /^([A-Za-z_][\w-]*)\s*:\s*(.*)$/.exec(line.trim());
    if (!m) continue;
    const key = m[1]!;
    const value = stripQuotes(m[2]!.trim());
    if (key === "description") block.description = value;
    else if (key === "limit") {
      const n = Number(value);
      if (Number.isFinite(n)) block.limit = n;
    }
  }
  return block;
}

/** Serialize a block back to disk, emitting frontmatter only when there is metadata to record. */
export function serializeBlock(block: MemoryBlock): string {
  const hasMeta = block.description !== undefined || block.limit !== undefined;
  if (!hasMeta) return block.body;
  const lines = [FENCE];
  if (block.description !== undefined) lines.push(`description: ${block.description}`);
  if (block.limit !== undefined) lines.push(`limit: ${block.limit}`);
  lines.push(FENCE, "");
  return lines.join("\n") + block.body;
}

/** The text we index for search: description + body, so a file is findable by either. */
export function indexableText(block: MemoryBlock): string {
  return [block.description ?? "", block.body].filter(Boolean).join("\n");
}

function stripQuotes(s: string): string {
  if (s.length >= 2 && ((s.startsWith('"') && s.endsWith('"')) || (s.startsWith("'") && s.endsWith("'")))) {
    return s.slice(1, -1);
  }
  return s;
}
