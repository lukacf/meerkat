//! Shell tool configuration
//!
//! This module defines [`ShellConfig`] which controls shell tool behavior,
//! and [`ShellError`] for shell-related errors.

use meerkat_core::ShellDefaults;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Errors that can occur during shell operations
#[derive(Debug, Error)]
pub enum ShellError {
    /// Shell executable not found on system
    #[error("Shell not installed: {0}. Install from https://nushell.sh")]
    ShellNotInstalled(String),

    /// Command matches a blocked pattern
    #[error("Command blocked: contains '{0}'")]
    BlockedCommand(String),

    /// Working directory escapes project root
    #[error("Working directory '{0}' is outside project root")]
    WorkingDirEscape(String),

    /// Working directory does not exist
    #[error("Working directory not found: {0}")]
    WorkingDirNotFound(String),

    /// Job not found in job manager
    #[error("Job not found: {0}")]
    JobNotFound(String),

    /// Job is not in running state
    #[error("Job is not running")]
    JobNotRunning,

    /// Background execution not configured
    #[error("Background execution not configured")]
    BackgroundNotConfigured,

    /// IO error during shell operations
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Configuration for the shell tool
///
/// Controls shell execution behavior including timeouts, working directory
/// restrictions, and which shell to use.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellConfig {
    /// Whether shell tool is enabled (default: false)
    pub enabled: bool,

    /// Default timeout in seconds (default: 30)
    pub default_timeout_secs: u64,

    /// Restrict working_dir to project root (default: true)
    pub restrict_to_project: bool,

    /// Shell executable name (default: "nu")
    pub shell: String,

    /// Explicit shell path (overrides detection)
    ///
    /// If set and the path exists, this will be used directly without
    /// falling back to PATH lookup or other detection methods.
    #[serde(default)]
    pub shell_path: Option<PathBuf>,

    /// Project root directory
    pub project_root: PathBuf,

    /// Maximum number of completed jobs to retain (default: 100)
    ///
    /// When this limit is exceeded, the oldest completed jobs are automatically
    /// removed during new job spawning. Set to 0 for unlimited (not recommended).
    #[serde(default = "default_max_completed_jobs")]
    pub max_completed_jobs: usize,

    /// Seconds after which completed jobs are eligible for cleanup (default: 300)
    ///
    /// Jobs that completed more than this many seconds ago will be removed
    /// during cleanup, even if under the max_completed_jobs limit.
    #[serde(default = "default_completed_job_ttl_secs")]
    pub completed_job_ttl_secs: u64,

    /// Maximum number of concurrent running processes (default: 10)
    ///
    /// When this limit is reached, new job spawn requests will be rejected.
    /// Set to 0 for unlimited.
    #[serde(default = "default_max_concurrent_processes")]
    pub max_concurrent_processes: usize,
}

fn default_max_completed_jobs() -> usize {
    100
}

fn default_completed_job_ttl_secs() -> u64 {
    300 // 5 minutes
}

fn default_max_concurrent_processes() -> usize {
    10
}

impl Default for ShellConfig {
    fn default() -> Self {
        let defaults = ShellDefaults::default();
        Self {
            enabled: false,
            default_timeout_secs: defaults.timeout_secs,
            restrict_to_project: true,
            shell: defaults.program,
            shell_path: None,
            project_root: PathBuf::new(),
            max_completed_jobs: default_max_completed_jobs(),
            completed_job_ttl_secs: default_completed_job_ttl_secs(),
            max_concurrent_processes: default_max_concurrent_processes(),
        }
    }
}

impl ShellConfig {
    fn resolve_working_dir_candidate(&self, dir: &Path) -> PathBuf {
        if dir.is_absolute() {
            dir.to_path_buf()
        } else {
            self.project_root.join(dir)
        }
    }

    /// Create a new ShellConfig with the given project root
    ///
    /// All other fields use default values.
    pub fn with_project_root(path: PathBuf) -> Self {
        Self {
            project_root: path,
            ..Default::default()
        }
    }

