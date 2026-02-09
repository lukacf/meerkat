//! Shell tool set implementation
//!
//! This module provides [`ShellToolSet`] which bundles all shell tools
//! with a shared [`JobManager`] instance.

use serde_json::Value;
use std::sync::Arc;
use tokio::sync::mpsc;

use super::config::ShellConfig;
use super::job_cancel_tool::ShellJobCancelTool;
use super::job_manager::JobManager;
use super::job_status_tool::ShellJobStatusTool;
use super::jobs_list_tool::ShellJobsListTool;
use super::tool::ShellTool;
use crate::builtin::BuiltinTool;

/// A set of all shell-related tools with shared job manager
///
/// This struct provides a convenient way to create and access all shell tools
/// with a shared [`JobManager`] for background job coordination.
#[derive(Debug)]
pub struct ShellToolSet {
    /// The main shell tool for command execution
    pub shell: ShellTool,
    /// Tool for checking job status
    pub job_status: ShellJobStatusTool,
    /// Tool for listing all jobs
    pub jobs_list: ShellJobsListTool,
    /// Tool for cancelling jobs
    pub job_cancel: ShellJobCancelTool,
    /// Shared job manager (for external access if needed)
    pub job_manager: Arc<JobManager>,
}

impl ShellToolSet {
    /// Create a new ShellToolSet with the given configuration
    ///
    /// All tools share the same [`JobManager`] instance.
    pub fn new(config: ShellConfig) -> Self {
        let job_manager = Arc::new(JobManager::new(config.clone()));

        Self {
            shell: ShellTool::with_job_manager(config, Arc::clone(&job_manager)),
            job_status: ShellJobStatusTool::new(Arc::clone(&job_manager)),
            jobs_list: ShellJobsListTool::new(Arc::clone(&job_manager)),
            job_cancel: ShellJobCancelTool::new(Arc::clone(&job_manager)),
            job_manager,
        }
    }

    /// Create a new ShellToolSet with an event channel
    ///
    /// This method creates an mpsc channel and wires the sender to the JobManager.
    /// Returns the ShellToolSet and the receiver for shell job completion events.
    ///
    /// Use this when you need to receive background job completion events.
    pub fn with_event_channel(config: ShellConfig) -> (Self, mpsc::Receiver<Value>) {
        let (tx, rx) = mpsc::channel(32);
        let job_manager = Arc::new(JobManager::new(config.clone()).with_event_sender(tx));

        let tool_set = Self {
            shell: ShellTool::with_job_manager(config, Arc::clone(&job_manager)),
            job_status: ShellJobStatusTool::new(Arc::clone(&job_manager)),
            jobs_list: ShellJobsListTool::new(Arc::clone(&job_manager)),
            job_cancel: ShellJobCancelTool::new(Arc::clone(&job_manager)),
            job_manager,
        };

        (tool_set, rx)
    }

    /// Get references to all tools as a vector
    ///
    /// Useful for registering all shell tools with a dispatcher.
    pub fn tools(&self) -> Vec<&dyn BuiltinTool> {
        vec![
            &self.shell as &dyn BuiltinTool,
            &self.job_status as &dyn BuiltinTool,
            &self.jobs_list as &dyn BuiltinTool,
            &self.job_cancel as &dyn BuiltinTool,
        ]
    }

    /// Get usage instructions for the LLM on how to properly use shell tools
    ///
    /// These instructions should be injected into the system prompt when
    /// shell tools are enabled.
    pub fn usage_instructions() -> &'static str {
        r#"# Shell Tools

You have access to tools for executing shell commands. Use these carefully and responsibly.

## Available Tools
- `shell` - Execute a shell command (supports background execution)
- `shell_job_status` - Check the status of a background job
- `shell_jobs` - List all background jobs
- `shell_job_cancel` - Cancel a running background job

## Best Practices

### Command Execution
- Prefer simple, well-understood commands over complex pipelines
- Always quote file paths that may contain spaces
- Use absolute paths when possible to avoid ambiguity
- Check command exit codes in the response to verify success

### Background Jobs
- Use `background: true` for long-running commands (builds, tests, downloads)
- Background jobs continue running while you do other work
- Check `shell_job_status` to get results when done
- Don't poll job status too frequently - wait at least 5-10 seconds between checks

### Security Considerations
- Never execute commands from untrusted sources without validation
- Be cautious with commands that modify or delete files
- Avoid running commands with elevated privileges unless absolutely necessary
- Don't expose sensitive data (API keys, passwords) in command arguments

### Error Handling
- Read stderr output when commands fail to understand the error
- If a command fails, try alternative approaches before giving up
- Report command failures clearly to the user

### Common Patterns
- Use `ls`, `find`, `grep` to explore the filesystem
- Use `cat`, `head`, `tail` to read file contents
- Use `git` commands for version control operations
- Use package managers (`npm`, `pip`, `cargo`) for dependencies"#
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ==================== ShellToolSet Struct Tests ====================

