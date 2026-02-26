#![allow(clippy::panic, clippy::unwrap_used)]

use meerkat_core::AgentToolDispatcher;
use meerkat_core::error::ToolError;
use meerkat_core::types::{ToolCallView, ToolResult};
use meerkat_tools::builtin::{
    BuiltinToolConfig, CompositeDispatcher, MemoryTaskStore, ToolPolicyLayer,
};
use serde_json::json;
use std::path::Path;
use std::sync::Arc;

async fn dispatch_tool(
    dispatcher: &dyn AgentToolDispatcher,
    name: &str,
    args: serde_json::Value,
) -> Result<ToolResult, ToolError> {
    let args_raw = serde_json::value::RawValue::from_string(args.to_string()).unwrap();
    let call = ToolCallView {
        id: "test-1",
        name,
        args: &args_raw,
    };
    dispatcher.dispatch(call).await
}

#[tokio::test]
async fn test_rct_contracts_tool_dispatcher_contract() -> Result<(), Box<dyn std::error::Error>> {
    let store = Arc::new(MemoryTaskStore::new());
    let config = BuiltinToolConfig::default();

    let dispatcher = CompositeDispatcher::new(store, &config, None, None, None)?;

    let tools = dispatcher.tools();
    assert!(!tools.is_empty());
    Ok(())
}

#[test]
fn test_rct_contracts_agent_list_schema_contract() -> Result<(), Box<dyn std::error::Error>> {
    let config = BuiltinToolConfig {
        policy: ToolPolicyLayer::new().enable_tool("agent_list"),
        ..Default::default()
    };

    let encoded = serde_json::to_value(&config)?;
    let decoded: BuiltinToolConfig = serde_json::from_value(encoded)?;
    assert!(decoded.policy.enable.contains("agent_list"));
    Ok(())
}

#[tokio::test]
async fn test_rct_contracts_task_store_persistence_contract()
-> Result<(), Box<dyn std::error::Error>> {
    let store = Arc::new(MemoryTaskStore::new());
    let config = BuiltinToolConfig::default();
    let tool = CompositeDispatcher::new(store, &config, None, None, None)?;

    let result = dispatch_tool(
        &tool,
        "task_create",
        json!({"subject":"Test","description":"Persist"}),
    )
    .await?;
    let value: serde_json::Value =
        serde_json::from_str(&result.content).unwrap_or_else(|_| json!(result.content));
    assert!(value.get("id").is_some());
    Ok(())
}

#[tokio::test]
async fn test_rct_contracts_inv_004_task_tools_session_id() -> Result<(), Box<dyn std::error::Error>>
{
    let store = Arc::new(MemoryTaskStore::new());
    let config = BuiltinToolConfig::default();
    let tool =
        CompositeDispatcher::new(store, &config, None, None, Some("test-session-123".into()))?;

    let result = dispatch_tool(
        &tool,
        "task_create",
        json!({"subject":"Task","description":"Track session"}),
    )
    .await?;
    let value: serde_json::Value =
        serde_json::from_str(&result.content).unwrap_or_else(|_| json!(result.content));
    assert_eq!(
        value.get("created_by_session").and_then(|v| v.as_str()),
        Some("test-session-123")
    );
    Ok(())
}

#[test]
fn test_rct_contracts_all_builtin_schemas_have_required_field()
-> Result<(), Box<dyn std::error::Error>> {
    let store = Arc::new(MemoryTaskStore::new());
    let config = BuiltinToolConfig::default();
    let dispatcher = CompositeDispatcher::new(store, &config, None, None, None)?;

    for tool in dispatcher.tools().iter() {
        let schema = &tool.input_schema;
        assert_eq!(schema["type"], "object");
        let _props = schema.get("properties").ok_or("missing properties")?;
    }
    Ok(())
}

#[test]
fn test_rct_contracts_inv_007_builtin_task_persistence_strategy()
-> Result<(), Box<dyn std::error::Error>> {
    use meerkat_core::AgentToolDispatcher;
    // Contract: Builtin task storage strategy must be configurable
    let store = Arc::new(MemoryTaskStore::new());
    let config = BuiltinToolConfig::default();

    let dispatcher = CompositeDispatcher::new(store, &config, None, None, None)?;

    // Verify task_create is available as a tool
    let tools = dispatcher.tools();
    assert!(tools.iter().any(|t| t.name == "task_create"));
    Ok(())
}