    /// Validate and resolve a working directory
    ///
    /// If `dir` is relative, it is joined with `project_root`.
    /// If `restrict_to_project` is true, verifies the resolved path is within project_root.
    ///
    /// # Errors
    ///
    /// - [`ShellError::WorkingDirNotFound`] if the path does not exist
    /// - [`ShellError::WorkingDirEscape`] if the path escapes project root (when restricted)
    pub fn validate_working_dir(&self, dir: &Path) -> Result<PathBuf, ShellError> {
        // Resolve the path: if relative, join with project_root
        let resolved = self.resolve_working_dir_candidate(dir);

        // Canonicalize to resolve symlinks and .. components
        let canonical = resolved
            .canonicalize()
            .map_err(|_| ShellError::WorkingDirNotFound(resolved.display().to_string()))?;

        // Check if path is within project root when restricted
        if self.restrict_to_project {
            let canonical_root = self.project_root.canonicalize().map_err(|_| {
                ShellError::WorkingDirNotFound(self.project_root.display().to_string())
            })?;

            if !canonical.starts_with(&canonical_root) {
                return Err(ShellError::WorkingDirEscape(dir.display().to_string()));
            }
        }

        Ok(canonical)
    }

    /// Async variant of [`ShellConfig::validate_working_dir`] that avoids blocking the tokio
    /// executor by using async filesystem operations.
    pub async fn validate_working_dir_async(&self, dir: &Path) -> Result<PathBuf, ShellError> {
        let resolved = self.resolve_working_dir_candidate(dir);

        let canonical = tokio::fs::canonicalize(&resolved)
            .await
            .map_err(|_| ShellError::WorkingDirNotFound(resolved.display().to_string()))?;

        if self.restrict_to_project {
            let canonical_root =
                tokio::fs::canonicalize(&self.project_root)
                    .await
                    .map_err(|_| {
                        ShellError::WorkingDirNotFound(self.project_root.display().to_string())
                    })?;

            if !canonical.starts_with(&canonical_root) {
                return Err(ShellError::WorkingDirEscape(dir.display().to_string()));
            }
        }

        Ok(canonical)
    }

    /// Resolve the shell executable path using config and environment (sync).
    ///
    /// Uses the explicit `shell_path` if present, then PATH lookup.
    pub fn resolve_shell_path(&self) -> Result<PathBuf, ShellError> {
        self.resolve_shell_path_with_fallbacks(&[])
    }

    pub(crate) fn resolve_shell_path_with_fallbacks(
        &self,
        fallbacks: &[&str],
    ) -> Result<PathBuf, ShellError> {
        let mut tried = Vec::new();

        let try_candidate = |candidate: &str, tried: &mut Vec<String>| -> Option<PathBuf> {
            if candidate.is_empty() {
                return None;
            }

            let path = PathBuf::from(candidate);
            if path.is_absolute() {
                if path.exists() {
                    return Some(path);
                }
            } else if let Ok(found) = which::which(candidate) {
                return Some(found);
            }

            if !tried.iter().any(|entry| entry == candidate) {
                tried.push(candidate.to_string());
            }
            None
        };

        // 1. Try explicit shell_path from config if set
        if let Some(ref path) = self.shell_path {
            if path.exists() {
                return Ok(path.clone());
            }
            tried.push(path.display().to_string());
        }

        // 2. Try configured shell name via which
        if let Ok(path) = which::which(&self.shell) {
            return Ok(path);
        }
        if !tried.iter().any(|entry| entry == &self.shell) {
            tried.push(self.shell.clone());
        }

        // 3. Try configured fallbacks (if any)
        for shell in fallbacks {
            if let Some(path) = try_candidate(shell, &mut tried) {
                return Ok(path);
            }
        }

        let details = if tried.is_empty() {
            self.shell.clone()
        } else {
            format!("{} (tried: {})", self.shell, tried.join(", "))
        };
        Err(ShellError::ShellNotInstalled(details))
    }

