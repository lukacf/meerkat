//! Integration tests for shell job sharing between ShellTool and job control tools.
//!
//! These tests verify that background jobs spawned via the `shell` tool are visible
//! to the `shell_jobs`, `shell_job_status`, and `shell_job_cancel` tools.

#![cfg(feature = "integration-real-tests")]
#![allow(clippy::panic, clippy::unwrap_used)]

use meerkat_core::AgentToolDispatcher;
use meerkat_core::ToolCallView;
use meerkat_core::error::ToolError;
use meerkat_tools::builtin::shell::ShellConfig;
use meerkat_tools::builtin::{
    BuiltinToolConfig, CompositeDispatcher, MemoryTaskStore, ToolPolicyLayer,
};
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;

async fn dispatch_json(
    dispatcher: &dyn AgentToolDispatcher,
    name: &str,
    args: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let args_raw = serde_json::value::RawValue::from_string(args.to_string()).unwrap();
    let call = ToolCallView {
        id: "test-1",
        name,
        args: &args_raw,
    };
    let result = dispatcher.dispatch(call).await?;
    serde_json::from_str(&result.content).or(Ok(serde_json::Value::String(result.content)))
}

/// Create a CompositeDispatcher with shell tools enabled.
fn create_dispatcher_with_shell(
    temp_dir: &TempDir,
) -> Result<CompositeDispatcher, Box<dyn std::error::Error>> {
    let shell_config = ShellConfig {
        enabled: true,
        project_root: temp_dir.path().to_path_buf(),
        shell: "sh".to_string(), // Use sh for portability
        ..Default::default()
    };

    let config = BuiltinToolConfig {
        policy: ToolPolicyLayer::new()
            .enable_tool("shell")
            .enable_tool("shell_job_status")
            .enable_tool("shell_jobs")
            .enable_tool("shell_job_cancel"),
        ..Default::default()
    };

    let store = Arc::new(MemoryTaskStore::new());
    let dispatcher = CompositeDispatcher::new(store, &config, Some(shell_config), None, None)?;
    Ok(dispatcher)
}

