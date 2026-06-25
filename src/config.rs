//! Environment loading + validation.
//!
//! The bot REFUSES to start if a billing-flip API key is set, because it silently moves the active
//! backend off its subscription onto metered API billing. Which key is fatal depends on the backend:
//! codex (default) refuses `OPENAI_API_KEY` / `CODEX_API_KEY` (ChatGPT subscription → OpenAI
//! billing); claude refuses `ANTHROPIC_API_KEY` (Claude Pro/Max subscription → Anthropic billing).
//! The Claude backend rides the subscription via `CLAUDE_CODE_OAUTH_TOKEN`; Codex via `CODEX_HOME`.

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use thiserror::Error;

/// Which agent backend drives the manager thread and the workers. Both ride a subscription, never
/// metered API billing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentBackend {
    /// OpenAI Codex (the default / original).
    Codex,
    /// Anthropic Claude (Agent SDK).
    Claude,
}

impl AgentBackend {
    /// All backends, for help text and validation.
    pub const ALL: [AgentBackend; 2] = [AgentBackend::Codex, AgentBackend::Claude];

    /// Canonical lowercase name.
    pub fn as_str(self) -> &'static str {
        match self {
            AgentBackend::Codex => "codex",
            AgentBackend::Claude => "claude",
        }
    }

    /// The subscription this backend rides (for billing-guard messaging).
    fn subscription(self) -> &'static str {
        match self {
            AgentBackend::Codex => "ChatGPT subscription",
            AgentBackend::Claude => "Claude Pro/Max subscription",
        }
    }

    /// The API keys that, if set, would flip this backend onto metered billing.
    fn billing_flip_keys(self) -> &'static [&'static str] {
        match self {
            AgentBackend::Codex => &["OPENAI_API_KEY", "CODEX_API_KEY"],
            AgentBackend::Claude => &["ANTHROPIC_API_KEY"],
        }
    }
}

impl fmt::Display for AgentBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for AgentBackend {
    type Err = ConfigError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "codex" => Ok(AgentBackend::Codex),
            "claude" => Ok(AgentBackend::Claude),
            other => Err(ConfigError::Invalid(format!(
                "AGENT_BACKEND must be one of codex, claude (got \"{other}\")"
            ))),
        }
    }
}

/// Filesystem sandbox level (Codex vocabulary; mirrored to the Codex SDK `SandboxMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxMode {
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}

impl SandboxMode {
    pub fn as_str(self) -> &'static str {
        match self {
            SandboxMode::ReadOnly => "read-only",
            SandboxMode::WorkspaceWrite => "workspace-write",
            SandboxMode::DangerFullAccess => "danger-full-access",
        }
    }
}

impl FromStr for SandboxMode {
    type Err = ConfigError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "read-only" => Ok(SandboxMode::ReadOnly),
            "workspace-write" => Ok(SandboxMode::WorkspaceWrite),
            "danger-full-access" => Ok(SandboxMode::DangerFullAccess),
            other => Err(ConfigError::Invalid(format!(
                "CODEX_SANDBOX_MODE must be read-only, workspace-write, or danger-full-access (got \"{other}\")"
            ))),
        }
    }
}

/// The manager thread's reasoning effort.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningEffort {
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
}

impl ReasoningEffort {
    pub fn as_str(self) -> &'static str {
        match self {
            ReasoningEffort::Minimal => "minimal",
            ReasoningEffort::Low => "low",
            ReasoningEffort::Medium => "medium",
            ReasoningEffort::High => "high",
            ReasoningEffort::XHigh => "xhigh",
        }
    }
}

impl FromStr for ReasoningEffort {
    type Err = ConfigError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "minimal" => Ok(ReasoningEffort::Minimal),
            "low" => Ok(ReasoningEffort::Low),
            "medium" => Ok(ReasoningEffort::Medium),
            "high" => Ok(ReasoningEffort::High),
            "xhigh" => Ok(ReasoningEffort::XHigh),
            other => Err(ConfigError::Invalid(format!(
                "MANAGER_REASONING_EFFORT must be minimal|low|medium|high|xhigh (got \"{other}\")"
            ))),
        }
    }
}

/// Default models per backend (Codex uses the SDK default = `None`).
const CLAUDE_MANAGER_MODEL: &str = "claude-opus-4-8";
const CLAUDE_WORKER_MODEL: &str = "claude-sonnet-4-6";

