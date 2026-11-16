//! The `rkat-mob-mcp` binary, driven over stdio, answers a reserved label or
//! context key on the two public spawn tools as the JSON-RPC spawn methods
//! do: invalid params with the shared metadata rule's message, and no member
//! is spawned. Ordinary labels and context are taken and stored.

#![allow(clippy::expect_used, clippy::panic)]

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

const MOB: &str = "stdio-spawn-metadata";

/// Cargo sets `CARGO_BIN_EXE_rkat-mob-mcp` for every integration test of the
/// package that owns the binary, and this test's generated Bazel target wires
/// the same variable to the package's binary target
/// (`scripts/generate-bazel-rust-builds.mjs`). An unset variable fails the
/// test: there is no skip arm.
fn mob_mcp_binary() -> PathBuf {
    let path =
        PathBuf::from(std::env::var_os("CARGO_BIN_EXE_rkat-mob-mcp").expect(
            "CARGO_BIN_EXE_rkat-mob-mcp is set by cargo and by the generated Bazel targets",
        ));
    path.canonicalize().unwrap_or_else(|err| {
        panic!(
            "rkat-mob-mcp binary {} is not reachable: {err}",
            path.display()
        )
    })
}

struct MobMcpProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    next_id: u64,
}

impl MobMcpProcess {
    fn start(home: &std::path::Path) -> Self {
        let mut child = Command::new(mob_mcp_binary())
            .current_dir(home)
            .env("HOME", home)
            .env("XDG_DATA_HOME", home.join("data"))
            .env_remove("OPENAI_API_KEY")
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("GEMINI_API_KEY")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("start rkat-mob-mcp");
        let stdin = child.stdin.take().expect("the server's stdin");
        let stdout = child.stdout.take().expect("the server's stdout");
        let (sender, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            stdin: Some(stdin),
            lines,
            next_id: 1,
        }
    }