#[test]
fn test_rct_contracts_shell_defaults_contract() -> Result<(), Box<dyn std::error::Error>> {
    use meerkat_tools::builtin::shell::ShellConfig;
    use std::path::PathBuf;

    let tool = ShellConfig {
        enabled: true,
        default_timeout_secs: 30,
        restrict_to_project: true,
        shell: "nu".into(),
        shell_path: None,
        project_root: PathBuf::from("/tmp"),
        max_completed_jobs: 10,
        completed_job_ttl_secs: 60,
        max_concurrent_processes: 5,
        security_mode: meerkat_core::SecurityMode::Unrestricted,
        security_patterns: vec![],
    };
    let json_str = serde_json::to_string(&tool)?;
    let json_val: serde_json::Value = serde_json::from_str(&json_str)?;
    assert_eq!(json_val["shell"], "nu");
    Ok(())
}

#[test]
fn test_rct_contracts_no_manual_tool_schema_literals() -> Result<(), Box<dyn std::error::Error>> {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = crate_root
        .parent()
        .ok_or("missing workspace root")?
        .to_path_buf();

    let mut offenders = Vec::new();
    scan_for_manual_input_schema_literals(&workspace_root, &workspace_root, &mut offenders)?;

    assert!(
        offenders.is_empty(),
        "Manual tool schema literals found:\n{}",
        offenders.join("\n")
    );
    Ok(())
}

fn scan_for_manual_input_schema_literals(
    root: &Path,
    dir: &Path,
    offenders: &mut Vec<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();

        if path.is_dir() {
            match file_name.as_ref() {
                ".git" | "target" | "tests" => continue,
                _ => {}
            }

            scan_for_manual_input_schema_literals(root, &path, offenders)?;
            continue;
        }

        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }

        let contents = std::fs::read_to_string(&path)?;
        let mut in_test_module = false;
        let mut test_brace_depth = 0;

        for (idx, line) in contents.lines().enumerate() {
            // Track entry into #[cfg(test)] modules
            if line.contains("#[cfg(test)]") {
                in_test_module = true;
                test_brace_depth = 0;
            }

            // Track brace depth when inside test module
            if in_test_module {
                test_brace_depth += line.chars().filter(|&c| c == '{').count();
                test_brace_depth =
                    test_brace_depth.saturating_sub(line.chars().filter(|&c| c == '}').count());
                if test_brace_depth == 0 && line.contains('}') {
                    in_test_module = false;
                }
                continue; // Skip test code
            }

            if contains_manual_input_schema_literal(line) {
                let rel = path.strip_prefix(root).unwrap_or(&path);
                offenders.push(format!("{}:{}: {}", rel.display(), idx + 1, line.trim()));
            }
        }
    }

    Ok(())
}

fn contains_manual_input_schema_literal(line: &str) -> bool {
    for needle in [
        "input_schema: serde_json::json!(",
        "input_schema: json!(",
        "input_schema:serde_json::json!(",
        "input_schema:json!(",
    ] {
        let Some(pos) = line.find(needle) else {
            continue;
        };

        // Don't trip over our own pattern strings.
        if line[..pos].ends_with('"') {
            continue;
        }

        return true;
    }

    false
}

// =============================================================================
// Regression tests for PR review findings
// =============================================================================

/// Regression test: Shell job tools must use correct names in allowlist.
/// The tool names are shell_job_status, shell_jobs, shell_job_cancel (not job_*).
#[tokio::test]
async fn test_regression_shell_job_tool_names() -> Result<(), Box<dyn std::error::Error>> {
    use meerkat_tools::builtin::shell::ShellConfig;
    use tempfile::TempDir;

    let temp_dir = TempDir::new()?;
    let shell_config = ShellConfig {
        enabled: true,
        project_root: temp_dir.path().to_path_buf(),
        ..Default::default()
    };

    // Enable shell tools via allowlist with CORRECT names
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

    let tools = dispatcher.tools();
    let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();

    // Verify all shell tools are exposed with correct names
    assert!(tool_names.contains(&"shell"), "shell tool missing");
    assert!(
        tool_names.contains(&"shell_job_status"),
        "shell_job_status missing (was 'job_status' exposed instead?)"
    );
    assert!(
        tool_names.contains(&"shell_jobs"),
        "shell_jobs missing (was 'jobs_list' exposed instead?)"
    );
    assert!(
        tool_names.contains(&"shell_job_cancel"),
        "shell_job_cancel missing (was 'job_cancel' exposed instead?)"
    );

    // Verify OLD incorrect names are NOT present
    assert!(
        !tool_names.contains(&"job_status"),
        "job_status should not exist - use shell_job_status"
    );
    assert!(
        !tool_names.contains(&"jobs_list"),
        "jobs_list should not exist - use shell_jobs"
    );
    assert!(
        !tool_names.contains(&"job_cancel"),
        "job_cancel should not exist - use shell_job_cancel"
    );

    Ok(())
}

