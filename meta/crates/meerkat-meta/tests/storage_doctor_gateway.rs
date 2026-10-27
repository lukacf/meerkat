#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::uninlined_format_args
)]

//! M1 wire visibility: the rpc_gateway serves `meta/storage/doctor` — a
//! read-only state-directory diagnosis with the live H1/H2 durability census
//! attached in persistent mode, and the typed capability error when no
//! `state_dir` is given.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};

use serde_json::{Value, json};

const MOB_CONFIG: &str = r#"
[mob]
id = "storage-doctor-gateway-test"

[profiles.default]
model = "gpt-5.5"
"#;

struct GatewayProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
}

impl GatewayProcess {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_rpc_gateway"))
            .arg("--persistent")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn rpc_gateway");
        let stdin = child.stdin.take().expect("gateway stdin");
        let stdout = BufReader::new(child.stdout.take().expect("gateway stdout"));
        Self {
            child,
            stdin,
            stdout,
        }
    }

    fn rpc(&mut self, request: Value) -> Value {
        writeln!(
            self.stdin,
            "{}",
            serde_json::to_string(&request).expect("request json")
        )
        .expect("write request");
        self.stdin.flush().expect("flush request");

        let mut line = String::new();
        self.stdout.read_line(&mut line).expect("read response");
        assert!(!line.is_empty(), "gateway closed stdout before response");
        serde_json::from_str(line.trim()).expect("response json")
    }

    fn init(&mut self, params: Value) -> Value {
        self.rpc(json!({
            "jsonrpc": "2.0",
            "id": "init",
            "method": "meta/init",
            "params": params
        }))
    }
}

impl Drop for GatewayProcess {
    fn drop(&mut self) {
        let _ = self.stdin.flush();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn doctor_reports_twins_and_live_durability_on_persistent_gateway() {
    let state_dir = tempfile::tempdir().expect("state dir");
    let mut gateway = GatewayProcess::start();
    let init = gateway.init(json!({
        "persistent_state": state_dir.path(),
        "mob_config": MOB_CONFIG
    }));
    assert!(
        init["result"]["contract_version"].is_string(),
        "init failed: {init}"
    );

    // Fabricate split-brain session twins beside the live gateway. Its own
    // sessions live in Core's pair on runtime.sqlite, so both spellings are
    // planted.
    std::fs::write(state_dir.path().join("sessions.db"), b"").expect("twin file");
    std::fs::write(state_dir.path().join("sessions.sqlite"), b"").expect("twin file");

    let resp = gateway.rpc(json!({
        "jsonrpc": "2.0",
        "id": "doctor",
        "method": "meta/storage/doctor",
        "params": { "state_dir": state_dir.path() }
    }));
    let result = &resp["result"];
    assert_eq!(
        result["storage"]["blob_durability"],
        json!("persistent_disk"),
        "live gateway attaches the H1/H2 summary: {resp:#?}"
    );
    let findings = result["diagnosis"]["findings"]
        .as_array()
        .expect("findings array");
    assert!(
        findings
            .iter()
            .any(|f| f["code"] == "file-name-twins" && f["severity"] == "error"),
        "{findings:#?}"
    );
    assert!(
        findings.iter().any(|f| f["code"] == "blob-durability"),
        "live durability census rides the findings: {findings:#?}"
    );
    assert!(
        !findings
            .iter()
            .any(|f| f["code"] == "durability-census-unavailable"),
        "{findings:#?}"
    );
    let inventory = result["diagnosis"]["inventory"]
        .as_array()
        .expect("inventory array");
    assert_eq!(inventory.len(), 1, "{inventory:#?}");
    assert!(
        inventory[0]["databases"]
            .as_array()
            .is_some_and(|dbs| !dbs.is_empty()),
        "the gateway's own databases are inventoried: {inventory:#?}"
    );
}

#[test]
fn doctor_without_state_dir_is_a_typed_capability_error() {
    let mut gateway = GatewayProcess::start();
    let init = gateway.init(json!({
        "mob_config": MOB_CONFIG
    }));
    assert!(
        init["result"]["contract_version"].is_string(),
        "init failed: {init}"
    );

    let resp = gateway.rpc(json!({
        "jsonrpc": "2.0",
        "id": "doctor-missing",
        "method": "meta/storage/doctor",
        "params": {}
    }));
    assert_eq!(resp["error"]["code"], json!(-32004), "{resp:#?}");
    assert!(
        resp["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("state_dir"),
        "{resp:#?}"
    );
}
