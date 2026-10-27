#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::uninlined_format_args
)]

//! H1/H2 wire visibility: the rpc_gateway reports the composition-time
//! storage durability resolution on `meta/status` and
//! `meta/capabilities` — disk-backed blobs plus the incremental session
//! capability in persistent mode, and the declared-ephemeral posture on the
//! default launch.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};

use serde_json::{Value, json};

const MOB_CONFIG: &str = r#"
[mob]
id = "storage-health-gateway-test"

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

    fn status_storage(&mut self) -> Value {
        let status = self.rpc(json!({
            "jsonrpc": "2.0",
            "id": "status",
            "method": "meta/status",
            "params": {}
        }));
        status["result"]["storage"].clone()
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
fn persistent_gateway_reports_persistent_disk_blobs_and_incremental_sessions() {
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

    let storage = gateway.status_storage();
    assert_eq!(
        storage["blob_durability"],
        json!("persistent_disk"),
        "unexpected storage object: {storage}"
    );
    assert_eq!(storage["blob_store_persistent"], json!(true));
    assert_eq!(
        storage["session_store_incremental"],
        json!(true),
        "SqliteSessionStore advertises incremental persistence"
    );

    // The capabilities object carries the same storage summary.
    let caps = gateway.rpc(json!({
        "jsonrpc": "2.0",
        "id": "caps",
        "method": "meta/capabilities",
        "params": {}
    }));
    assert_eq!(
        caps["result"]["storage"]["blob_durability"],
        json!("persistent_disk"),
        "capabilities must mirror the status storage object: {}",
        caps["result"]["storage"]
    );
}

#[test]
fn ephemeral_gateway_reports_declared_ephemeral_blobs() {
    let mut gateway = GatewayProcess::start();
    let init = gateway.init(json!({
        "mob_config": MOB_CONFIG
    }));
    assert!(
        init["result"]["contract_version"].is_string(),
        "init failed: {init}"
    );

    let storage = gateway.status_storage();
    assert_eq!(
        storage["blob_durability"],
        json!("declared_ephemeral"),
        "unexpected storage object: {storage}"
    );
    assert_eq!(storage["blob_store_persistent"], json!(false));
    assert!(
        storage["session_store_incremental"].is_null(),
        "the default ephemeral launch has no durable session-store declaration: {storage}"
    );
}
