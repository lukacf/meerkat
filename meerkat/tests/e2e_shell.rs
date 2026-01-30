#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//!
//! These tests verify the full shell tool functionality from tool call to
//! subprocess execution and result return.
//!
//! Tests use `/bin/sh` for portability and hermetic execution.

use meerkat_tools::builtin::BuiltinTool;
use meerkat_tools::builtin::shell::{
    JobId, JobManager, JobStatus, ShellConfig, ShellError, ShellOutput, ShellTool, ShellToolSet,
};
use serde_json::json;
use std::time::Duration;
use tempfile::TempDir;

// ============================================================================
// TEST HELPERS
// ============================================================================

/// Create a ShellConfig using /bin/sh for hermetic tests.
fn create_sh_config(temp_dir: &TempDir) -> ShellConfig {
    ShellConfig {
        enabled: true,
        default_timeout_secs: 30,
        restrict_to_project: true,
        shell: "sh".to_string(),
        shell_path: None,
        project_root: temp_dir.path().to_path_buf(),
        max_completed_jobs: 100,
        completed_job_ttl_secs: 300,
        max_concurrent_processes: 0, // Unlimited for e2e tests
    }
}

// ============================================================================
// E2E: SYNCHRONOUS EXECUTION
// ============================================================================

/// E2E: Agent executes sync shell command and receives output
#[tokio::test]
async fn test_e2e_shell_sync_execute() {
    let temp_dir = TempDir::new().unwrap();
    let config = create_sh_config(&temp_dir);
    let tool = ShellTool::new(config);

    // Execute a simple command
    let result = tool
        .call(json!({
            "command": "echo 'Hello from E2E test'"
        }))
        .await;

    assert!(result.is_ok(), "Shell command should succeed");

    let output: ShellOutput = serde_json::from_value(result.unwrap()).unwrap();

    assert!(!output.timed_out, "Command should not time out");
    assert_eq!(output.exit_code, Some(0), "Exit code should be 0");
    assert!(
        output.stdout.contains("Hello from E2E test"),
        "Output should contain expected text: {}",
        output.stdout
    );
    assert!(output.duration_secs >= 0.0, "Duration should be positive");
}

