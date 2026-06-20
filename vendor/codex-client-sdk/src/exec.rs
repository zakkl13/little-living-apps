use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use async_stream::try_stream;
use futures::stream::BoxStream;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdout, Command};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::codex_options::CodexConfigObject;
use crate::errors::{Error, Result};
use crate::thread_options::{ApprovalMode, ModelReasoningEffort, SandboxMode, WebSearchMode};

const INTERNAL_ORIGINATOR_ENV: &str = "CODEX_INTERNAL_ORIGINATOR_OVERRIDE";
const RUST_SDK_ORIGINATOR: &str = "codex_sdk_rust";

/// Arguments used by [`CodexExec::run`].
#[derive(Debug, Clone, Default)]
pub struct CodexExecArgs {
    /// Prompt text passed to Codex via stdin.
    pub input: String,
    /// Optional `OPENAI_BASE_URL` override.
    pub base_url: Option<String>,
    /// Optional `CODEX_API_KEY` override.
    pub api_key: Option<String>,
    /// Existing thread id to resume. When set, `resume <thread_id>` is added.
    pub thread_id: Option<String>,
    /// Local image paths passed via repeated `--image` flags.
    pub images: Vec<String>,
    /// Model override passed as `--model`.
    pub model: Option<String>,
    /// Sandbox mode passed as `--sandbox`.
    pub sandbox_mode: Option<SandboxMode>,
    /// Working directory passed as `--cd`.
    pub working_directory: Option<String>,
    /// Additional directories passed as repeated `--add-dir` flags.
    pub additional_directories: Vec<String>,
    /// Whether to append `--skip-git-repo-check`.
    pub skip_git_repo_check: bool,
    /// Path passed to `--output-schema`.
    pub output_schema_file: Option<String>,
    /// Model reasoning effort translated to a `--config` override.
    pub model_reasoning_effort: Option<ModelReasoningEffort>,
    /// Network access override translated to a `--config` entry.
    pub network_access_enabled: Option<bool>,
    /// Explicit web search mode translated to a `--config` entry.
    pub web_search_mode: Option<WebSearchMode>,
    /// Legacy boolean web search toggle used when `web_search_mode` is absent.
    pub web_search_enabled: Option<bool>,
    /// Approval policy translated to a `--config` entry.
    pub approval_policy: Option<ApprovalMode>,
    /// Optional cancellation token that aborts the running subprocess.
    pub cancellation_token: Option<CancellationToken>,
}

/// Process runner for the Codex CLI.
#[derive(Debug, Clone)]
pub struct CodexExec {
    executable_path: String,
    env_override: Option<HashMap<String, String>>,
    config_overrides: Option<CodexConfigObject>,
}

impl CodexExec {
    /// Creates a Codex subprocess runner.
    ///
    /// When `executable_path_override` is not supplied, the executable is
    /// discovered from standard local and global install locations.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use codex::CodexExec;
    ///
    /// let _exec = CodexExec::new(None, None, None)?;
    /// # Ok::<(), codex::Error>(())
    /// ```
    pub fn new(
        executable_path_override: Option<String>,
        env_override: Option<HashMap<String, String>>,
        config_overrides: Option<CodexConfigObject>,
    ) -> Result<Self> {
        let executable_path = match executable_path_override {
            Some(path) => path,
            None => find_codex_path()?,
        };

        Ok(Self {
            executable_path,
            env_override,
            config_overrides,
        })
    }