    /// Send one request and return the response that carries its id.
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let mut line =
            json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string();
        line.push('\n');
        let stdin = self.stdin.as_mut().expect("the server's stdin is open");
        stdin.write_all(line.as_bytes()).expect("write the request");
        stdin.flush().expect("flush the request");
        loop {
            let line = self
                .lines
                .recv_timeout(Duration::from_secs(120))
                .unwrap_or_else(|err| panic!("no response to {method} request {id}: {err}"));
            if line.trim().is_empty() {
                continue;
            }
            let value: Value = serde_json::from_str(line.trim())
                .unwrap_or_else(|err| panic!("stdout carries JSON only, got '{line}': {err}"));
            if value.get("id").and_then(Value::as_u64) == Some(id) {
                return value;
            }
        }
    }

    fn call(&mut self, tool: &str, arguments: Value) -> Value {
        self.request("tools/call", json!({"name": tool, "arguments": arguments}))
    }

    /// The labels of every `member_spawned` event of the mob, by member.
    fn spawned_labels(&mut self) -> Vec<(String, Value)> {
        let response = self.call(
            "meerkat_mob_events",
            json!({"mob_id": MOB, "after_cursor": 0, "limit": 1000}),
        );
        let text = response["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("the events tool answers a text payload: {response}"));
        let payload: Value = serde_json::from_str(text).expect("the events payload is JSON");
        payload["events"]
            .as_array()
            .expect("the events payload lists events")
            .iter()
            .filter(|event| {
                event["kind"]["type"]
                    .as_str()
                    .is_some_and(|kind| kind.replace('_', "").eq_ignore_ascii_case("memberspawned"))
            })
            .map(|event| {
                (
                    event["kind"]["agent_identity"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    event["kind"]
                        .get("labels")
                        .cloned()
                        .unwrap_or_else(|| json!({})),
                )
            })
            .collect()
    }

    /// Close stdin so the server ends on its own, and wait for it (bounded:
    /// a server that does not end is stopped and fails the test).
    fn finish(mut self) {
        drop(self.stdin.take());
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        loop {
            if let Some(status) = self.child.try_wait().expect("poll rkat-mob-mcp") {
                assert!(status.success(), "rkat-mob-mcp ended with {status}");
                return;
            }
            if std::time::Instant::now() >= deadline {
                let _ = self.child.kill();
                let _ = self.child.wait();
                panic!("rkat-mob-mcp did not end within 60 s of its stdin closing");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

fn with_key(key: &str) -> Value {
    let mut metadata = serde_json::Map::new();
    metadata.insert("org".to_string(), json!("acme"));
    metadata.insert(key.to_string(), json!("caller-value"));
    Value::Object(metadata)
}

/// `meerkat_mob_spawn_many` gets an entry with no labels and no context
/// first, then the entry that carries them.
fn arguments(tool: &str, member: &str, labels: Value, context: Value) -> Value {
    if tool == "meerkat_mob_spawn" {
        json!({
            "mob_id": MOB,
            "profile": "worker",
            "agent_identity": member,
            "runtime_mode": "turn_driven",
            "labels": labels,
            "context": context,
        })
    } else {
        json!({
            "mob_id": MOB,
            "specs": [
                {
                    "profile": "worker",
                    "agent_identity": format!("{member}-plain"),
                    "runtime_mode": "turn_driven",
                },
                {
                    "profile": "worker",
                    "agent_identity": member,
                    "runtime_mode": "turn_driven",
                    "labels": labels,
                    "context": context,
                },
            ],
        })
    }
}

#[test]
fn the_binary_refuses_reserved_spawn_metadata_keys_over_stdio() {
    let home = tempfile::tempdir().expect("a private home");
    let mut server = MobMcpProcess::start(home.path());

    let initialized = server.request("initialize", json!({}));
    assert_eq!(
        initialized["result"]["serverInfo"]["name"], "rkat-mob-mcp",
        "{initialized}"
    );
    let created = server.call(
        "meerkat_mob_create",
        json!({
            "definition": {
                "id": MOB,
                "profiles": {
                    "worker": { "model": "gpt-5.4", "tools": {"comms": true} }
                }
            }
        }),
    );
    assert!(created.get("error").is_none(), "create the mob: {created}");

    let mut cases = Vec::new();
    for tool in ["meerkat_mob_spawn", "meerkat_mob_spawn_many"] {
        for key in [
            "mob_id",
            "role",
            "meerkat_id",
            "agent_identity",
            "meerkat.runtime_id",
            "Role",
        ] {
            cases.push((
                tool,
                with_key(key),
                Value::Null,
                format!("metadata label key '{key}' is reserved for Meerkat-owned runtime facts"),
            ));
        }
        cases.push((
            tool,
            Value::Null,
            with_key("meerkat.runtime_id"),
            "app_context key 'meerkat.runtime_id' is reserved for Meerkat-owned runtime facts"
                .to_string(),
        ));
    }
    let mut wrong = Vec::new();
    for (case, (tool, labels, context, message)) in cases.into_iter().enumerate() {
        let response = server.call(tool, arguments(tool, &format!("w-{case}"), labels, context));
        let refused = response["error"]["code"] == json!(-32602)
            && response["error"]["message"] == json!(message);
        if !refused {
            wrong.push(format!(
                "{tool} (wanted error -32602 '{message}') answered {response}"
            ));
        }
    }
    let spawned = server.spawned_labels();
    assert!(
        wrong.is_empty() && spawned.is_empty(),
        "calls with a reserved key were not refused as the rule answers:\n{}\nmember_spawned events: {spawned:?}",
        wrong.join("\n")
    );

    let ordinary = json!({"org": "acme", "tier": "gold"});
    let context = json!({"client_ref": {"view": "compact"}, "role": "reviewer"});
    let one = server.call(
        "meerkat_mob_spawn",
        arguments(
            "meerkat_mob_spawn",
            "w-one",
            ordinary.clone(),
            context.clone(),
        ),
    );
    assert!(one.get("error").is_none(), "ordinary labels spawn: {one}");
    let many = server.call(
        "meerkat_mob_spawn_many",
        arguments(
            "meerkat_mob_spawn_many",
            "w-many",
            ordinary.clone(),
            context,
        ),
    );
    assert!(many.get("error").is_none(), "ordinary labels spawn: {many}");
    let mut spawned = server.spawned_labels();
    spawned.sort_by(|left, right| left.0.cmp(&right.0));
    assert_eq!(
        spawned,
        vec![
            ("w-many".to_string(), ordinary.clone()),
            ("w-many-plain".to_string(), json!({})),
            ("w-one".to_string(), ordinary),
        ],
        "the spawn events carry the caller's ordinary labels"
    );

    server.finish();
}
