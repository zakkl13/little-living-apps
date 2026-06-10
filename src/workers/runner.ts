// Codex integration via the official @openai/codex-sdk threads API (SPEC §7).
//
// One Codex *thread* per Telegram chat. The thread id (from the `thread.started` event) is
// persisted by the handler so follow-ups resume the same conversation. We stream events so the
// handler can surface live progress to Telegram instead of a long silence.
//
// Auth: we never pass `apiKey`, so the SDK rides the cached ChatGPT-subscription login in
// CODEX_HOME (~/.codex/auth.json). Defense in depth: we strip OPENAI_API_KEY / CODEX_API_KEY
// from the env handed to the CLI so a stray key can't flip us to metered API billing.

import { spawn } from "node:child_process";
import { Codex, type ThreadItem } from "@openai/codex-sdk";
import type { Config } from "../config.js";
import { logger } from "../logger.js";

export interface CodexTurn {
  ok: boolean;
  /** Thread id for resuming this conversation (present once a turn has started). */
  threadId?: string;
  /** Final agent message, suitable for sending back to the user. */
  finalResponse: string;
}

export type ProgressFn = (note: string) => void;

export interface CodexRunArgs {
  prompt: string;
  resumeThreadId?: string;
  onProgress?: ProgressFn;
  /** Abort the in-flight turn (drives subagent_steer = abort+resume, and subagent_cancel). */
  signal?: AbortSignal;
  /** Fired as soon as the thread id is known, so the orchestrator can resume/steer it later. */
  onThreadId?: (id: string) => void;
}

export interface CodexRunner {
  run(args: CodexRunArgs): Promise<CodexTurn>;
  loginStatus(): Promise<{ ok: boolean; detail: string }>;
}

const NO_OUTPUT = "(Codex produced no output.)";

/** Build the env handed to the Codex CLI: inherit everything except billing-flip keys, then layer
 *  on any extras (the manager passes LILA_MCP_TOKEN this way). Shared by the worker runner and the
 *  manager thread factory so both strip the keys identically. */
export function sanitizedEnv(extra: Record<string, string> = {}): Record<string, string> {
  const env: Record<string, string> = {};
  for (const [k, v] of Object.entries(process.env)) {
    if (v !== undefined && k !== "OPENAI_API_KEY" && k !== "CODEX_API_KEY") env[k] = v;
  }
  return { ...env, ...extra };
}

/** Render a non-message thread item as a short live-progress line (or skip it). */
export function formatItem(item: ThreadItem): string | undefined {
  switch (item.type) {
    case "command_execution": {
      const cmd = item.command.replace(/\s+/g, " ").trim();
      return `$ ${truncate(cmd, 120)}`;
    }
    case "file_change": {
      const n = item.changes?.length ?? 0;
      return `✏️ ${n} file${n === 1 ? "" : "s"} changed`;
    }
    case "mcp_tool_call":
      return `🔧 ${item.server}.${item.tool}`;
    case "web_search":
      return `🔍 ${truncate(item.query, 120)}`;
    case "error":
      return `⚠️ ${truncate(item.message, 200)}`;
    default:
      // agent_message is the final answer (handled separately); reasoning/todo_list are noisy.
      return undefined;
  }
}

export function friendlyError(detail: string): string {
  const clipped = detail.slice(0, 1500);
  const authish = /login|auth|401|unauthor|credential|expired/i.test(clipped);
  return authish
    ? `⚠️ Codex couldn't run — looks like an auth problem. Re-run \`codex login\` on the host ` +
        `(against the persistent CODEX_HOME).\n\n${clipped}`
    : `⚠️ Codex error.\n\n${clipped}`;
}

export function createCodexRunner(config: Config): CodexRunner {
  const codex = new Codex({
    env: sanitizedEnv(),
    ...(config.codexPathOverride ? { codexPathOverride: config.codexPathOverride } : {}),
  });

  const threadOptions = {
    workingDirectory: config.workspaceDir,
    skipGitRepoCheck: true,
    sandboxMode: config.sandboxMode,
    approvalPolicy: "never" as const,
  };

  return {
    async run({ prompt, resumeThreadId, onProgress, signal, onThreadId }: CodexRunArgs): Promise<CodexTurn> {
      const thread = resumeThreadId
        ? codex.resumeThread(resumeThreadId, threadOptions)
        : codex.startThread(threadOptions);

      logger.debug("Codex turn", { resume: Boolean(resumeThreadId), cwd: config.workspaceDir });

      try {
        const { events } = await thread.runStreamed(prompt, signal ? { signal } : undefined);
        if (thread.id && onThreadId) onThreadId(thread.id);
        const agentMessages: string[] = [];
        let failure: string | undefined;

        for await (const event of events) {
          switch (event.type) {
            case "item.completed": {
              const item = event.item;
              if (item.type === "agent_message") agentMessages.push(item.text);
              else {
                const note = formatItem(item);
                if (note && onProgress) onProgress(note);
              }
              break;
            }
            case "turn.failed":
              failure = event.error?.message ?? "Codex turn failed.";
              break;
            case "error":
              failure = event.message;
              break;
            default:
              break;
          }
        }

        const threadId = thread.id ?? resumeThreadId;
        if (failure) return { ok: false, threadId, finalResponse: friendlyError(failure) };

        const finalResponse = agentMessages.join("\n\n").trim() || NO_OUTPUT;
        return { ok: true, threadId, finalResponse };
      } catch (err) {
        return {
          ok: false,
          threadId: thread.id ?? resumeThreadId,
          finalResponse: friendlyError((err as Error).message),
        };
      }
    },

    loginStatus: () => codexLoginStatus(config.codexPathOverride ?? "codex"),
  };
}

function truncate(s: string, max: number): string {
  return s.length > max ? `${s.slice(0, max - 1)}…` : s;
}

/**
 * Probe ChatGPT-subscription auth via `codex login status` (SPEC §4). The SDK has no
 * login-status API, so we shell out to the CLI (same CODEX_HOME, so the same auth).
 */
export function codexLoginStatus(
  bin: string,
  env: NodeJS.ProcessEnv = process.env,
): Promise<{ ok: boolean; detail: string }> {
  const childEnv = { ...env };
  delete childEnv.OPENAI_API_KEY;
  delete childEnv.CODEX_API_KEY;
  return new Promise((resolve) => {
    const child = spawn(bin, ["login", "status"], {
      env: childEnv,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let out = "";
    child.stdout.on("data", (d: Buffer) => (out += d.toString()));
    child.stderr.on("data", (d: Buffer) => (out += d.toString()));
    child.on("error", (err) => resolve({ ok: false, detail: err.message }));
    child.on("close", (code) => resolve({ ok: code === 0, detail: out.trim() }));
  });
}