    /// Runs one `codex exec --experimental-json` invocation and returns a stream
    /// of stdout JSONL lines.
    ///
    /// The returned stream yields lines as they arrive and finishes only after
    /// the subprocess exits successfully. Non-zero exit status is returned as
    /// [`Error::Process`].
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use codex::{CodexExec, CodexExecArgs};
    /// use futures::StreamExt;
    ///
    /// # async fn example() -> codex::Result<()> {
    /// let exec = CodexExec::new(None, None, None)?;
    /// let mut stream = exec
    ///     .run(CodexExecArgs {
    ///         input: "Say hello".to_string(),
    ///         ..Default::default()
    ///     })
    ///     .await?;
    ///
    /// let _first = stream.next().await;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn run(&self, args: CodexExecArgs) -> Result<BoxStream<'static, Result<String>>> {
        if args
            .cancellation_token
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            return Err(Error::Cancelled);
        }

        let command_args = self.build_command_args(&args)?;

        let mut command = Command::new(&self.executable_path);
        command
            .args(&command_args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        command.env_clear();
        command.envs(build_env(&self.env_override, &args));

        let mut child = command
            .spawn()
            .map_err(|e| Error::Spawn(format!("{} ({e})", self.executable_path)))?;

        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::Spawn("child process has no stdin".to_string()))?;
        stdin.write_all(args.input.as_bytes()).await?;
        stdin.shutdown().await?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::Spawn("child process has no stdout".to_string()))?;
        let stderr = child.stderr.take();

        let mut lines = BufReader::new(stdout).lines();
        let cancellation_token = args.cancellation_token;
        let mut stderr_task = Some(spawn_stderr_reader(stderr));

        let output = try_stream! {
            loop {
                let next = next_line_or_cancel(
                    &mut lines,
                    cancellation_token.as_ref(),
                    &mut child,
                    &mut stderr_task,
                ).await?;

                match next {
                    Some(line) => yield line,
                    None => break,
                }
            }

            let status = child.wait().await?;
            let stderr = take_stderr(&mut stderr_task).await;

            if !status.success() {
                let detail = match status.code() {
                    Some(code) => format!("code {code}"),
                    None => "signal termination".to_string(),
                };
                Err(Error::Process {
                    detail,
                    stderr,
                    code: status.code(),
                })?;
            }
        };

        Ok(Box::pin(output))
    }

    fn build_command_args(&self, args: &CodexExecArgs) -> Result<Vec<String>> {
        let mut command_args = vec!["exec".to_string(), "--experimental-json".to_string()];

        if let Some(config_overrides) = &self.config_overrides {
            for override_value in serialize_config_overrides(config_overrides)? {
                command_args.push("--config".to_string());
                command_args.push(override_value);
            }
        }

        if let Some(model) = &args.model {
            command_args.push("--model".to_string());
            command_args.push(model.clone());
        }

        if let Some(sandbox_mode) = args.sandbox_mode {
            command_args.push("--sandbox".to_string());
            command_args.push(sandbox_mode_to_str(sandbox_mode).to_string());
        }

        if let Some(working_directory) = &args.working_directory {
            command_args.push("--cd".to_string());
            command_args.push(working_directory.clone());
        }

        for dir in &args.additional_directories {
            command_args.push("--add-dir".to_string());
            command_args.push(dir.clone());
        }

        if args.skip_git_repo_check {
            command_args.push("--skip-git-repo-check".to_string());
        }

        if let Some(output_schema_file) = &args.output_schema_file {
            command_args.push("--output-schema".to_string());
            command_args.push(output_schema_file.clone());
        }

        if let Some(reasoning_effort) = args.model_reasoning_effort {
            command_args.push("--config".to_string());
            command_args.push(format!(
                "model_reasoning_effort=\"{}\"",
                model_reasoning_effort_to_str(reasoning_effort)
            ));
        }

        if let Some(network_access_enabled) = args.network_access_enabled {
            command_args.push("--config".to_string());
            command_args.push(format!(
                "sandbox_workspace_write.network_access={network_access_enabled}"
            ));
        }

        if let Some(web_search_mode) = args.web_search_mode {
            command_args.push("--config".to_string());
            command_args.push(format!(
                "web_search=\"{}\"",
                web_search_mode_to_str(web_search_mode)
            ));
        } else if let Some(web_search_enabled) = args.web_search_enabled {
            command_args.push("--config".to_string());
            let mode = if web_search_enabled {
                "live"
            } else {
                "disabled"
            };
            command_args.push(format!("web_search=\"{mode}\""));
        }

        if let Some(approval_policy) = args.approval_policy {
            command_args.push("--config".to_string());
            command_args.push(format!(
                "approval_policy=\"{}\"",
                approval_mode_to_str(approval_policy)
            ));
        }

        if let Some(thread_id) = &args.thread_id {
            command_args.push("resume".to_string());
            command_args.push(thread_id.clone());
        }

        for image in &args.images {
            command_args.push("--image".to_string());
            command_args.push(image.clone());
        }

        Ok(command_args)
    }
}

