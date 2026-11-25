#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::disallowed_methods
)]

use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::Stdio;
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::time::{Duration, timeout};

/// Cargo, nextest, the e2e lane harness and the Bazel graph advertise the
/// crate's own binary through `CARGO_BIN_EXE_rkat-mcp`, so a missing binary
/// fails the test instead of skipping it.
fn mcp_binary_path() -> PathBuf {
    let advertised = PathBuf::from(
        std::env::var_os("CARGO_BIN_EXE_rkat-mcp")
            .expect("cargo advertises CARGO_BIN_EXE_rkat-mcp to the crate's integration tests"),
    );
    // Bazel wires a runfiles-relative path; the server below is spawned
    // with its own working directory, so resolve it first.
    advertised.canonicalize().unwrap_or_else(|error| {
        panic!(
            "CARGO_BIN_EXE_rkat-mcp names a missing binary {}: {error}",
            advertised.display()
        )
    })
}

struct McpProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl McpProcess {
    async fn send(&mut self, request: &Value) -> Result<(), Box<dyn std::error::Error>> {
        let mut line = serde_json::to_string(request)?;
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    async fn read_response(&mut self, id: u64) -> Result<Value, Box<dyn std::error::Error>> {
        loop {
            let mut line = String::new();
            let read = timeout(Duration::from_secs(30), self.stdout.read_line(&mut line)).await??;
            if read == 0 {
                return Err(format!("MCP stdout closed before response {id}").into());
            }
            if line.trim().is_empty() {
                continue;
            }
            let value: Value = serde_json::from_str(line.trim())?;
            if value.get("id").and_then(Value::as_u64) == Some(id) {
                return Ok(value);
            }
        }
    }
}

fn unwrap_tool_payload(value: &Value) -> Value {
    let raw = value["result"]["content"][0]["text"]
        .as_str()
        .expect("wrapped MCP payload text");
    serde_json::from_str(raw).expect("wrapped payload json")
}

async fn spawn_mcp_stdio(
    cwd: &std::path::Path,
    state_root: &std::path::Path,
) -> Result<McpProcess, Box<dyn std::error::Error>> {
    let binary = mcp_binary_path();
    let mut child = Command::new(binary)
        .current_dir(cwd)
        .env_clear()
        .env("HOME", cwd)
        .env("XDG_CONFIG_HOME", cwd.join("config"))
        .env("XDG_STATE_HOME", cwd.join("state"))
        .env("XDG_CACHE_HOME", cwd.join("cache"))
        .env("XDG_DATA_HOME", cwd.join("data"))
        .env("RKAT_TEST_CLIENT", "1")
        .args([
            "--realm",
            "mcp-binary-e2e",
            "--state-root",
            state_root.to_str().ok_or("invalid state root")?,
            "--context-root",
            cwd.to_str().ok_or("invalid cwd")?,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()?;

    let stdin = child.stdin.take().ok_or("missing stdin")?;
    let stdout = BufReader::new(child.stdout.take().ok_or("missing stdout")?);
    Ok(McpProcess {
        child,
        stdin,
        stdout,
    })
}

#[tokio::test]
#[ignore = "lane:e2e-build"]
async fn stdio_e2e_run_and_resume() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDir::new()?;
    let project_dir = temp.path().join("project");
    let state_root = temp.path().join("state");
    tokio::fs::create_dir_all(project_dir.join(".rkat")).await?;
    tokio::fs::create_dir_all(project_dir.join("data")).await?;
    tokio::fs::create_dir_all(&state_root).await?;
    tokio::fs::write(
        project_dir.join(".rkat/config.toml"),
        "[agent]\nmodel = \"claude-sonnet-4-5\"\nmax_tokens_per_turn = 128\n",
    )
    .await?;

    let mut mcp = spawn_mcp_stdio(&project_dir, &state_root).await?;

    mcp.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {}
    }))
    .await?;
    let initialize = mcp.read_response(1).await?;
    assert_eq!(initialize["result"]["serverInfo"]["name"], "rkat-mcp");

    mcp.send(&json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized",
        "params": {}
    }))
    .await?;

    mcp.send(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    }))
    .await?;
    let tools = mcp.read_response(2).await?;
    assert!(
        tools["result"]["tools"]
            .as_array()
            .is_some_and(|entries| entries.iter().any(|entry| entry["name"] == "meerkat_run"))
    );

    mcp.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "meerkat_run",
            "arguments": {
                "prompt": "Remember the token BINARY_MCP_42 and reply with ok.",
                "model": "claude-sonnet-4-5"
            }
        }
    }))
    .await?;
    let created = mcp.read_response(3).await?;
    let created_payload = unwrap_tool_payload(&created);
    let session_id = created_payload["session_id"]
        .as_str()
        .ok_or("missing session_id")?
        .to_string();
    assert_eq!(created_payload["content"][0]["text"], "ok");

    mcp.send(&json!({
        "jsonrpc": "2.0",
        "id": 35,
        "method": "tools/call",
        "params": {
            "name": "meerkat_sessions",
            "arguments": {}
        }
    }))
    .await?;
    let listed_after_run = unwrap_tool_payload(&mcp.read_response(35).await?);
    assert!(
        listed_after_run["sessions"]
            .as_array()
            .is_some_and(|sessions| sessions
                .iter()
                .any(|entry| entry["session_id"].as_str() == Some(session_id.as_str())))
    );

    mcp.send(&json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/call",
        "params": {
            "name": "meerkat_resume",
            "arguments": {
                "session_id": session_id,
                "prompt": "What token was I asked to remember? Reply with the token only."
            }
        }
    }))
    .await?;
    let resumed = mcp.read_response(4).await?;
    let resumed_payload = unwrap_tool_payload(&resumed);
    assert_eq!(
        resumed_payload["session_id"].as_str(),
        Some(session_id.as_str())
    );
    assert_eq!(resumed_payload["content"][0]["text"], "ok");

    mcp.send(&json!({
        "jsonrpc": "2.0",
        "id": 36,
        "method": "tools/call",
        "params": {
            "name": "meerkat_sessions",
            "arguments": {}
        }
    }))
    .await?;
    let listed_after_resume = unwrap_tool_payload(&mcp.read_response(36).await?);
    assert!(
        listed_after_resume["sessions"]
            .as_array()
            .is_some_and(|sessions| sessions
                .iter()
                .filter(|entry| entry["session_id"].as_str() == Some(session_id.as_str()))
                .count()
                == 1),
        "resume should not create a duplicate session entry: {listed_after_resume}"
    );

    let _ = mcp.child.kill().await;
    Ok(())
}

