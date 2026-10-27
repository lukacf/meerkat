//! TWO-PROCESS cross-mob round trip: the full-fidelity proof that two real
//! `rpc_gateway --persistent` OS processes can wire members over the
//! cross-mob control channel and deliver messages in both directions with
//! nothing shared but TCP sockets.
//!
//! Two children are spawned from the shipped binary (the
//! `identity_first_subprocess_reboot.rs` harness idiom), each with:
//!
//! * `--control-listen tcp://127.0.0.1:<port>` - the cross-mob control
//!   listener, with the port probed up front so BOTH contact directories
//!   can be written at init time (A needs B's address and B needs A's; an
//!   ephemeral `:0` bind on both sides would be circular). The init
//!   response's new `control_listen_address` field is asserted against the
//!   requested address.
//! * `runtime_options.member_comms_address = "127.0.0.1:0"` - every member
//!   binds its own signed envelope listener, so the descriptors installed
//!   by the wire are dialable across the process boundary.
//! * `runtime_options.contacts_toml` - the inline contact directory
//!   pointing at the OTHER process's control listener.
//! * `runtime_options.control_grants_toml` - a closed, member-scoped grant
//!   for the OTHER process's persisted gateway key.
//! * `runtime_options.demo_llm` - deterministic `TestClient`, no network.
//!
//! Proven across the process boundary:
//!
//! 1. `meta/cross_mob/wire` from A reaches B's control listener over real
//!    TCP, installs signed descriptors on BOTH sides (asserted via each
//!    gateway's own `meta/get_member` `wired_to` projection - the far
//!    side's row is the reverse leg).
//! 2. `meta/cross_mob/send` delivers A -> B and B -> A: each exact RPC
//!    receipt returns the receiving member's bridge session id from the
//!    OTHER process. Both contact entries pin the remote process's persisted
//!    Ed25519 key, so the response signature is the attribution proof. No
//!    event count, latest-event query, sleep, or filesystem polling is
//!    accepted as success authority.
//! 3. `meta/cross_mob/unwire` from A clears the peering on BOTH sides.
//!
//! # What is deliberately NOT here
//!
//! A member-driven signed PeerMessage between the two processes. Members
//! only emit peer messages from inside an agent turn (the comms `send`
//! tool); `demo_llm` makes no tool calls, and no RPC surface invokes a
//! member's session tools directly (`meta/call_tool` routes to modules).
//! Driving it would need a live LLM (the e2e-live lane) or a test-only
//! RPC. The signed envelope plane itself - real TCP dial, ingress trust
//! check against the wired descriptor, ack signed by the addressed
//! member's keypair - is already proven socket-for-socket by
//! `tests/cross_mob_control_round_trip.rs`; what THIS file adds is the
//! process boundary, which the control plane and both wire legs do cross.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args
)]

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use meerkat_meta::GatewayPeerKeys;
use serde_json::{Value, json};

#[path = "support/gateway_shutdown.rs"]
mod gateway_shutdown;

const INIT_TIMEOUT: Duration = Duration::from_mins(3);
const RPC_TIMEOUT: Duration = Duration::from_mins(1);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_mins(3);

const MOB_A: &str = r#"
[mob]
id = "two-proc-a"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.worker.tools]
comms = true
"#;

const MOB_B: &str = r#"
[mob]
id = "two-proc-b"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.worker.tools]
comms = true
"#;

const MARKER_A_TO_B: &str = "cross-mob-marker-alpha-to-bravo-7f3c";
const MARKER_B_TO_A: &str = "cross-mob-marker-bravo-to-alpha-2e91";

/// Probe a free loopback port. The port is released before the gateway
/// binds it, so another process could race onto it; the same accepted
/// caveat as `cross_mob_tcp.rs::ephemeral_port` (both directories must be
/// written before either child boots, so `:0` cannot be used here).
fn probe_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("probe port")
        .local_addr()
        .expect("probe addr")
        .port()
}

struct Gateway {
    label: String,
    child: Child,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<String>,
    stderr: Arc<Mutex<Vec<String>>>,
}

impl Gateway {
    fn start(label: &str, control_listen: &str) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rpc_gateway"));
        command
            .arg("--persistent")
            .arg("--control-listen")
            .arg(control_listen)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().expect("spawn rpc_gateway --persistent");