fn find_codex_path() -> Result<String> {
    if let Ok(path) = which::which("codex") {
        return Ok(path.to_string_lossy().into_owned());
    }

    let cwd = std::env::current_dir().ok();
    let home = home_dir();
    if let Some(path) = find_codex_path_from(cwd.as_deref(), home.as_deref()) {
        return Ok(path);
    }

    Err(Error::CliNotFound(
        "codex executable was not found. Checked PATH, local node_modules, platform vendor binaries, and common global install locations. Set codex_path_override or install @openai/codex".to_string(),
    ))
}

fn build_env(
    env_override: &Option<HashMap<String, String>>,
    args: &CodexExecArgs,
) -> HashMap<String, String> {
    let mut env = match env_override {
        Some(override_map) => override_map.clone(),
        None => std::env::vars().collect(),
    };

    env.entry(INTERNAL_ORIGINATOR_ENV.to_string())
        .or_insert_with(|| RUST_SDK_ORIGINATOR.to_string());

    if let Some(base_url) = &args.base_url {
        env.insert("OPENAI_BASE_URL".to_string(), base_url.clone());
    }
    if let Some(api_key) = &args.api_key {
        env.insert("CODEX_API_KEY".to_string(), api_key.clone());
    }

    env
}

fn find_codex_path_from(start_dir: Option<&Path>, home_dir: Option<&Path>) -> Option<String> {
    if let Some(start_dir) = start_dir {
        for dir in start_dir.ancestors() {
            let local_bin = dir
                .join("node_modules")
                .join(".bin")
                .join(codex_binary_name());
            if local_bin.is_file() {
                return Some(local_bin.to_string_lossy().into_owned());
            }

            if let Some(vendor_path) = local_vendor_binary_path(dir) {
                return Some(vendor_path.to_string_lossy().into_owned());
            }
        }
    }

    for path in common_global_locations(home_dir) {
        if path.is_file() {
            return Some(path.to_string_lossy().into_owned());
        }
    }

    None
}

fn local_vendor_binary_path(base_dir: &Path) -> Option<PathBuf> {
    let target_triple = platform_target_triple()?;
    let package = platform_package_for_target(target_triple)?;

    let candidate = base_dir
        .join("node_modules")
        .join(package)
        .join("vendor")
        .join(target_triple)
        .join("codex")
        .join(codex_binary_name());

    if candidate.is_file() {
        Some(candidate)
    } else {
        None
    }
}

fn common_global_locations(home_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut locations = Vec::new();
    if let Some(home) = home_dir {
        locations.push(
            home.join(".npm-global")
                .join("bin")
                .join(codex_binary_name()),
        );
        locations.push(home.join(".local").join("bin").join(codex_binary_name()));
        locations.push(
            home.join("node_modules")
                .join(".bin")
                .join(codex_binary_name()),
        );
        locations.push(home.join(".yarn").join("bin").join(codex_binary_name()));
        locations.push(home.join(".codex").join("local").join(codex_binary_name()));
    }
    locations.push(PathBuf::from("/usr/local/bin").join(codex_binary_name()));
    locations
}

fn codex_binary_name() -> &'static str {
    if cfg!(windows) { "codex.exe" } else { "codex" }
}

fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

fn platform_target_triple() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some("x86_64-unknown-linux-musl"),
        ("linux", "aarch64") => Some("aarch64-unknown-linux-musl"),
        ("android", "x86_64") => Some("x86_64-unknown-linux-musl"),
        ("android", "aarch64") => Some("aarch64-unknown-linux-musl"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        ("windows", "x86_64") => Some("x86_64-pc-windows-msvc"),
        ("windows", "aarch64") => Some("aarch64-pc-windows-msvc"),
        _ => None,
    }
}

