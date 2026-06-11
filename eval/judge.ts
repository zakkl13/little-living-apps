// LLM-as-judge, used ONLY for soft qualities code can't grade (tone, narration discipline, scope
// quality) and always behind a scenario-specific rubric — never as the primary grader. The judge is
// a plain, tool-less Codex thread: zero new dependencies, and it rides the same ChatGPT subscription
// as everything else (this repo's hard rule: no metered API plane).
//
// Known limits (why judge scores are reported separately and never gate pass/fail by default):
// judges drift, miss regressions, and reward verbosity. Calibrate rubrics against transcripts you've
// scored yourself before trusting trends.

import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { Codex } from "@openai/codex-sdk";

import { sanitizedEnv } from "../src/workers/runner.js";
import type { TrialReport } from "./types.js";

export interface JudgeVerdict {
  score: number; // 0..1
  reasoning: string;
}

/** The slice of a trial the judge reads (so callers can pass a not-yet-complete report). */
export type JudgeInput = Pick<TrialReport, "timeline" | "workerPrompts" | "conversation">;

const clip = (s: string, n: number): string => (s.length > n ? s.slice(0, n) + "\n…(clipped)" : s);

function renderTranscript(t: JudgeInput): string {
  const owner = t.timeline
    .filter((e) => e.type === "owner_msg" || e.type === "delivery")
    .map((e) => (e.type === "owner_msg" ? `OWNER: ${e.text}` : `MANAGER→OWNER: ${e.text}`))
    .join("\n");
  const prompts = t.workerPrompts
    .map((p) => `[turn ${p.turnId}] ${p.kind} → ${p.prompt}`)
    .join("\n---\n");
  const internal = t.conversation
    .map((m) => {
      const parts = m.content.map((b) => {
        if (b.type === "text") return `text: ${(b as { text?: string }).text ?? ""}`;
        if (b.type === "tool_use") return `tool_use: ${(b as { name?: string }).name ?? "?"} ${JSON.stringify((b as { input?: unknown }).input ?? {})}`;
        if (b.type === "tool_result") return `tool_result: ${(b as { content?: string }).content ?? ""}`;
        if (b.type === "thinking") return `(private reasoning)`;
        return b.type;
      });
      return `${m.role}: ${parts.join(" | ")}`;
    })
    .join("\n");

  return [
    "## Owner-visible exchange (what the user experienced)",
    owner || "(nothing delivered)",
    "## Worker dispatches (objectives the manager wrote for its subagents)",
    clip(prompts || "(none)", 8_000),
    "## Internal log (tool calls and replies; private reasoning elided)",
    clip(internal || "(empty)", 12_000),
  ].join("\n\n");
}

export function buildJudgePrompt(scenarioDescription: string, rubric: string, t: JudgeInput): string {
  return `You are a strict evaluator of an AI manager agent. The manager talks to one owner over chat,
delegates real work to background subagents, keeps a memory, and is judged on outcomes delivered
without making the owner babysit the work.

## What this scenario exercises
${scenarioDescription}

## Rubric — score ONLY this
${rubric}

${renderTranscript(t)}

## Your verdict
Score the manager's behavior against the rubric only (other qualities are graded elsewhere).
Be strict: reserve scores above 0.8 for behavior you could not meaningfully improve.
Reply with ONLY a JSON object, no markdown fence, exactly:
{"score": <number between 0 and 1>, "reasoning": "<one or two terse sentences of evidence>"}`;
}

export function parseVerdict(raw: string): JudgeVerdict {
  const match = raw.match(/\{[\s\S]*\}/);
  if (!match) throw new Error(`judge returned no JSON: ${clip(raw, 200)}`);
  const parsed = JSON.parse(match[0]) as { score?: unknown; reasoning?: unknown };
  const score = Number(parsed.score);
  if (!Number.isFinite(score) || score < 0 || score > 1) {
    throw new Error(`judge score out of range: ${String(parsed.score)}`);
  }
  return { score, reasoning: typeof parsed.reasoning === "string" ? parsed.reasoning : "" };
}

export async function judgeTrial(
  scenarioDescription: string,
  rubric: string,
  transcript: JudgeInput,
): Promise<JudgeVerdict> {
  const dir = mkdtempSync(join(tmpdir(), "lila-judge-"));
  try {
    const codex = new Codex({ env: sanitizedEnv() });
    const thread = codex.startThread({
      workingDirectory: dir,
      skipGitRepoCheck: true,
      sandboxMode: "read-only",
      networkAccessEnabled: false,
      webSearchEnabled: false,
      modelReasoningEffort: "medium",
      approvalPolicy: "never",
    });
    const turn = await thread.run(buildJudgePrompt(scenarioDescription, rubric, transcript));
    return parseVerdict(turn.finalResponse ?? "");
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}
