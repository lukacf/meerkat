//! Shell jobs list tool implementation
//!
//! This module provides the [`ShellJobsListTool`] which lists all background
//! shell jobs and their status.

use crate::schema::empty_object_schema;
use async_trait::async_trait;
use meerkat_core::ToolDef;
use serde_json::Value;
use std::sync::Arc;

use super::job_manager::JobManager;
use crate::builtin::{BuiltinTool, BuiltinToolError};

/// Tool for listing all background shell jobs
///
/// Returns an array of [`JobSummary`] with basic info about each job.
#[derive(Debug)]
pub struct ShellJobsListTool {
    /// Shared job manager instance
    job_manager: Arc<JobManager>,
}

impl ShellJobsListTool {
    /// Create a new ShellJobsListTool with the given job manager
    pub fn new(job_manager: Arc<JobManager>) -> Self {
        Self { job_manager }
    }
}

#[async_trait]
impl BuiltinTool for ShellJobsListTool {
    fn name(&self) -> &'static str {
        "shell_jobs"
    }

    fn def(&self) -> ToolDef {
        ToolDef {
            name: "shell_jobs".into(),
            description: "List all background shell jobs".into(),
            input_schema: empty_object_schema(),
        }
    }

    fn default_enabled(&self) -> bool {
        false
    }

    async fn call(&self, _args: Value) -> Result<Value, BuiltinToolError> {
        let jobs = self.job_manager.list_jobs().await;

        serde_json::to_value(jobs).map_err(|e| BuiltinToolError::execution_failed(e.to_string()))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::builtin::shell::config::ShellConfig;
    use serde_json::json;
    #[cfg(feature = "integration-real-tests")]
    use tempfile::TempDir;

    // ==================== ShellJobsListTool Struct Tests ====================

    #[test]
    fn test_shell_jobs_list_tool_struct() {
        let config = ShellConfig::default();
        let manager = Arc::new(JobManager::new(config));
        let _tool = ShellJobsListTool::new(Arc::clone(&manager));

        // Tool should hold Arc<JobManager>
        assert!(Arc::strong_count(&manager) >= 2);
    }

    // ==================== BuiltinTool Trait Tests ====================

    #[test]
    fn test_shell_jobs_list_tool_builtin() {
        // Verify ShellJobsListTool implements BuiltinTool trait
        fn assert_builtin_tool<T: BuiltinTool>() {}
        assert_builtin_tool::<ShellJobsListTool>();
    }

    #[test]
    fn test_shell_jobs_list_tool_name() {
        let config = ShellConfig::default();
        let manager = Arc::new(JobManager::new(config));
        let tool = ShellJobsListTool::new(manager);

        assert_eq!(tool.name(), "shell_jobs");
    }

    #[test]
    fn test_shell_jobs_list_tool_schema() {
        let config = ShellConfig::default();
        let manager = Arc::new(JobManager::new(config));
        let tool = ShellJobsListTool::new(manager);

        let def = tool.def();

        assert_eq!(def.name, "shell_jobs");
        assert!(def.description.contains("List") || def.description.contains("list"));

        // Verify schema structure - should be empty object (no required params)
        let schema = &def.input_schema;
        assert_eq!(schema["type"], "object");

        // Properties should be empty
        let props = schema.get("properties").unwrap();
        assert!(props.as_object().is_none_or(|o| o.is_empty()));

        // Should have empty required fields array
        assert_eq!(schema["required"], serde_json::json!([]));
    }

    // ==================== Output Tests ====================

    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: spawns shell processes"]
    async fn integration_real_shell_jobs_list_tool_output() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.shell = "sh".to_string();

        let manager = Arc::new(JobManager::new(config));
        let tool = ShellJobsListTool::new(Arc::clone(&manager));

        // Initially empty
        let result = tool.call(json!({})).await.unwrap();
        let jobs = result.as_array().unwrap();
        assert!(jobs.is_empty());

        // Spawn some jobs
        let _id1 = manager.spawn_job("echo one", None, 30).await.unwrap();
        let _id2 = manager.spawn_job("echo two", None, 30).await.unwrap();

        // Now should have 2 jobs
        let result = tool.call(json!({})).await.unwrap();
        let jobs = result.as_array().unwrap();
        assert_eq!(jobs.len(), 2);

        // Each job should have JobSummary fields
        for job in jobs {
            assert!(job.get("id").is_some());
            assert!(job.get("command").is_some());
            assert!(job.get("status").is_some());
            assert!(job.get("started_at_unix").is_some());
        }
    }

    #[tokio::test]
    async fn test_shell_jobs_list_tool_empty_list() {
        let config = ShellConfig::default();
        let manager = Arc::new(JobManager::new(config));
        let tool = ShellJobsListTool::new(manager);

        // Empty list should return empty array
        let result = tool.call(json!({})).await.unwrap();
        let jobs = result.as_array().unwrap();
        assert!(jobs.is_empty());
    }
}
