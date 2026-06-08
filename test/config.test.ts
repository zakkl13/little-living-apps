import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { ConfigError, loadConfig } from "../src/config.js";

const base: NodeJS.ProcessEnv = {
  TELEGRAM_BOT_TOKEN: "tok",
  ALLOWED_USER_IDS: "1,2,3",
  ANTHROPIC_API_KEY: "sk-ant-test",
};

describe("loadConfig", () => {
  it("parses required values and applies defaults", () => {
    const c = loadConfig(base);
    assert.deepEqual(c.allowedUserIds, [1, 2, 3]);
    assert.equal(c.sandboxMode, "danger-full-access");
    assert.equal(c.workspaceDir, "/srv/app");
    assert.equal(c.appPublicUrl, "", "no app published by default");
    assert.equal(c.telegramApiBaseUrl, "https://api.telegram.org");
  });

  it("loads v0.2 manager config (key + memory/state dirs + models)", () => {
    const c = loadConfig(base);
    assert.equal(c.anthropicApiKey, "sk-ant-test");
    assert.equal(c.managerModel, "claude-opus-4-8");
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
    // Missing TELEGRAM_BOT_TOKEN.
    assert.throws(() => loadConfig({ ALLOWED_USER_IDS: "1", ANTHROPIC_API_KEY: "k" }), ConfigError);
    // Missing ALLOWED_USER_IDS.
    assert.throws(() => loadConfig({ TELEGRAM_BOT_TOKEN: "t", ANTHROPIC_API_KEY: "k" }), ConfigError);
  });

  it("rejects non-integer user ids and unknown sandbox modes", () => {
    assert.throws(() => loadConfig({ ...base, ALLOWED_USER_IDS: "1,abc" }), ConfigError);
    assert.throws(() => loadConfig({ ...base, CODEX_SANDBOX_MODE: "nonsense" }), ConfigError);
  });

  it("trims trailing slashes on the app URL and the Telegram API base URL", () => {
    const c = loadConfig({
      ...base,
      APP_PUBLIC_URL: "https://app.example.com/",
      TELEGRAM_API_BASE_URL: "https://api.telegram.org/",
    });
    assert.equal(c.appPublicUrl, "https://app.example.com");
    assert.equal(c.telegramApiBaseUrl, "https://api.telegram.org");
  });
});