        let stdin = child.stdin.take().expect("gateway stdin");
        let stdout = child.stdout.take().expect("gateway stdout");
        let child_stderr = child.stderr.take().expect("gateway stderr");

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });

        let stderr = Arc::new(Mutex::new(Vec::new()));
        let stderr_sink = stderr.clone();
        thread::spawn(move || {
            for line in BufReader::new(child_stderr).lines() {
                let Ok(line) = line else { break };
                let Ok(mut sink) = stderr_sink.lock() else {
                    break;
                };
                if sink.len() < 500 {
                    sink.push(line);
                }
            }
        });

        Self {
            label: label.to_string(),
            child,
            stdin: Some(stdin),
            lines: rx,
            stderr,
        }
    }

    fn send(&mut self, value: &Value) {
        let stdin = self.stdin.as_mut().expect("gateway stdin remains open");
        writeln!(
            stdin,
            "{}",
            serde_json::to_string(value).expect("serialize request")
        )
        .expect("write request to gateway stdin");
        stdin.flush().expect("flush gateway stdin");
    }

    fn close_stdin(&mut self) {
        drop(self.stdin.take());
    }

    fn stderr_tail(&self) -> String {
        let Ok(sink) = self.stderr.lock() else {
            return "[gateway stderr unavailable: poisoned lock]".to_string();
        };
        let start = sink.len().saturating_sub(40);
        format!(
            "--- [{}] gateway stderr (last {} of {} lines) ---\n{}",
            self.label,
            sink.len() - start,
            sink.len(),
            sink[start..].join("\n")
        )
    }

    /// Send one request and return its response. This harness declares no
    /// host providers, so any `callback/*` arriving is a composition bug
    /// the test must fail on, not stub around.
    fn call(&mut self, id: &str, method: &str, params: Value, deadline: Duration) -> Value {
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }));
        let start = Instant::now();
        loop {
            let Some(remaining) = deadline.checked_sub(start.elapsed()) else {
                panic!(
                    "[{}] no response to {method} within {:?}\n{}",
                    self.label,
                    deadline,
                    self.stderr_tail()
                );
            };
            let Ok(line) = self.lines.recv_timeout(remaining) else {
                panic!(
                    "[{}] gateway stdout closed while waiting for {method}\n{}",
                    self.label,
                    self.stderr_tail()
                );
            };
            let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            if let Some(callback) = message.get("method").and_then(Value::as_str) {
                assert!(
                    !callback.starts_with("callback/"),
                    "[{}] unexpected host callback {callback}: this harness declares no \
                     providers, so a stub answer would invalidate the run",
                    self.label
                );
                continue; // notification
            }
            if message.get("id").and_then(Value::as_str) == Some(id) {
                assert!(
                    message.get("error").is_none(),
                    "[{}] {method} failed: {message}\n{}",
                    self.label,
                    self.stderr_tail()
                );
                return message;
            }
        }
    }

    fn shutdown_and_reap(&mut self) {
        let response = self.call("shutdown", "meta/shutdown", json!({}), SHUTDOWN_TIMEOUT);
        // A persistent stock gateway stands its durable child mobs down and
        // releases their root: cleanup is complete.
        gateway_shutdown::assert_persistent_shutdown_completed(
            &response,
            &format!("[{}] shutdown\n{}", self.label, self.stderr_tail()),
        );
        self.close_stdin();
        let status = self.child.wait().expect("wait for gateway exit");
        assert!(
            status.success(),
            "[{}] rpc_gateway exited with {status}\n{}",
            self.label,
            self.stderr_tail()
        );
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Boot one gateway: spawn with `--control-listen`, init with the inline
/// contact directory pointing at the peer, and assert the init response
/// reports the requested control address back.
fn boot(
    label: &str,
    state: &Path,
    mob_config: &str,
    control_listen: &str,
    contacts_toml: &str,
    control_grants_toml: &str,
) -> Gateway {
    let mut gateway = Gateway::start(label, control_listen);
    let response = gateway.call(
        "init",
        "meta/init",
        json!({
            "persistent_state": state,
            "mob_config": mob_config,
            "runtime_options": {
                "demo_llm": true,
                "member_comms_address": "127.0.0.1:0",
                "contacts_toml": contacts_toml,
                "control_grants_toml": control_grants_toml,
            }
        }),
        INIT_TIMEOUT,
    );
    assert_eq!(
        response["result"]["control_listen_address"],
        json!(control_listen),
        "[{label}] init response must report the bound control-listener address: {response}"
    );
    gateway
}

fn spawn_member(gateway: &mut Gateway, member: &str) {
    let response = gateway.call(
        "spawn",
        "meta/spawn_member",
        json!({ "profile": "worker", "meerkat_id": member }),
        RPC_TIMEOUT,
    );
    assert_eq!(
        response["result"]["accepted"],
        json!(true),
        "[{}] spawn_member {member} not accepted: {response}",
        gateway.label
    );
}

fn wired_to(gateway: &mut Gateway, member: &str) -> Vec<String> {
    let response = gateway.call(
        "get-member",
        "meta/get_member",
        json!({ "member_id": member }),
        RPC_TIMEOUT,
    );
    response["result"]["wired_to"]
        .as_array()
        .map(|peers| {
            peers
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn two_process_cross_mob_wire_and_delivery_round_trip() {
    let state_a = tempfile::tempdir().expect("state dir a");
    let state_b = tempfile::tempdir().expect("state dir b");
    let port_a = probe_port();
    let port_b = probe_port();
    let control_a = format!("tcp://127.0.0.1:{port_a}");
    let control_b = format!("tcp://127.0.0.1:{port_b}");
    let keys_a = GatewayPeerKeys::load_or_create(state_a.path()).expect("gateway A keys");
    let keys_b = GatewayPeerKeys::load_or_create(state_b.path()).expect("gateway B keys");
    let contacts_a = format!(
        "[mobs]\n\"two-proc-b\" = {{ transport = \"{control_b}\", pubkey = \"{}\" }}\n",
        keys_b.pubkey_b64()
    );
    let contacts_b = format!(
        "[mobs]\n\"two-proc-a\" = {{ transport = \"{control_a}\", pubkey = \"{}\" }}\n",
        keys_a.pubkey_b64()
    );
    let grants_a = format!(
        "[control_grants.gateway-b]\npubkey = \"{}\"\nverbs = [\"lookup_member\", \"wire\", \"unwire\", \"inject\"]\nmembers = [\"alice\"]\n",
        keys_b.pubkey_b64()
    );
    let grants_b = format!(
        "[control_grants.gateway-a]\npubkey = \"{}\"\nverbs = [\"lookup_member\", \"wire\", \"unwire\", \"inject\"]\nmembers = [\"bob\"]\n",
        keys_a.pubkey_b64()
    );

    let mut gateway_a = boot(
        "A",
        state_a.path(),
        MOB_A,
        &control_a,
        &contacts_a,
        &grants_a,
    );
    let mut gateway_b = boot(
        "B",
        state_b.path(),
        MOB_B,
        &control_b,
        &contacts_b,
        &grants_b,
    );

    spawn_member(&mut gateway_a, "alice");
    spawn_member(&mut gateway_b, "bob");

    // Wire across the process boundary: A's runtime talks to B's control
    // listener over real TCP (LookupMember for bob's facts, then the Wire
    // request carrying alice's member address + pubkey).
    let response = gateway_a.call(
        "wire",
        "meta/cross_mob/wire",
        json!({
            "local_member_id": "alice",
            "remote_member_id": "bob",
            "remote_mob_id": "two-proc-b",
        }),
        RPC_TIMEOUT,
    );
    assert_eq!(response["result"]["accepted"], json!(true));

    // Both rosters project the peering - B's row is the reverse leg,
    // installed by a request that crossed the process boundary.
    let alice_peers = wired_to(&mut gateway_a, "alice");
    assert!(
        alice_peers.iter().any(|peer| peer.contains("bob")),
        "[A] alice must be wired to bob, got {alice_peers:?}"
    );
    let bob_peers = wired_to(&mut gateway_b, "bob");
    assert!(
        bob_peers.iter().any(|peer| peer.contains("alice")),
        "[B] bob must be wired to alice (reverse leg across processes), got {bob_peers:?}"
    );

    // Delivery A -> B: the response's session id is minted in B's process.
    let response = gateway_a.call(
        "send-a-to-b",
        "meta/cross_mob/send",
        json!({
            "from_member_id": "alice",
            "remote_member_id": "bob",
            "remote_mob_id": "two-proc-b",
            "content": MARKER_A_TO_B,
        }),
        RPC_TIMEOUT,
    );
    assert!(
        response["result"]["session_id"].is_string(),
        "[A] signed cross_mob/send receipt has no remote session id: {response}"
    );

    // Delivery B -> A, over B's own contact directory and A's listener.
    let response = gateway_b.call(
        "send-b-to-a",
        "meta/cross_mob/send",
        json!({
            "from_member_id": "bob",
            "remote_member_id": "alice",
            "remote_mob_id": "two-proc-a",
            "content": MARKER_B_TO_A,
        }),
        RPC_TIMEOUT,
    );
    assert!(
        response["result"]["session_id"].is_string(),
        "[B] signed cross_mob/send receipt has no remote session id: {response}"
    );

    // Unwire across the boundary and confirm both projections drop it.
    let response = gateway_a.call(
        "unwire",
        "meta/cross_mob/unwire",
        json!({
            "local_member_id": "alice",
            "remote_member_id": "bob",
            "remote_mob_id": "two-proc-b",
        }),
        RPC_TIMEOUT,
    );
    assert_eq!(response["result"]["accepted"], json!(true));
    let alice_peers = wired_to(&mut gateway_a, "alice");
    assert!(
        !alice_peers.iter().any(|peer| peer.contains("bob")),
        "[A] alice must no longer be wired to bob after unwire, got {alice_peers:?}"
    );
    let bob_peers = wired_to(&mut gateway_b, "bob");
    assert!(
        !bob_peers.iter().any(|peer| peer.contains("alice")),
        "[B] bob must no longer be wired to alice after remote unwire, got {bob_peers:?}"
    );

    // Each process returns its exact shutdown terminal before stdin closes;
    // process exit is then awaited directly rather than inferred by polling.
    gateway_a.shutdown_and_reap();
    gateway_b.shutdown_and_reap();
}
