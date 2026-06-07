import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { ConfigError, loadConfig } from "../src/config.js";

const base: NodeJS.ProcessEnv = {
  TELEGRAM_BOT_TOKEN: "tok",
  ALLOWED_USER_IDS: "1,2,3",
  TELEGRAM_WEBHOOK_SECRET: "s3cr3t",
  ANTHROPIC_API_KEY: "sk-ant-test",
};

describe("loadConfig", () => {
  it("parses required values and applies defaults", () => {
    const c = loadConfig(base);
    assert.deepEqual(c.allowedUserIds, [1, 2, 3]);
    assert.equal(c.sandboxMode, "danger-full-access");
    assert.equal(c.port, 8080);
    assert.equal(c.workspaceDir, "/workspace/project");
    assert.equal(c.webhookPath, "/tg/s3cr3t");
    assert.equal(c.telegramApiBaseUrl, "https://api.telegram.org");
  });

  it("loads v0.2 manager config (key + memory/state dirs + models)", () => {
    const c = loadConfig(base);
    assert.equal(c.anthropicApiKey, "sk-ant-test");
    assert.equal(c.managerModel, "claude-opus-4-8");
    assert.equal(c.utilityModel, "claude-haiku-4-5");
    assert.match(c.memoryDir, /memory$/);
    assert.match(c.managerStateDir, /state$/);
  });

  it("requires ANTHROPIC_API_KEY (the manager's paid plane)", () => {
    const { ANTHROPIC_API_KEY: _omit, ...withoutKey } = base;
    assert.throws(
      () => loadConfig(withoutKey),
      (e: unknown) => e instanceof ConfigError && /ANTHROPIC_API_KEY/.test((e as Error).message),
    );
  });

  it("refuses to start when a billing-flip API key is set", () => {
    assert.throws(
      () => loadConfig({ ...base, OPENAI_API_KEY: "sk-123" }),
      (e: unknown) => e instanceof ConfigError && /OPENAI_API_KEY/.test((e as Error).message),
    );
    assert.throws(
      () => loadConfig({ ...base, CODEX_API_KEY: "sk-456" }),
      (e: unknown) => e instanceof ConfigError && /CODEX_API_KEY/.test((e as Error).message),
    );
  });

  it("throws on missing required vars", () => {
    assert.throws(() => loadConfig({ ALLOWED_USER_IDS: "1", TELEGRAM_WEBHOOK_SECRET: "x" }), ConfigError);
    assert.throws(() => loadConfig({ TELEGRAM_BOT_TOKEN: "t", TELEGRAM_WEBHOOK_SECRET: "x" }), ConfigError);
  });

  it("rejects non-integer user ids and unknown sandbox modes", () => {
    assert.throws(() => loadConfig({ ...base, ALLOWED_USER_IDS: "1,abc" }), ConfigError);
    assert.throws(() => loadConfig({ ...base, CODEX_SANDBOX_MODE: "nonsense" }), ConfigError);
  });

  it("honors an explicit webhook path and trims trailing slashes on URLs", () => {
    const c = loadConfig({
      ...base,
      WEBHOOK_PATH: "hook",
      PUBLIC_URL: "https://x.fly.dev/",
      TELEGRAM_API_BASE_URL: "https://api.telegram.org/",
    });
    assert.equal(c.webhookPath, "/hook");
    assert.equal(c.publicUrl, "https://x.fly.dev");
    assert.equal(c.telegramApiBaseUrl, "https://api.telegram.org");
  });
});