fn platform_package_for_target(target_triple: &str) -> Option<&'static str> {
    match target_triple {
        "x86_64-unknown-linux-musl" => Some("@openai/codex-linux-x64"),
        "aarch64-unknown-linux-musl" => Some("@openai/codex-linux-arm64"),
        "x86_64-apple-darwin" => Some("@openai/codex-darwin-x64"),
        "aarch64-apple-darwin" => Some("@openai/codex-darwin-arm64"),
        "x86_64-pc-windows-msvc" => Some("@openai/codex-win32-x64"),
        "aarch64-pc-windows-msvc" => Some("@openai/codex-win32-arm64"),
        _ => None,
    }
}

fn spawn_stderr_reader(stderr: Option<tokio::process::ChildStderr>) -> JoinHandle<String> {
    tokio::spawn(async move {
        let mut stderr_buffer = Vec::new();
        if let Some(mut stderr) = stderr {
            let _ = stderr.read_to_end(&mut stderr_buffer).await;
        }
        String::from_utf8_lossy(&stderr_buffer).into_owned()
    })
}

async fn take_stderr(stderr_task: &mut Option<JoinHandle<String>>) -> String {
    let Some(task) = stderr_task.take() else {
        return String::new();
    };
    (task.await).unwrap_or_default()
}

async fn next_line_or_cancel(
    lines: &mut tokio::io::Lines<BufReader<ChildStdout>>,
    cancellation_token: Option<&CancellationToken>,
    child: &mut Child,
    stderr_task: &mut Option<JoinHandle<String>>,
) -> Result<Option<String>> {
    match cancellation_token {
        Some(token) => {
            tokio::select! {
                _ = token.cancelled() => {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    let _ = take_stderr(stderr_task).await;
                    Err(Error::Cancelled)
                }
                line = lines.next_line() => line.map_err(Error::from),
            }
        }
        None => lines.next_line().await.map_err(Error::from),
    }
}

fn serialize_config_overrides(config_overrides: &CodexConfigObject) -> Result<Vec<String>> {
    let mut overrides = Vec::new();
    flatten_config_overrides(&Value::Object(config_overrides.clone()), "", &mut overrides)?;
    Ok(overrides)
}

fn flatten_config_overrides(
    value: &Value,
    prefix: &str,
    overrides: &mut Vec<String>,
) -> Result<()> {
    let Some(object) = value.as_object() else {
        if prefix.is_empty() {
            return Err(Error::InvalidConfig(
                "Codex config overrides must be a plain object".to_string(),
            ));
        }

        overrides.push(format!("{prefix}={}", to_toml_value(value, prefix)?));
        return Ok(());
    };

    if prefix.is_empty() && object.is_empty() {
        return Ok(());
    }
    if !prefix.is_empty() && object.is_empty() {
        overrides.push(format!("{prefix}={{}}"));
        return Ok(());
    }

    for (key, child) in object {
        if key.is_empty() {
            return Err(Error::InvalidConfig(
                "Codex config override keys must be non-empty strings".to_string(),
            ));
        }

        let formatted_key = format_toml_key(key);
        let path = if prefix.is_empty() {
            formatted_key
        } else {
            format!("{prefix}.{formatted_key}")
        };

        if child.is_object() {
            flatten_config_overrides(child, &path, overrides)?;
        } else {
            overrides.push(format!("{path}={}", to_toml_value(child, &path)?));
        }
    }

    Ok(())
}

fn to_toml_value(value: &Value, path: &str) -> Result<String> {
    match value {
        Value::String(s) => Ok(serde_json::to_string(s)?),
        Value::Number(n) => {
            if let Some(f) = n.as_f64()
                && !f.is_finite()
            {
                return Err(Error::InvalidConfig(format!(
                    "Codex config override at {path} must be a finite number"
                )));
            }
            Ok(n.to_string())
        }
        Value::Bool(b) => Ok(if *b { "true" } else { "false" }.to_string()),
        Value::Array(items) => {
            let mut rendered = Vec::with_capacity(items.len());
            for (index, item) in items.iter().enumerate() {
                rendered.push(to_toml_value(item, &format!("{path}[{index}]"))?);
            }
            Ok(format!("[{}]", rendered.join(", ")))
        }
        Value::Object(map) => {
            let mut parts = Vec::with_capacity(map.len());
            for (key, child) in map {
                if key.is_empty() {
                    return Err(Error::InvalidConfig(
                        "Codex config override keys must be non-empty strings".to_string(),
                    ));
                }
                let child_value = to_toml_value(child, &format!("{path}.{key}"))?;
                parts.push(format!("{} = {child_value}", format_toml_key(key)));
            }
            Ok(format!("{{{}}}", parts.join(", ")))
        }
        Value::Null => Err(Error::InvalidConfig(format!(
            "Codex config override at {path} cannot be null"
        ))),
    }
}