/// Validated runtime configuration.
#[derive(Debug, Clone)]
pub struct Config {
    pub agent_backend: AgentBackend,
    pub telegram_bot_token: String,
    pub allowed_user_ids: Vec<i64>,
    /// Where the app the agent builds is served (derived from `LILA_DOMAIN`). Empty = unpublished.
    pub app_public_url: String,
    /// Holds the app the agent builds and maintains.
    pub workspace_dir: String,
    /// systemd unit name for the app, so a worker restarts the RIGHT app.
    pub app_service_name: String,
    /// Active stack plugin (`stacks/<stack>/`), chosen by `LILA_STACK` (default `rails-pwa`). Decides
    /// the *kind of app* the team builds — scaffold, serve, prompts, eval fixture all read it.
    pub stack: String,
    /// Active design choice for this instance, chosen by `LILA_DESIGN` (default `random`). One of
    /// `random` (blind draw from the safe default pool), `random:<seed>` (reproducible), or a
    /// `<brand>` pin (the escape hatch, reaching any pool). Resolved against the vendored catalog at
    /// scaffold time; see [`crate::design`].
    pub design: String,
    pub sandbox_mode: SandboxMode,
    /// Telegram Bot API base URL (overridden in tests).
    pub telegram_api_base_url: String,
    /// Absolute path to a specific codex binary; `None` = resolve from PATH / SDK default.
    pub codex_path_override: Option<String>,
    /// Absolute path to a specific claude binary; `None` = resolve from PATH.
    pub claude_path_override: Option<String>,

    // --- manager tier ---
    /// Manager memory repo, exposed as `/memories` (git markdown + FTS).
    pub memory_dir: String,
    /// Thread-id + queue snapshots for cold-restart recovery.
    pub manager_state_dir: String,
    /// Working directory holding the manager's AGENTS.md.
    pub manager_dir: String,
    /// Strongest model driving the manager; `None` → backend default.
    pub manager_model: Option<String>,
    /// Model for ephemeral workers; `None` → backend default.
    pub worker_model: Option<String>,
    /// Manager reasoning effort.
    pub manager_reasoning_effort: ReasoningEffort,
    /// Loopback port for the Lila MCP server; `None` → a free port is chosen.
    pub lila_mcp_port: Option<u16>,
    /// Bearer token for the Lila MCP server; `None` → auto-generated per boot.
    pub lila_mcp_token: Option<String>,

    // --- Inspector ---
    pub inspector_enabled: bool,
    pub inspector_port: u16,
    pub inspector_token: Option<String>,
}

/// Configuration errors (fatal at startup / on `/backend` swap).
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Missing required env var: {0}")]
    Missing(String),
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    BillingGuard(String),
}

/// Every billing-flip key, across backends: a stray one moves the active agent off its subscription
/// onto metered API billing. We strip ALL of them from the env handed to any spawned CLI — defense
/// in depth, regardless of which backend is active.
pub const ALL_BILLING_FLIP_KEYS: &[&str] =
    &["OPENAI_API_KEY", "CODEX_API_KEY", "ANTHROPIC_API_KEY"];

/// Build the env handed to a spawned CLI: inherit everything except billing-flip keys, then layer on
/// `extra`.
pub fn sanitized_env(extra: &[(&str, &str)]) -> HashMap<String, String> {
    let mut env: HashMap<String, String> = std::env::vars()
        .filter(|(k, _)| !ALL_BILLING_FLIP_KEYS.contains(&k.as_str()))
        .collect();
    for (k, v) in extra {
        env.insert((*k).to_string(), (*v).to_string());
    }
    env
}

/// A read-only view over environment variables, so config loading is testable without `std::env`.
pub type Env = HashMap<String, String>;

/// Snapshot the real process environment into an [`Env`].
pub fn process_env() -> Env {
    std::env::vars().collect()
}

fn get_trimmed(env: &Env, key: &str) -> Option<String> {
    env.get(key)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn required(env: &Env, key: &str) -> Result<String, ConfigError> {
    get_trimmed(env, key).ok_or_else(|| ConfigError::Missing(key.to_string()))
}

/// If a backend's billing-flip key is set in `env`, return an explanatory error string.
pub fn billing_guard_error(backend: AgentBackend, env: &Env) -> Option<String> {
    for key in backend.billing_flip_keys() {
        if get_trimmed(env, key).is_some() {
            return Some(format!(
                "{key} is set. This would switch the {backend} backend to metered API billing \
                 instead of the {}. Unset it first.",
                backend.subscription()
            ));
        }
    }
    None
}

fn parse_user_ids(raw: &str) -> Result<Vec<i64>, ConfigError> {
    let ids: Vec<i64> = raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.parse::<i64>().map_err(|_| {
                ConfigError::Invalid(format!(
                    "ALLOWED_USER_IDS contains a non-integer value: \"{s}\""
                ))
            })
        })
        .collect::<Result<_, _>>()?;
    if ids.is_empty() {
        return Err(ConfigError::Invalid(
            "ALLOWED_USER_IDS must contain at least one user id".to_string(),
        ));
    }
    Ok(ids)
}

fn parse_u16(env: &Env, key: &str, fallback: u16) -> u16 {
    get_trimmed(env, key)
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(fallback)
}