/// P1 Bug Test: ShellTool and job control tools must share the same JobManager.
///
/// When a job is spawned via the `shell` tool with `background: true`, the returned
/// job_id must be visible via `shell_jobs` and queryable via `shell_job_status`.
///
/// This test will FAIL if ShellTool creates its own internal JobManager instead of
/// sharing the JobManager with job control tools.
#[tokio::test]
#[ignore = "integration-real: spawns shell processes"]
async fn integration_real_p1_shell_tool_shares_job_manager_with_job_control_tools()
-> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = TempDir::new()?;
    let dispatcher = create_dispatcher_with_shell(&temp_dir)?;

    // Step 1: Spawn a background job via the shell tool
    let spawn_result = dispatch_json(
        &dispatcher,
        "shell",
        json!({
            "command": "sleep 60",
            "background": true
        }),
    )
    .await?;

    // Extract the job_id from the response
    let job_id = spawn_result
        .get("job_id")
        .and_then(|v| v.as_str())
        .ok_or("shell tool should return job_id for background jobs")?;

    assert!(
        job_id.starts_with("job_"),
        "job_id should have job_ prefix, got: {job_id}"
    );

    // Step 2: Verify the job is visible via shell_jobs (list all jobs)
    let list_result = dispatch_json(&dispatcher, "shell_jobs", json!({})).await?;

    let jobs = list_result
        .as_array()
        .ok_or("shell_jobs should return an array")?;

    // This is the key assertion that will fail if JobManagers aren't shared
    assert!(
        !jobs.is_empty(),
        "BUG: shell_jobs returns empty list! ShellTool has its own JobManager \
         instead of sharing with job control tools. Jobs spawned via shell tool \
         are invisible to shell_jobs/shell_job_status/shell_job_cancel."
    );

    // Find our specific job
    let our_job = jobs
        .iter()
        .find(|j| j.get("id").and_then(|v| v.as_str()) == Some(job_id));

    assert!(
        our_job.is_some(),
        "Job {job_id} spawned via shell tool should be visible in shell_jobs list. \
         Jobs found: {jobs:?}"
    );

    // Verify job status is running
    let job = our_job.unwrap();
    assert_eq!(
        job.get("status").and_then(|v| v.as_str()),
        Some("running"),
        "Job should be running"
    );

    // Step 3: Verify the job is queryable via shell_job_status
    let status_result =
        dispatch_json(&dispatcher, "shell_job_status", json!({ "job_id": job_id })).await?;

    // If JobManagers aren't shared, this will return null/error
    assert!(
        !status_result.is_null(),
        "BUG: shell_job_status returns null for job {job_id}! \
         ShellTool uses a different JobManager than shell_job_status."
    );

    // Verify the status response has expected fields
    assert!(
        status_result.get("id").is_some(),
        "shell_job_status should return job info with id field"
    );
    assert!(
        status_result.get("status").is_some(),
        "shell_job_status should return job info with status field"
    );

    // Step 4: Verify the job can be cancelled via shell_job_cancel
    let cancel_result =
        dispatch_json(&dispatcher, "shell_job_cancel", json!({ "job_id": job_id })).await;

    assert!(
        cancel_result.is_ok(),
        "BUG: shell_job_cancel failed for job {}! Error: {:?}. \
         ShellTool uses a different JobManager than shell_job_cancel.",
        job_id,
        cancel_result.err()
    );

    // Verify job is now cancelled
    let final_status =
        dispatch_json(&dispatcher, "shell_job_status", json!({ "job_id": job_id })).await?;

    let status_obj = final_status.get("status");
    // JobStatus is serialized with serde tag="status", so it looks like:
    // {"duration_secs": 0.0, "status": "cancelled"}
    let is_cancelled = status_obj
        .is_some_and(|s| {
            // Check various serialization formats:
            // 1. Direct string: "cancelled"
            s.as_str() == Some("cancelled")
                // 2. Object with "Cancelled" key (enum variant)
                || s.get("Cancelled").is_some()
                // 3. Object with inner "status" field containing "cancelled" (serde tag format)
                || s.get("status").and_then(|v| v.as_str()) == Some("cancelled")
                // 4. Check first key contains "cancel"
                || s.as_object()
                    .and_then(|o| o.keys().next())
                    .is_some_and(|k| k.to_lowercase().contains("cancel"))
        });

    assert!(
        is_cancelled,
        "Job should be cancelled after shell_job_cancel. Got status: {final_status:?}"
    );

    Ok(())
}

/// Regression test: Multiple background jobs should all be tracked.
#[tokio::test]
#[ignore = "integration-real: spawns shell processes"]
async fn integration_real_multiple_background_jobs_tracked()
-> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = TempDir::new()?;
    let dispatcher = create_dispatcher_with_shell(&temp_dir)?;

    // Spawn 3 background jobs
    let mut job_ids = Vec::new();
    for i in 0..3 {
        let result = dispatch_json(
            &dispatcher,
            "shell",
            json!({
                "command": format!("sleep {}", 60 + i),
                "background": true
            }),
        )
        .await?;

        let job_id = result
            .get("job_id")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        job_ids.push(job_id);
    }

    // Verify all 3 jobs are visible
    let list_result = dispatch_json(&dispatcher, "shell_jobs", json!({})).await?;
    let jobs = list_result.as_array().unwrap();

    assert_eq!(
        jobs.len(),
        3,
        "All 3 background jobs should be visible. Got: {jobs:?}"
    );

    // Verify each job_id is in the list
    for job_id in &job_ids {
        let found = jobs
            .iter()
            .any(|j| j.get("id").and_then(|v| v.as_str()) == Some(job_id));
        assert!(found, "Job {job_id} should be in the list");
    }

    // Clean up: cancel all jobs
    for job_id in &job_ids {
        let _ = dispatch_json(&dispatcher, "shell_job_cancel", json!({ "job_id": job_id })).await;
    }

    Ok(())
}