/// Regression test: ToolDispatcherBuilder must populate registry from router tools.
/// Without this, dispatch_call validation always fails with NotFound.
#[tokio::test]
async fn test_regression_builder_populates_registry() -> Result<(), Box<dyn std::error::Error>> {
    use meerkat_tools::builder::{
        BuiltinDispatcherConfig, ToolDispatcherBuilder, ToolDispatcherConfig, ToolDispatcherSource,
    };
    use std::time::Duration;

    let store = Arc::new(MemoryTaskStore::new());
    let config = BuiltinToolConfig::default();

    let dispatcher_config = ToolDispatcherConfig {
        source: ToolDispatcherSource::Composite(Box::new(BuiltinDispatcherConfig {
            store,
            config,
            shell_config: None,
            external: None,
            session_id: None,
        })),
        comms: None,
        default_timeout: Duration::from_secs(30),
    };

    let dispatcher = ToolDispatcherBuilder::new(dispatcher_config)
        .build()
        .await?;

    // The dispatcher should have tools from the router
    let tools = dispatcher.tools();
    assert!(!tools.is_empty(), "Registry should have tools from router");
    assert!(
        tools.iter().any(|t| t.name == "task_list"),
        "task_list should be registered"
    );

    // dispatch_call should succeed for a valid tool (not NotFound)
    let args_raw = serde_json::value::RawValue::from_string(json!({}).to_string()).unwrap();
    let call = ToolCallView {
        id: "test-1",
        name: "task_list",
        args: &args_raw,
    };

    let result = dispatcher.dispatch_call(call).await;
    assert!(
        result.is_ok(),
        "dispatch_call should succeed, not return NotFound: {result:?}"
    );

    Ok(())
}

/// Regression test: ToolDispatcher::dispatch must enforce timeout.
/// Without timeout, a hanging tool blocks the agent loop indefinitely.
#[tokio::test]
async fn test_regression_dispatcher_timeout_enforced() -> Result<(), Box<dyn std::error::Error>> {
    use async_trait::async_trait;
    use meerkat_core::types::ToolDef;
    use meerkat_tools::dispatcher::ToolDispatcher;
    use meerkat_tools::registry::ToolRegistry;
    use meerkat_tools::schema::empty_object_schema;
    use std::time::Duration;

    /// A tool dispatcher that hangs forever
    struct HangingDispatcher;

    #[async_trait]
    impl AgentToolDispatcher for HangingDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::from([Arc::new(ToolDef {
                name: "hang".to_string(),
                description: "Hangs forever".to_string(),
                input_schema: empty_object_schema(),
            })])
        }

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
            let _ = call;
            // Hang forever
            std::future::pending().await
        }
    }

    let router = Arc::new(HangingDispatcher);
    let mut registry = ToolRegistry::new();
    registry.register(ToolDef {
        name: "hang".to_string(),
        description: "Hangs forever".to_string(),
        input_schema: empty_object_schema(),
    });

    // Very short timeout
    let dispatcher = ToolDispatcher::new(registry, router).with_timeout(Duration::from_millis(50));

    let start = std::time::Instant::now();
    let result = dispatch_tool(&dispatcher, "hang", json!({})).await;
    let elapsed = start.elapsed();

    // Should timeout quickly, not hang
    assert!(
        elapsed < Duration::from_secs(1),
        "dispatch should timeout quickly, took {elapsed:?}"
    );
    assert!(result.is_err(), "dispatch should return timeout error");

    // Verify it's a Timeout error (not ExecutionFailed)
    // Regression: Previously timeouts were mapped to ExecutionFailed, which broke
    // error classification (metrics, retries) that rely on the "timeout" error code.
    match result {
        Ok(_) => return Err("Expected timeout error, got Ok".into()),
        Err(ToolError::Timeout { name, timeout_ms }) => {
            assert_eq!(name, "hang", "Timeout should include tool name");
            assert_eq!(
                timeout_ms, 50,
                "Timeout should include configured timeout_ms"
            );
        }
        Err(ToolError::ExecutionFailed { message }) => {
            return Err(format!(
                "Regression: Timeout was incorrectly mapped to ExecutionFailed: {message}"
            )
            .into());
        }
        Err(other) => {
            return Err(format!("Expected Timeout error, got: {other:?}").into());
        }
    }

    // Verify the error code is "timeout" (for classification)
    let err = ToolError::timeout("test", 100);
    assert_eq!(err.error_code(), "timeout");

    Ok(())
}

