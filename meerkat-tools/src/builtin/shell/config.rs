//! Shell tool configuration
//!
//! This module defines [`ShellConfig`] which controls shell tool behavior,
//! and [`ShellError`] for shell-related errors.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Errors that can occur during shell operations
#[derive(Debug, Error)]
pub enum ShellError {
    /// Shell executable not found on system
    #[error("Shell '{shell}' not found at path, tried fallbacks: {tried:?}")]
    ShellNotFound {
        /// The requested shell name
        shell: String,
        /// List of paths that were tried
        tried: Vec<String>,
    },

    /// Command matches a blocked pattern
    #[error("Command blocked: contains '{pattern}'")]
    BlockedCommand {
        /// The blocked pattern that was matched
        pattern: String,
    },

    /// Working directory escapes project root
    #[error("Working directory '{path}' is outside project root '{project_root}'")]
    WorkingDirEscape {
        /// The path that was attempted
        path: String,
        /// The project root directory
        project_root: String,
    },

    /// Working directory does not exist
    #[error("Working directory not found: {path}")]
    WorkingDirNotFound {
        /// The path that was not found
        path: String,
    },

    /// Job not found in job manager
    #[error("Job {job_id} not found while attempting {operation}")]
    JobNotFound {
        /// The job ID that was not found
        job_id: String,
        /// The operation that was attempted
        operation: &'static str,
    },

    /// Job is not in running state
    #[error("Job {job_id} already completed (status: {status}) while attempting {operation}")]
    JobAlreadyCompleted {
        /// The job ID
        job_id: String,
        /// The current status of the job
        status: String,
        /// The operation that was attempted
        operation: &'static str,
    },

    /// Background execution not configured
    #[error("Background execution not configured")]
    BackgroundNotConfigured,

    /// Concurrency limit exceeded
    #[error("Concurrency limit exceeded: {current} running jobs, limit is {limit}")]
    ConcurrencyLimitExceeded {
        /// Current number of running jobs
        current: usize,
        /// Configured limit
        limit: usize,
    },

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
    /// When this limit is reached, new job spawn requests will be rejected
    /// with a ConcurrencyLimitExceeded error. Set to 0 for unlimited.
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
        Self {
            enabled: false,
            default_timeout_secs: 30,
            restrict_to_project: true,
            shell: "nu".to_string(),
            shell_path: None,
            project_root: PathBuf::new(),
            max_completed_jobs: default_max_completed_jobs(),
            completed_job_ttl_secs: default_completed_job_ttl_secs(),
            max_concurrent_processes: default_max_concurrent_processes(),
        }
    }
}

impl ShellConfig {
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
        let resolved = if dir.is_absolute() {
            dir.to_path_buf()
        } else {
            self.project_root.join(dir)
        };

        // Canonicalize to resolve symlinks and .. components
        let canonical = resolved
            .canonicalize()
            .map_err(|_| ShellError::WorkingDirNotFound {
                path: resolved.display().to_string(),
            })?;

        // Check if path is within project root when restricted
        if self.restrict_to_project {
            let canonical_root =
                self.project_root
                    .canonicalize()
                    .map_err(|_| ShellError::WorkingDirNotFound {
                        path: self.project_root.display().to_string(),
                    })?;

            if !canonical.starts_with(&canonical_root) {
                return Err(ShellError::WorkingDirEscape {
                    path: dir.display().to_string(),
                    project_root: self.project_root.display().to_string(),
                });
            }
        }

        Ok(canonical)
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
        assert!(json.contains("\"project_root\":\"/tmp/test\""));

        // Deserialize back
        let parsed: ShellConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.enabled, config.enabled);
        assert_eq!(parsed.default_timeout_secs, config.default_timeout_secs);
        assert_eq!(parsed.restrict_to_project, config.restrict_to_project);
        assert_eq!(parsed.shell, config.shell);
        assert_eq!(parsed.project_root, config.project_root);
        assert_eq!(parsed.max_completed_jobs, config.max_completed_jobs);
        assert_eq!(parsed.completed_job_ttl_secs, config.completed_job_ttl_secs);
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
        assert_eq!(parsed.project_root, config.project_root);
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
        assert!(matches!(result, Err(ShellError::WorkingDirEscape { .. })));

        // Test absolute path outside project
        let result = config.validate_working_dir(Path::new("/tmp"));
        assert!(matches!(result, Err(ShellError::WorkingDirEscape { .. })));
    }

    #[test]
    fn test_shell_config_validate_nonexistent_dir() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().to_path_buf();

        let config = ShellConfig::with_project_root(project_root);

        // Test nonexistent directory
        let result = config.validate_working_dir(Path::new("nonexistent"));
        assert!(matches!(result, Err(ShellError::WorkingDirNotFound { .. })));
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
        let err = ShellError::ShellNotFound {
            shell: "nu".to_string(),
            tried: vec!["/bin/nu".to_string(), "/usr/bin/nu".to_string()],
        };
        assert_eq!(
            err.to_string(),
            "Shell 'nu' not found at path, tried fallbacks: [\"/bin/nu\", \"/usr/bin/nu\"]"
        );

        let err = ShellError::BlockedCommand {
            pattern: "rm -rf /".to_string(),
        };
        assert_eq!(err.to_string(), "Command blocked: contains 'rm -rf /'");

        let err = ShellError::WorkingDirEscape {
            path: "../../../etc".to_string(),
            project_root: "/home/user/project".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "Working directory '../../../etc' is outside project root '/home/user/project'"
        );

        let err = ShellError::WorkingDirNotFound {
            path: "/nonexistent".to_string(),
        };
        assert_eq!(err.to_string(), "Working directory not found: /nonexistent");

        let err = ShellError::JobNotFound {
            job_id: "job_123".to_string(),
            operation: "get_status",
        };
        assert_eq!(
            err.to_string(),
            "Job job_123 not found while attempting get_status"
        );

        let err = ShellError::JobAlreadyCompleted {
            job_id: "job_456".to_string(),
            status: "completed".to_string(),
            operation: "cancel",
        };
        assert_eq!(
            err.to_string(),
            "Job job_456 already completed (status: completed) while attempting cancel"
        );

        let err = ShellError::BackgroundNotConfigured;
        assert_eq!(err.to_string(), "Background execution not configured");

        let err = ShellError::ConcurrencyLimitExceeded {
            current: 10,
            limit: 10,
        };
        assert_eq!(
            err.to_string(),
            "Concurrency limit exceeded: 10 running jobs, limit is 10"
        );
    }

    #[test]
    fn test_shell_error_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let err: ShellError = io_err.into();
        assert!(err.to_string().contains("IO error"));
    }
}