fn format_toml_key(key: &str) -> String {
    if is_bare_toml_key(key) {
        key.to_string()
    } else {
        // JSON quoting is also valid TOML basic string quoting.
        serde_json::to_string(key).unwrap_or_else(|_| "\"\"".to_string())
    }
}

fn is_bare_toml_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

fn sandbox_mode_to_str(mode: SandboxMode) -> &'static str {
    match mode {
        SandboxMode::ReadOnly => "read-only",
        SandboxMode::WorkspaceWrite => "workspace-write",
        SandboxMode::DangerFullAccess => "danger-full-access",
    }
}

fn model_reasoning_effort_to_str(mode: ModelReasoningEffort) -> &'static str {
    match mode {
        ModelReasoningEffort::Minimal => "minimal",
        ModelReasoningEffort::Low => "low",
        ModelReasoningEffort::Medium => "medium",
        ModelReasoningEffort::High => "high",
        ModelReasoningEffort::XHigh => "xhigh",
    }
}

fn web_search_mode_to_str(mode: WebSearchMode) -> &'static str {
    match mode {
        WebSearchMode::Disabled => "disabled",
        WebSearchMode::Cached => "cached",
        WebSearchMode::Live => "live",
    }
}

fn approval_mode_to_str(mode: ApprovalMode) -> &'static str {
    match mode {
        ApprovalMode::Never => "never",
        ApprovalMode::OnRequest => "on-request",
        ApprovalMode::OnFailure => "on-failure",
        ApprovalMode::Untrusted => "untrusted",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        codex_binary_name, find_codex_path_from, platform_package_for_target,
        platform_target_triple,
    };

    #[test]
    fn finds_codex_in_local_node_modules_bin() {
        let root = tempfile::tempdir().expect("tempdir");
        let bin = root.path().join("node_modules").join(".bin");
        std::fs::create_dir_all(&bin).expect("create bin");
        let codex = bin.join(codex_binary_name());
        std::fs::write(&codex, "").expect("create file");

        let nested = root.path().join("packages").join("app");
        std::fs::create_dir_all(&nested).expect("create nested");

        let found = find_codex_path_from(Some(&nested), None).expect("path");
        assert_eq!(found, codex.to_string_lossy());
    }

    #[test]
    fn finds_codex_in_platform_vendor_package() {
        let Some(target) = platform_target_triple() else {
            return;
        };
        let Some(package) = platform_package_for_target(target) else {
            return;
        };

        let root = tempfile::tempdir().expect("tempdir");
        let codex = root
            .path()
            .join("node_modules")
            .join(package)
            .join("vendor")
            .join(target)
            .join("codex")
            .join(codex_binary_name());
        std::fs::create_dir_all(codex.parent().expect("parent")).expect("mkdir");
        std::fs::write(&codex, "").expect("write");

        let nested = root.path().join("workspace").join("crate");
        std::fs::create_dir_all(&nested).expect("nested");

        let found = find_codex_path_from(Some(&nested), None).expect("path");
        assert_eq!(found, codex.to_string_lossy());
    }

    #[test]
    fn finds_codex_in_common_global_location() {
        let home = tempfile::tempdir().expect("tempdir");
        let codex = home
            .path()
            .join(".npm-global")
            .join("bin")
            .join(codex_binary_name());
        std::fs::create_dir_all(codex.parent().expect("parent")).expect("mkdir");
        std::fs::write(&codex, "").expect("write");

        let found = find_codex_path_from(None, Some(home.path())).expect("path");
        assert_eq!(found, codex.to_string_lossy());
    }
}
