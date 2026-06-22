#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;

use codex::{Codex, CodexOptions, Result};
use serde::Deserialize;
use serde_json::Value;
use tempfile::TempDir;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[derive(Debug, Clone, Deserialize)]
pub struct InvocationLog {
    pub args: Vec<String>,
    pub stdin: String,
    pub env: HashMap<String, String>,
    pub call_index: usize,
    pub output_schema_path: Option<String>,
    pub output_schema_exists: bool,
    pub output_schema: Option<Value>,
}

pub struct MockCodexHarness {
    _temp_dir: TempDir,
    script_path: PathBuf,
    events_path: PathBuf,
    log_path: PathBuf,
    call_index_path: PathBuf,
}

impl MockCodexHarness {
    pub fn new(events_per_call: Vec<Vec<Value>>) -> Self {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let script_path = temp_dir.path().join("mock_codex_cli.py");
        let events_path = temp_dir.path().join("events.json");
        let log_path = temp_dir.path().join("invocations.jsonl");
        let call_index_path = temp_dir.path().join("call_index.txt");

        std::fs::copy(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests")
                .join("fixtures")
                .join("mock_codex_cli.py"),
            &script_path,
        )
        .expect("copy mock cli");

        #[cfg(unix)]
        {
            let mut perms = std::fs::metadata(&script_path)
                .expect("metadata")
                .permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&script_path, perms).expect("chmod");
        }

        std::fs::write(
            &events_path,
            serde_json::to_vec(&events_per_call).expect("serialize events"),
        )
        .expect("write events");
        std::fs::write(&log_path, "").expect("initialize log");
        std::fs::write(&call_index_path, "0").expect("initialize call index");

        Self {
            _temp_dir: temp_dir,
            script_path,
            events_path,
            log_path,
            call_index_path,
        }
    }

    pub fn codex(&self, mut options: CodexOptions) -> Result<Codex> {
        options.codex_path_override = Some(self.script_path.to_string_lossy().into_owned());

        let mut env = self.base_env();
        if let Some(custom_env) = options.env.take() {
            env.extend(custom_env);
        }
        options.env = Some(env);

        Codex::new(Some(options))
    }

    pub fn logs(&self) -> Vec<InvocationLog> {
        let content = std::fs::read_to_string(&self.log_path).expect("read logs");
        content
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str::<InvocationLog>(line).expect("parse log line"))
            .collect()
    }

    pub fn path_exists(&self, path: &str) -> bool {
        PathBuf::from(path).exists()
    }

    fn base_env(&self) -> HashMap<String, String> {
        let mut env = HashMap::new();
        env.insert(
            "CODEX_MOCK_EVENTS".to_string(),
            self.events_path.to_string_lossy().into_owned(),
        );
        env.insert(
            "CODEX_MOCK_LOG".to_string(),
            self.log_path.to_string_lossy().into_owned(),
        );
        env.insert(
            "CODEX_MOCK_CALL_INDEX".to_string(),
            self.call_index_path.to_string_lossy().into_owned(),
        );
        env.insert("CODEX_MOCK_ENFORCE_GIT_CHECK".to_string(), "1".to_string());
        if let Ok(path) = std::env::var("PATH") {
            env.insert("PATH".to_string(), path);
        }
        env
    }
}

pub fn expect_pair(args: &[String], pair: (&str, &str)) {
    let index = args
        .windows(2)
        .position(|window| window[0] == pair.0 && window[1] == pair.1);
    assert!(
        index.is_some(),
        "Pair {} {} not found in args: {:?}",
        pair.0,
        pair.1,
        args
    );
}

pub fn collect_config_values(args: &[String], key: &str) -> Vec<String> {
    let mut values = Vec::new();
    for window in args.windows(2) {
        if window[0] == "--config" && window[1].starts_with(&format!("{key}=")) {
            values.push(window[1].to_string());
        }
    }
    values
}
