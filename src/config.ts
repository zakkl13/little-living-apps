// Environment loading + validation (SPEC §9, §4).
//
// The bot REFUSES to start if OPENAI_API_KEY or CODEX_API_KEY is set, because either silently
// flips Codex from the ChatGPT subscription to metered API billing (SPEC §4 / §13.1).

// Sandbox vocabulary is the Codex SDK's own (ThreadOptions.sandboxMode). "danger-full-access"
// is full dangerous access with NO sandbox init (no Landlock/seccomp) — the Sprite is the
// isolation boundary (SPEC §7) — and we pair it with approvalPolicy "never" in the runner.
export type SandboxMode = "read-only" | "workspace-write" | "danger-full-access";

const SANDBOX_MODES: readonly SandboxMode[] = ["read-only", "workspace-write", "danger-full-access"];

export interface Config {
  telegramBotToken: string;
  allowedUserIds: number[];
  webhookSecret: string;
  webhookPath: string;
  port: number;
  /** Public HTTPS base URL of the Sprite, used to register the webhook. Empty = manual. */
  publicUrl: string;
  /** Holds project repos that workers operate on (DESIGN §10). */
  workspaceDir: string;
  sessionStorePath: string;
  sandboxMode: SandboxMode;
  /** Telegram Bot API base URL; overridden in tests. */
  telegramApiBaseUrl: string;
  /** Absolute path to a specific codex binary for the SDK; undefined = SDK default. */
  codexPathOverride?: string;

  // --- v0.2 manager tier (DESIGN §10) ---
  /** Anthropic API key — the manager's only paid plane. Required in v0.2. */
  anthropicApiKey: string;
  /** Manager memory repo, exposed to the memory tool as /memories (git markdown + FTS). */
  memoryDir: string;
  /** Transcript + queue snapshots for cold-wake recovery (DESIGN §11). */
  managerStateDir: string;
  /** Opus-class model driving the manager loop. */
  managerModel: string;
  /** Cheap model for condensing over-long worker output + idle memory hygiene. */
  utilityModel: string;
  /** Anthropic Messages base URL; overridden in tests to point at a fake (no real API). */
  anthropicBaseUrl?: string;
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
  const webhookSecret = required(env, "TELEGRAM_WEBHOOK_SECRET");
  const anthropicApiKey = required(env, "ANTHROPIC_API_KEY");

  const sandboxRaw = (env.CODEX_SANDBOX_MODE ?? "danger-full-access").trim() as SandboxMode;
  if (!SANDBOX_MODES.includes(sandboxRaw)) {
    throw new ConfigError(
      `CODEX_SANDBOX_MODE must be one of ${SANDBOX_MODES.join(", ")} (got "${sandboxRaw}")`,
    );
  }

  const webhookPath = normalizePath(env.WEBHOOK_PATH?.trim() || `/tg/${webhookSecret}`);

  const portRaw = env.PORT?.trim() || "8080";
  const port = Number(portRaw);
  if (!Number.isInteger(port) || port < 0 || port > 65535) {
    throw new ConfigError(`PORT must be a valid port number (got "${portRaw}")`);
  }

  const codexPathOverride = env.CODEX_BIN?.trim() || undefined;

  return {
    telegramBotToken,
    allowedUserIds,
    webhookSecret,
    webhookPath,
    port,
    publicUrl: (env.PUBLIC_URL?.trim() ?? "").replace(/\/+$/, ""),
    workspaceDir: env.WORKSPACE_DIR?.trim() || "/workspace/project",
    sessionStorePath: env.SESSION_STORE_PATH?.trim() || "/workspace/.sessions.json",
    sandboxMode: sandboxRaw,
    telegramApiBaseUrl: (env.TELEGRAM_API_BASE_URL?.trim() || "https://api.telegram.org").replace(
      /\/+$/,
      "",
    ),
    ...(codexPathOverride ? { codexPathOverride } : {}),

    anthropicApiKey,
    memoryDir: env.MEMORY_DIR?.trim() || "/workspace/.manager/memory",
    managerStateDir: env.MANAGER_STATE_DIR?.trim() || "/workspace/.manager/state",
    managerModel: env.MANAGER_MODEL?.trim() || "claude-opus-4-8",
    utilityModel: env.UTILITY_MODEL?.trim() || "claude-haiku-4-5",
    ...(env.ANTHROPIC_BASE_URL?.trim() ? { anthropicBaseUrl: env.ANTHROPIC_BASE_URL.trim() } : {}),
  };
}

function normalizePath(p: string): string {
  return p.startsWith("/") ? p : `/${p}`;
}