    /// Async variant of [`ShellConfig::resolve_shell_path`] that avoids blocking
    /// the tokio executor by running in a blocking task.
    pub async fn resolve_shell_path_async(&self) -> Result<PathBuf, ShellError> {
        let config = self.clone();
        tokio::task::spawn_blocking(move || config.resolve_shell_path())
            .await
            .map_err(|err| {
                ShellError::Io(std::io::Error::other(format!(
                    "Shell path resolution task failed: {err}"
                )))
            })?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // ==================== ShellConfig Struct Test ====================

    #[test]
    fn test_shell_config_struct() {
        // Test that ShellConfig can be constructed with all fields
        let config = ShellConfig {
            enabled: true,
            default_timeout_secs: 60,
            restrict_to_project: false,
            shell: "bash".to_string(),
            shell_path: Some(PathBuf::from("/usr/bin/bash")),
            project_root: PathBuf::from("/home/user/project"),
            max_completed_jobs: 50,
            completed_job_ttl_secs: 600,
            max_concurrent_processes: 5,
        };

        assert!(config.enabled);
        assert_eq!(config.default_timeout_secs, 60);
        assert!(!config.restrict_to_project);
        assert_eq!(config.shell, "bash");
        assert_eq!(config.shell_path, Some(PathBuf::from("/usr/bin/bash")));
        assert_eq!(config.project_root, PathBuf::from("/home/user/project"));
        assert_eq!(config.max_completed_jobs, 50);
        assert_eq!(config.completed_job_ttl_secs, 600);
        assert_eq!(config.max_concurrent_processes, 5);
    }

    // ==================== Default Test ====================

    #[test]
    fn test_shell_config_defaults() {
        let config = ShellConfig::default();

        // Verify spec defaults
        assert!(!config.enabled, "enabled should default to false");
        assert_eq!(
            config.default_timeout_secs, 30,
            "default_timeout_secs should be 30"
        );
        assert!(
            config.restrict_to_project,
            "restrict_to_project should default to true"
        );
        assert_eq!(config.shell, "nu", "shell should default to 'nu'");
        assert_eq!(
            config.project_root,
            PathBuf::new(),
            "project_root should default to empty PathBuf"
        );
        assert_eq!(
            config.max_completed_jobs, 100,
            "max_completed_jobs should default to 100"
        );
        assert_eq!(
            config.completed_job_ttl_secs, 300,
            "completed_job_ttl_secs should default to 300"
        );
        assert_eq!(
            config.max_concurrent_processes, 10,
            "max_concurrent_processes should default to 10"
        );
    }

    // ==================== Serde Roundtrip Test ====================

    #[test]
    fn test_shell_config_serde_roundtrip() {
        let config = ShellConfig {
            enabled: true,
            default_timeout_secs: 120,
            restrict_to_project: false,
            shell: "bash".to_string(),
            shell_path: Some(PathBuf::from("/usr/bin/bash")),
            project_root: PathBuf::from("/tmp/test"),
            max_completed_jobs: 200,
            completed_job_ttl_secs: 600,
            max_concurrent_processes: 15,
        };

        // Serialize to JSON
        let json = serde_json::to_string(&config).unwrap();

        // Verify JSON contains expected fields
        assert!(json.contains("\"enabled\":true"));
        assert!(json.contains("\"default_timeout_secs\":120"));
        assert!(json.contains("\"restrict_to_project\":false"));
        assert!(json.contains("\"shell\":\"bash\""));
        assert!(json.contains("\"shell_path\":\"/usr/bin/bash\""));
        assert!(json.contains("\"project_root\":\"/tmp/test\""));
        assert!(json.contains("\"max_concurrent_processes\":15"));

        // Deserialize back
        let parsed: ShellConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.enabled, config.enabled);
        assert_eq!(parsed.default_timeout_secs, config.default_timeout_secs);
        assert_eq!(parsed.restrict_to_project, config.restrict_to_project);
        assert_eq!(parsed.shell, config.shell);
        assert_eq!(parsed.shell_path, config.shell_path);
        assert_eq!(parsed.project_root, config.project_root);
        assert_eq!(parsed.max_completed_jobs, config.max_completed_jobs);
        assert_eq!(parsed.completed_job_ttl_secs, config.completed_job_ttl_secs);
        assert_eq!(
            parsed.max_concurrent_processes,
            config.max_concurrent_processes
        );
    }