/// Regression test: CompositeDispatcher must deduplicate external tools.
/// Without deduplication, LLMs receive duplicate tool definitions and may
/// generate args for the wrong schema.
#[tokio::test]
async fn test_regression_composite_deduplicates_external_tools()
-> Result<(), Box<dyn std::error::Error>> {
    use async_trait::async_trait;
    use meerkat_core::types::ToolDef;

    /// An external dispatcher that provides a tool with the same name as a builtin
    struct DuplicatingDispatcher;

    #[async_trait]
    impl AgentToolDispatcher for DuplicatingDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::from([
                // Duplicate of builtin
                Arc::new(ToolDef {
                    name: "task_list".to_string(),
                    description: "External task_list (should be shadowed)".to_string(),
                    input_schema: json!({"type": "object", "properties": {"external": {"type": "boolean"}}}),
                }),
                // Unique external tool
                Arc::new(ToolDef {
                    name: "external_only".to_string(),
                    description: "External-only tool".to_string(),
                    input_schema: json!({"type": "object"}),
                }),
            ])
        }

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
            let value = json!({"from": "external", "tool": call.name});
            Ok(ToolResult::new(
                call.id.to_string(),
                value.to_string(),
                false,
            ))
        }
    }

    let store = Arc::new(MemoryTaskStore::new());
    let config = BuiltinToolConfig::default();
    let external = Arc::new(DuplicatingDispatcher) as Arc<dyn AgentToolDispatcher>;

    let dispatcher = CompositeDispatcher::new(store, &config, None, Some(external), None)?;
    let tools = dispatcher.tools();

    // Count occurrences of task_list
    let task_list_count = tools.iter().filter(|t| t.name == "task_list").count();
    assert_eq!(
        task_list_count, 1,
        "task_list should appear exactly once, not duplicated (found {task_list_count})"
    );

    // Verify the builtin version is kept (check description doesn't say "External")
    let task_list = tools
        .iter()
        .find(|t| t.name == "task_list")
        .ok_or("task_list should exist")?;
    assert!(
        !task_list.description.contains("External"),
        "Builtin task_list should take precedence over external"
    );

    // Verify unique external tool is still included
    assert!(
        tools.iter().any(|t| t.name == "external_only"),
        "Unique external tools should still be included"
    );

    Ok(())
}

