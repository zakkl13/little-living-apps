//! Backend CLI discovery and setup preflights.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::config::{AgentBackend, Config};

/// Result of resolving the executable that backs a manager/worker backend.
#[derive(Debug, Clone)]
pub struct BackendCliStatus {
    pub backend: AgentBackend,
    pub bin: &'static str,
    pub override_path: Option<String>,
    pub resolved_path: Option<PathBuf>,
}

impl BackendCliStatus {
    pub fn found(&self) -> bool {
        self.resolved_path.is_some()
    }

    pub fn found_message(&self) -> String {
        match &self.resolved_path {
            Some(path) => format!("{} CLI found ({})", self.bin, path.display()),
            None => format!("{} CLI found", self.bin),
        }
    }

    pub fn missing_message(&self) -> String {
        let hint = match self.backend {
            AgentBackend::Codex => {
                "Use the Docker image built from this repo, or install `@openai/codex` on PATH."
            }
            AgentBackend::Claude => {
                "Use the Docker image built from this repo, or install `@anthropic-ai/claude-code` on PATH."
            }
        };
        match &self.override_path {
            Some(path) => format!(
                "{} CLI NOT found at {}={path}. {hint}",
                self.bin,
                cli_override_key(self.backend)
            ),
            None => format!("{} CLI NOT found on PATH. {hint}", self.bin),
        }
    }
}

/// Resolve the CLI for `backend` using the current process PATH and config overrides.
pub fn backend_cli_status(cfg: &Config, backend: AgentBackend) -> BackendCliStatus {
    backend_cli_status_with_path(cfg, backend, std::env::var_os("PATH").as_deref())
}

/// Return an actionable error if the selected backend cannot be launched.
pub fn ensure_backend_cli(cfg: &Config, backend: AgentBackend) -> Result<(), String> {
    let status = backend_cli_status(cfg, backend);
    if status.found() {
        Ok(())
    } else {
        Err(status.missing_message())
    }
}

/// Return the exact executable path the selected backend should be launched through.
pub fn resolve_backend_cli_path(cfg: &Config, backend: AgentBackend) -> Result<PathBuf, String> {
    let status = backend_cli_status(cfg, backend);
    status
        .resolved_path
        .clone()
        .ok_or_else(|| status.missing_message())
}

fn backend_cli_status_with_path(
    cfg: &Config,
    backend: AgentBackend,
    path: Option<&OsStr>,
) -> BackendCliStatus {
    let bin = cli_bin(backend);
    let override_path = cli_override_path(cfg, backend);
    let resolved_path = match &override_path {
        Some(path) => executable_file(Path::new(path)).then(|| PathBuf::from(path)),
        None => which_on_path(bin, path),
    };
    BackendCliStatus {
        backend,
        bin,
        override_path,
        resolved_path,
    }
}

fn cli_bin(backend: AgentBackend) -> &'static str {
    match backend {
        AgentBackend::Codex => "codex",
        AgentBackend::Claude => "claude",
    }
}

fn cli_override_path(cfg: &Config, backend: AgentBackend) -> Option<String> {
    match backend {
        AgentBackend::Codex => cfg.codex_path_override.clone(),
        AgentBackend::Claude => cfg.claude_path_override.clone(),
    }
}

fn cli_override_key(backend: AgentBackend) -> &'static str {
    match backend {
        AgentBackend::Codex => "CODEX_BIN",
        AgentBackend::Claude => "CLAUDE_BIN",
    }
}

fn which_on_path(bin: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    let path = path?;
    std::env::split_paths(path)
        .map(|dir| dir.join(bin))
        .find(|candidate| executable_file(candidate))
}

fn executable_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Env;

    fn cfg() -> Config {
        Config::from_env(&Env::from([
            ("TELEGRAM_BOT_TOKEN".into(), "tok".into()),
            ("ALLOWED_USER_IDS".into(), "42".into()),
        ]))
        .unwrap()
    }

    #[test]
    fn reports_missing_backend_cli_on_empty_path() {
        let status =
            backend_cli_status_with_path(&cfg(), AgentBackend::Claude, Some(OsStr::new("")));
        assert!(!status.found());
        assert!(
            status
                .missing_message()
                .contains("claude CLI NOT found on PATH")
        );
    }

    #[test]
    fn finds_backend_cli_on_supplied_path() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("claude");
        std::fs::write(&bin, "").unwrap();
        make_executable(&bin);

        let status = backend_cli_status_with_path(
            &cfg(),
            AgentBackend::Claude,
            Some(tmp.path().as_os_str()),
        );
        assert_eq!(status.resolved_path, Some(bin));
    }

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }

    #[cfg(not(unix))]
    fn make_executable(_path: &Path) {}

    #[test]
    fn codex_bin_override_is_checked_directly() {
        let mut env = Env::from([
            ("TELEGRAM_BOT_TOKEN".into(), "tok".into()),
            ("ALLOWED_USER_IDS".into(), "42".into()),
        ]);
        env.insert("CODEX_BIN".into(), "/definitely/missing/codex".into());
        let cfg = Config::from_env(&env).unwrap();

        let status = backend_cli_status_with_path(&cfg, AgentBackend::Codex, Some(OsStr::new("")));
        assert!(!status.found());
        assert!(
            status
                .missing_message()
                .contains("CODEX_BIN=/definitely/missing/codex")
        );
    }

    #[test]
    fn claude_bin_override_is_checked_directly() {
        let mut env = Env::from([
            ("TELEGRAM_BOT_TOKEN".into(), "tok".into()),
            ("ALLOWED_USER_IDS".into(), "42".into()),
        ]);
        env.insert("CLAUDE_BIN".into(), "/definitely/missing/claude".into());
        let cfg = Config::from_env(&env).unwrap();

        let status = backend_cli_status_with_path(&cfg, AgentBackend::Claude, Some(OsStr::new("")));
        assert!(!status.found());
        assert!(
            status
                .missing_message()
                .contains("CLAUDE_BIN=/definitely/missing/claude")
        );
    }
}