    #[test]
    fn test_shell_config_serde_defaults_roundtrip() {
        let config = ShellConfig::default();

        let json = serde_json::to_string(&config).unwrap();
        let parsed: ShellConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.enabled, config.enabled);
        assert_eq!(parsed.default_timeout_secs, config.default_timeout_secs);
        assert_eq!(parsed.restrict_to_project, config.restrict_to_project);
        assert_eq!(parsed.shell, config.shell);
        assert_eq!(parsed.shell_path, config.shell_path);
        assert_eq!(parsed.project_root, config.project_root);
        assert_eq!(
            parsed.max_concurrent_processes,
            config.max_concurrent_processes
        );
    }

    // ==================== with_project_root Test ====================

    #[test]
    fn test_shell_config_with_project_root() {
        let path = PathBuf::from("/home/user/my-project");
        let config = ShellConfig::with_project_root(path.clone());

        // project_root should be set
        assert_eq!(config.project_root, path);

        // All other fields should have defaults
        assert!(!config.enabled);
        assert_eq!(config.default_timeout_secs, 30);
        assert!(config.restrict_to_project);
        assert_eq!(config.shell, "nu");
    }

    // ==================== validate_working_dir Tests ====================

    #[test]
    fn test_shell_config_validate_working_dir() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().to_path_buf();

        // Create a subdirectory
        let subdir = project_root.join("src");
        std::fs::create_dir(&subdir).unwrap();

        let config = ShellConfig::with_project_root(project_root.clone());

        // Test absolute path within project
        let result = config.validate_working_dir(&subdir);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), subdir.canonicalize().unwrap());

        // Test relative path
        let result = config.validate_working_dir(Path::new("src"));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), subdir.canonicalize().unwrap());

        // Test project root itself
        let result = config.validate_working_dir(&project_root);
        assert!(result.is_ok());
    }

    #[test]
    fn test_shell_config_rejects_escape() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().to_path_buf();

        let config = ShellConfig::with_project_root(project_root);

        // Test escape with ..
        let result = config.validate_working_dir(Path::new("../"));
        assert!(matches!(result, Err(ShellError::WorkingDirEscape(_))));

        // Test absolute path outside project
        let result = config.validate_working_dir(Path::new("/tmp"));
        assert!(matches!(result, Err(ShellError::WorkingDirEscape(_))));
    }

    #[test]
    fn test_shell_config_validate_nonexistent_dir() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().to_path_buf();

        let config = ShellConfig::with_project_root(project_root);

        // Test nonexistent directory
        let result = config.validate_working_dir(Path::new("nonexistent"));
        assert!(matches!(result, Err(ShellError::WorkingDirNotFound(_))));
    }

    #[test]
    fn test_shell_config_validate_unrestricted() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().to_path_buf();

        let mut config = ShellConfig::with_project_root(project_root);
        config.restrict_to_project = false;

        // When unrestricted, /tmp should be allowed
        let result = config.validate_working_dir(Path::new("/tmp"));
        assert!(result.is_ok());
    }

    // ==================== ShellError Tests ====================

    #[test]
    fn test_shell_error_display() {
        let err = ShellError::ShellNotInstalled("nu".to_string());
        assert_eq!(
            err.to_string(),
            "Shell not installed: nu. Install from https://nushell.sh"
        );

        let err = ShellError::BlockedCommand("rm -rf /".to_string());
        assert_eq!(err.to_string(), "Command blocked: contains 'rm -rf /'");

        let err = ShellError::WorkingDirEscape("../../../etc".to_string());
        assert_eq!(
            err.to_string(),
            "Working directory '../../../etc' is outside project root"
        );

        let err = ShellError::WorkingDirNotFound("/nonexistent".to_string());
        assert_eq!(err.to_string(), "Working directory not found: /nonexistent");

        let err = ShellError::JobNotFound("job_123".to_string());
        assert_eq!(err.to_string(), "Job not found: job_123");

        let err = ShellError::JobNotRunning;
        assert_eq!(err.to_string(), "Job is not running");

        let err = ShellError::BackgroundNotConfigured;
        assert_eq!(err.to_string(), "Background execution not configured");
    }

    #[test]
    fn test_shell_error_variants() {
        let err = ShellError::ShellNotInstalled("nu".to_string());
        assert!(matches!(err, ShellError::ShellNotInstalled(_)));

        let err = ShellError::BlockedCommand("rm -rf /".to_string());
        assert!(matches!(err, ShellError::BlockedCommand(_)));

        let err = ShellError::WorkingDirEscape("../../../etc".to_string());
        assert!(matches!(err, ShellError::WorkingDirEscape(_)));

        let err = ShellError::WorkingDirNotFound("/nonexistent".to_string());
        assert!(matches!(err, ShellError::WorkingDirNotFound(_)));

        let err = ShellError::JobNotFound("job_123".to_string());
        assert!(matches!(err, ShellError::JobNotFound(_)));

        let err = ShellError::JobNotRunning;
        assert!(matches!(err, ShellError::JobNotRunning));

        let err = ShellError::BackgroundNotConfigured;
        assert!(matches!(err, ShellError::BackgroundNotConfigured));

        let err = ShellError::Io(std::io::Error::other("io error"));
        assert!(matches!(err, ShellError::Io(_)));
    }

    #[test]
    fn test_shell_error_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let err: ShellError = io_err.into();
        assert!(err.to_string().contains("IO error"));
    }

    #[test]
    fn test_shell_error_from_io() {
        let io_err = std::io::Error::other("boom");
        let err: ShellError = io_err.into();
        assert!(matches!(err, ShellError::Io(_)));
    }
}