/// Regression test: FilteredDispatcher must enforce tool access policies.
/// Previously, tool_access was parsed but never applied in agent_spawn/agent_fork,
/// so sub-agents could access all tools regardless of allow/deny configuration.
#[test]
fn test_regression_filtered_dispatcher_enforces_tool_access_policy() {
    use async_trait::async_trait;
    use meerkat_core::ops::ToolAccessPolicy;
    use meerkat_core::types::ToolDef;
    use meerkat_tools::dispatcher::FilteredDispatcher;

    /// Mock dispatcher with security-sensitive tools
    struct MockDispatcherWithSensitiveTools;

    #[async_trait]
    impl AgentToolDispatcher for MockDispatcherWithSensitiveTools {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::from([
                Arc::new(ToolDef {
                    name: "shell".to_string(),
                    description: "Execute shell commands (security sensitive)".to_string(),
                    input_schema: json!({"type": "object"}),
                }),
                Arc::new(ToolDef {
                    name: "agent_spawn".to_string(),
                    description: "Spawn sub-agents (nesting sensitive)".to_string(),
                    input_schema: json!({"type": "object"}),
                }),
                Arc::new(ToolDef {
                    name: "task_list".to_string(),
                    description: "List tasks (safe)".to_string(),
                    input_schema: json!({"type": "object"}),
                }),
                Arc::new(ToolDef {
                    name: "wait".to_string(),
                    description: "Wait (safe)".to_string(),
                    input_schema: json!({"type": "object"}),
                }),
            ])
        }

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
            let value = json!({"called": call.name});
            Ok(ToolResult::new(
                call.id.to_string(),
                value.to_string(),
                false,
            ))
        }
    }

    let inner = Arc::new(MockDispatcherWithSensitiveTools);

    // Test 1: DenyList should block specified tools
    {
        let policy =
            ToolAccessPolicy::DenyList(vec!["shell".to_string(), "agent_spawn".to_string()]);
        let filtered = FilteredDispatcher::new(inner.clone(), &policy);

        let tool_names: Vec<_> = filtered.tools().iter().map(|t| t.name.clone()).collect();

        // Shell and agent_spawn should be blocked
        assert!(
            !tool_names.contains(&"shell".to_string()),
            "shell should be blocked by deny list but tools are: {tool_names:?}"
        );
        assert!(
            !tool_names.contains(&"agent_spawn".to_string()),
            "agent_spawn should be blocked by deny list"
        );

        // Safe tools should still be available
        assert!(
            tool_names.contains(&"task_list".to_string()),
            "task_list should be available"
        );
        assert!(
            tool_names.contains(&"wait".to_string()),
            "wait should be available"
        );
    }

    // Test 2: AllowList should only permit specified tools
    {
        let policy = ToolAccessPolicy::AllowList(vec!["task_list".to_string()]);
        let filtered = FilteredDispatcher::new(inner.clone(), &policy);

        let tool_names: Vec<_> = filtered.tools().iter().map(|t| t.name.clone()).collect();

        // Only task_list should be available
        assert_eq!(
            tool_names.len(),
            1,
            "AllowList should restrict to exactly 1 tool"
        );
        assert_eq!(
            tool_names[0], "task_list",
            "Only task_list should be in allow list"
        );

        // Everything else should be blocked
        assert!(
            !tool_names.contains(&"shell".to_string()),
            "shell should be blocked (not in allow list)"
        );
        assert!(
            !tool_names.contains(&"agent_spawn".to_string()),
            "agent_spawn should be blocked (not in allow list)"
        );
        assert!(
            !tool_names.contains(&"wait".to_string()),
            "wait should be blocked (not in allow list)"
        );
    }

    // Test 3: Inherit should pass through all tools
    {
        let policy = ToolAccessPolicy::Inherit;
        let filtered = FilteredDispatcher::new(inner, &policy);

        let tool_names: Vec<_> = filtered.tools().iter().map(|t| t.name.clone()).collect();

        assert_eq!(
            tool_names.len(),
            4,
            "Inherit should pass all 4 tools through"
        );
    }
}

/// Regression test: Blocked tools must return NotFound on dispatch attempts.
/// Even if a tool exists in the inner dispatcher, if it's blocked by policy,
/// dispatch must fail with NotFound (not silently succeed).
#[tokio::test]
async fn test_regression_filtered_dispatcher_dispatch_blocked_returns_not_found() {
    use async_trait::async_trait;
    use meerkat_core::error::ToolError;
    use meerkat_core::ops::ToolAccessPolicy;
    use meerkat_core::types::ToolDef;
    use meerkat_tools::dispatcher::FilteredDispatcher;

    struct MockDispatcher;

    #[async_trait]
    impl AgentToolDispatcher for MockDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::from([Arc::new(ToolDef {
                name: "shell".to_string(),
                description: "Shell".to_string(),
                input_schema: json!({"type": "object"}),
            })])
        }

        async fn dispatch(&self, _call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
            // This should NOT be called for blocked tools
            panic!("Dispatch should not be called for blocked tools!");
        }
    }

    let inner = Arc::new(MockDispatcher);
    let policy = ToolAccessPolicy::DenyList(vec!["shell".to_string()]);
    let filtered = FilteredDispatcher::new(inner, &policy);

    // Attempting to dispatch a blocked tool should return NotFound
    let result = dispatch_tool(&filtered, "shell", json!({})).await;

    match result {
        Err(ToolError::NotFound { name }) => {
            assert_eq!(name, "shell", "NotFound error should name the blocked tool");
        }
        Ok(_) => panic!("Dispatch to blocked tool should fail, not succeed!"),
        Err(other) => panic!("Expected NotFound error, got: {other:?}"),
    }
}
