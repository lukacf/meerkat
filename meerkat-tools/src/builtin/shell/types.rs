//! Shell tool types for background job execution
//!
//! This module defines the core types for shell job management including
//! [`JobId`], [`JobStatus`], [`BackgroundJob`], and [`JobSummary`].

use serde::{Deserialize, Serialize};

/// Unique identifier for background jobs
///
/// Format: "job_" + UUID v7 (36 chars)
/// Example: "job_01hx7z8k-9m2n-3p4q-5r6s-7t8u9v"
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct JobId(pub String);

impl JobId {
    /// Create a new JobId with a generated UUID v7
    ///
    /// The format is "job_" followed by a lowercase UUID v7.
    pub fn new() -> Self {
        Self(format!("job_{}", uuid::Uuid::now_v7()))
    }

    /// Create a JobId from an existing string
    pub fn from_string(s: impl Into<String>) -> Self {
        Self(s.into())
    }
}

impl Default for JobId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for JobId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl AsRef<str> for JobId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Status of a background job in its lifecycle
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
#[non_exhaustive]
pub enum JobStatus {
    /// Job is currently executing
    Running {
        /// Unix timestamp when the job started
        started_at_unix: u64,
    },

    /// Job completed successfully
    Completed {
        /// Exit code from the process (None if process was killed)
        exit_code: Option<i32>,
        /// Standard output captured from the process
        stdout: String,
        /// Standard error captured from the process
        stderr: String,
        /// Duration of execution in seconds
        duration_secs: f64,
    },

    /// Job failed to execute (spawn error, etc.)
    Failed {
        /// Error message describing the failure
        error: String,
        /// Duration before failure in seconds
        duration_secs: f64,
    },

    /// Job exceeded timeout
    TimedOut {
        /// Partial stdout captured before timeout
        stdout: String,
        /// Partial stderr captured before timeout
        stderr: String,
        /// Duration (should be approximately the timeout value)
        duration_secs: f64,
    },

    /// Job was cancelled by user
    Cancelled {
        /// Duration before cancellation in seconds
        duration_secs: f64,
    },
}

/// A background job in the job management system
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackgroundJob {
    /// Unique identifier for this job
    pub id: JobId,
    /// The command being executed
    pub command: String,
    /// Working directory for the command (None means current directory)
    pub working_dir: Option<String>,
    /// Timeout in seconds for the command
    pub timeout_secs: u64,
    /// Current status of the job
    pub status: JobStatus,
}

