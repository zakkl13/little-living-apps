import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { ConfigError, loadConfig } from "../src/config.js";

const base: NodeJS.ProcessEnv = {
  TELEGRAM_BOT_TOKEN: "tok",
  ALLOWED_USER_IDS: "1,2,3",
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

  it("loads v0.3 Codex-manager config (no API key; memory/state/manager dirs + effort)", () => {
    const c = loadConfig(base);
    assert.equal(c.managerModel, undefined, "defaults to the Codex SDK's own model");
    assert.equal(c.managerReasoningEffort, "xhigh");
    assert.match(c.memoryDir, /memory$/);
    assert.match(c.managerStateDir, /state$/);
    assert.match(c.managerDir, /state\/manager$/, "manager dir defaults under the state dir");
  });

  it("honors explicit manager model / effort / MCP overrides", () => {
    const c = loadConfig({
      ...base,
      MANAGER_MODEL: "gpt-5-codex",
      MANAGER_REASONING_EFFORT: "high",
      MANAGER_DIR: "/tmp/mgr",
      LILA_MCP_PORT: "8765",
      LILA_MCP_TOKEN: "secret",
    });
    assert.equal(c.managerModel, "gpt-5-codex");
    assert.equal(c.managerReasoningEffort, "high");
    assert.equal(c.managerDir, "/tmp/mgr");
    assert.equal(c.lilaMcpPort, 8765);
    assert.equal(c.lilaMcpToken, "secret");
  });

  it("rejects an unknown reasoning effort", () => {
    assert.throws(
      () => loadConfig({ ...base, MANAGER_REASONING_EFFORT: "ultra" }),
      (e: unknown) => e instanceof ConfigError && /MANAGER_REASONING_EFFORT/.test((e as Error).message),
    );
  });

  it("refuses to start when a billing-flip API key is set (the only billing guard now)", () => {
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
    assert.throws(() => loadConfig({ ALLOWED_USER_IDS: "1" }), ConfigError); // no TELEGRAM_BOT_TOKEN
    assert.throws(() => loadConfig({ TELEGRAM_BOT_TOKEN: "t" }), ConfigError); // no ALLOWED_USER_IDS
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
