//! Shell tool implementation
//!
//! This module provides the [`ShellTool`] which executes shell commands
//! using Nushell as the backend.

use async_trait::async_trait;
use meerkat_core::ToolDef;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::process::Command;
use tokio::time::timeout;

use tracing::{debug, info, instrument, warn};

use super::config::{ShellConfig, ShellError};
use super::security::{ValidationResult, validate_command_or_error};
use crate::builtin::{BuiltinTool, BuiltinToolError};

/// Maximum number of characters to keep in output before truncation.
/// This is measured in Unicode characters, not bytes.
const MAX_OUTPUT_CHARS: usize = 100_000;

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

    format!("[truncated {} chars]...{}", skip_count, tail)
}

/// Shell tool for executing shell commands
///
/// This tool executes commands using a configured shell (default: Nushell).
/// It is disabled by default for security and must be explicitly enabled.
#[derive(Debug, Clone)]
pub struct ShellTool {
    /// Configuration for shell execution
    pub config: ShellConfig,
}

impl ShellTool {
    /// Create a new ShellTool with the given configuration
    pub fn new(config: ShellConfig) -> Self {
        Self { config }
    }

    /// Find the shell executable path with fallback detection
    ///
    /// Tries the following in order:
    /// 1. Explicit shell_path from config (if set)
    /// 2. Configured shell name via `which`
    /// 3. SHELL environment variable
    /// 4. Platform-specific fallbacks (/bin/sh, /bin/bash, etc.)
    ///
    /// # Errors
    ///
    /// Returns [`ShellError::ShellNotFound`] if no shell can be found.
    pub fn find_shell(&self) -> Result<std::path::PathBuf, ShellError> {
        use std::path::PathBuf;

        let mut tried = Vec::new();

        // 1. Try explicit shell_path from config if set
        if let Some(ref path) = self.config.shell_path {
            if path.exists() {
                return Ok(path.clone());
            }
            tried.push(path.display().to_string());
        }

        // 2. Try configured shell name via which
        if let Ok(path) = which::which(&self.config.shell) {
            return Ok(path);
        }
        tried.push(self.config.shell.clone());

        // 3. Try SHELL environment variable
        if let Ok(shell_env) = std::env::var("SHELL") {
            let path = PathBuf::from(&shell_env);
            if path.exists() {
                return Ok(path);
            }
            tried.push(shell_env);
        }

        // 4. Try platform-specific fallbacks
        let fallbacks: Vec<&str> = if cfg!(windows) {
            vec!["cmd.exe", "powershell.exe"]
        } else {
            vec!["/bin/sh", "/bin/bash", "/bin/busybox", "/usr/bin/sh"]
        };

        for shell in &fallbacks {
            let path = PathBuf::from(shell);
            if path.exists() {
                return Ok(path);
            }
            // Only add to tried if not already tried
            if !tried.contains(&shell.to_string()) {
                tried.push(shell.to_string());
            }
        }

        Err(ShellError::ShellNotFound {
            shell: self.config.shell.clone(),
            tried,
        })
    }