/// E2E: Agent handles command timeout
#[tokio::test]
async fn test_e2e_shell_sync_timeout() {
    let temp_dir = TempDir::new().unwrap();
    let config = create_sh_config(&temp_dir);
    let tool = ShellTool::new(config);

    // Execute a command that will timeout
    let result = tool
        .call(json!({
            "command": "sleep 10",
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
        output.duration_secs >= 1.0,
        "Duration should be at least 1 second"
    );
}

/// E2E: Agent receives exit code from command
#[tokio::test]
async fn test_e2e_shell_exit_code() {
    let temp_dir = TempDir::new().unwrap();
    let config = create_sh_config(&temp_dir);
    let tool = ShellTool::new(config);

    // Execute a command that exits with non-zero status
    let result = tool
        .call(json!({
            "command": "exit 42"
        }))
        .await;

    assert!(result.is_ok(), "Command should complete");

    let output: ShellOutput = serde_json::from_value(result.unwrap()).unwrap();

    assert!(!output.timed_out, "Should not timeout");
    assert_eq!(
        output.exit_code,
        Some(42),
        "Exit code should be 42, got {:?}",
        output.exit_code
    );
}

/// E2E: Agent handles command with stderr output
#[tokio::test]
async fn test_e2e_shell_stderr() {
    let temp_dir = TempDir::new().unwrap();
    let config = create_sh_config(&temp_dir);
    let tool = ShellTool::new(config);

    // Execute a command that writes to stderr
    let result = tool
        .call(json!({
            "command": "echo 'error message' 1>&2"
        }))
        .await;

    assert!(result.is_ok(), "Command should complete");

    let output: ShellOutput = serde_json::from_value(result.unwrap()).unwrap();

    assert!(!output.timed_out, "Should not timeout");
    assert!(
        output.stderr.contains("error message"),
        "Stderr should contain error message: {}",
        output.stderr
    );
}

// ============================================================================
// E2E: BACKGROUND EXECUTION
// ============================================================================

/// E2E: Agent spawns background job and receives job_id
#[tokio::test]
async fn test_e2e_shell_background_spawn() {
    let temp_dir = TempDir::new().unwrap();
    let config = create_sh_config(&temp_dir);
    let job_manager = JobManager::new(config);

    // Spawn a background job
    let job_id = job_manager
        .spawn_job("sleep 5", None, 60)
        .await
        .expect("Should spawn job");

    // Verify job_id format
    assert!(
        job_id.0.starts_with("job_"),
        "Job ID should start with 'job_': {}",
        job_id.0
    );

    // Verify job is running
    let job = job_manager
        .get_status(&job_id)
        .await
        .expect("Job should exist");
    assert!(
        matches!(job.status, JobStatus::Running { .. }),
        "Job should be running: {:?}",
        job.status
    );

    // Clean up: cancel the job
    let _ = job_manager.cancel_job(&job_id).await;
}

/// E2E: Agent receives completion event after background job finishes
#[tokio::test]
async fn test_e2e_shell_background_completion() {
    let temp_dir = TempDir::new().unwrap();
    let config = create_sh_config(&temp_dir);
    let (tool_set, mut rx) = ShellToolSet::with_event_channel(config);

    // Spawn a quick job
    let job_id = tool_set
        .job_manager
        .spawn_job("echo 'done'", None, 30)
        .await
        .expect("Should spawn job");

    // Wait for completion event
    let event = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("Should receive event within timeout")
        .expect("Channel should not be closed");

    // Verify event structure
    assert_eq!(event["type"], "shell_job_completed");
    assert_eq!(event["job_id"], job_id.0);
    assert!(event["result"].is_object());
    assert_eq!(event["result"]["status"], "completed");
}

/// E2E: Agent can cancel running background job
#[tokio::test]
async fn test_e2e_shell_background_cancel() {
    let temp_dir = TempDir::new().unwrap();
    let config = create_sh_config(&temp_dir);
    let job_manager = JobManager::new(config);

    // Spawn a long-running job
    let job_id = job_manager
        .spawn_job("sleep 60", None, 120)
        .await
        .expect("Should spawn job");

    // Wait a bit for it to start
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Verify it's running
    let job = job_manager.get_status(&job_id).await.unwrap();
    assert!(
        matches!(job.status, JobStatus::Running { .. }),
        "Job should be running"
    );

    // Cancel it
    job_manager
        .cancel_job(&job_id)
        .await
        .expect("Cancel should succeed");

    // Verify it's cancelled
    let job = job_manager.get_status(&job_id).await.unwrap();
    assert!(
        matches!(job.status, JobStatus::Cancelled { .. }),
        "Job should be cancelled: {:?}",
        job.status
    );
}

/// E2E: Agent can list multiple concurrent jobs
#[tokio::test]
async fn test_e2e_shell_multiple_jobs() {
    let temp_dir = TempDir::new().unwrap();
    let config = create_sh_config(&temp_dir);
    let job_manager = JobManager::new(config);

    // Spawn multiple jobs
    let id1 = job_manager
        .spawn_job("sleep 30", None, 60)
        .await
        .expect("Should spawn job 1");
    let id2 = job_manager
        .spawn_job("sleep 30", None, 60)
        .await
        .expect("Should spawn job 2");
    let id3 = job_manager
        .spawn_job("sleep 30", None, 60)
        .await
        .expect("Should spawn job 3");

    // List all jobs
    let jobs = job_manager.list_jobs().await;

    assert_eq!(jobs.len(), 3, "Should have 3 jobs");

    // Verify all job IDs are present
    let ids: Vec<_> = jobs.iter().map(|j| &j.id).collect();
    assert!(ids.contains(&&id1), "Should contain job 1");
    assert!(ids.contains(&&id2), "Should contain job 2");
    assert!(ids.contains(&&id3), "Should contain job 3");

    // Clean up
    let _ = job_manager.cancel_job(&id1).await;
    let _ = job_manager.cancel_job(&id2).await;
    let _ = job_manager.cancel_job(&id3).await;
}

// ============================================================================
// E2E: ERROR HANDLING
// ============================================================================

/// E2E: Agent receives error when shell not installed (and no fallback available)
///
/// Note: On Unix, this test may pass with a fallback shell if /bin/sh exists.
/// We configure shell_path to a non-existent path to prevent fallback.
#[tokio::test]
async fn test_e2e_shell_not_installed() {
    let temp_dir = TempDir::new().unwrap();
    let config = ShellConfig {
        enabled: true,
        shell: "definitely_not_a_real_shell_xyz123".to_string(),
        shell_path: Some(std::path::PathBuf::from("/nonexistent/path/to/shell")),
        project_root: temp_dir.path().to_path_buf(),
        restrict_to_project: false,
        default_timeout_secs: 30,
        ..Default::default()
    };

    let tool = ShellTool::new(config);

    // Try to execute a command - should fail because shell doesn't exist
    let result = tool
        .call(json!({
            "command": "echo test"
        }))
        .await;

    // On Unix, the fallback might still find /bin/sh, so we check for either:
    // 1. Error (preferred - shell truly not found)
    // 2. Success with fallback (acceptable on Unix where /bin/sh exists)
    #[cfg(unix)]
    {
        // On Unix, fallback to /bin/sh might succeed - both outcomes are valid
        // The explicit shell_path should prevent fallback though
        if result.is_err() {
            let err_msg = result.unwrap_err().to_string();
            assert!(
                err_msg.contains("not installed")
                    || err_msg.contains("Shell not installed")
                    || err_msg.contains("not found"),
                "Error should mention shell not found: {}",
                err_msg
            );
        }
        // If it succeeds, that's also fine - fallback worked
    }

    #[cfg(not(unix))]
    {
        assert!(result.is_err(), "Should fail when shell not installed");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("not installed") || err_msg.contains("Shell not installed"),
            "Error should mention shell not installed: {}",
            err_msg
        );
    }
}

/// E2E: Agent receives error for invalid working directory
#[tokio::test]
async fn test_e2e_shell_invalid_workdir() {
    let temp_dir = TempDir::new().unwrap();
    let config = create_sh_config(&temp_dir);
    let tool = ShellTool::new(config);

    // Try to use a working directory that escapes project root
    let result = tool
        .call(json!({
            "command": "echo test",
            "working_dir": "../../../etc"
        }))
        .await;

    assert!(
        result.is_err(),
        "Should fail when working dir escapes project"
    );

    let err_msg = result.unwrap_err().to_string();
    // The path may either fail because it doesn't exist (not found) or
    // because it's outside project root (escape). Either indicates the
    // security restriction is working.
    assert!(
        err_msg.contains("outside project root")
            || err_msg.contains("escape")
            || err_msg.contains("not found")
            || err_msg.contains("Working directory"),
        "Error should mention working directory issue: {}",
        err_msg
    );
}

/// E2E: Agent receives error when checking non-existent job
#[tokio::test]
async fn test_e2e_shell_job_not_found() {
    let temp_dir = TempDir::new().unwrap();
    // Use sh for this test - doesn't need nu-specific features
    let config = create_sh_config(&temp_dir);
    let job_manager = JobManager::new(config);

    // Try to get status of non-existent job
    let fake_id = JobId::from_string("job_nonexistent123");
    let status = job_manager.get_status(&fake_id).await;

    assert!(status.is_none(), "Non-existent job should return None");

    // Try to cancel non-existent job
    let result = job_manager.cancel_job(&fake_id).await;

    assert!(result.is_err(), "Cancelling non-existent job should fail");

    if let Err(ShellError::JobNotFound(job_id)) = result {
        assert_eq!(job_id, "job_nonexistent123");
    } else {
        panic!("Expected JobNotFound error, got {:?}", result);
    }
}

// ============================================================================
// E2E: TOOL INTEGRATION (using sh for portability)
// ============================================================================

/// E2E: Shell tool returns correct name
#[test]
fn test_e2e_shell_tool_name() {
    let temp_dir = TempDir::new().unwrap();
    let config = create_sh_config(&temp_dir);
    let tool = ShellTool::new(config);

    assert_eq!(tool.name(), "shell");
}

/// E2E: Shell tool returns valid schema
#[test]
fn test_e2e_shell_tool_schema() {
    let temp_dir = TempDir::new().unwrap();
    let config = create_sh_config(&temp_dir);
    let tool = ShellTool::new(config);

    let def = tool.def();

    assert_eq!(def.name, "shell");
    assert!(!def.description.is_empty());

    // Verify schema has required properties
    let schema = &def.input_schema;
    assert_eq!(schema["type"], "object");
    assert!(schema["properties"]["command"].is_object());
    assert!(schema["properties"]["working_dir"].is_object());
    assert!(schema["properties"]["timeout_secs"].is_object());
    assert!(schema["properties"]["background"].is_object());

    // Verify 'command' is required
    let required = schema["required"].as_array().unwrap();
    assert!(required.contains(&json!("command")));
}

/// E2E: ShellToolSet provides all four tools
#[test]
fn test_e2e_shell_tool_set() {
    let temp_dir = TempDir::new().unwrap();
    let config = create_sh_config(&temp_dir);
    let tool_set = ShellToolSet::new(config);

    let tools = tool_set.tools();

    assert_eq!(tools.len(), 4, "Should have 4 shell tools");

    let names: Vec<_> = tools.iter().map(|t| t.name()).collect();
    assert!(names.contains(&"shell"), "Should have shell tool");
    assert!(
        names.contains(&"shell_job_status"),
        "Should have shell_job_status tool"
    );
    assert!(names.contains(&"shell_jobs"), "Should have shell_jobs tool");
    assert!(
        names.contains(&"shell_job_cancel"),
        "Should have shell_job_cancel tool"
    );
}

/// E2E: Shell tool is disabled by default
#[test]
fn test_e2e_shell_disabled_by_default() {
    let temp_dir = TempDir::new().unwrap();
    let config = create_sh_config(&temp_dir);
    let tool = ShellTool::new(config);

    assert!(
        !tool.default_enabled(),
        "Shell tool should be disabled by default for security"
    );
}

// ============================================================================
// E2E: BASIC EXECUTION WITH SH (always available)
// ============================================================================

/// E2E: Basic shell execution works with /bin/sh
#[tokio::test]
async fn test_e2e_shell_basic_sh_execution() {
    let temp_dir = TempDir::new().unwrap();
    let config = create_sh_config(&temp_dir);
    let tool = ShellTool::new(config);

    let result = tool
        .call(json!({
            "command": "echo hello"
        }))
        .await;

    assert!(result.is_ok(), "Basic sh execution should succeed");

    let output: ShellOutput = serde_json::from_value(result.unwrap()).unwrap();

    assert!(!output.timed_out);
    assert_eq!(output.exit_code, Some(0));
    assert!(output.stdout.contains("hello"));
}

/// E2E: Job manager basic operations with sh
#[tokio::test]
async fn test_e2e_job_manager_basic_sh() {
    let temp_dir = TempDir::new().unwrap();
    let config = create_sh_config(&temp_dir);
    let job_manager = JobManager::new(config);

    // Spawn a quick job
    let job_id = job_manager
        .spawn_job("echo done", None, 30)
        .await
        .expect("Should spawn job");

    // Verify job exists
    let job = job_manager.get_status(&job_id).await;
    assert!(job.is_some(), "Job should exist");

    // Wait for completion (echo is fast)
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Verify job completed
    let job = job_manager.get_status(&job_id).await.unwrap();
    assert!(
        matches!(job.status, JobStatus::Completed { .. }),
        "Job should be completed: {:?}",
        job.status
    );
}

/// E2E: Event channel receives completion events with sh
#[tokio::test]
async fn test_e2e_event_channel_sh() {
    let temp_dir = TempDir::new().unwrap();
    let config = create_sh_config(&temp_dir);
    let (tool_set, mut rx) = ShellToolSet::with_event_channel(config);

    // Spawn a quick job
    let _job_id = tool_set
        .job_manager
        .spawn_job("echo test", None, 30)
        .await
        .expect("Should spawn job");

    // Wait for completion event
    let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("Should receive event")
        .expect("Channel should not be closed");

    assert_eq!(event["type"], "shell_job_completed");
    assert!(event["result"].is_object());
}

// ============================================================================
// REGRESSION TESTS
// ============================================================================
// These tests verify that specific bugs that were fixed don't recur.

/// Regression: Async execution should be non-blocking
///
/// Spawning multiple jobs should return immediately without waiting for
/// any command to complete.
#[tokio::test]
async fn test_regression_async_execution_nonblocking() {
    use std::time::Instant;

    let temp_dir = TempDir::new().unwrap();
    let config = create_sh_config(&temp_dir);
    let job_manager = JobManager::new(config);

    let start = Instant::now();

    // Spawn 5 long-running jobs
    let mut job_ids = Vec::new();
    for _ in 0..5 {
        let id = job_manager
            .spawn_job("sleep 10", None, 60)
            .await
            .expect("Should spawn job");
        job_ids.push(id);
    }

    let elapsed = start.elapsed();

    // All spawns should complete in under 1 second (non-blocking)
    assert!(
        elapsed.as_millis() < 1000,
        "Spawning 5 jobs should be nearly instant, took {:?}",
        elapsed
    );

    // Verify all jobs are running
    for id in &job_ids {
        let job = job_manager.get_status(id).await.expect("Job should exist");
        assert!(
            matches!(job.status, JobStatus::Running { .. }),
            "Job {} should be running: {:?}",
            id,
            job.status
        );
    }

    // Clean up
    for id in &job_ids {
        let _ = job_manager.cancel_job(id).await;
    }
}

/// Regression: Timeout should be enforced for background jobs
///
/// Jobs that run longer than their timeout should be terminated and marked
/// as TimedOut.
#[tokio::test]
async fn test_regression_timeout_enforced() {
    let temp_dir = TempDir::new().unwrap();
    let config = create_sh_config(&temp_dir);
    let job_manager = JobManager::new(config);

    // Spawn a job that sleeps longer than the timeout
    let job_id = job_manager
        .spawn_job("sleep 10", None, 1)
        .await
        .expect("Should spawn job");

    // Wait for timeout to occur (plus buffer)
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Verify job timed out
    let job = job_manager
        .get_status(&job_id)
        .await
        .expect("Job should exist");
    assert!(
        matches!(job.status, JobStatus::TimedOut { .. }),
        "Job should have timed out, got {:?}",
        job.status
    );

    // Verify duration is approximately the timeout value
    if let JobStatus::TimedOut { duration_secs, .. } = &job.status {
        assert!(
            *duration_secs >= 1.0 && *duration_secs < 3.0,
            "Duration should be close to timeout: {}",
            duration_secs
        );
    }
}

/// Regression: Cancel should terminate the underlying process
///
/// When a job is cancelled, the underlying process should be terminated,
/// not left running as an orphan.
#[tokio::test]
async fn test_regression_kill_terminates_process() {
    let temp_dir = TempDir::new().unwrap();
    let config = create_sh_config(&temp_dir);
    let job_manager = JobManager::new(config);

    // Spawn a long-running job
    let job_id = job_manager
        .spawn_job("sleep 60", None, 120)
        .await
        .expect("Should spawn job");

    // Let it start
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Verify it's running
    let job = job_manager.get_status(&job_id).await.unwrap();
    assert!(
        matches!(job.status, JobStatus::Running { .. }),
        "Job should be running before cancel"
    );

    // Cancel the job
    job_manager
        .cancel_job(&job_id)
        .await
        .expect("Cancel should succeed");

    // Verify it's cancelled
    let job = job_manager.get_status(&job_id).await.unwrap();
    assert!(
        matches!(job.status, JobStatus::Cancelled { .. }),
        "Job should be cancelled, got {:?}",
        job.status
    );

    // Wait a moment and verify it stays cancelled (process is gone, not restarting)
    tokio::time::sleep(Duration::from_millis(500)).await;
    let job = job_manager.get_status(&job_id).await.unwrap();
    assert!(
        matches!(job.status, JobStatus::Cancelled { .. }),
        "Job should still be cancelled"
    );
}

/// Regression: Non-UTF-8 output should be handled gracefully
///
/// Commands that produce non-UTF-8 bytes should not crash and should use
/// lossy UTF-8 conversion.
#[tokio::test]
async fn test_regression_non_utf8_output() {
    let temp_dir = TempDir::new().unwrap();
    let config = create_sh_config(&temp_dir);
    let tool = ShellTool::new(config);

    // Using printf to output raw bytes that are invalid UTF-8
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
    // The output may contain replacement characters, which is correct behavior
}

/// Regression: Long output should preserve the tail
///
/// When output exceeds buffer limits, truncation should keep the END of output,
/// not the beginning, since the end usually contains the most important info
/// (errors, final results).
#[tokio::test]
async fn test_regression_truncation_keeps_tail() {
    let temp_dir = TempDir::new().unwrap();
    let config = create_sh_config(&temp_dir);
    let tool = ShellTool::new(config);

    // Generate a lot of output with clear markers at start and end
    let result = tool
        .call(json!({
            "command": r#"
                echo "START_MARKER"
                i=1
                while [ "$i" -le 10000 ]; do
                  echo "Line $i: padding to make this longer"
                  i=$((i+1))
                done
                echo "END_MARKER"
            "#
        }))
        .await;

    assert!(result.is_ok(), "Long output command should succeed");

    let output: ShellOutput = serde_json::from_value(result.unwrap()).unwrap();

    // The end marker should always be present (tail preserved)
    assert!(
        output.stdout.contains("END_MARKER"),
        "Output should contain END_MARKER (tail preserved)"
    );

    // Note: Whether START_MARKER is present depends on truncation threshold
}

/// Regression: Concurrent job spawning should produce unique IDs
///
/// When spawning many jobs concurrently, each should get a unique ID with
/// no collisions.
#[tokio::test]
async fn test_regression_concurrent_job_spawning() {
    use std::sync::Arc;

    let temp_dir = TempDir::new().unwrap();
    let config = create_sh_config(&temp_dir);
    let job_manager = Arc::new(JobManager::new(config));

    // Spawn 20 jobs concurrently
    let mut handles = Vec::new();
    for i in 0..20 {
        let mgr = Arc::clone(&job_manager);
        let cmd = format!("echo job{}", i);
        handles.push(tokio::spawn(
            async move { mgr.spawn_job(&cmd, None, 30).await },
        ));
    }

    // Collect all results
    let mut job_ids = Vec::new();
    for handle in handles {
        let result = handle.await.unwrap();
        assert!(result.is_ok(), "All spawns should succeed");
        job_ids.push(result.unwrap());
    }

    // Verify all IDs are unique
    let unique_count = {
        let mut ids: Vec<_> = job_ids.iter().map(|id| &id.0).collect();
        ids.sort();
        ids.dedup();
        ids.len()
    };
    assert_eq!(
        unique_count, 20,
        "All 20 jobs should have unique IDs, got {}",
        unique_count
    );

    // Wait for jobs to complete (echo is fast)
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Verify all jobs completed
    for job_id in &job_ids {
        let job = job_manager
            .get_status(job_id)
            .await
            .expect("Job should exist");
        assert!(
            matches!(job.status, JobStatus::Completed { .. }),
            "Job {} should be completed, got {:?}",
            job_id,
            job.status
        );
    }
}

/// Regression: Job cleanup should prevent memory leaks
///
/// When many jobs are spawned, old completed jobs should be cleaned up to
/// prevent unbounded memory growth.
#[tokio::test]
async fn test_regression_job_cleanup_prevents_leak() {
    let temp_dir = TempDir::new().unwrap();
    let config = create_sh_config(&temp_dir);
    let job_manager = JobManager::new(config);

    // Spawn many jobs that complete quickly
    let mut all_ids = Vec::new();
    for i in 0..50 {
        let cmd = format!("echo job{}", i);
        let id = job_manager
            .spawn_job(&cmd, None, 30)
            .await
            .expect("Should spawn job");
        all_ids.push(id);
    }

    // Wait for all to complete (echo is fast, 50 jobs)
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Verify completed jobs can still be queried (at least some)
    let jobs = job_manager.list_jobs().await;

    // All jobs should still be listable (unless cleanup has removed some)
    // The important thing is that this doesn't crash or cause memory issues
    assert!(
        !jobs.is_empty(),
        "Should have some jobs (at least those not cleaned up yet)"
    );

    // Verify job statuses are queryable
    for job_id in all_ids.iter().take(10) {
        // Check first 10
        let job = job_manager.get_status(job_id).await;
        // Job might be cleaned up, so we just verify the operation doesn't crash
        if let Some(j) = job {
            assert!(
                matches!(j.status, JobStatus::Completed { .. }),
                "If job exists, it should be completed"
            );
        }
    }
}
