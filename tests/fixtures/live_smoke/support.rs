#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn first_env(vars: &[&str]) -> Option<String> {
    vars.iter().find_map(|name| {
        std::env::var(name)
            .ok()
            .filter(|value| !value.trim().is_empty())
    })
}

pub fn anthropic_api_key() -> Option<String> {
    first_env(&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"])
}

pub fn openai_api_key() -> Option<String> {
    first_env(&["RKAT_OPENAI_API_KEY", "OPENAI_API_KEY"])
}

pub fn gemini_api_key() -> Option<String> {
    first_env(&["RKAT_GEMINI_API_KEY", "GEMINI_API_KEY", "GOOGLE_API_KEY"])
}

pub fn smoke_model() -> String {
    std::env::var("SMOKE_MODEL").unwrap_or_else(|_| "claude-sonnet-4-5".to_string())
}

pub fn live_timeout() -> Duration {
    Duration::from_secs(
        std::env::var("LIVE_SMOKE_TIMEOUT_SECS")
            .ok()
            .and_then(|raw| raw.parse::<u64>().ok())
            .unwrap_or(120),
    )
}

pub fn cargo_bin(bin_name: &str) -> PathBuf {
    let key = format!("CARGO_BIN_EXE_{bin_name}");
    PathBuf::from(
        std::env::var(&key).unwrap_or_else(|_| panic!("missing cargo bin env var: {key}")),
    )
}

pub struct LiveSmokeDir {
    path: PathBuf,
}

impl LiveSmokeDir {
    pub fn new(prefix: &str) -> Self {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("meerkat-{prefix}-{}-{suffix}", std::process::id()));
        std::fs::create_dir_all(&path)
            .unwrap_or_else(|err| panic!("failed to create temp dir {}: {err}", path.display()));
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for LiveSmokeDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
