//! Shell job status tool implementation
//!
//! This module provides the [`ShellJobStatusTool`] which checks the status
//! of a background shell job.

use async_trait::async_trait;
use meerkat_core::ToolDef;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

use super::config::ShellError;
use super::job_manager::JobManager;
use super::types::JobId;
use crate::builtin::{BuiltinTool, BuiltinToolError};

/// Tool for checking the status of a background shell job
///
/// Returns the full [`BackgroundJob`] information for a given job ID.
#[derive(Debug)]
pub struct ShellJobStatusTool {
    /// Shared job manager instance
    job_manager: Arc<JobManager>,
}

impl ShellJobStatusTool {
    /// Create a new ShellJobStatusTool with the given job manager
    pub fn new(job_manager: Arc<JobManager>) -> Self {
        Self { job_manager }
    }
}

/// Input arguments for the job status tool
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
struct JobStatusInput {
    /// The job ID to check
    #[schemars(description = "The job ID to check")]
    job_id: String,
}

#[async_trait]
impl BuiltinTool for ShellJobStatusTool {
    fn name(&self) -> &'static str {
        "shell_job_status"
    }

    fn def(&self) -> ToolDef {
        ToolDef {
            name: "shell_job_status".into(),
            description: "Check status of a background shell job".into(),
            input_schema: crate::schema::schema_for::<JobStatusInput>(),
        }
    }

    fn default_enabled(&self) -> bool {
        false
    }

    async fn call(&self, args: Value) -> Result<Value, BuiltinToolError> {
        let input: JobStatusInput = serde_json::from_value(args)
            .map_err(|e| BuiltinToolError::invalid_args(e.to_string()))?;

        let job_id = JobId::from_string(&input.job_id);

        let job = self.job_manager.get_status(&job_id).await.ok_or_else(|| {
            BuiltinToolError::execution_failed(
                ShellError::JobNotFound(input.job_id.clone()).to_string(),
            )
        })?;

        serde_json::to_value(job).map_err(|e| BuiltinToolError::execution_failed(e.to_string()))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::builtin::shell::config::ShellConfig;
    use serde_json::json;
    use tempfile::TempDir;

    // ==================== ShellJobStatusTool Struct Tests ====================

    #[test]
    fn test_shell_job_status_tool_struct() {
        let config = ShellConfig::default();
        let manager = Arc::new(JobManager::new(config));
        let _tool = ShellJobStatusTool::new(Arc::clone(&manager));

        // Tool should hold Arc<JobManager>
        assert!(Arc::strong_count(&manager) >= 2);
    }

    // ==================== BuiltinTool Trait Tests ====================

    #[test]
    fn test_shell_job_status_tool_builtin() {
        // Verify ShellJobStatusTool implements BuiltinTool trait
        fn assert_builtin_tool<T: BuiltinTool>() {}
        assert_builtin_tool::<ShellJobStatusTool>();
    }

    #[test]
    fn test_shell_job_status_tool_name() {
        let config = ShellConfig::default();
        let manager = Arc::new(JobManager::new(config));
        let tool = ShellJobStatusTool::new(manager);

        assert_eq!(tool.name(), "shell_job_status");
    }

    #[test]
    fn test_shell_job_status_tool_schema() {
        let config = ShellConfig::default();
        let manager = Arc::new(JobManager::new(config));
        let tool = ShellJobStatusTool::new(manager);

        let def = tool.def();

        assert_eq!(def.name, "shell_job_status");
        assert!(def.description.contains("status"));

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
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: spawns shell processes"]
    async fn integration_real_shell_job_status_tool_output() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.shell = "sh".to_string();

        let manager = Arc::new(JobManager::new(config));
        let tool = ShellJobStatusTool::new(Arc::clone(&manager));

        // Spawn a job
        let job_id = manager.spawn_job("echo test", None, 30).await.unwrap();

        // Get status
        let result = tool
            .call(json!({
                "job_id": job_id.0
            }))
            .await
            .unwrap();

        // Verify output has expected fields
        assert!(result.get("id").is_some());
        assert!(result.get("command").is_some());
        assert!(result.get("status").is_some());
        assert!(result.get("timeout_secs").is_some());

        // Verify values
        assert_eq!(result["id"], job_id.0);
        assert_eq!(result["command"], "echo test");
    }

    // ==================== Error Tests ====================

    #[tokio::test]
    async fn test_shell_job_status_tool_not_found() {
        let config = ShellConfig::default();
        let manager = Arc::new(JobManager::new(config));
        let tool = ShellJobStatusTool::new(manager);

        // Try to get status of non-existent job
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
    async fn test_shell_job_status_tool_invalid_args() {
        let config = ShellConfig::default();
        let manager = Arc::new(JobManager::new(config));
        let tool = ShellJobStatusTool::new(manager);

        // Missing job_id
        let result = tool.call(json!({})).await;
        assert!(matches!(result, Err(BuiltinToolError::InvalidArgs(_))));
    }
}