fn is_truthy(v: Option<String>) -> bool {
    matches!(
        v.as_deref(),
        Some("1" | "true" | "yes" | "TRUE" | "True" | "YES" | "Yes")
    )
}

/// Pick the manager/worker models: explicit env wins; else Claude pins defaults, Codex uses `None`.
fn pick_model(env: &Env, key: &str, claude_default: &str, claude: bool) -> Option<String> {
    get_trimmed(env, key).or_else(|| claude.then(|| claude_default.to_string()))
}

/// The fallible (required + validated-enum) tier of config, resolved in one place.
struct Validated {
    telegram_bot_token: String,
    allowed_user_ids: Vec<i64>,
    sandbox_mode: SandboxMode,
    manager_reasoning_effort: ReasoningEffort,
}

impl Validated {
    fn from_env(env: &Env) -> Result<Self, ConfigError> {
        Ok(Self {
            telegram_bot_token: required(env, "TELEGRAM_BOT_TOKEN")?,
            allowed_user_ids: parse_user_ids(&required(env, "ALLOWED_USER_IDS")?)?,
            sandbox_mode: get_trimmed(env, "CODEX_SANDBOX_MODE")
                .map_or(Ok(SandboxMode::DangerFullAccess), |s| s.parse())?,
            manager_reasoning_effort: get_trimmed(env, "MANAGER_REASONING_EFFORT")
                .map_or(Ok(ReasoningEffort::XHigh), |s| s.parse())?,
        })
    }
}

/// The filesystem-path tier of config, resolved together to keep `from_env` simple.
struct Paths {
    workspace_dir: String,
    app_service_name: String,
    memory_dir: String,
    manager_dir: String,
    manager_state_dir: String,
}

impl Paths {
    fn from_env(env: &Env) -> Self {
        let manager_state_dir =
            get_trimmed(env, "MANAGER_STATE_DIR").unwrap_or_else(|| "/var/lib/lila/state".into());
        let manager_dir = get_trimmed(env, "MANAGER_DIR")
            .unwrap_or_else(|| format!("{manager_state_dir}/manager"));
        Self {
            workspace_dir: get_trimmed(env, "WORKSPACE_DIR")
                .unwrap_or_else(|| "/srv/primary".into()),
            app_service_name: get_trimmed(env, "LILA_APP_SERVICE")
                .unwrap_or_else(|| "lila-app@primary".into()),
            memory_dir: get_trimmed(env, "MEMORY_DIR")
                .unwrap_or_else(|| "/var/lib/lila/memory".into()),
            manager_dir,
            manager_state_dir,
        }
    }
}

/// Derive the public URL from a bare `LILA_DOMAIN` host (empty = unpublished).
fn derive_public_url(env: &Env) -> String {
    let domain = get_trimmed(env, "LILA_DOMAIN")
        .unwrap_or_default()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string();
    if domain.is_empty() {
        String::new()
    } else {
        format!("https://{domain}")
    }
}

impl Config {
    /// Load and validate configuration from the process environment.
    pub fn load() -> Result<Self, ConfigError> {
        Self::from_env(&process_env())
    }

    /// Load and validate from an explicit environment map (used by tests).
    pub fn from_env(env: &Env) -> Result<Self, ConfigError> {
        let agent_backend = get_trimmed(env, "AGENT_BACKEND")
            .map_or(Ok(AgentBackend::Codex), |s| s.parse::<AgentBackend>())?;

        // Hard stop: a stray API key would move the active backend off its subscription.
        if let Some(msg) = billing_guard_error(agent_backend, env) {
            return Err(ConfigError::BillingGuard(msg));
        }

        let valid = Validated::from_env(env)?;
        let paths = Paths::from_env(env);
        let claude = agent_backend == AgentBackend::Claude;

        Ok(Config {
            agent_backend,
            telegram_bot_token: valid.telegram_bot_token,
            allowed_user_ids: valid.allowed_user_ids,
            app_public_url: derive_public_url(env),
            workspace_dir: paths.workspace_dir,
            app_service_name: paths.app_service_name,
            stack: get_trimmed(env, "LILA_STACK").unwrap_or_else(|| "rails-pwa".into()),
            design: get_trimmed(env, "LILA_DESIGN").unwrap_or_else(|| "random".into()),
            sandbox_mode: valid.sandbox_mode,
            telegram_api_base_url: get_trimmed(env, "TELEGRAM_API_BASE_URL")
                .unwrap_or_else(|| "https://api.telegram.org".into())
                .trim_end_matches('/')
                .to_string(),
            codex_path_override: get_trimmed(env, "CODEX_BIN"),
            claude_path_override: get_trimmed(env, "CLAUDE_BIN"),
            memory_dir: paths.memory_dir,
            manager_dir: paths.manager_dir,
            manager_state_dir: paths.manager_state_dir,
            manager_model: pick_model(env, "MANAGER_MODEL", CLAUDE_MANAGER_MODEL, claude),
            worker_model: pick_model(env, "WORKER_MODEL", CLAUDE_WORKER_MODEL, claude),
            manager_reasoning_effort: valid.manager_reasoning_effort,
            lila_mcp_port: get_trimmed(env, "LILA_MCP_PORT").and_then(|v| v.parse().ok()),
            lila_mcp_token: get_trimmed(env, "LILA_MCP_TOKEN"),
            inspector_enabled: is_truthy(get_trimmed(env, "INSPECTOR_ENABLED")),
            inspector_port: parse_u16(env, "INSPECTOR_PORT", 9090),
            inspector_token: get_trimmed(env, "INSPECTOR_TOKEN"),
        })
    }