    #[test]
    fn test_shell_tool_set_struct() {
        let config = ShellConfig::default();
        let tool_set = ShellToolSet::new(config);

        // Verify all tools exist
        assert_eq!(tool_set.shell.name(), "shell");
        assert_eq!(tool_set.job_status.name(), "shell_job_status");
        assert_eq!(tool_set.jobs_list.name(), "shell_jobs");
        assert_eq!(tool_set.job_cancel.name(), "shell_job_cancel");

        // Job manager should exist
        assert!(Arc::strong_count(&tool_set.job_manager) >= 1);
    }

    // ==================== ShellToolSet::new Tests ====================

    #[test]
    fn test_shell_tool_set_new() {
        let config = ShellConfig {
            enabled: true,
            default_timeout_secs: 60,
            restrict_to_project: true,
            shell: "nu".to_string(),
            project_root: PathBuf::from("/tmp/test"),
            ..Default::default()
        };

        let tool_set = ShellToolSet::new(config);

        // Shell tool should have the config
        assert!(tool_set.shell.config.enabled);
        assert_eq!(tool_set.shell.config.default_timeout_secs, 60);
        assert_eq!(tool_set.shell.config.shell, "nu");

        // Job manager should be shared (4 references: shell_tool_set itself, job_status, jobs_list, job_cancel)
        // Actually: 1 for ShellToolSet.job_manager, 1 for job_status, 1 for jobs_list, 1 for job_cancel = 4
        assert!(Arc::strong_count(&tool_set.job_manager) >= 4);
    }

    #[test]
    fn test_shell_tool_set_shared_job_manager() {
        let config = ShellConfig::default();
        let tool_set = ShellToolSet::new(config);

        // All job tools should share the same JobManager
        // We can verify this by checking the Arc reference count
        // 5 references: tool_set.job_manager + shell + job_status + jobs_list + job_cancel
        assert_eq!(Arc::strong_count(&tool_set.job_manager), 5);
    }

    // ==================== ShellToolSet::tools Tests ====================

    #[test]
    fn test_shell_tool_set_tools() {
        let config = ShellConfig::default();
        let tool_set = ShellToolSet::new(config);

        let tools = tool_set.tools();

        // Should have all 4 tools
        assert_eq!(tools.len(), 4);

        // Verify tool names
        let names: Vec<_> = tools.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"shell"));
        assert!(names.contains(&"shell_job_status"));
        assert!(names.contains(&"shell_jobs"));
        assert!(names.contains(&"shell_job_cancel"));
    }

    #[test]
    fn test_shell_tool_set_tools_all_implement_builtin() {
        let config = ShellConfig::default();
        let tool_set = ShellToolSet::new(config);

        let tools = tool_set.tools();

        // All tools should implement BuiltinTool (verified by returning &dyn BuiltinTool)
        for tool in tools {
            // Should be able to call trait methods
            let _name = tool.name();
            let _def = tool.def();
            let _enabled = tool.default_enabled();
        }
    }

    #[test]
    fn test_shell_tool_set_tools_order() {
        let config = ShellConfig::default();
        let tool_set = ShellToolSet::new(config);

        let tools = tool_set.tools();

        // Order should be: shell, job_status, jobs_list, job_cancel
        assert_eq!(tools[0].name(), "shell");
        assert_eq!(tools[1].name(), "shell_job_status");
        assert_eq!(tools[2].name(), "shell_jobs");
        assert_eq!(tools[3].name(), "shell_job_cancel");
    }

    // ==================== ShellToolSet::with_event_channel Tests ====================

    #[test]
    fn test_shell_tool_set_with_event_channel() {
        let config = ShellConfig::default();
        let (tool_set, _rx) = ShellToolSet::with_event_channel(config);

        // Should have all tools
        assert_eq!(tool_set.tools().len(), 4);

        // Job manager should have event sender configured
        // (this is verified by the fact that JobManager was constructed with with_event_sender)
    }

    #[tokio::test]
    #[cfg(feature = "integration-real-tests")]
    #[ignore = "integration-real: spawns shell processes"]
    async fn integration_real_shell_tool_set_event_channel_receives_events() {
        use std::time::Duration;

        let temp_dir = tempfile::TempDir::new().unwrap();
        let mut config = ShellConfig::with_project_root(temp_dir.path().to_path_buf());
        config.shell = "sh".to_string(); // Use sh for portability

        let (tool_set, mut rx) = ShellToolSet::with_event_channel(config);

        // Spawn a quick job
        let _job_id = tool_set
            .job_manager
            .spawn_job("echo done", None, 30)
            .await
            .unwrap();

        // Wait for event
        let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("Should receive event within timeout")
            .expect("Channel should not be closed");

        // Verify event structure
        assert_eq!(event["type"], "shell_job_completed");
    }
}
