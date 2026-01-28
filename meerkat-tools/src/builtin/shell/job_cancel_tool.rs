//! Shell job cancel tool implementation
//!
//! This module provides the [`ShellJobCancelTool`] which cancels a running
//! background shell job.

use crate::schema::SchemaBuilder;
use async_trait::async_trait;
use meerkat_core::ToolDef;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

use super::job_manager::JobManager;
use super::types::JobId;
use crate::builtin::{BuiltinTool, BuiltinToolError};

/// Tool for cancelling a running background shell job
///
/// Sends termination signals to the job's process and updates its status
/// to [`JobStatus::Cancelled`].
#[derive(Debug)]
pub struct ShellJobCancelTool {
    /// Shared job manager instance
    job_manager: Arc<JobManager>,
}

impl ShellJobCancelTool {
    /// Create a new ShellJobCancelTool with the given job manager
    pub fn new(job_manager: Arc<JobManager>) -> Self {
        Self { job_manager }
    }
}

/// Input arguments for the job cancel tool
#[derive(Debug, Clone, Deserialize)]
struct JobCancelInput {
    /// The job ID to cancel
    job_id: String,
}

#[async_trait]
impl BuiltinTool for ShellJobCancelTool {
    fn name(&self) -> &'static str {
        "shell_job_cancel"
    }

    fn def(&self) -> ToolDef {
        ToolDef {
            name: "shell_job_cancel".to_string(),
            description: "Cancel a running background shell job".to_string(),
            input_schema: SchemaBuilder::new()
                .property(
                    "job_id",
                    json!({
                        "type": "string",
                        "description": "The job ID to cancel"
                    }),
                )
                .required("job_id")
                .build(),
        }
    }

    fn default_enabled(&self) -> bool {
        false
    }

    async fn call(&self, args: Value) -> Result<Value, BuiltinToolError> {
        let input: JobCancelInput = serde_json::from_value(args)
            .map_err(|e| BuiltinToolError::invalid_args(e.to_string()))?;

        let job_id = JobId::from_string(&input.job_id);

        self.job_manager
            .cancel_job(&job_id)
            .await
            .map_err(|e| BuiltinToolError::execution_failed(e.to_string()))?;

        Ok(json!({
            "job_id": input.job_id,
            "status": "cancelled"
        }))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::builtin::shell::config::ShellConfig;
    use crate::builtin::shell::types::JobStatus;
    use std::time::Duration;
    use tempfile::TempDir;

    // ==================== ShellJobCancelTool Struct Tests ====================

    #[test]
    fn test_shell_job_cancel_tool_struct() {
        let config = ShellConfig::default();
        let manager = Arc::new(JobManager::new(config));
        let _tool = ShellJobCancelTool::new(Arc::clone(&manager));

        // Tool should hold Arc<JobManager>
        assert!(Arc::strong_count(&manager) >= 2);
    }

    // ==================== BuiltinTool Trait Tests ====================

    #[test]
    fn test_shell_job_cancel_tool_builtin() {
        // Verify ShellJobCancelTool implements BuiltinTool trait
        fn assert_builtin_tool<T: BuiltinTool>() {}
        assert_builtin_tool::<ShellJobCancelTool>();
    }

    #[test]
    fn test_shell_job_cancel_tool_name() {
        let config = ShellConfig::default();
        let manager = Arc::new(JobManager::new(config));
        let tool = ShellJobCancelTool::new(manager);

        assert_eq!(tool.name(), "shell_job_cancel");
    }

    #[test]
    fn test_shell_job_cancel_tool_schema() {
        let config = ShellConfig::default();
        let manager = Arc::new(JobManager::new(config));
        let tool = ShellJobCancelTool::new(manager);

        let def = tool.def();

        assert_eq!(def.name, "shell_job_cancel");
        assert!(def.description.contains("Cancel") || def.description.contains("cancel"));

        // Verify schema structure
        let schema = &def.input_schema;
        assert_eq!(schema["type"], "object");

        // Check job_id property exists
        let props = &schema["properties"];
        assert!(props.get("job_id").is_some());
        assert_eq!(props["job_id"]["type"], "string");

        // Check required fields
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("job_id")));
    }

    // ==================== Output Tests ====================

    #[tokio::test]
    async fn test_shell_job_cancel_tool_output() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.shell = "sh".to_string();

        let manager = Arc::new(JobManager::new(config));
        let tool = ShellJobCancelTool::new(Arc::clone(&manager));

        // Spawn a long-running job
        let job_id = manager.spawn_job("sleep 60", None, 120).await.unwrap();

        // Give it a moment to start
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Cancel it
        let result = tool
            .call(json!({
                "job_id": job_id.0
            }))
            .await
            .unwrap();

        // Verify output
        assert_eq!(result["job_id"], job_id.0);
        assert_eq!(result["status"], "cancelled");
    }

    // ==================== Error Tests ====================

    #[tokio::test]
    async fn test_shell_job_cancel_tool_not_found() {
        let config = ShellConfig::default();
        let manager = Arc::new(JobManager::new(config));
        let tool = ShellJobCancelTool::new(manager);

        // Try to cancel non-existent job
        let result = tool
            .call(json!({
                "job_id": "job_nonexistent123456789012"
            }))
            .await;

        assert!(matches!(result, Err(BuiltinToolError::ExecutionFailed(_))));

        if let Err(BuiltinToolError::ExecutionFailed(msg)) = result {
            assert!(msg.contains("not found") || msg.contains("Job not found"));
        }
    }

    #[tokio::test]
    async fn test_shell_job_cancel_tool_not_running() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.shell = "sh".to_string();

        let manager = Arc::new(JobManager::new(config));
        let tool = ShellJobCancelTool::new(Arc::clone(&manager));

        // Spawn a quick job that will complete fast
        let job_id = manager.spawn_job("echo done", None, 30).await.unwrap();

        // Wait for it to complete
        tokio::time::sleep(Duration::from_secs(1)).await;

        // Verify it completed
        let job = manager.get_status(&job_id).await.unwrap();
        assert!(
            matches!(job.status, JobStatus::Completed { .. }),
            "Job should be completed for this test"
        );

        // Try to cancel already-completed job
        let result = tool
            .call(json!({
                "job_id": job_id.0
            }))
            .await;

        assert!(matches!(result, Err(BuiltinToolError::ExecutionFailed(_))));

        if let Err(BuiltinToolError::ExecutionFailed(msg)) = result {
            assert!(
                msg.contains("already completed") || msg.contains("not running"),
                "Expected 'already completed' in error message, got: {}",
                msg
            );
        }
    }

    #[tokio::test]
    async fn test_shell_job_cancel_tool_invalid_args() {
        let config = ShellConfig::default();
        let manager = Arc::new(JobManager::new(config));
        let tool = ShellJobCancelTool::new(manager);

        // Missing job_id
        let result = tool.call(json!({})).await;
        assert!(matches!(result, Err(BuiltinToolError::InvalidArgs(_))));
    }
}
