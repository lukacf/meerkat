//! Shell tool implementation
//!
//! This module provides the [`ShellTool`] which executes shell commands
//! using Nushell as the backend.

use async_trait::async_trait;
use meerkat_core::ToolDef;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::time::timeout;

use tracing::{debug, info, instrument, warn};

use super::config::{ShellConfig, ShellError};
use crate::builtin::{BuiltinTool, BuiltinToolError};

/// Maximum number of characters to keep in output before truncation.
/// This is measured in Unicode characters, not bytes.
const MAX_OUTPUT_CHARS: usize = 100_000;
const MAX_OUTPUT_BYTES: usize = MAX_OUTPUT_CHARS * 4;

/// Truncate output to keep the tail (most recent output).
///
/// When output exceeds `max_chars`, this function keeps the last `max_chars`
/// characters and prepends a truncation indicator. This is more useful than
/// keeping the head because recent output (like error messages or final results)
/// is typically more important.
///
/// # Arguments
///
/// * `s` - The string to potentially truncate
/// * `max_chars` - Maximum number of characters to keep (not bytes)
///
/// # Returns
///
/// The original string if within limit, or a truncated version with indicator
fn truncate_to_tail(s: &str, max_chars: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_chars {
        return s.to_string();
    }

    // Calculate how many chars to skip
    let skip_count = char_count - max_chars;

    // Skip the first `skip_count` characters and collect the rest
    let tail: String = s.chars().skip(skip_count).collect();

    format!("[truncated {skip_count} chars]...{tail}")
}

#[cfg(unix)]
async fn graceful_kill(child: &mut tokio::process::Child) -> std::io::Result<()> {
    use nix::sys::signal::{Signal, killpg};
    use nix::unistd::Pid;

    if let Some(pid) = child.id() {
        let pgid = Pid::from_raw(pid as i32);

        let _ = killpg(pgid, Signal::SIGTERM);

        tokio::select! {
            () = tokio::time::sleep(Duration::from_secs(2)) => {
                let _ = killpg(pgid, Signal::SIGKILL);
                let _ = child.wait().await;
            }
            result = child.wait() => {
                return result.map(|_| ());
            }
        }
    }
    Ok(())
}

#[cfg(not(unix))]
async fn graceful_kill(child: &mut tokio::process::Child) -> std::io::Result<()> {
    child.kill().await
}

struct TailBuffer {
    buffer: VecDeque<u8>,
    max_bytes: usize,
}

impl TailBuffer {
    fn new(max_bytes: usize) -> Self {
        Self {
            buffer: VecDeque::new(),
            max_bytes,
        }
    }

    fn extend(&mut self, data: &[u8]) {
        if self.max_bytes == 0 {
            return;
        }

        if data.len() >= self.max_bytes {
            self.buffer.clear();
            self.buffer
                .extend(data[data.len() - self.max_bytes..].iter().copied());
            return;
        }

        let overflow = (self.buffer.len() + data.len()).saturating_sub(self.max_bytes);
        if overflow > 0 {
            self.buffer.drain(0..overflow);
        }
        self.buffer.extend(data.iter().copied());
    }

    fn into_vec(self) -> Vec<u8> {
        self.buffer.into_iter().collect()
    }
}

async fn read_stream_tail<R>(mut reader: R, max_bytes: usize) -> std::io::Result<Vec<u8>>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut buffer = TailBuffer::new(max_bytes);
    let mut chunk = [0u8; 8192];

    loop {
        let n = reader.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        buffer.extend(&chunk[..n]);
    }

    Ok(buffer.into_vec())
}

async fn join_output(
    handle: tokio::task::JoinHandle<std::io::Result<Vec<u8>>>,
    label: &str,
) -> Vec<u8> {
    match handle.await {
        Ok(Ok(buf)) => buf,
        Ok(Err(err)) => {
            warn!("Failed to read {}: {}", label, err);
            Vec::new()
        }
        Err(err) => {
            warn!("{} reader task failed: {}", label, err);
            Vec::new()
        }
    }
}

/// Shell tool for executing shell commands
///
/// This tool executes commands using a configured shell (default: Nushell).
/// It is disabled by default for security and must be explicitly enabled.
#[derive(Debug, Clone)]
pub struct ShellTool {
    /// Configuration for shell execution
    pub config: ShellConfig,
    resolved_shell_path: Arc<Mutex<Option<PathBuf>>>,
    /// Job manager for background execution and concurrency tracking
    pub job_manager: Arc<super::job_manager::JobManager>,
}

impl ShellTool {
    /// Create a new ShellTool with the given configuration
    pub fn new(config: ShellConfig) -> Self {
        let job_manager = Arc::new(super::job_manager::JobManager::new(config.clone()));
        Self {
            config,
            resolved_shell_path: Arc::new(Mutex::new(None)),
            job_manager,
        }
    }

    /// Create a new ShellTool with a shared job manager
    pub fn with_job_manager(
        config: ShellConfig,
        job_manager: Arc<super::job_manager::JobManager>,
    ) -> Self {
        Self {
            config,
            resolved_shell_path: Arc::new(Mutex::new(None)),
            job_manager,
        }
    }

    /// Find the shell executable path.
    ///
    /// Tries the following in order:
    /// 1. Explicit shell_path from config (if set)
    /// 2. Configured shell name via `which`
    ///
    /// # Errors
    ///
    /// Returns [`ShellError::ShellNotInstalled`] if no shell can be found.
    pub fn find_shell(&self) -> Result<std::path::PathBuf, ShellError> {
        self.config.resolve_shell_path_auto()
    }

