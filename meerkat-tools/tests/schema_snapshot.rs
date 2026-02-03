use meerkat_core::AgentToolDispatcher;
use meerkat_tools::builtin::shell::ShellConfig;
use meerkat_tools::builtin::{
    BuiltinToolConfig, CompositeDispatcher, MemoryTaskStore, ToolPolicyLayer,
};
use serde_json::{Map, Value};
use std::path::Path;
use std::sync::Arc;

fn canonicalize(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(String, Value)> = map.into_iter().collect();
            entries.sort_by(|(a, _), (b, _)| a.cmp(b));

            let mut ordered = Map::new();
            for (k, v) in entries {
                ordered.insert(k, canonicalize(v));
            }
            Value::Object(ordered)
        }
        Value::Array(items) => Value::Array(items.into_iter().map(canonicalize).collect()),
        other => other,
    }
}

fn snapshot_path() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
        .join("builtin_tool_schemas.json")
}

fn generate_snapshot() -> Result<Value, Box<dyn std::error::Error>> {
    let policy = ToolPolicyLayer::new()
        .enable_tool("shell")
        .enable_tool("shell_job_status")
        .enable_tool("shell_job_cancel")
        .enable_tool("shell_jobs");
    let config = BuiltinToolConfig {
        policy,
        ..Default::default()
    };

    let store = Arc::new(MemoryTaskStore::new());
    let temp_root = tempfile::tempdir()?;
    let shell_config = ShellConfig::with_project_root(temp_root.path().to_path_buf());

    let dispatcher = CompositeDispatcher::new(store, &config, Some(shell_config), None, None)?;

    let mut tools: Vec<_> = dispatcher.tools().iter().cloned().collect();
    tools.sort_by(|a, b| a.name.cmp(&b.name));

    let mut schemas = Map::new();
    for tool in tools {
        schemas.insert(tool.name.clone(), canonicalize(tool.input_schema.clone()));
    }

    Ok(Value::Object(schemas))
}

#[test]
fn test_builtin_tool_schema_snapshot() -> Result<(), Box<dyn std::error::Error>> {
    let generated = generate_snapshot()?;
    let path = snapshot_path();

    if std::env::var("UPDATE_SCHEMA_SNAPSHOT").as_deref() == Ok("1") {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(&generated)?;
        std::fs::write(&path, json)?;
        return Ok(());
    }

    let expected_text = std::fs::read_to_string(&path)?;
    let expected: Value = serde_json::from_str(&expected_text)?;

    assert_eq!(
        generated, expected,
        "Schema snapshot mismatch. To update: UPDATE_SCHEMA_SNAPSHOT=1 cargo test -p meerkat-tools --test schema_snapshot"
    );

    Ok(())
}