/// Lightweight job info for listing
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobSummary {
    /// Unique identifier for this job
    pub id: JobId,
    /// The command being executed
    pub command: String,
    /// Status as a simple string: "running", "completed", "failed", "timed_out", "cancelled"
    pub status: String,
    /// Unix timestamp when the job started
    pub started_at_unix: u64,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    // ==================== JobId Tests ====================

    #[test]
    fn test_job_id_format() {
        let id = JobId::new();

        // Should start with "job_"
        assert!(id.0.starts_with("job_"), "JobId should start with 'job_'");

        // Should be job_ (4 chars) + UUID (36 chars) = 40 chars total
        assert_eq!(id.0.len(), 40, "JobId should be 40 characters");

        // The UUID part should be valid
        let uuid_part = &id.0[4..];
        assert!(uuid::Uuid::parse_str(uuid_part).is_ok());
    }

    #[test]
    fn test_job_id_new_unique() {
        let id1 = JobId::new();
        let id2 = JobId::new();

        // Should generate different IDs
        assert_ne!(id1, id2, "Generated JobIds should be unique");
    }

    #[test]
    fn test_job_id_serde_roundtrip() {
        let id = JobId::from_string("job_01hx7z8k9m2n3p4q5r6s7t8u9v");

        // Serialize to JSON
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"job_01hx7z8k9m2n3p4q5r6s7t8u9v\"");

        // Deserialize back
        let parsed: JobId = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, id);
    }

    #[test]
    fn test_job_id_display() {
        let id = JobId::from_string("job_01hx7z8k9m2n3p4q5r6s7t8u9v");
        assert_eq!(format!("{}", id), "job_01hx7z8k9m2n3p4q5r6s7t8u9v");
    }

    #[test]
    fn test_job_id_as_ref() {
        let id = JobId::from_string("job_01hx7z8k9m2n3p4q5r6s7t8u9v");
        let s: &str = id.as_ref();
        assert_eq!(s, "job_01hx7z8k9m2n3p4q5r6s7t8u9v");
    }

    #[test]
    fn test_job_id_default() {
        let id = JobId::default();
        assert!(id.0.starts_with("job_"));
        assert_eq!(id.0.len(), 40);
    }

    // ==================== JobStatus Tests ====================

    #[test]
    fn test_job_status_variants() {
        // Test that all variants can be constructed
        let running = JobStatus::Running {
            started_at_unix: 1706123456,
        };
        let completed = JobStatus::Completed {
            exit_code: Some(0),
            stdout: "output".to_string(),
            stderr: "".to_string(),
            duration_secs: 1.5,
        };
        let failed = JobStatus::Failed {
            error: "spawn error".to_string(),
            duration_secs: 0.1,
        };
        let timed_out = JobStatus::TimedOut {
            stdout: "partial".to_string(),
            stderr: "".to_string(),
            duration_secs: 30.0,
        };
        let cancelled = JobStatus::Cancelled { duration_secs: 5.0 };

        // Verify they are distinct
        assert_ne!(running, completed);
        assert_ne!(completed, failed);
        assert_ne!(failed, timed_out);
        assert_ne!(timed_out, cancelled);
    }

    #[test]
    fn test_job_status_serde_roundtrip() {
        // Test Running
        let running = JobStatus::Running {
            started_at_unix: 1706123456,
        };
        let json = serde_json::to_string(&running).unwrap();
        assert!(json.contains("\"status\":\"running\""));
        assert!(json.contains("\"started_at_unix\":1706123456"));
        let parsed: JobStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, running);

        // Test Completed
        let completed = JobStatus::Completed {
            exit_code: Some(0),
            stdout: "hello".to_string(),
            stderr: "warning".to_string(),
            duration_secs: 2.5,
        };
        let json = serde_json::to_string(&completed).unwrap();
        assert!(json.contains("\"status\":\"completed\""));
        let parsed: JobStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, completed);

        // Test Failed
        let failed = JobStatus::Failed {
            error: "command not found".to_string(),
            duration_secs: 0.01,
        };
        let json = serde_json::to_string(&failed).unwrap();
        assert!(json.contains("\"status\":\"failed\""));
        let parsed: JobStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, failed);

        // Test TimedOut
        let timed_out = JobStatus::TimedOut {
            stdout: "partial output".to_string(),
            stderr: "".to_string(),
            duration_secs: 30.0,
        };
        let json = serde_json::to_string(&timed_out).unwrap();
        assert!(json.contains("\"status\":\"timed_out\""));
        let parsed: JobStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, timed_out);

        // Test Cancelled
        let cancelled = JobStatus::Cancelled { duration_secs: 5.5 };
        let json = serde_json::to_string(&cancelled).unwrap();
        assert!(json.contains("\"status\":\"cancelled\""));
        let parsed: JobStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, cancelled);
    }

    #[test]
    fn test_job_status_completed_fields() {
        let completed = JobStatus::Completed {
            exit_code: Some(1),
            stdout: "some output".to_string(),
            stderr: "some error".to_string(),
            duration_secs: 10.25,
        };

        let json = serde_json::to_string(&completed).unwrap();
        let json_val: serde_json::Value = serde_json::from_str(&json).unwrap();
        let obj = json_val
            .as_object()
            .expect("JobStatus should serialize to JSON object");

        assert_eq!(
            obj.get("status").and_then(|v| v.as_str()),
            Some("completed")
        );
        assert!(obj.contains_key("exit_code"));
        assert!(obj.contains_key("stdout"));
        assert!(obj.contains_key("stderr"));
        assert!(obj.contains_key("duration_secs"));

        // Test with None exit_code (process killed)
        let completed_no_exit = JobStatus::Completed {
            exit_code: None,
            stdout: "".to_string(),
            stderr: "".to_string(),
            duration_secs: 0.0,
        };

        let json = serde_json::to_string(&completed_no_exit).unwrap();
        let json_val: serde_json::Value = serde_json::from_str(&json).unwrap();
        let obj = json_val
            .as_object()
            .expect("JobStatus should serialize to JSON object");

        assert_eq!(
            obj.get("status").and_then(|v| v.as_str()),
            Some("completed")
        );
        assert!(matches!(obj.get("exit_code"), Some(v) if v.is_null()));
        assert!(obj.contains_key("stdout"));
        assert!(obj.contains_key("stderr"));
        assert!(obj.contains_key("duration_secs"));
    }

    // ==================== BackgroundJob Tests ====================

    #[test]
    fn test_background_job_struct() {
        let job = BackgroundJob {
            id: JobId::from_string("job_01hx7z8k9m2n3p4q5r6s7t8u9v"),
            command: "cargo build".to_string(),
            working_dir: Some("/project".to_string()),
            timeout_secs: 300,
            status: JobStatus::Running {
                started_at_unix: 1706123456,
            },
        };

        assert_eq!(job.id.0, "job_01hx7z8k9m2n3p4q5r6s7t8u9v");
        assert_eq!(job.command, "cargo build");
        assert_eq!(job.working_dir, Some("/project".to_string()));
        assert_eq!(job.timeout_secs, 300);
        assert!(matches!(job.status, JobStatus::Running { .. }));
    }

    #[test]
    fn test_background_job_serde_roundtrip() {
        let job = BackgroundJob {
            id: JobId::from_string("job_01hx7z8k9m2n3p4q5r6s7t8u9v"),
            command: "cargo test".to_string(),
            working_dir: None,
            timeout_secs: 60,
            status: JobStatus::Completed {
                exit_code: Some(0),
                stdout: "All tests passed".to_string(),
                stderr: "".to_string(),
                duration_secs: 15.3,
            },
        };

        let json = serde_json::to_string_pretty(&job).unwrap();
        let parsed: BackgroundJob = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.id, job.id);
        assert_eq!(parsed.command, job.command);
        assert_eq!(parsed.working_dir, job.working_dir);
        assert_eq!(parsed.timeout_secs, job.timeout_secs);

        // Verify status
        if let JobStatus::Completed {
            exit_code,
            stdout,
            stderr,
            duration_secs,
        } = &parsed.status
        {
            assert_eq!(*exit_code, Some(0));
            assert_eq!(stdout, "All tests passed");
            assert_eq!(stderr, "");
            assert!((*duration_secs - 15.3).abs() < f64::EPSILON);
        } else {
            unreachable!("Expected Completed status");
        }
    }

    // ==================== JobSummary Tests ====================

    #[test]
    fn test_job_summary_struct() {
        let summary = JobSummary {
            id: JobId::from_string("job_01hx7z8k9m2n3p4q5r6s7t8u9v"),
            command: "npm test".to_string(),
            status: "running".to_string(),
            started_at_unix: 1706123500,
        };

        assert_eq!(summary.id.0, "job_01hx7z8k9m2n3p4q5r6s7t8u9v");
        assert_eq!(summary.command, "npm test");
        assert_eq!(summary.status, "running");
        assert_eq!(summary.started_at_unix, 1706123500);
    }

    #[test]
    fn test_job_summary_serde_roundtrip() {
        let summary = JobSummary {
            id: JobId::from_string("job_01hx7z9abcdefghijklmnopqr"),
            command: "make build".to_string(),
            status: "completed".to_string(),
            started_at_unix: 1706123456,
        };

        let json = serde_json::to_string(&summary).unwrap();
        let parsed: JobSummary = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.id, summary.id);
        assert_eq!(parsed.command, summary.command);
        assert_eq!(parsed.status, summary.status);
        assert_eq!(parsed.started_at_unix, summary.started_at_unix);
    }

    #[test]
    fn test_job_summary_status_values() {
        // Test all valid status string values
        let statuses = ["running", "completed", "failed", "timed_out", "cancelled"];

        for status in statuses {
            let summary = JobSummary {
                id: JobId::from_string("job_test"),
                command: "test".to_string(),
                status: status.to_string(),
                started_at_unix: 0,
            };
            assert_eq!(summary.status, status);
        }
    }
}