    async fn resolved_shell_path(&self) -> Result<PathBuf, ShellError> {
        {
            let guard = self.resolved_shell_path.lock().await;
            if let Some(path) = guard.as_ref() {
                return Ok(path.clone());
            }
        }

        let path = self.config.resolve_shell_path_auto_async().await?;
        if self.config.shell == "nu"
            && path
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|name| name != "nu")
        {
            debug!(
                configured_shell = %self.config.shell,
                fallback_shell = %path.display(),
                "Configured shell unavailable; using fallback shell"
            );
        }
        let mut guard = self.resolved_shell_path.lock().await;
        *guard = Some(path.clone());
        Ok(path)
    }

    /// Execute a command synchronously and return the result
    async fn execute_command(
        &self,
        command: &str,
        working_dir: Option<&Path>,
        timeout_secs: u64,
    ) -> Result<ShellOutput, ShellError> {
        // Enforce concurrency limit via job manager
        let _guard = self.job_manager.acquire_sync_slot().await?;

        let shell_path = self.resolved_shell_path().await?;

        let start = Instant::now();

        // Build the command
        let mut cmd = Command::new(&shell_path);
        cmd.arg("-c").arg(command);

        // Set working directory
        if let Some(dir) = working_dir {
            cmd.current_dir(dir);
            cmd.env("PWD", dir);
        } else {
            cmd.current_dir(&self.config.project_root);
            cmd.env("PWD", &self.config.project_root);
        }

        // Capture stdout/stderr
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        // Create new process group on Unix for proper cleanup of child processes.
        // This ensures that when we kill the process, all its children are also killed.
        #[cfg(unix)]
        cmd.process_group(0);

        // Spawn the process with timeout
        let timeout_duration = Duration::from_secs(timeout_secs);
        let mut child = cmd.spawn().map_err(ShellError::Io)?;

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let stdout_handle = tokio::spawn(async move {
            if let Some(out) = stdout {
                read_stream_tail(out, MAX_OUTPUT_BYTES).await
            } else {
                Ok(Vec::new())
            }
        });

        let stderr_handle = tokio::spawn(async move {
            if let Some(err) = stderr {
                read_stream_tail(err, MAX_OUTPUT_BYTES).await
            } else {
                Ok(Vec::new())
            }
        });

        let wait_result = timeout(timeout_duration, child.wait()).await;
        let duration_secs = start.elapsed().as_secs_f64();

        let mut wait_error: Option<std::io::Error> = None;
        let (exit_code, timed_out) = match wait_result {
            Ok(Ok(status)) => (status.code(), false),
            Ok(Err(e)) => {
                wait_error = Some(e);
                (None, false)
            }
            Err(_) => {
                let _ = graceful_kill(&mut child).await;
                (None, true)
            }
        };

        let stdout_bytes = join_output(stdout_handle, "stdout").await;
        let stderr_bytes = join_output(stderr_handle, "stderr").await;

        if let Some(err) = wait_error {
            return Err(ShellError::Io(err));
        }

        let stdout_lossy = String::from_utf8(stdout_bytes.clone()).is_err();
        let stderr_lossy = String::from_utf8(stderr_bytes.clone()).is_err();

        let stdout_raw = String::from_utf8_lossy(&stdout_bytes).into_owned();
        let stderr_raw = String::from_utf8_lossy(&stderr_bytes).into_owned();

        Ok(ShellOutput {
            exit_code,
            stdout: truncate_to_tail(&stdout_raw, MAX_OUTPUT_CHARS),
            stderr: truncate_to_tail(&stderr_raw, MAX_OUTPUT_CHARS),
            timed_out,
            duration_secs,
            stdout_lossy,
            stderr_lossy,
        })
    }
}

/// Output from a shell command execution
///
/// Note: stdout and stderr are converted from raw bytes using lossy UTF-8 conversion.
/// Invalid UTF-8 sequences are replaced with the Unicode replacement character (U+FFFD).
/// The `stdout_lossy` and `stderr_lossy` fields indicate if any replacement occurred.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellOutput {
    /// Exit code from the process (None if process was killed or timed out)
    pub exit_code: Option<i32>,
    /// Standard output (lossy UTF-8 converted)
    pub stdout: String,
    /// Standard error (lossy UTF-8 converted)
    pub stderr: String,
    /// Whether the command timed out
    pub timed_out: bool,
    /// Duration of execution in seconds
    pub duration_secs: f64,
    /// True if stdout contained invalid UTF-8 bytes that were replaced
    #[serde(default)]
    pub stdout_lossy: bool,
    /// True if stderr contained invalid UTF-8 bytes that were replaced
    #[serde(default)]
    pub stderr_lossy: bool,
}

/// Input arguments for the shell tool
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
struct ShellInput {
    /// The command to execute
    #[schemars(description = "The command to execute (POSIX-style parsing for policy checks)")]
    command: String,
    /// Working directory (optional)
    #[schemars(description = "Working directory (relative to project root)")]
    working_dir: Option<String>,
    /// Timeout in seconds (optional, uses config default)
    #[schemars(description = "Timeout in seconds (uses config default if not specified)")]
    timeout_secs: Option<u64>,
    /// Run in background (optional, default false)
    #[serde(default)]
    #[schemars(description = "If true, run in background and return job ID immediately")]
    background: bool,
}