/// The public MCP wire returns usable evidence continuations, including its
/// terminal page and the protocol error for a cursor whose item has changed.
/// All state and evidence are synthetic; this row never starts a model turn.
#[tokio::test]
#[ignore = "lane:e2e-build"]
async fn stdio_workgraph_evidence_cursor_walk_and_stale_revision()
-> Result<(), Box<dyn std::error::Error>> {
    use futures::FutureExt as _;
    use std::panic::AssertUnwindSafe;

    async fn call(
        mcp: &mut McpProcess,
        next_id: &mut u64,
        tool: &str,
        arguments: Value,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        let id = *next_id;
        *next_id += 1;
        mcp.send(&json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {"name": tool, "arguments": arguments}
        }))
        .await?;
        mcp.read_response(id).await
    }

    let temp = TempDir::new()?;
    let project_dir = temp.path().join("project");
    let state_root = temp.path().join("state");
    for directory in [".rkat", "data", "config", "state", "cache"] {
        tokio::fs::create_dir_all(project_dir.join(directory)).await?;
    }
    tokio::fs::create_dir_all(&state_root).await?;
    let mut mcp = spawn_mcp_stdio(&project_dir, &state_root).await?;
    // An overall deadline bounds writes, response-ID filtering and all calls,
    // while the normal harness retains its per-read timeout. Catch panics so
    // the owned child is killed and reaped before any first failure is returned.
    let outcome = AssertUnwindSafe(timeout(Duration::from_mins(2), async {
        mcp.send(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}})).await?;
        let initialized = mcp.read_response(1).await?;
        assert_eq!(initialized["result"]["serverInfo"]["name"], "rkat-mcp");
        mcp.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}})).await?;
        mcp.send(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}})).await?;
        let advertised = mcp.read_response(2).await?;
        assert!(advertised["result"]["tools"].as_array().expect("tools/list array")
            .iter().any(|tool| tool["name"] == "workgraph_get"));

        let mut next_id = 3;
        let created = call(&mut mcp, &mut next_id, "workgraph_create",
            json!({"title": "MCP evidence pagination fixture"})).await?;
        let mut item = unwrap_tool_payload(&created)["item"].clone();
        let item_id = item["id"].as_str().expect("created item ID").to_owned();
        let evidence: Vec<Value> = (1..=5).map(|index| json!({
            "kind": "note", "id": format!("mcp-evidence-{index}"),
            "summary": format!("Synthetic fixture evidence {index}")
        })).collect();
        for entry in &evidence {
            let added = call(&mut mcp, &mut next_id, "workgraph_add_evidence", json!({
                "id": item_id, "expected_revision": item["revision"], "evidence": entry
            })).await?;
            item = unwrap_tool_payload(&added)["item"].clone();
        }
        assert_eq!(item["evidence_refs"], json!(evidence));

        let mut next_cursor: Option<String> = None;
        let mut first_cursor = None;
        let mut seen_ids = Vec::new();
        let mut seen_positions = Vec::new();
        for (page_number, expected_positions) in [&[4_usize, 3][..], &[2, 1][..], &[0][..]].into_iter().enumerate() {
            let mut arguments = json!({"id": item_id, "evidence_limit": 2});
            if let Some(cursor) = &next_cursor {
                arguments["evidence_cursor"] = json!(cursor);
            }
            let response = call(&mut mcp, &mut next_id, "workgraph_get", arguments).await?;
            assert!(response.get("error").is_none(), "page {}: {response}", page_number + 1);
            let payload = unwrap_tool_payload(&response);
            assert_eq!(payload["item"]["id"], item_id);
            assert_eq!(payload["item"]["revision"], item["revision"]);
            assert!(payload["item"].get("evidence_refs").is_none());
            let page = &payload["evidence_page"];
            assert_eq!(page["order"], "append_position_desc");
            assert_eq!(page["total"], 5);
            assert_eq!(page["limit"], 2);
            assert_eq!(page["returned"], expected_positions.len());
            let entries = page["entries"].as_array().expect("wire evidence entries");
            assert_eq!(entries.len(), expected_positions.len());
            for (entry, &position) in entries.iter().zip(expected_positions) {
                assert_eq!(entry["position"], position);
                assert_eq!(entry["evidence"], evidence[position]);
                seen_positions.push(position);
                seen_ids.push(entry["evidence"]["id"].as_str().expect("evidence ID").to_owned());
            }
            if page_number < 2 {
                assert_eq!(page["truncated"], true);
                let returned = page["next_cursor"].as_str().filter(|cursor| !cursor.is_empty())
                    .expect("nonterminal wire page returns a cursor").to_owned();
                if page_number == 0 { first_cursor = Some(returned.clone()); }
                next_cursor = Some(returned);
            } else {
                assert_eq!(page["truncated"], false);
                assert_eq!(page.get("next_cursor"), Some(&Value::Null));
            }
        }
        assert_eq!(seen_positions, [4, 3, 2, 1, 0]);
        assert_eq!(seen_ids.iter().collect::<std::collections::BTreeSet<_>>().len(), 5);
        let whole = call(&mut mcp, &mut next_id, "workgraph_get", json!({"id": item_id})).await?;
        assert_eq!(unwrap_tool_payload(&whole), json!({"item": item}));

        let changed = call(&mut mcp, &mut next_id, "workgraph_add_evidence", json!({
            "id": item_id, "expected_revision": item["revision"],
            "evidence": {"kind": "note", "id": "mcp-evidence-6", "summary": "Synthetic revision change"}
        })).await?;
        let changed = unwrap_tool_payload(&changed);
        assert!(changed["item"]["revision"].as_u64().expect("new revision")
            > item["revision"].as_u64().expect("original revision"));
        let stale = call(&mut mcp, &mut next_id, "workgraph_get", json!({
            "id": item_id, "evidence_limit": 2,
            "evidence_cursor": first_cursor.expect("first returned cursor")
        })).await?;
        assert!(stale.get("result").is_none(), "stale cursor must be a protocol error: {stale}");
        assert_eq!(stale["error"]["code"], -32602);
        assert!(stale["error"]["message"].as_str().expect("protocol error message")
            .contains("evidence cursor revision is stale"));
        eprintln!("MCP evidence paging: page sizes=2,2,1; positions=4,3,2,1,0; distinct=5; terminal=null; full-read=5; revision-changed=true; stale-code=-32602");
        Ok::<(), Box<dyn std::error::Error>>(())
    })).catch_unwind().await;

    let cleanup = timeout(Duration::from_secs(10), mcp.child.kill()).await;
    if !matches!(&cleanup, Ok(Ok(()))) {
        eprintln!("MCP evidence fixture child cleanup failed: {cleanup:?}");
    }
    match outcome {
        Err(panic) => std::panic::resume_unwind(panic),
        Ok(Err(elapsed)) => {
            Err(format!("MCP evidence pagination overall deadline: {elapsed}").into())
        }
        Ok(Ok(result)) => {
            result?;
            cleanup??;
            Ok(())
        }
    }
}