    /// The owner's chat id (first allowed user).
    pub fn owner_user_id(&self) -> i64 {
        self.allowed_user_ids[0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_env() -> Env {
        Env::from([
            ("TELEGRAM_BOT_TOKEN".into(), "tok".into()),
            ("ALLOWED_USER_IDS".into(), "42".into()),
        ])
    }

    #[test]
    fn loads_defaults() {
        let cfg = Config::from_env(&base_env()).expect("loads");
        assert_eq!(cfg.agent_backend, AgentBackend::Codex);
        assert_eq!(cfg.manager_reasoning_effort, ReasoningEffort::XHigh);
        assert_eq!(cfg.allowed_user_ids, vec![42]);
        assert!(cfg.manager_model.is_none(), "codex uses SDK default model");
        assert_eq!(cfg.app_public_url, "");
        assert_eq!(cfg.stack, "rails-pwa", "default stack is rails-pwa");
        assert_eq!(cfg.design, "random", "default design is a safe blind draw");
    }

    #[test]
    fn lila_design_overrides_the_default() {
        let mut env = base_env();
        env.insert("LILA_DESIGN".into(), "stripe".into());
        let cfg = Config::from_env(&env).expect("loads");
        assert_eq!(cfg.design, "stripe");
    }

    #[test]
    fn lila_stack_overrides_the_default() {
        let mut env = base_env();
        env.insert("LILA_STACK".into(), "node-react".into());
        let cfg = Config::from_env(&env).expect("loads");
        assert_eq!(cfg.stack, "node-react");
    }

    #[test]
    fn claude_backend_pins_models() {
        let mut env = base_env();
        env.insert("AGENT_BACKEND".into(), "claude".into());
        let cfg = Config::from_env(&env).expect("loads");
        assert_eq!(cfg.manager_model.as_deref(), Some(CLAUDE_MANAGER_MODEL));
        assert_eq!(cfg.worker_model.as_deref(), Some(CLAUDE_WORKER_MODEL));
    }

    #[test]
    fn cli_path_overrides_are_loaded() {
        let mut env = base_env();
        env.insert("CODEX_BIN".into(), "/opt/bin/codex".into());
        env.insert("CLAUDE_BIN".into(), "/opt/bin/claude".into());
        let cfg = Config::from_env(&env).expect("loads");
        assert_eq!(cfg.codex_path_override.as_deref(), Some("/opt/bin/codex"));
        assert_eq!(cfg.claude_path_override.as_deref(), Some("/opt/bin/claude"));
    }

    #[test]
    fn billing_guard_refuses_codex_with_openai_key() {
        let mut env = base_env();
        env.insert("OPENAI_API_KEY".into(), "sk-xxx".into());
        let err = Config::from_env(&env).expect_err("must refuse");
        assert!(matches!(err, ConfigError::BillingGuard(_)));
    }

    #[test]
    fn billing_guard_ignores_other_backends_key() {
        // Codex active + ANTHROPIC_API_KEY set: not the active backend's flip key → allowed.
        let mut env = base_env();
        env.insert("ANTHROPIC_API_KEY".into(), "sk-ant".into());
        assert!(Config::from_env(&env).is_ok());
    }

    #[test]
    fn derives_public_url_from_domain() {
        let mut env = base_env();
        env.insert("LILA_DOMAIN".into(), "https://app.example.com/".into());
        let cfg = Config::from_env(&env).expect("loads");
        assert_eq!(cfg.app_public_url, "https://app.example.com");
    }

    #[test]
    fn rejects_non_integer_user_id() {
        let mut env = base_env();
        env.insert("ALLOWED_USER_IDS".into(), "42,nope".into());
        assert!(Config::from_env(&env).is_err());
    }

    #[test]
    fn missing_token_is_error() {
        let env = Env::from([("ALLOWED_USER_IDS".into(), "1".into())]);
        assert!(matches!(
            Config::from_env(&env),
            Err(ConfigError::Missing(_))
        ));
    }
}