#[async_trait]
impl BuiltinTool for ShellTool {
    fn name(&self) -> &'static str {
        "shell"
    }

    fn def(&self) -> ToolDef {
        ToolDef {
            name: "shell".into(),
            description:
                "Execute a shell command (POSIX-style parsing for policy checks; runs via Nushell or fallback shell)"
                    .into(),
            input_schema: crate::schema::schema_for::<ShellInput>(),
        }
    }

    fn default_enabled(&self) -> bool {
        false
    }

    #[instrument(skip(self, args), fields(tool = "shell"))]
    async fn call(&self, args: Value) -> Result<Value, BuiltinToolError> {
        let input: ShellInput = serde_json::from_value(args)
            .map_err(|e| BuiltinToolError::invalid_args(e.to_string()))?;

        info!(command = %input.command, background = %input.background, "Executing shell command");

        // Security validation: check policy (mode + patterns)
        // We validate the command but execute the original to preserve shell syntax (pipes, redirections).
        let _invocation = self
            .config
            .check_allowlist(&input.command)
            .map_err(|e| BuiltinToolError::execution_failed(e.to_string()))?;

        // Validate and resolve working directory
        let working_dir = if let Some(ref dir) = input.working_dir {
            debug!(working_dir = %dir, "Validating working directory");
            let path = std::path::Path::new(dir);
            let resolved = self
                .config
                .validate_working_dir_async(path)
                .await
                .map_err(|e| BuiltinToolError::execution_failed(e.to_string()))?;
            Some(resolved)
        } else {
            None
        };

        // Handle background execution
        if input.background {
            let job_id = self
                .job_manager
                .spawn_job(
                    &input.command,
                    working_dir.as_deref(),
                    input
                        .timeout_secs
                        .unwrap_or(self.config.default_timeout_secs),
                )
                .await
                .map_err(|e| BuiltinToolError::execution_failed(e.to_string()))?;

            return Ok(serde_json::json!({
                "job_id": job_id.to_string(),
                "status": "running",
                "message": format!("Command started in background with job ID: {}", job_id)
            }));
        }

        // Get timeout (use provided or config default)
        let timeout_secs = input
            .timeout_secs
            .unwrap_or(self.config.default_timeout_secs);
        debug!(timeout_secs = %timeout_secs, "Using timeout");

        // Execute the ORIGINAL command (not re-quoted) to preserve shell syntax (pipes, redirections)
        let output = self
            .execute_command(&input.command, working_dir.as_deref(), timeout_secs)
            .await
            .map_err(|e| {
                warn!(error = %e, "Command execution failed");
                BuiltinToolError::execution_failed(e.to_string())
            })?;

        debug!(
            exit_code = ?output.exit_code,
            timed_out = %output.timed_out,
            duration_secs = %output.duration_secs,
            "Command completed"
        );

        // Return structured JSON output
        Ok(serde_json::to_value(output)
            .map_err(|e| BuiltinToolError::execution_failed(e.to_string()))?)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::builtin::shell::security::SecurityMode;
    use serde_json::json;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[cfg(feature = "integration-real-tests")]
    fn nu_available() -> bool {
        which::which("nu").is_ok()
    }

    #[cfg(feature = "integration-real-tests")]
    fn skip_if_no_nu() -> bool {
        if nu_available() {
            return false;
        }

        eprintln!("Skipping: Nushell not installed (install `nu` to run this test)");
        true
    }

    // ==================== ShellTool Struct Tests ====================

    #[test]
    fn test_shell_tool_struct() {
        let config = ShellConfig::default();
        let tool = ShellTool::new(config);

        assert_eq!(tool.config.shell, "nu");
        assert!(!tool.config.enabled);
    }

    #[test]
    fn test_shell_tool_schema_mentions_posix_parsing() {
        let schema = crate::schema::schema_for::<ShellInput>();
        let description = schema["properties"]["command"]["description"]
            .as_str()
            .unwrap_or_default();
        assert!(
            description.contains("POSIX"),
            "expected command description to mention POSIX parsing, got: {description}"
        );
    }

    // ==================== ShellTool::new Tests ====================

    #[test]
    fn test_shell_tool_new() {
        let config = ShellConfig {
            enabled: true,
            default_timeout_secs: 60,
            restrict_to_project: false,
            shell: "bash".to_string(),
            project_root: PathBuf::from("/tmp"),
            security_mode: SecurityMode::Unrestricted,
            security_patterns: Vec::new(),
            ..Default::default()
        };

        let tool = ShellTool::new(config.clone());

        assert_eq!(tool.config.enabled, config.enabled);
        assert_eq!(
            tool.config.default_timeout_secs,
            config.default_timeout_secs
        );
        assert_eq!(tool.config.shell, config.shell);
    }

    // ==================== BuiltinTool Trait Tests ====================

    #[test]
    fn test_shell_tool_implements_builtin() {
        // Verify ShellTool implements BuiltinTool trait
        fn assert_builtin_tool<T: BuiltinTool>() {}
        assert_builtin_tool::<ShellTool>();
    }

    #[test]
    fn test_shell_tool_name() {
        let config = ShellConfig::default();
        let tool = ShellTool::new(config);

        assert_eq!(tool.name(), "shell");
    }

    #[test]
    fn test_shell_tool_default_enabled() {
        let config = ShellConfig::default();
        let tool = ShellTool::new(config);

        // Shell tool should be disabled by default for security
        assert!(!tool.default_enabled());
    }

    #[test]
    fn test_shell_tool_schema() {
        let config = ShellConfig::default();
        let tool = ShellTool::new(config);

        let def = tool.def();

        assert_eq!(def.name, "shell");
        assert!(def.description.contains("shell"));

        // Verify schema structure
        let schema = &def.input_schema;
        assert_eq!(schema["type"], "object");

        // Check properties exist
        let props = &schema["properties"];
        assert!(props.get("command").is_some());
        assert!(props.get("working_dir").is_some());
        assert!(props.get("timeout_secs").is_some());
        assert!(props.get("background").is_some());

        // Check required fields
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("command")));
    }

    // ==================== Shell Detection Tests ====================

    #[test]
    fn test_shell_tool_detect_nu() {
        let config = ShellConfig::default();
        let tool = ShellTool::new(config);

        // If nu is installed, it should be found. Otherwise fall back or return ShellNotInstalled.
        match tool.find_shell() {
            Ok(path) => {
                assert!(path.exists(), "Resolved shell path should exist");
            }
            Err(ShellError::ShellNotInstalled(details)) => {
                assert!(details.contains("nu"));
            }
            Err(e) => unreachable!("Unexpected error: {}", e),
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_shell_tool_falls_back_when_nu_missing() {
        let temp_dir = TempDir::new().unwrap();
        let fake_shell = temp_dir.path().join("fallback_shell");
        std::fs::write(&fake_shell, "#!/bin/sh\necho test").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake_shell, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let shell_value = fake_shell.to_string_lossy().to_string();
        let config = ShellConfig::default();
        let result = config
            .resolve_shell_path_auto_in(Some(std::ffi::OsStr::new("")), Some(shell_value.as_str()))
            .expect("fallback shell should resolve");
        assert_eq!(result, fake_shell);
    }

    #[test]
    fn test_shell_tool_explicit_path_takes_priority() {
        let temp_dir = TempDir::new().unwrap();
        // Create a fake shell script
        let fake_shell = temp_dir.path().join("my_shell");
        std::fs::write(&fake_shell, "#!/bin/sh\necho test").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake_shell, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let config = ShellConfig {
            shell_path: Some(fake_shell.clone()),
            ..Default::default()
        };

        let tool = ShellTool::new(config);
        let result = tool.find_shell();

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), fake_shell);
    }

    #[test]
    fn test_shell_tool_find_shell_custom() {
        let config = ShellConfig {
            shell: "definitely_not_a_real_shell_xyz123".to_string(),
            ..Default::default()
        };
        let result =
            config.resolve_shell_path_auto_in(Some(std::ffi::OsStr::new("")), Some("/bin/sh"));
        match result {
            Err(ShellError::ShellNotInstalled(details)) => {
                assert!(details.contains("definitely_not_a_real_shell_xyz123"));
            }
            Ok(path) => unreachable!("Expected ShellNotInstalled, got Ok({:?})", path),
            Err(e) => unreachable!("Unexpected error: {}", e),
        }
    }

    #[test]
    fn test_shell_tool_not_installed_error() {
        let config = ShellConfig {
            shell: "definitely_not_a_real_shell_xyz123".to_string(),
            shell_path: Some(PathBuf::from("/nonexistent/path/to/shell")),
            ..Default::default()
        };
        let tool = ShellTool::new(config);
        let result = tool
            .config
            .resolve_shell_path_with_fallbacks_in(&[], Some(std::ffi::OsStr::new("")));
        match result {
            Err(ShellError::ShellNotInstalled(details)) => {
                assert!(details.contains("definitely_not_a_real_shell_xyz123"));
            }
            Ok(path) => unreachable!("Expected ShellNotInstalled, got Ok({:?})", path),
            Err(other) => unreachable!("Unexpected error: {}", other),
        }
    }

    // ==================== Synchronous Execution Tests ====================

    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: executes shell commands"]
    async fn integration_real_shell_tool_sync_execute() {
        if skip_if_no_nu() {
            return;
        }

        let temp_dir = TempDir::new().unwrap();
        let config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());

        let tool = ShellTool::new(config);

        // Simple echo command
        let output = tool.execute_command("echo hello", None, 30).await.unwrap();

        assert!(!output.timed_out);
        assert_eq!(output.exit_code, Some(0));
        assert!(output.stdout.trim().contains("hello"));
        assert!(output.duration_secs >= 0.0);
    }

    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: executes shell commands"]
    async fn integration_real_shell_tool_timeout() {
        if skip_if_no_nu() {
            return;
        }

        let temp_dir = TempDir::new().unwrap();
        let config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());

        let tool = ShellTool::new(config);

        // Sleep longer than timeout
        let output = tool.execute_command("sleep 10sec", None, 1).await.unwrap();

        assert!(output.timed_out);
        assert!(output.exit_code.is_none());
        assert!(output.duration_secs >= 1.0);
    }

    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: executes shell commands"]
    async fn integration_real_shell_tool_output_format() {
        if skip_if_no_nu() {
            return;
        }

        let temp_dir = TempDir::new().unwrap();
        let config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());

        let tool = ShellTool::new(config);

        // Command that writes to both stdout and stderr
        let output = tool
            .execute_command("print hello; print -e error", None, 30)
            .await
            .unwrap();

        assert!(!output.timed_out);
        assert!(output.stdout.contains("hello"));
        assert!(output.stderr.contains("error"));
    }

    // ==================== Environment Tests ====================

    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: executes shell commands"]
    async fn integration_real_shell_tool_env_inheritance() {
        if skip_if_no_nu() {
            return;
        }

        let temp_dir = TempDir::new().unwrap();
        let config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());

        let tool = ShellTool::new(config);

        // Check that HOME is inherited (common env var)
        let output = tool.execute_command("$env.HOME", None, 30).await.unwrap();

        assert!(!output.timed_out);
        assert_eq!(output.exit_code, Some(0));
        // HOME should be set to something
        assert!(!output.stdout.trim().is_empty());
    }

    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: executes shell commands"]
    async fn integration_real_shell_tool_pwd_override() {
        if skip_if_no_nu() {
            return;
        }

        let temp_dir = TempDir::new().unwrap();
        let subdir = temp_dir.path().join("subdir");
        std::fs::create_dir(&subdir).unwrap();

        let config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        let tool = ShellTool::new(config);

        // Check PWD is set to working_dir
        let output = tool
            .execute_command("$env.PWD", Some(&subdir), 30)
            .await
            .unwrap();

        assert!(!output.timed_out);
        let pwd = output.stdout.trim();
        assert!(
            pwd.contains("subdir"),
            "PWD should contain 'subdir', got: {pwd}"
        );
    }

    // ==================== Tool Call Tests ====================

    #[tokio::test]
    async fn test_shell_tool_call_invalid_args() {
        let temp_dir = TempDir::new().unwrap();
        let config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        let tool = ShellTool::new(config);

        // Missing required 'command' field
        let result = tool.call(json!({})).await;
        assert!(matches!(result, Err(BuiltinToolError::InvalidArgs(_))));
    }

    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: executes shell commands"]
    async fn integration_real_shell_tool_call_background_success() {
        let temp_dir = TempDir::new().unwrap();
        let config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        let tool = ShellTool::new(config);

        // Try background execution
        let result = tool
            .call(json!({
                "command": "echo test",
                "background": true
            }))
            .await;

        assert!(result.is_ok());
        let val = result.unwrap();
        assert!(val["job_id"].is_string());
        assert_eq!(val["status"], "running");
    }

    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: executes shell commands"]
    async fn integration_real_shell_tool_call_success() {
        if skip_if_no_nu() {
            return;
        }

        let temp_dir = TempDir::new().unwrap();
        let config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        let tool = ShellTool::new(config);

        let result = tool
            .call(json!({
                "command": "echo hello"
            }))
            .await
            .unwrap();

        // Parse the output
        let output: ShellOutput = serde_json::from_value(result).unwrap();
        assert!(!output.timed_out);
        assert_eq!(output.exit_code, Some(0));
        assert!(output.stdout.contains("hello"));
    }

    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: executes shell commands"]
    async fn integration_real_shell_tool_call_with_working_dir() {
        if skip_if_no_nu() {
            return;
        }

        let temp_dir = TempDir::new().unwrap();
        let subdir = temp_dir.path().join("mydir");
        std::fs::create_dir(&subdir).unwrap();

        let config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        let tool = ShellTool::new(config);

        let result = tool
            .call(json!({
                "command": "pwd",
                "working_dir": "mydir"
            }))
            .await
            .unwrap();

        let output: ShellOutput = serde_json::from_value(result).unwrap();
        assert!(output.stdout.contains("mydir"));
    }

    #[tokio::test]
    async fn test_shell_tool_call_working_dir_escape() {
        let temp_dir = TempDir::new().unwrap();
        let config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        let tool = ShellTool::new(config);

        // Try to escape project root
        let result = tool
            .call(json!({
                "command": "echo test",
                "working_dir": "../../../etc"
            }))
            .await;

        assert!(matches!(result, Err(BuiltinToolError::ExecutionFailed(_))));
    }

    // ==================== ShellOutput Tests ====================

    #[test]
    fn test_shell_output_serde_roundtrip() {
        let output = ShellOutput {
            exit_code: Some(0),
            stdout: "hello world".to_string(),
            stderr: "".to_string(),
            timed_out: false,
            duration_secs: 1.234,
            stdout_lossy: false,
            stderr_lossy: false,
        };

        let json = serde_json::to_string(&output).unwrap();
        let parsed: ShellOutput = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.exit_code, output.exit_code);
        assert_eq!(parsed.stdout, output.stdout);
        assert_eq!(parsed.stderr, output.stderr);
        assert_eq!(parsed.timed_out, output.timed_out);
        assert!((parsed.duration_secs - output.duration_secs).abs() < f64::EPSILON);
        assert_eq!(parsed.stdout_lossy, output.stdout_lossy);
        assert_eq!(parsed.stderr_lossy, output.stderr_lossy);
    }

    #[test]
    fn test_shell_output_timeout_format() {
        let output = ShellOutput {
            exit_code: None,
            stdout: "partial output".to_string(),
            stderr: "".to_string(),
            timed_out: true,
            duration_secs: 30.0,
            stdout_lossy: false,
            stderr_lossy: false,
        };

        let json_value = serde_json::to_value(&output).unwrap();

        assert!(json_value["timed_out"].as_bool().unwrap());
        assert!(json_value["exit_code"].is_null());
    }

    // ==================== Regression Tests ====================

    /// Regression test: timeout should be enforced for sync execution
    ///
    /// Commands that run longer than the specified timeout should be
    /// terminated and return timed_out: true in the output.
    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: executes shell commands"]
    async fn integration_real_timeout_enforced_sync() {
        if skip_if_no_nu() {
            return;
        }

        let temp_dir = TempDir::new().unwrap();
        let config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());

        let tool = ShellTool::new(config);

        // Execute a command that sleeps longer than timeout
        let result = tool
            .call(json!({
                "command": "sleep 10sec",
                "timeout_secs": 1
            }))
            .await;

        assert!(
            result.is_ok(),
            "Timeout should return Ok with timed_out flag"
        );

        let output: ShellOutput = serde_json::from_value(result.unwrap()).unwrap();

        assert!(output.timed_out, "Command should have timed out");
        assert!(
            output.exit_code.is_none(),
            "Exit code should be None for timed out command"
        );
        assert!(
            output.duration_secs >= 1.0,
            "Duration should be at least 1 second, got {}",
            output.duration_secs
        );
    }

    /// Regression test: non-UTF-8 output should be handled gracefully
    ///
    /// Commands that produce non-UTF-8 bytes should not crash and should
    /// use lossy UTF-8 conversion.
    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: executes shell commands"]
    async fn integration_real_non_utf8_output() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.shell = "sh".to_string();

        let tool = ShellTool::new(config);

        // Using printf to output raw bytes that are invalid UTF-8
        // \xff\xfe are invalid UTF-8 sequences
        let result = tool
            .call(json!({
                "command": r"printf '\xff\xfe'"
            }))
            .await;

        // Should not panic or error - lossy conversion should handle it
        assert!(
            result.is_ok(),
            "Non-UTF-8 output should be handled gracefully: {result:?}"
        );

        let output: ShellOutput = serde_json::from_value(result.unwrap()).unwrap();

        assert!(!output.timed_out, "Should not timeout");
        assert_eq!(output.exit_code, Some(0), "Exit code should be 0");
    }

    /// Regression test: long output should be captured completely
    ///
    /// Commands with large output should capture all of it (within reason).
    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: executes shell commands"]
    async fn integration_real_long_output_captured() {
        if skip_if_no_nu() {
            return;
        }

        let temp_dir = TempDir::new().unwrap();
        let config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());

        let tool = ShellTool::new(config);

        // Generate a lot of output (1000 lines)
        let result = tool
            .call(json!({
                "command": "1..1000 | each { |i| $'Line ($i)' } | str join (char newline)"
            }))
            .await;

        assert!(result.is_ok(), "Long output command should succeed");

        let output: ShellOutput = serde_json::from_value(result.unwrap()).unwrap();

        assert!(!output.timed_out, "Should not timeout");
        assert_eq!(output.exit_code, Some(0), "Exit code should be 0");

        // Verify we captured many lines (at least the end)
        assert!(
            output.stdout.contains("Line 1000"),
            "Should contain the last line: {}...",
            &output.stdout[output.stdout.len().saturating_sub(100)..]
        );
    }

    /// Regression test: multiple parallel sync executions don't block each other
    ///
    /// Multiple sync tool calls should be able to run concurrently via tokio.
    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: executes shell commands"]
    async fn integration_real_sync_parallel_execution() {
        use std::time::Instant;

        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.shell = "sh".to_string();

        let tool = ShellTool::new(config);

        let start = Instant::now();

        // Run 3 commands that each sleep for 1 second concurrently
        let (r1, r2, r3) = tokio::join!(
            tool.call(json!({"command": "sleep 1 && echo done1"})),
            tool.call(json!({"command": "sleep 1 && echo done2"})),
            tool.call(json!({"command": "sleep 1 && echo done3"})),
        );

        let elapsed = start.elapsed();

        // All should succeed
        assert!(r1.is_ok(), "First command should succeed");
        assert!(r2.is_ok(), "Second command should succeed");
        assert!(r3.is_ok(), "Third command should succeed");

        // If truly parallel, should complete in ~1 second, not ~3 seconds
        // Allow some slack for overhead
        assert!(
            elapsed.as_secs() < 3,
            "Parallel execution should complete faster than sequential: {elapsed:?}"
        );
    }

    // ==================== Truncation Tests ====================

    #[test]
    fn test_truncate_to_tail_no_truncation() {
        // Short strings should not be truncated
        let short = "hello world";
        assert_eq!(truncate_to_tail(short, 100), short);

        // Exactly at limit should not be truncated
        let exact = "a".repeat(100);
        assert_eq!(truncate_to_tail(&exact, 100), exact);
    }

    #[test]
    fn test_truncate_to_tail_truncates() {
        // Create a string with known content
        let long = "abcdefghij"; // 10 chars

        // Truncate to 5 chars, keeping the tail "fghij"
        let result = truncate_to_tail(long, 5);

        // Should have truncation indicator and tail
        assert!(result.contains("[truncated 5 chars]"));
        assert!(result.ends_with("fghij"));
    }

    #[test]
    fn test_truncate_to_tail_keeps_tail() {
        // The most important part: recent output (tail) is preserved
        let mut lines = String::new();
        for i in 1..=100 {
            use std::fmt::Write;
            let _ = writeln!(lines, "Line {i}");
        }

        // Truncate to a smaller size
        let result = truncate_to_tail(&lines, 50);

        // Should contain recent lines (tail), not early lines (head)
        assert!(
            result.contains("Line 100"),
            "Should contain most recent line: {result}"
        );
        assert!(
            !result.contains("Line 1\n"),
            "Should not contain early line: {result}"
        );
    }

    #[test]
    fn test_truncate_to_tail_unicode() {
        // Test with Unicode characters (important: chars not bytes!)
        let unicode = "Hello, \u{4e16}\u{754c}! \u{1f600}"; // "Hello, 世界! 😀" - mixed ASCII and Unicode

        let char_count = unicode.chars().count();
        assert!(char_count > 5);

        // Truncate to 5 chars - should keep last 5 Unicode code points
        let result = truncate_to_tail(unicode, 5);

        // Last 5 chars of "Hello, 世界! 😀" are "界! 😀" (界, !, space, 😀)
        // Wait, let me verify: H e l l o , space 世 界 ! space 😀 = 12 chars
        // Last 5: 界 ! space 😀 - but that's only 4 grapheme clusters
        // In Rust chars(): 界 is 1 char, ! is 1, space is 1, 😀 is 1 = the last 5 chars would be "! 世界! 😀"
        // Actually: H(1) e(2) l(3) l(4) o(5) ,(6) (7) 世(8) 界(9) !(10) (11) 😀(12)
        // Last 5 chars at positions 8-12: "世界! 😀"
        assert!(result.contains("😀"), "Should contain emoji: {result}");
        assert!(result.contains("[truncated"));
    }

    #[test]
    fn test_truncate_to_tail_empty() {
        let empty = "";
        assert_eq!(truncate_to_tail(empty, 100), "");
    }

    // ==================== Regression Tests for Bug Fixes ====================

    /// Regression test: truncate_to_tail handles multi-byte UTF-8 correctly
    ///
    /// Verifies that truncation works at character boundaries (not byte boundaries)
    /// so we never split a multi-byte UTF-8 character. This would panic with
    /// byte-based slicing like `&output[skip..]`.
    #[test]
    fn test_truncate_multibyte_utf8() {
        // Test with emoji (4 bytes each in UTF-8)
        let emoji_str = "Hello 🎉🎊🎈 World";

        // Truncate at various sizes - should never panic
        for max_chars in 1..emoji_str.chars().count() + 5 {
            let result = truncate_to_tail(emoji_str, max_chars);
            // Verify result is valid UTF-8 (implicitly checked by being a String)
            // and has reasonable content
            assert!(
                result.is_empty() || result.chars().count() > 0,
                "Result should be valid: {result}"
            );
        }

        // Test with Chinese characters (3 bytes each in UTF-8)
        let chinese = "你好世界Hello";
        for max_chars in 1..chinese.chars().count() + 5 {
            let result = truncate_to_tail(chinese, max_chars);
            assert!(
                result.is_empty() || result.chars().count() > 0,
                "Result should be valid for Chinese: {result}"
            );
        }

        // Test specific case: truncate to exactly 1 char
        let result = truncate_to_tail("🎉🎊🎈", 1);
        assert!(
            result.contains("🎈"),
            "Should keep last emoji when truncating to 1: {result}"
        );

        // Test edge case: max_chars = 0
        let result = truncate_to_tail("Hello 🎉", 0);
        // With 0 max chars, we expect empty or truncation indicator only
        assert!(
            result.contains("[truncated") || result.is_empty(),
            "Should handle max_chars=0: {result}"
        );

        // Test mixed multi-byte sequences don't split
        let mixed = "a🎉b世c界d";
        for max_chars in 1..=mixed.chars().count() {
            let result = truncate_to_tail(mixed, max_chars);
            // Each character in result should be valid
            for c in result.chars() {
                assert!(
                    c.len_utf8() >= 1,
                    "Character should be valid UTF-8: {c:?}"
                );
            }
        }
    }

    // ==================== Regression Tests for Task #11: Shell Detection ====================

    /// Regression test for Task #11: Shell detection with explicit shell_path
    ///
    /// Verifies that when shell_path is explicitly set in config and the path exists,
    /// it is used directly without falling back to other detection methods.
    #[test]
    fn test_shell_detection_explicit_path() {
        let temp_dir = TempDir::new().unwrap();

        // Create a fake shell executable
        let fake_shell = temp_dir.path().join("my_shell");
        std::fs::write(&fake_shell, "#!/bin/sh\necho fake").unwrap();

        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.shell_path = Some(fake_shell.clone());

        let tool = ShellTool::new(config);
        let result = tool.find_shell();

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), fake_shell);
    }

    /// Regression test for Task #11: Shell detection does not fall back to env var
    ///
    /// Verifies that when configured shell is not found, the SHELL env var is ignored.
    #[test]
    fn test_shell_detection_env_fallback() {
        let config = ShellConfig {
            shell: "definitely_not_a_real_shell_xyz123".to_string(),
            ..Default::default()
        };
        let result =
            config.resolve_shell_path_auto_in(Some(std::ffi::OsStr::new("")), Some("/bin/sh"));
        match result {
            Err(ShellError::ShellNotInstalled(details)) => {
                assert!(details.contains("definitely_not_a_real_shell_xyz123"));
            }
            Ok(path) => unreachable!("Expected ShellNotInstalled, got Ok({:?})", path),
            Err(e) => unreachable!("Unexpected error: {:?}", e),
        }
    }

    /// Regression test for Task #11: Shell detection does not use platform fallbacks
    ///
    /// Verifies that platform-specific fallback shells are not used when configured
    /// shell is not found.
    #[test]
    fn test_shell_detection_platform_fallbacks() {
        // Use a shell that definitely doesn't exist
        let config = ShellConfig {
            shell: "nonexistent_shell_xyz123".to_string(),
            ..Default::default()
        };

        let tool = ShellTool::new(config);
        let result = tool.find_shell();

        match result {
            Err(ShellError::ShellNotInstalled(details)) => {
                assert!(details.contains("nonexistent_shell_xyz123"));
            }
            Ok(path) => unreachable!("Expected ShellNotInstalled, got Ok({:?})", path),
            Err(e) => unreachable!("Unexpected error: {:?}", e),
        }
    }

    /// Regression test for Task #11: ShellNotInstalled error includes details
    ///
    /// Verifies that when no shell is found, the error includes all paths that were tried.
    #[test]
    fn test_shell_detection_error_includes_tried() {
        // Configure with nonexistent explicit path and nonexistent shell name
        let config = ShellConfig {
            shell: "completely_fake_shell_abc".to_string(),
            shell_path: Some(PathBuf::from("/nonexistent/path/to/shell")),
            ..Default::default()
        };

        let tool = ShellTool::new(config);
        let result = tool.find_shell();

        match result {
            Err(ShellError::ShellNotInstalled(details)) => {
                assert!(
                    details.contains("completely_fake_shell_abc"),
                    "Should include requested shell in details: {details:?}"
                );
                assert!(
                    details.contains("nonexistent"),
                    "Should include explicit path in details: {details:?}"
                );
            }
            Ok(path) => unreachable!("Expected ShellNotInstalled, got Ok({:?})", path),
            Err(e) => unreachable!("Unexpected error type: {:?}", e),
        }
    }

    // ==================== Security Validation Integration Tests ====================

    /// Test that commands are blocked when not in allowlist
    #[tokio::test]
    async fn test_shell_tool_blocks_unauthorized() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.security_mode = SecurityMode::AllowList;
        config.security_patterns = vec!["ls".to_string()];
        let tool = ShellTool::new(config);

        let result = tool
            .call(json!({
                "command": "rm -rf /"
            }))
            .await;

        assert!(result.is_err(), "Unauthorized command should be blocked");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Security policy violation"),
            "Error should mention policy violation: {err}"
        );
    }

    /// Test that authorized commands pass validation
    #[test]
    fn test_shell_tool_allows_authorized() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.security_mode = SecurityMode::AllowList;
        config.security_patterns = vec!["echo *".to_string()];

        let result = config.check_allowlist("echo hello");
        assert!(result.is_ok(), "Authorized command should be allowed");
    }

    // ==================== P2 Regression Tests: Nushell Pipeline Support ====================

    /// Regression test for P2: Nushell pipelines must work correctly
    ///
    /// Commands with pipes like `echo hello | cat` should execute as pipelines,
    /// not have the `|` become a literal argument to echo.
    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: executes shell commands"]
    async fn integration_real_nushell_pipeline_works() {
        if skip_if_no_nu() {
            return;
        }

        let temp_dir = TempDir::new().unwrap();
        let config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        let tool = ShellTool::new(config);

        // A simple pipeline: echo produces output, cat passes it through
        let result = tool
            .call(json!({
                "command": "echo hello | cat"
            }))
            .await;

        assert!(
            result.is_ok(),
            "Pipeline command should succeed: {result:?}"
        );

        let output: ShellOutput = serde_json::from_value(result.unwrap()).unwrap();
        assert!(!output.timed_out, "Should not timeout");
        assert_eq!(output.exit_code, Some(0), "Exit code should be 0");

        // The key assertion: output should contain "hello"
        // If the pipe is broken (treated as literal arg), we'd get "hello | cat" or similar
        assert!(
            output.stdout.trim() == "hello",
            "Pipeline should work correctly. Expected 'hello', got: '{}'",
            output.stdout.trim()
        );
    }

    /// Regression test for P2: Nushell redirections must work correctly
    ///
    /// Commands with redirections like `echo hello | save file.txt` should write to files,
    /// not have the `|` become a literal argument.
    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: executes shell commands"]
    async fn integration_real_nushell_redirection_works() {
        if skip_if_no_nu() {
            return;
        }

        let temp_dir = TempDir::new().unwrap();
        let config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        let tool = ShellTool::new(config);

        let output_file = temp_dir.path().join("output.txt");

        // Use Nushell's save command for redirection (more portable than >)
        let result = tool
            .call(json!({
                "command": format!("'hello from nushell' | save {}", output_file.display())
            }))
            .await;

        assert!(
            result.is_ok(),
            "Redirection command should succeed: {result:?}"
        );

        let output: ShellOutput = serde_json::from_value(result.unwrap()).unwrap();
        assert_eq!(output.exit_code, Some(0), "Exit code should be 0");

        // Verify the file was actually created with the content
        let content = std::fs::read_to_string(&output_file).expect("Output file should exist");
        assert!(
            content.trim() == "hello from nushell",
            "File should contain the expected content, got: '{}'",
            content.trim()
        );
    }
}
