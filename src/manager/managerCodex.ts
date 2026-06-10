// The manager's model seam (MIGRATION-CODEX.md §4). The manager is no longer an Anthropic
// createMessage loop — it IS a long-lived Codex thread. This module owns the locked-down Codex
// construction and exposes a tiny factory the ManagerDriver consumes, so tests can inject a scripted
// fake thread instead of the real SDK (the same seam discipline the worker CodexRunner uses).
//
// Capability boundary (§4): shell_tool off, web_search off, read-only sandbox, network off — the
// manager operates ONLY through the Lila MCP tools. view_image stays ON so it can see owner-sent
// screenshots. Auth rides the cached ChatGPT-subscription login; the billing-flip keys are stripped.

import {
  Codex,
  type Input,
  type ModelReasoningEffort,
  type ThreadEvent,
} from "@openai/codex-sdk";

import { sanitizedEnv } from "../workers/runner.js";

/** The slice of a Codex Thread the driver drives. `Thread` from the SDK satisfies this structurally,
 *  so the real factory just returns startThread()/resumeThread(); fakes implement it directly. */
export interface ManagerThread {
  /** Thread id, populated once the first turn has started; used to resume after a restart. */
  readonly id: string | null;
  runStreamed(input: Input, opts?: { signal?: AbortSignal }): Promise<{ events: AsyncGenerator<ThreadEvent> }>;
}

export interface ManagerThreadFactory {
  /** Begin a brand-new manager thread (no prior rollout). */
  start(): ManagerThread;
  /** Resume a persisted manager thread by id (Codex owns the rollout on disk). */
  resume(threadId: string): ManagerThread;
}

export interface ManagerCodexOptions {
  /** Strongest Codex model; undefined → the SDK/CLI default. */
  model?: string;
  reasoningEffort: ModelReasoningEffort;
  /** Working directory holding the manager's AGENTS.md. */
  managerDir: string;
  /** Loopback URL of the Lila MCP server (…/mcp). */
  mcpUrl: string;
  /** Bearer token for the Lila MCP server; injected into the CLI env under this var name. */
  mcpToken: string;
  /** Optional override for the codex binary (host bring-up gotcha). */
  codexPathOverride?: string;
}

const MCP_TOKEN_ENV_VAR = "LILA_MCP_TOKEN";

export function createManagerThreadFactory(opts: ManagerCodexOptions): ManagerThreadFactory {
  const codex = new Codex({
    env: sanitizedEnv({ [MCP_TOKEN_ENV_VAR]: opts.mcpToken }),
    ...(opts.codexPathOverride ? { codexPathOverride: opts.codexPathOverride } : {}),
    config: {
      features: { shell_tool: false },
      tools: { web_search: false, view_image: true },
      web_search: false,
      mcp_servers: {
        lila: { url: opts.mcpUrl, bearer_token_env_var: MCP_TOKEN_ENV_VAR },
      },
    },
  });

  const threadOptions = {
    ...(opts.model ? { model: opts.model } : {}),
    workingDirectory: opts.managerDir,
    skipGitRepoCheck: true,
    sandboxMode: "read-only" as const,
    networkAccessEnabled: false,
    webSearchEnabled: false,
    modelReasoningEffort: opts.reasoningEffort,
    approvalPolicy: "never" as const,
  };

  return {
    start: () => codex.startThread(threadOptions),
    resume: (threadId) => codex.resumeThread(threadId, threadOptions),
  };
}
