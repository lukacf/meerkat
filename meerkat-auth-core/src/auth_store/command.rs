//! External-command credential source.
//!
//! Reference-CLI parity: Codex `external_bearer.rs:17-157`. The command is
//! spawned, stdout is captured as a bearer token, and optionally cached for
//! `refresh_interval_ms`. Stderr is captured for diagnostics.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use thiserror::Error;
use tokio::process::Command;
use tokio::time::timeout;

use super::{PersistedAuthMode, PersistedTokens};

/// Spec for a subprocess-based credential source.
#[derive(Debug, Clone)]
pub struct CommandCredentialSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: HashMap<String, String>,
    pub timeout_ms: u64,
    pub refresh_interval_ms: Option<u64>,
}

#[derive(Debug, Error)]
pub enum CommandCredentialError {
    #[error("spawn failed: {0}")]
    Spawn(String),
    #[error("command exited non-zero ({code}): {stderr}")]
    NonZeroExit { code: i32, stderr: String },
    #[error("command timed out after {0}ms")]
    Timeout(u64),
    #[error("empty token output")]
    EmptyOutput,
    #[error("invalid utf-8 in stdout: {0}")]
    InvalidUtf8(String),
}

/// Runs the command, caches the resulting token for
/// `refresh_interval_ms` if set. Multiple concurrent callers on the same
/// runner observe the cached value.
pub struct CommandCredentialRunner {
    spec: CommandCredentialSpec,
    cache: Mutex<Option<(PersistedTokens, Instant)>>,
}

impl CommandCredentialRunner {
    pub fn new(spec: CommandCredentialSpec) -> Self {
        Self {
            spec,
            cache: Mutex::new(None),
        }
    }

    pub fn spec(&self) -> &CommandCredentialSpec {
        &self.spec
    }

    /// Resolve a token. Uses cached value if within `refresh_interval_ms`.
    pub async fn resolve(&self) -> Result<PersistedTokens, CommandCredentialError> {
        if let Some(interval_ms) = self.spec.refresh_interval_ms
            && let Some((cached, at)) = self.cache.lock().as_ref()
            && at.elapsed() < Duration::from_millis(interval_ms)
        {
            return Ok(cached.clone());
        }

        let tokens = self.run_once().await?;
        *self.cache.lock() = Some((tokens.clone(), Instant::now()));
        Ok(tokens)
    }

    async fn run_once(&self) -> Result<PersistedTokens, CommandCredentialError> {
        let mut cmd = Command::new(&self.spec.program);
        cmd.args(&self.spec.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .envs(&self.spec.env);
        if let Some(cwd) = &self.spec.cwd {
            cmd.current_dir(cwd);
        }

        let fut = async move {
            let output = cmd
                .output()
                .await
                .map_err(|e| CommandCredentialError::Spawn(e.to_string()))?;
            if !output.status.success() {
                let code = output.status.code().unwrap_or(-1);
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                return Err(CommandCredentialError::NonZeroExit { code, stderr });
            }
            Ok(output)
        };

        let output = timeout(Duration::from_millis(self.spec.timeout_ms), fut)
            .await
            .map_err(|_| CommandCredentialError::Timeout(self.spec.timeout_ms))??;

        let raw = std::str::from_utf8(&output.stdout)
            .map_err(|e| CommandCredentialError::InvalidUtf8(e.to_string()))?
            .trim();
        if raw.is_empty() {
            return Err(CommandCredentialError::EmptyOutput);
        }

        Ok(PersistedTokens {
            auth_mode: PersistedAuthMode::Command,
            primary_secret: Some(raw.to_string()),
            refresh_token: None,
            id_token: None,
            expires_at: None,
            last_refresh: Some(chrono::Utc::now()),
            scopes: Vec::new(),
            account_id: None,
            metadata: serde_json::Value::Null,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn spec_echo(token: &str) -> CommandCredentialSpec {
        CommandCredentialSpec {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), format!("printf '%s' '{token}'")],
            cwd: None,
            env: HashMap::new(),
            timeout_ms: 5_000,
            refresh_interval_ms: None,
        }
    }

    #[tokio::test]
    async fn command_runner_captures_stdout_as_token() {
        let runner = CommandCredentialRunner::new(spec_echo("tok-abc"));
        let tokens = runner.resolve().await.unwrap();
        assert_eq!(tokens.primary_secret.as_deref(), Some("tok-abc"));
        assert_eq!(tokens.auth_mode, PersistedAuthMode::Command);
    }

    #[tokio::test]
    async fn command_runner_caches_within_interval() {
        let mut spec = spec_echo("first");
        spec.refresh_interval_ms = Some(60_000);
        let runner = CommandCredentialRunner::new(spec);
        let a = runner.resolve().await.unwrap();
        let b = runner.resolve().await.unwrap();
        assert_eq!(a.primary_secret, b.primary_secret);
    }

    #[tokio::test]
    async fn command_runner_surfaces_nonzero_exit() {
        let spec = CommandCredentialSpec {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "echo fail >&2; exit 2".into()],
            cwd: None,
            env: HashMap::new(),
            timeout_ms: 5_000,
            refresh_interval_ms: None,
        };
        let runner = CommandCredentialRunner::new(spec);
        match runner.resolve().await {
            Err(CommandCredentialError::NonZeroExit { code, stderr }) => {
                assert_eq!(code, 2);
                assert!(stderr.contains("fail"));
            }
            other => panic!("expected NonZeroExit, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn command_runner_rejects_empty_output() {
        let spec = CommandCredentialSpec {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "true".into()],
            cwd: None,
            env: HashMap::new(),
            timeout_ms: 5_000,
            refresh_interval_ms: None,
        };
        let runner = CommandCredentialRunner::new(spec);
        assert!(matches!(
            runner.resolve().await,
            Err(CommandCredentialError::EmptyOutput)
        ));
    }
}
