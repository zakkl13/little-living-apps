// Environment loading + validation (SPEC §9, §4).
//
// The bot REFUSES to start if OPENAI_API_KEY or CODEX_API_KEY is set, because either silently
// flips Codex from the ChatGPT subscription to metered API billing (SPEC §4 / §13.1).

// Sandbox vocabulary is the Codex SDK's own (ThreadOptions.sandboxMode). "danger-full-access"
// is full dangerous access with NO sandbox init (no Landlock/seccomp) — the disposable VM is the
// isolation boundary — and we pair it with approvalPolicy "never" in the runner.
export type SandboxMode = "read-only" | "workspace-write" | "danger-full-access";

const SANDBOX_MODES: readonly SandboxMode[] = ["read-only", "workspace-write", "danger-full-access"];

// The manager thread's reasoning effort (Codex ModelReasoningEffort). xhigh is the target (§4).
export type ReasoningEffort = "minimal" | "low" | "medium" | "high" | "xhigh";

const REASONING_EFFORTS: readonly ReasoningEffort[] = ["minimal", "low", "medium", "high", "xhigh"];

export interface Config {
  telegramBotToken: string;
  allowedUserIds: number[];
  /** Where the app the agent builds is served (env APP_PUBLIC_URL), surfaced to the manager
   *  prompt. Empty = not yet published (the app is private until you choose to expose it). */
  appPublicUrl: string;
  /** Holds the app the agent builds and maintains (DESIGN §10). */
  workspaceDir: string;
  sandboxMode: SandboxMode;
  /** Telegram Bot API base URL; overridden in tests. */
  telegramApiBaseUrl: string;
  /** Absolute path to a specific codex binary for the SDK; undefined = SDK default. */
  codexPathOverride?: string;

  // --- v0.3 manager tier: the manager is a Codex thread (MIGRATION-CODEX.md) ---
  /** Manager memory repo, exposed to the memory tools as /memories (git markdown + FTS). */
  memoryDir: string;
  /** Thread-id + queue snapshots for cold-wake recovery (MIGRATION-CODEX.md §7). */
  managerStateDir: string;
  /** Working directory holding the manager's AGENTS.md (defaults under MANAGER_STATE_DIR). */
  managerDir: string;
  /** Strongest Codex model driving the manager thread; undefined → the SDK/CLI default. */
  managerModel?: string;
  /** Manager reasoning effort (default xhigh). */
  managerReasoningEffort: ReasoningEffort;
  /** Loopback port for the in-process Lila MCP server; undefined → a free port is chosen. */
  lilaMcpPort?: number;
  /** Bearer token for the Lila MCP server; undefined → auto-generated per boot. */
  lilaMcpToken?: string;

  // --- Inspector: read-only observability plane (off by default) ---
  /** Stand up the localhost Inspector HTTP server (env INSPECTOR_ENABLED=true). */
  inspectorEnabled: boolean;
  /** Port the Inspector binds on 127.0.0.1; Caddy fronts it at /_inspect. */
  inspectorPort: number;
  /** Shared secret required on every Inspector request (defense in depth even on localhost). */
  inspectorToken?: string;
}

export class ConfigError extends Error {}

function required(env: NodeJS.ProcessEnv, key: string): string {
  const v = env[key];
  if (v === undefined || v.trim() === "") {
    throw new ConfigError(`Missing required env var: ${key}`);
  }
  return v.trim();
}

function parseUserIds(raw: string): number[] {
  const ids = raw
    .split(",")
    .map((s) => s.trim())
    .filter((s) => s.length > 0)
    .map((s) => {
      const n = Number(s);
      if (!Number.isInteger(n)) {
        throw new ConfigError(`ALLOWED_USER_IDS contains a non-integer value: "${s}"`);
      }
      return n;
    });
  if (ids.length === 0) {
    throw new ConfigError("ALLOWED_USER_IDS must contain at least one user id");
  }
  return ids;
}

export function loadConfig(env: NodeJS.ProcessEnv = process.env): Config {
  // Hard stop: a stray API key would move us to metered API billing (SPEC §4).
  for (const key of ["OPENAI_API_KEY", "CODEX_API_KEY"] as const) {
    if (env[key] && env[key]!.trim() !== "") {
      throw new ConfigError(
        `${key} is set. This would switch Codex to metered API billing instead of the ` +
          `ChatGPT subscription. Unset it before starting the bot (SPEC §4).`,
      );
    }
  }

  const telegramBotToken = required(env, "TELEGRAM_BOT_TOKEN");
  const allowedUserIds = parseUserIds(required(env, "ALLOWED_USER_IDS"));

  const sandboxRaw = (env.CODEX_SANDBOX_MODE ?? "danger-full-access").trim() as SandboxMode;
  if (!SANDBOX_MODES.includes(sandboxRaw)) {
    throw new ConfigError(
      `CODEX_SANDBOX_MODE must be one of ${SANDBOX_MODES.join(", ")} (got "${sandboxRaw}")`,
    );
  }

  const reasoningRaw = (env.MANAGER_REASONING_EFFORT ?? "xhigh").trim() as ReasoningEffort;
  if (!REASONING_EFFORTS.includes(reasoningRaw)) {
    throw new ConfigError(
      `MANAGER_REASONING_EFFORT must be one of ${REASONING_EFFORTS.join(", ")} (got "${reasoningRaw}")`,
    );
  }

  const codexPathOverride = env.CODEX_BIN?.trim() || undefined;
  const managerStateDir = env.MANAGER_STATE_DIR?.trim() || "/var/lib/lila/state";
  const lilaMcpPort = env.LILA_MCP_PORT?.trim() ? numEnv(env.LILA_MCP_PORT, 0) : undefined;
  const lilaMcpToken = env.LILA_MCP_TOKEN?.trim() || undefined;
  const managerModel = env.MANAGER_MODEL?.trim() || undefined;

  const inspectorEnabled = /^(1|true|yes)$/i.test(env.INSPECTOR_ENABLED?.trim() ?? "");
  const inspectorPort = numEnv(env.INSPECTOR_PORT, 9090);
  const inspectorToken = env.INSPECTOR_TOKEN?.trim() || undefined;

  return {
    telegramBotToken,
    allowedUserIds,
    appPublicUrl: (env.APP_PUBLIC_URL?.trim() ?? "").replace(/\/+$/, ""),
    workspaceDir: env.WORKSPACE_DIR?.trim() || "/srv/app",
    sandboxMode: sandboxRaw,
    telegramApiBaseUrl: (env.TELEGRAM_API_BASE_URL?.trim() || "https://api.telegram.org").replace(
      /\/+$/,
      "",
    ),
    ...(codexPathOverride ? { codexPathOverride } : {}),

    memoryDir: env.MEMORY_DIR?.trim() || "/var/lib/lila/memory",
    managerStateDir,
    managerDir: env.MANAGER_DIR?.trim() || `${managerStateDir}/manager`,
    ...(managerModel ? { managerModel } : {}),
    managerReasoningEffort: reasoningRaw,
    ...(lilaMcpPort !== undefined ? { lilaMcpPort } : {}),
    ...(lilaMcpToken ? { lilaMcpToken } : {}),

    inspectorEnabled,
    inspectorPort,
    ...(inspectorToken ? { inspectorToken } : {}),
  };
}

function numEnv(raw: string | undefined, fallback: number): number {
  const n = Number(raw?.trim());
  return Number.isFinite(n) && n >= 0 ? n : fallback;
}