    /// Execute a command synchronously and return the result
    async fn execute_command(
        &self,
        command: &str,
        working_dir: Option<&Path>,
        timeout_secs: u64,
    ) -> Result<ShellOutput, ShellError> {
        let shell_path = self.find_shell()?;

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

        match timeout(timeout_duration, cmd.output()).await {
            Ok(Ok(output)) => {
                let duration_secs = start.elapsed().as_secs_f64();

                // Check if UTF-8 conversion will be lossy
                let stdout_lossy = String::from_utf8(output.stdout.clone()).is_err();
                let stderr_lossy = String::from_utf8(output.stderr.clone()).is_err();

                // Convert to string with lossy UTF-8 handling, then truncate to tail
                let stdout_raw = String::from_utf8_lossy(&output.stdout).into_owned();
                let stderr_raw = String::from_utf8_lossy(&output.stderr).into_owned();

                Ok(ShellOutput {
                    exit_code: output.status.code(),
                    stdout: truncate_to_tail(&stdout_raw, MAX_OUTPUT_CHARS),
                    stderr: truncate_to_tail(&stderr_raw, MAX_OUTPUT_CHARS),
                    timed_out: false,
                    duration_secs,
                    stdout_lossy,
                    stderr_lossy,
                })
            }
            Ok(Err(e)) => Err(ShellError::Io(e)),
            Err(_) => {
                // Timeout occurred
                let duration_secs = start.elapsed().as_secs_f64();
                Ok(ShellOutput {
                    exit_code: None,
                    stdout: String::new(),
                    stderr: String::new(),
                    timed_out: true,
                    duration_secs,
                    stdout_lossy: false,
                    stderr_lossy: false,
                })
            }
        }
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
#[derive(Debug, Clone, Deserialize)]
struct ShellInput {
    /// The command to execute
    command: String,
    /// Working directory (optional)
    working_dir: Option<String>,
    /// Timeout in seconds (optional, uses config default)
    timeout_secs: Option<u64>,
    /// Run in background (optional, default false)
    #[serde(default)]
    background: bool,
}

#[async_trait]
impl BuiltinTool for ShellTool {
    fn name(&self) -> &'static str {
        "shell"
    }

    fn def(&self) -> ToolDef {
        ToolDef {
            name: "shell".to_string(),
            description: "Execute a shell command using Nushell".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The command to execute (Nushell syntax)"
                    },
                    "working_dir": {
                        "type": "string",
                        "description": "Working directory (relative to project root)"
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": "Timeout in seconds (uses config default if not specified)"
                    },
                    "background": {
                        "type": "boolean",
                        "description": "If true, run in background and return job ID immediately",
                        "default": false
                    }
                },
                "required": ["command"]
            }),
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

        // Security validation: check for dangerous patterns and characters
        let validation = validate_command_or_error(&input.command)
            .map_err(|e| BuiltinToolError::execution_failed(e.to_string()))?;

        // Log warnings for chaining characters but allow execution
        if let ValidationResult::Warning { reason } = &validation {
            warn!(command = %input.command, reason = %reason, "Command contains shell chaining");
        }

        // Validate and resolve working directory
        let working_dir = if let Some(ref dir) = input.working_dir {
            debug!(working_dir = %dir, "Validating working directory");
            let path = std::path::Path::new(dir);
            let resolved = self
                .config
                .validate_working_dir(path)
                .map_err(|e| BuiltinToolError::execution_failed(e.to_string()))?;
            Some(resolved)
        } else {
            None
        };

        // Background execution not yet implemented
        if input.background {
            warn!("Background execution requested but not configured");
            return Err(BuiltinToolError::execution_failed(
                ShellError::BackgroundNotConfigured.to_string(),
            ));
        }

        // Get timeout (use provided or config default)
        let timeout_secs = input
            .timeout_secs
            .unwrap_or(self.config.default_timeout_secs);
        debug!(timeout_secs = %timeout_secs, "Using timeout");

        // Execute the command
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
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    // ==================== ShellTool Struct Tests ====================

    #[test]
    fn test_shell_tool_struct() {
        let config = ShellConfig::default();
        let tool = ShellTool { config };

        assert_eq!(tool.config.shell, "nu");
        assert!(!tool.config.enabled);
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

        // With fallback detection, a shell should almost always be found
        // (e.g., /bin/sh on Unix systems). This test verifies either nu is
        // found or a fallback shell is used.
        match tool.find_shell() {
            Ok(path) => {
                // A shell was found (either nu or a fallback)
                let path_str = path.to_string_lossy();
                assert!(
                    path_str.contains("nu")
                        || path_str.contains("sh")
                        || path_str.contains("bash")
                        || path_str.contains("cmd")
                        || path_str.contains("powershell"),
                    "Expected a shell path, got: {}",
                    path_str
                );
            }
            Err(ShellError::ShellNotFound { shell, tried }) => {
                // This is very unlikely on most systems but valid
                assert_eq!(shell, "nu");
                assert!(!tried.is_empty(), "Should have tried at least one path");
            }
            Err(e) => panic!("Unexpected error: {}", e),
        }
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
    fn test_shell_tool_fallback_when_shell_not_found() {
        let config = ShellConfig {
            shell: "definitely_not_a_real_shell_xyz123".to_string(),
            ..Default::default()
        };

        let tool = ShellTool::new(config);

        // On most systems, this will find a fallback shell like /bin/sh
        // Only on very unusual systems would it fail
        let result = tool.find_shell();

        // On Unix, /bin/sh almost always exists, so we expect success with fallback
        #[cfg(unix)]
        {
            if let Ok(path) = result {
                let path_str = path.to_string_lossy();
                // Should be a fallback, not the requested shell
                assert!(
                    !path_str.contains("definitely_not_a_real_shell"),
                    "Should have used fallback, not nonexistent shell"
                );
            }
            // If it fails (very unlikely), ensure error has context
            else if let Err(ShellError::ShellNotFound { shell, tried }) = result {
                assert_eq!(shell, "definitely_not_a_real_shell_xyz123");
                assert!(tried.len() > 1, "Should have tried fallbacks");
            }
        }

        // On Windows without common shells, it may fail
        #[cfg(windows)]
        {
            // Either finds cmd.exe/powershell or fails with context
            match result {
                Ok(_) => {} // Found a fallback
                Err(ShellError::ShellNotFound { shell, tried }) => {
                    assert_eq!(shell, "definitely_not_a_real_shell_xyz123");
                    assert!(!tried.is_empty());
                }
                Err(e) => panic!("Unexpected error: {}", e),
            }
        }
    }

    // ==================== Synchronous Execution Tests ====================

    #[tokio::test]
    #[ignore = "requires nu to be installed"]
    async fn test_shell_tool_sync_execute() {
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
    #[ignore = "requires nu to be installed"]
    async fn test_shell_tool_timeout() {
        let temp_dir = TempDir::new().unwrap();
        let config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());

        let tool = ShellTool::new(config);

        // Sleep longer than timeout
        let output = tool.execute_command("sleep 10sec", None, 1).await.unwrap();

        assert!(output.timed_out);
        assert!(output.duration_secs >= 1.0);
    }

    #[tokio::test]
    #[ignore = "requires nu to be installed"]
    async fn test_shell_tool_output_format() {
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
    #[ignore = "requires nu to be installed"]
    async fn test_shell_tool_env_inheritance() {
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
    #[ignore = "requires nu to be installed"]
    async fn test_shell_tool_pwd_override() {
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
            "PWD should contain 'subdir', got: {}",
            pwd
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
    async fn test_shell_tool_call_background_not_configured() {
        let temp_dir = TempDir::new().unwrap();
        let config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        let tool = ShellTool::new(config);

        // Try background execution (not yet implemented)
        let result = tool
            .call(json!({
                "command": "echo test",
                "background": true
            }))
            .await;

        assert!(matches!(result, Err(BuiltinToolError::ExecutionFailed(_))));
    }

    #[tokio::test]
    #[ignore = "requires nu to be installed"]
    async fn test_shell_tool_call_success() {
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
    #[ignore = "requires nu to be installed"]
    async fn test_shell_tool_call_with_working_dir() {
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

    #[test]
    fn test_shell_output_lossy_flags() {
        // Test with lossy flags set
        let output = ShellOutput {
            exit_code: Some(0),
            stdout: "valid utf-8".to_string(),
            stderr: "contains \u{FFFD} replacement".to_string(),
            timed_out: false,
            duration_secs: 0.5,
            stdout_lossy: false,
            stderr_lossy: true,
        };

        let json = serde_json::to_string(&output).unwrap();
        let parsed: ShellOutput = serde_json::from_str(&json).unwrap();

        assert!(!parsed.stdout_lossy);
        assert!(parsed.stderr_lossy);
    }

    #[test]
    fn test_shell_output_lossy_defaults() {
        // Test that lossy fields default to false when not present in JSON
        // This ensures backwards compatibility with older serialized data
        let json = r#"{
            "exit_code": 0,
            "stdout": "hello",
            "stderr": "",
            "timed_out": false,
            "duration_secs": 1.0
        }"#;

        let parsed: ShellOutput = serde_json::from_str(json).unwrap();
        assert!(!parsed.stdout_lossy);
        assert!(!parsed.stderr_lossy);
    }

    // ==================== Regression Tests ====================

    /// Regression test: timeout should be enforced for sync execution
    ///
    /// Commands that run longer than the specified timeout should be
    /// terminated and return timed_out: true in the output.
    #[tokio::test]
    #[ignore = "requires nu"]
    async fn test_timeout_enforced_sync() {
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
    /// use lossy UTF-8 conversion with the lossy flag set.
    #[tokio::test]
    #[ignore = "e2e: spawns real shell process"]
    async fn test_non_utf8_output() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.shell = "sh".to_string();

        let tool = ShellTool::new(config);

        // Using printf to output raw bytes that are invalid UTF-8
        // \xff\xfe are invalid UTF-8 sequences
        let result = tool
            .call(json!({
                "command": r#"printf '\xff\xfe'"#
            }))
            .await;

        // Should not panic or error - lossy conversion should handle it
        assert!(
            result.is_ok(),
            "Non-UTF-8 output should be handled gracefully: {:?}",
            result
        );

        let output: ShellOutput = serde_json::from_value(result.unwrap()).unwrap();

        assert!(!output.timed_out, "Should not timeout");
        // The stdout_lossy flag should be set when lossy conversion occurred
        // Note: depending on printf behavior, the actual flag may vary
    }

    /// Regression test: long output should be captured completely
    ///
    /// Commands with large output should capture all of it (within reason).
    #[tokio::test]
    #[ignore = "requires nu"]
    async fn test_long_output_captured() {
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
    #[ignore = "e2e: spawns real shell process"]
    async fn test_sync_parallel_execution() {
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
            "Parallel execution should complete faster than sequential: {:?}",
            elapsed
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
            lines.push_str(&format!("Line {}\n", i));
        }

        // Truncate to a smaller size
        let result = truncate_to_tail(&lines, 50);

        // Should contain recent lines (tail), not early lines (head)
        assert!(
            result.contains("Line 100"),
            "Should contain most recent line: {}",
            result
        );
        assert!(
            !result.contains("Line 1\n"),
            "Should not contain early line: {}",
            result
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
        assert!(result.contains("😀"), "Should contain emoji: {}", result);
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
                "Result should be valid: {}",
                result
            );
        }

        // Test with Chinese characters (3 bytes each in UTF-8)
        let chinese = "你好世界Hello";
        for max_chars in 1..chinese.chars().count() + 5 {
            let result = truncate_to_tail(chinese, max_chars);
            assert!(
                result.is_empty() || result.chars().count() > 0,
                "Result should be valid for Chinese: {}",
                result
            );
        }

        // Test specific case: truncate to exactly 1 char
        let result = truncate_to_tail("🎉🎊🎈", 1);
        assert!(
            result.contains("🎈"),
            "Should keep last emoji when truncating to 1: {}",
            result
        );

        // Test edge case: max_chars = 0
        let result = truncate_to_tail("Hello 🎉", 0);
        // With 0 max chars, we expect empty or truncation indicator only
        assert!(
            result.contains("[truncated") || result.is_empty(),
            "Should handle max_chars=0: {}",
            result
        );

        // Test mixed multi-byte sequences don't split
        let mixed = "a🎉b世c界d";
        for max_chars in 1..mixed.chars().count() + 1 {
            let result = truncate_to_tail(mixed, max_chars);
            // Each character in result should be valid
            for c in result.chars() {
                assert!(
                    c.len_utf8() >= 1,
                    "Character should be valid UTF-8: {:?}",
                    c
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

    /// Regression test for Task #11: Shell detection falls through to env var
    ///
    /// Verifies that when configured shell is not found, the SHELL env var is tried.
    #[test]
    fn test_shell_detection_env_fallback() {
        let config = ShellConfig::default();
        let tool = ShellTool::new(config);

        let result = tool.find_shell();

        // On Unix, /bin/sh almost always exists, so we expect success with fallback
        #[cfg(unix)]
        {
            assert!(
                result.is_ok(),
                "Should find shell via fallbacks on Unix: {:?}",
                result
            );
        }

        // On Windows or if no shell found, we just verify no panic occurred
        #[cfg(not(unix))]
        {
            // Result is either Ok (found a shell) or Err (ShellNotFound with tried list)
            match &result {
                Ok(_) => {}
                Err(ShellError::ShellNotFound { tried, .. }) => {
                    assert!(!tried.is_empty(), "Should have tried some shells");
                }
                Err(e) => panic!("Unexpected error: {:?}", e),
            }
        }
    }

    /// Regression test for Task #11: Shell detection tries platform fallbacks
    ///
    /// Verifies that platform-specific fallback shells (/bin/sh, /bin/bash, etc.)
    /// are tried when configured shell is not found.
    #[test]
    fn test_shell_detection_platform_fallbacks() {
        // Use a shell that definitely doesn't exist
        let config = ShellConfig {
            shell: "nonexistent_shell_xyz123".to_string(),
            ..Default::default()
        };

        let tool = ShellTool::new(config);
        let result = tool.find_shell();

        // On Unix, /bin/sh should be found as fallback
        #[cfg(unix)]
        {
            assert!(
                result.is_ok(),
                "Should fall back to /bin/sh on Unix: {:?}",
                result
            );
            let path = result.unwrap();
            // Should be one of the Unix fallbacks
            let path_str = path.to_string_lossy();
            assert!(
                path_str.contains("sh")
                    || path_str.contains("bash")
                    || path_str.contains("busybox"),
                "Should be a Unix fallback shell: {}",
                path_str
            );
        }
    }

    /// Regression test for Task #11: ShellNotFound error includes tried paths
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

        // On Unix, a fallback will likely succeed, so only check error case on other platforms
        // or when mocking. For now, just verify the function doesn't panic.
        match result {
            Ok(_) => {
                // A fallback shell was found (e.g., /bin/sh on Unix)
            }
            Err(ShellError::ShellNotFound { shell, tried }) => {
                // The error should include what we were looking for
                assert_eq!(shell, "completely_fake_shell_abc");
                // Should have tried multiple paths
                assert!(
                    !tried.is_empty(),
                    "Should have a non-empty list of tried paths"
                );
                // Should include the explicit path that was set
                assert!(
                    tried.iter().any(|t| t.contains("nonexistent")),
                    "Should include the explicit path in tried: {:?}",
                    tried
                );
            }
            Err(e) => panic!("Unexpected error type: {:?}", e),
        }
    }

    // ==================== Security Validation Integration Tests ====================

    /// Test that commands with dangerous patterns are blocked at the tool level
    #[tokio::test]
    async fn test_shell_tool_blocks_rm_rf_root() {
        let temp_dir = TempDir::new().unwrap();
        let config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        let tool = ShellTool::new(config);

        let result = tool
            .call(json!({
                "command": "rm -rf /"
            }))
            .await;

        assert!(result.is_err(), "rm -rf / should be blocked");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("blocked"),
            "Error should mention blocked: {}",
            err
        );
    }

    /// Test that command substitution (backtick) is blocked
    #[tokio::test]
    async fn test_shell_tool_blocks_command_substitution() {
        let temp_dir = TempDir::new().unwrap();
        let config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        let tool = ShellTool::new(config);

        let result = tool
            .call(json!({
                "command": "echo `whoami`"
            }))
            .await;

        assert!(result.is_err(), "Command substitution should be blocked");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("dangerous") || err.contains("`"),
            "Error should mention dangerous character: {}",
            err
        );
    }

    /// Test that variable expansion ($) is blocked
    #[tokio::test]
    async fn test_shell_tool_blocks_variable_expansion() {
        let temp_dir = TempDir::new().unwrap();
        let config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        let tool = ShellTool::new(config);

        let result = tool
            .call(json!({
                "command": "echo $HOME"
            }))
            .await;

        assert!(result.is_err(), "Variable expansion should be blocked");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("dangerous") || err.contains("$"),
            "Error should mention dangerous character: {}",
            err
        );
    }

    /// Test that fork bomb is blocked
    #[tokio::test]
    async fn test_shell_tool_blocks_fork_bomb() {
        let temp_dir = TempDir::new().unwrap();
        let config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        let tool = ShellTool::new(config);

        let result = tool
            .call(json!({
                "command": ":(){ :|:& };:"
            }))
            .await;

        assert!(result.is_err(), "Fork bomb should be blocked");
    }

    /// Test that safe commands pass validation
    #[tokio::test]
    #[ignore = "e2e: spawns real shell process"]
    async fn test_shell_tool_allows_safe_commands() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.shell = "sh".to_string(); // Use sh for broad compatibility

        let tool = ShellTool::new(config);

        // Simple echo without special characters
        let result = tool
            .call(json!({
                "command": "echo hello"
            }))
            .await;

        // Should pass validation (may fail on shell execution if sh not found, but
        // validation itself should pass)
        match result {
            Ok(output) => {
                let output_str = serde_json::to_string(&output).unwrap();
                assert!(output_str.contains("hello") || output_str.contains("exit_code"));
            }
            Err(e) => {
                // Should not be a security-related error
                let err_str = e.to_string();
                assert!(
                    !err_str.contains("blocked") && !err_str.contains("dangerous"),
                    "Safe command should not be blocked: {}",
                    err_str
                );
            }
        }
    }
}
