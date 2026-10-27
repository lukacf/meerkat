//! COLD-PROCESS recovery: the reboot proof that a same-process harness
//! cannot give.
//!
//! # Why this file exists
//!
//! `identity_first_head_canonical_resume.rs` walks four "reboots" by
//! constructing and shutting down `UnifiedRuntime`s inside ONE test process.
//! That establishes durable-store behavior, but it does NOT establish cold
//! recovery, because meerkat keeps PROCESS-GLOBAL memos that outlive a
//! runtime:
//!
//! - the transcript-graph decode memo (`MEERKAT_DISABLE_GRAPH_DECODE_MEMO`
//!   is its kill switch),
//! - the digest accumulator's memo, and
//! - the slim-materialization snapshot registry that `SessionHead::from_session`
//!   seeds on the PRODUCER side of a save.
//!
//! Every one of those can hand a later same-process "boot" a decoded, digested
//! or fully materialized transcript that a real `execve` could never inherit.
//! A resume defect that only shows up when the bytes on disk are the sole
//! input therefore stays invisible in that file, by construction.
//!
//! # What this file does instead
//!
//! Each boot is a REAL OS PROCESS: the shipped `rpc_gateway --persistent`
//! binary, spawned over the same state directory, driven with the established
//! stdin-JSONL host protocol (the same idiom as
//! `gateway_concurrent_dispatch.rs` and `storage_layout_boot.rs`). The test
//! process is the SDK host: it answers `callback/roster_provider/roster` and
//! nothing else, so the gateway composes its own stores in the state dir:
//! Core's session and runtime stores, both on `runtime.sqlite` - the shipped
//! identity-first composition, not a builder arm assembled by the test.
//!
//! The LLM is `runtime_options.demo_llm` (meerkat's `TestClient`): no network,
//! no API key, deterministic single-turn replies.
//!
//! At least one boot runs with `MEERKAT_DISABLE_GRAPH_DECODE_MEMO=1` in the
//! CHILD's environment so the decode memo cannot mask a decode defect; the
//! other boots explicitly `env_remove` it, so "memo enabled" boots are
//! genuinely memo-enabled even when a developer exports it in their shell.
//!
//! # What is asserted
//!
//! Only DURABLE state, read back out of Core's session store on
//! `runtime.sqlite`, never an in-process handle:
//!
//! - the identity's session never changes across reboots (session rotation is
//!   a failure, not a recovery),
//! - the transcript a reboot inherits is EXACTLY the transcript the previous
//!   process left (element-wise JSON equality, same order, same length -
//!   restore may not mutate the durable transcript),
//! - each turn EXTENDS that transcript: the previous messages stay an exact
//!   prefix and every earlier boot's marker token is still present,
//! - the identity is `active` with no recorded wake refusal, and the
//!   gateway's shutdown handshake attests completed runtime cleanup (a wedged
//!   runtime reports `shutdown: false`).

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args
)]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use meerkat_meta::storage_layout::MetaStorageLayout;
use serde_json::{Value, json};

#[path = "support/gateway_shutdown.rs"]
mod gateway_shutdown;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// `turn_driven` so one `meta/send` is exactly one turn (the autonomous-host
/// default would keep looping and make durable message accounting
/// nondeterministic). Model and tool shape mirror the identity-first arm of
/// `gateway_concurrent_dispatch.rs`.
const MOB_CONFIG: &str = r#"
[mob]
id = "identity-first-subprocess-reboot"

[profiles.default]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.default.tools]
comms = true
"#;

const IDENTITY: &str = "agent:alice";

/// Meerkat's kill switch for the process-global transcript-graph decode memo.
const DECODE_MEMO_KILL_SWITCH: &str = "MEERKAT_DISABLE_GRAPH_DECODE_MEMO";

/// Messages one `turn_driven` turn must durably add: the user prompt and the
/// assistant reply.
const MESSAGES_PER_TURN: i64 = 2;

const INIT_TIMEOUT: Duration = Duration::from_mins(3);
const RPC_TIMEOUT: Duration = Duration::from_mins(1);
const TURN_TIMEOUT: Duration = Duration::from_mins(3);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_mins(3);
const EXIT_TIMEOUT: Duration = Duration::from_mins(1);

fn roster_spec() -> Value {
    json!({
        "identity": IDENTITY,
        "profile": "default",
        "addressability": "addressable",
        "display_name": null,
        "labels": {},
        "context": null,
        "additional_instructions": []
    })
}

// ---------------------------------------------------------------------------
// Child gateway process
// ---------------------------------------------------------------------------

/// One booted `rpc_gateway --persistent` child process, plus the reader
/// threads that pump its stdout (JSONL protocol) and stderr (diagnostics).
struct Gateway {
    label: String,
    child: Child,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<String>,
    stderr: Arc<Mutex<Vec<String>>>,
}

impl Gateway {
    /// Spawn the real binary. `disable_decode_memo` controls ONLY this
    /// child's environment; when false the variable is explicitly removed so
    /// an ambient export in the developer's shell cannot silently turn a
    /// memo-enabled boot into a memo-disabled one.
    fn start(label: &str, disable_decode_memo: bool) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rpc_gateway"));
        command
            .arg("--persistent")
            // This oracle observes the default INFO-level boot declaration.
            // An ambient RUST_LOG must not suppress the event under test.
            .env_remove("RUST_LOG")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if disable_decode_memo {
            command.env(DECODE_MEMO_KILL_SWITCH, "1");
        } else {
            command.env_remove(DECODE_MEMO_KILL_SWITCH);
        }
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

    fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Last lines the child wrote to stderr, for failure diagnosis.
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

    /// Read protocol messages until `predicate` matches; every other message
    /// is handed to `on_other` so host callbacks stay answered.
    fn wait_for(
        &mut self,
        deadline: Duration,
        mut on_other: impl FnMut(&mut Self, &Value),
        predicate: impl Fn(&Value) -> bool,
    ) -> Option<Value> {
        let start = Instant::now();
        loop {
            let remaining = deadline.checked_sub(start.elapsed())?;
            let Ok(line) = self.lines.recv_timeout(remaining) else {
                return None;
            };
            let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            if predicate(&message) {
                return Some(message);
            }
            on_other(self, &message);
        }
    }

    /// Send one request and return its response, answering host callbacks
    /// while waiting. Panics with the child's stderr tail on timeout.
    fn call(&mut self, id: &str, method: &str, params: Value, deadline: Duration) -> Value {
        let response = self.request(id, method, params, deadline);
        assert!(
            response.get("error").is_none(),
            "[{}] {method} failed: {response}\n{}",
            self.label,
            self.stderr_tail()
        );
        response
    }

    /// [`Self::call`] for a request that may be refused: the response is
    /// returned as it came, error or result.
    fn request(&mut self, id: &str, method: &str, params: Value, deadline: Duration) -> Value {
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }));
        let matched = self.wait_for(deadline, answer_host_callback, |message| {
            is_response_with_id(message, id)
        });
        matched.unwrap_or_else(|| {
            panic!(
                "[{}] no response to {method} within {:?}\n{}",
                self.label,
                deadline,
                self.stderr_tail()
            )
        })
    }

    /// The SDK shutdown handshake, then EOF, then reaping. The result is the
    /// runtime's cleanup attestation: complete, with the durable child root
    /// released and every other phase settled; a wedged runtime (drain
    /// timeout, mob stop failure, unreleased identity authority, orphaned
    /// module processes, a held child root) fails it.
    fn shutdown_and_reap(&mut self) {
        let response = self.call("shutdown", "meta/shutdown", json!({}), SHUTDOWN_TIMEOUT);
        // A persistent stock gateway stands its durable child mobs down and
        // releases their root: cleanup is complete.
        gateway_shutdown::assert_persistent_shutdown_completed(
            &response,
            &format!("[{}] shutdown\n{}", self.label, self.stderr_tail()),
        );
        self.close_stdin();
        let status = self.wait_for_exit(EXIT_TIMEOUT);
        assert!(
            status.success(),
            "[{}] rpc_gateway exited with {status}\n{}",
            self.label,
            self.stderr_tail()
        );
    }

    fn wait_for_exit(&mut self, deadline: Duration) -> ExitStatus {
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().expect("poll gateway exit") {
                return status;
            }
            if start.elapsed() >= deadline {
                let tail = self.stderr_tail();
                let _ = self.child.kill();
                let _ = self.child.wait();
                panic!(
                    "[{}] rpc_gateway did not exit within {:?}\n{}",
                    self.label, deadline, tail
                );
            }
            thread::sleep(Duration::from_millis(25));
        }
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn is_response_with_id(message: &Value, id: &str) -> bool {
    message.get("method").is_none() && message.get("id").and_then(Value::as_str) == Some(id)
}

/// The host side of the gateway's callback bridge. This harness declares ONE
/// provider (`has_roster_provider`), so the roster callback is the only one
/// that may arrive; anything else means the init params installed a bridge
/// this test does not implement, and answering it with a stub would make the
/// run meaningless.
fn answer_host_callback(gateway: &mut Gateway, message: &Value) {
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return;
    };
    // Notifications carry no id and expect no reply.
    let Some(id) = message.get("id").cloned() else {
        return;
    };
    match method {
        "callback/roster_provider/roster" => {
            gateway.send(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": [roster_spec()],
            }));
        }
        other if other.starts_with("callback/") => {
            panic!(
                "[{}] unexpected host callback {other}: this harness only provides a roster \
                 provider, so a stub answer would invalidate the run",
                gateway.label
            );
        }
        _ => {}
    }
}

/// Boot a gateway over `state`, complete the identity-first init handshake,
/// and return the live child.
fn boot(label: &str, state: &Path, disable_decode_memo: bool) -> Gateway {
    boot_with_role_migrations(label, state, disable_decode_memo, None)
}

/// `boot`, plus the boot-scoped member role migrations this activation is
/// authorized to perform. `None` is the ordinary case: no identity may migrate
/// its durable role on resume.
fn boot_with_role_migrations(
    label: &str,
    state: &Path,
    disable_decode_memo: bool,
    role_migrations: Option<Value>,
) -> Gateway {
    let mut gateway = Gateway::start(label, disable_decode_memo);
    let mut init_params = json!({
            "persistent_state": state,
            "mob_config": MOB_CONFIG,
            // Identity-first WITHOUT has_continuity_store/has_lease_provider:
            // the gateway opens its own continuity substrate in the state dir
            // and installs it as meerkat's session store. That is the shape
            // whose cold recovery is under test.
            "has_roster_provider": true,
            "runtime_options": {
                "demo_llm": true,
                "identity_bootstrap_mode": { "mode": "eager_materialize" }
            }
    });
    if let Some(declarations) = role_migrations {
        init_params["role_migrations"] = declarations;
    }
    let response = gateway.call("init", "meta/init", init_params, INIT_TIMEOUT);
    assert!(
        response["result"]["contract_version"].is_string(),
        "[{label}] identity-first init returned no contract version: {response}"
    );
    gateway
}

/// Queue one turn. `turn_driven` members process it as exactly one turn.
///
/// `content` is a bare JSON string: `ContentInput` is `#[serde(untagged)]`,
/// so its `Text` variant is the string itself (this is also exactly what the
/// Python/TypeScript SDKs put on the wire).
fn send_turn(gateway: &mut Gateway, prompt: &str) {
    let response = gateway.call(
        "send",
        "meta/send",
        json!({ "identity": IDENTITY, "content": prompt }),
        RPC_TIMEOUT,
    );
    assert!(
        response["result"]["fencing_token"].is_number(),
        "[{}] meta/send did not return a fencing token: {response}",
        gateway.label
    );
}

/// The session `meta/status_identity` names for the identity.
fn bound_session(gateway: &mut Gateway) -> String {
    let response = gateway.call(
        "status",
        "meta/status_identity",
        json!({ "identity": IDENTITY }),
        RPC_TIMEOUT,
    );
    response["result"]["session_id"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "[{}] the identity has no session: {response}",
                gateway.label
            )
        })
        .to_string()
}

/// Assert the restored identity is healthy and bound to `session_id`.
fn assert_identity_bound(gateway: &mut Gateway, session_id: &str) {
    let response = gateway.call(
        "status",
        "meta/status_identity",
        json!({ "identity": IDENTITY }),
        RPC_TIMEOUT,
    );
    let result = &response["result"];
    assert_eq!(
        result["state"],
        json!("active"),
        "[{}] restore left the identity non-active: {response}",
        gateway.label
    );
    assert_eq!(
        result["session_id"],
        json!(session_id),
        "[{}] restore rebound the identity to a different session: {response}",
        gateway.label
    );
    assert!(
        result["wake_refusal"].is_null(),
        "[{}] restore recorded a wake refusal: {response}",
        gateway.label
    );
}

// ---------------------------------------------------------------------------
// Durable state (read from disk, never from a handle)
// ---------------------------------------------------------------------------

/// The layout-authoritative runtime store path, derived rather than
/// hardcoded so a layout rename cannot turn the snapshot harness into a
/// silent no-op.
fn runtime_db(state: &Path) -> PathBuf {
    MetaStorageLayout::with_injected_roots(state.to_path_buf(), None).runtime_db()
}

/// The durable transcript, reconstructed exactly the way the store's own
/// reader does it: the single head row names a strand and a message count,
/// and the transcript is that strand's `0..message_count` prefix.
#[derive(Debug, Clone)]
struct DurableTranscript {
    session_id: String,
    strand: String,
    message_count: i64,
    messages: Vec<Value>,
}

impl DurableTranscript {
    /// Raw JSON of every durable message, for marker-token containment.
    fn joined_json(&self) -> String {
        self.messages
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Read `session_id`'s durable transcript from Core's session store (it shares
/// `runtime.sqlite` with the runtime store), failing loudly when Core holds no
/// such session.
fn read_durable_transcript(state: &Path, session_id: &str, phase: &str) -> DurableTranscript {
    use meerkat::SessionStore as _;
    use meerkat_core::session_store::IncrementalSessionStore as _;
    let store = meerkat_store::SqliteSessionStore::open(runtime_db(state))
        .unwrap_or_else(|error| panic!("({phase}) open Core's session store: {error}"));
    let id = meerkat_core::types::SessionId::parse(session_id).expect("a session id");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime for the store read");
    let (session, head) = runtime.block_on(async {
        let session = store
            .load(&id)
            .await
            .unwrap_or_else(|error| panic!("({phase}) read session {session_id}: {error}"));
        let head = store
            .load_canonical_head(&id)
            .await
            .unwrap_or_else(|error| panic!("({phase}) read the head of {session_id}: {error}"));
        (session, head)
    });
    let session = session
        .unwrap_or_else(|| panic!("({phase}) Core's session store holds no session {session_id}"));
    let messages = session
        .messages()
        .iter()
        .map(|message| serde_json::to_value(message).expect("a durable message encodes"))
        .collect::<Vec<_>>();
    DurableTranscript {
        session_id: session_id.to_string(),
        strand: head.map(|head| head.strand.to_string()).unwrap_or_default(),
        message_count: i64::try_from(messages.len()).expect("a message count"),
        messages,
    }
}

/// Turn-boundary barrier: block until Core's durable session reaches `floor`
/// messages, so that turn is durable.
fn wait_for_durable_messages(state: &Path, session_id: &str, floor: i64, what: &str) {
    let start = Instant::now();
    loop {
        let count = read_durable_transcript(state, session_id, what).message_count;
        if count >= floor {
            return;
        }
        assert!(
            start.elapsed() < TURN_TIMEOUT,
            "timed out waiting for {what}: want >= {floor} durable messages in {session_id}, \
             observed {count}"
        );
        thread::sleep(Duration::from_millis(150));
    }
}

/// The exact-survival assertion: `previous` must still be there, unchanged,
/// in order, as a prefix of `current`.
fn assert_exact_prefix(previous: &DurableTranscript, current: &DurableTranscript, phase: &str) {
    assert_eq!(
        previous.session_id, current.session_id,
        "({phase}) the durable session was ROTATED across the process boundary \
         ({} -> {}); a new session is data loss, not recovery",
        previous.session_id, current.session_id
    );
    assert!(
        current.messages.len() >= previous.messages.len(),
        "({phase}) the durable transcript SHRANK: {} -> {} messages",
        previous.messages.len(),
        current.messages.len()
    );
    for (index, before) in previous.messages.iter().enumerate() {
        assert_eq!(
            &current.messages[index], before,
            "({phase}) durable message {index} changed across the boundary \
             (session {}, strand {} -> {})",
            current.session_id, previous.strand, current.strand
        );
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Three real `rpc_gateway` processes over one state directory. Every boot
/// after the first starts with an empty address space: no decode memo, no
/// digest memo, no producer-seeded materialization snapshot. The transcript
/// and the identity binding must survive on the strength of the bytes on disk
/// alone.
#[test]
fn core_session_survives_real_process_restarts() {
    // `(label, disable the decode memo in this child)`. Boot 2 — the first
    // resume, and the one that must decode a transcript it did not write —
    // runs with the memo off. Boot 3 runs with it on, so the shipped
    // configuration is covered too.
    const BOOTS: [(&str, bool); 3] = [
        ("boot-1/memo-on", false),
        ("boot-2/memo-off", true),
        ("boot-3/memo-on", false),
    ];
    let tokens = [
        "MARKER-COLD-BOOT-1-ALFA",
        "MARKER-COLD-BOOT-2-BRAVO",
        "MARKER-COLD-BOOT-3-CHARLIE",
    ];

    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    std::fs::create_dir_all(&state).expect("create state dir");

    let mut previous: Option<DurableTranscript> = None;

    for (index, (label, disable_memo)) in BOOTS.iter().enumerate() {
        let mut gateway = boot(label, &state, *disable_memo);
        // Each boot really is an `execve`, not a fresh runtime inside this
        // address space — which is the whole point of the file.
        assert_ne!(
            gateway.pid(),
            std::process::id(),
            "({label}) the gateway must run as a separate OS process"
        );

        // Pre-turn: what this cold process INHERITED must be exactly what the
        // previous process left behind.
        if let Some(before) = previous.as_ref() {
            let inherited =
                read_durable_transcript(&state, &before.session_id, &format!("{label} pre-turn"));
            assert_eq!(
                inherited.message_count, before.message_count,
                "({label}) restore mutated the durable transcript length before any turn: \
                 {} -> {}",
                before.message_count, inherited.message_count
            );
            assert_exact_prefix(before, &inherited, &format!("{label} pre-turn"));
            assert_eq!(
                inherited.messages.len(),
                before.messages.len(),
                "({label}) restore appended messages before any turn"
            );
            assert_identity_bound(&mut gateway, &before.session_id);
        }

        let floor = previous
            .as_ref()
            .map_or(MESSAGES_PER_TURN, |t| t.message_count + MESSAGES_PER_TURN);
        send_turn(
            &mut gateway,
            &format!("Please note this token: {}", tokens[index]),
        );
        let session_id = match previous.as_ref() {
            Some(before) => before.session_id.clone(),
            None => bound_session(&mut gateway),
        };
        wait_for_durable_messages(&state, &session_id, floor, &format!("{label}'s turn"));
        gateway.shutdown_and_reap();

        // Post-exit: read the durable file with the writer gone.
        let after = read_durable_transcript(&state, &session_id, &format!("{label} post-turn"));
        eprintln!(
            "PROBE {label}: session={} strand={} messages={}",
            after.session_id, after.strand, after.message_count
        );
        assert!(
            after.message_count >= floor,
            "({label}) the turn did not extend the durable head: {} < {floor}",
            after.message_count
        );
        let joined = after.joined_json();
        for token in tokens.iter().take(index + 1).copied() {
            assert!(
                joined.contains(token),
                "({label}) the durable transcript lost marker {token} — a reboot dropped or \
                 replaced earlier turns. Durable messages: {}",
                after.message_count
            );
        }
        if let Some(before) = previous.as_ref() {
            assert_exact_prefix(before, &after, &format!("{label} post-turn"));
        }
        previous = Some(after);
    }

    let final_transcript = previous.expect("three boots produced a transcript");
    assert!(
        final_transcript.message_count >= BOOTS.len() as i64 * MESSAGES_PER_TURN,
        "after {} process restarts the durable head should hold one user+assistant pair per \
         boot, got {}",
        BOOTS.len(),
        final_transcript.message_count
    );
}

// ---------------------------------------------------------------------------
// Runtime-store snapshot harness (second test)
// ---------------------------------------------------------------------------

/// Wait for a stderr line: the reader thread pumps asynchronously, so a bare
/// read races the boot that produced the line.
fn wait_for_stderr(gateway: &Gateway, needle: &str, deadline: Duration) -> bool {
    let started = std::time::Instant::now();
    while started.elapsed() < deadline {
        if gateway.stderr_tail().contains(needle) {
            return true;
        }
        thread::sleep(Duration::from_millis(50));
    }
    false
}

/// A role migration declaration is BOOT-scoped: the next boot that declares
/// nothing has no migration authority.
///
/// This is the property that makes a leftover declaration safe, and it can only
/// fail on a LATER boot, so it is proved across two real OS processes sharing
/// one durable state dir rather than by inspecting a map in-process.
///
/// Boot 1 also covers the inert case end to end: `from_role` names a
/// predecessor this identity never had, and because the identity's stored role
/// still equals its current role, Meerkat returns before the declaration is
/// ever read. Materialization therefore proceeds exactly as it does without
/// one, which is why boot 1 is expected to come up healthy.
///
/// The two assertions cannot both pass vacuously: if the gateway stopped
/// recording declarations altogether, boot 1 fails. So a green boot-2 means
/// absence, not silence.
#[test]
fn a_role_migration_declaration_does_not_survive_into_the_next_boot() {
    const DECLARED: &str = "activation declares a member role migration";

    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    std::fs::create_dir_all(&state).expect("create state dir");

    let mut declaring = boot_with_role_migrations(
        "role-migration/boot-1",
        &state,
        false,
        Some(json!([{ "identity": IDENTITY, "from_role": "predecessor" }])),
    );
    assert!(
        wait_for_stderr(&declaring, DECLARED, Duration::from_secs(30)),
        "[boot-1] the gateway must record the declaration it was armed with: {}",
        declaring.stderr_tail()
    );
    declaring.shutdown_and_reap();

    let mut fresh = boot("role-migration/boot-2", &state, false);
    // Boot 2 has completed its init handshake, which is the same phase that
    // emitted the line on boot 1, so the line would already be here if it were
    // coming. The settle window covers only the stderr reader thread.
    thread::sleep(Duration::from_millis(500));
    assert!(
        !fresh.stderr_tail().contains(DECLARED),
        "[boot-2] a boot that declared nothing must arm nothing, but the \
         declaration came back from durable state: {}",
        fresh.stderr_tail()
    );
    fresh.shutdown_and_reap();
}

// ---------------------------------------------------------------------------
// Roster callback context at the SDK boundary
// ---------------------------------------------------------------------------

const ROSTER_CALLBACK: &str = "callback/roster_provider/roster";

/// `Gateway::call`, but every `callback/roster_provider/roster` request the
/// gateway makes while the call is in flight is appended to `recorded`, params
/// exactly as they crossed the wire. This is the host's view: what the SDK
/// dispatchers hand to the user's roster callback is `params.context`.
fn call_recording_roster_callbacks(
    gateway: &mut Gateway,
    id: &str,
    method: &str,
    params: Value,
    deadline: Duration,
    recorded: &mut Vec<Value>,
) -> Value {
    gateway.send(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    }));
    let matched = gateway.wait_for(
        deadline,
        |gateway, message| {
            if message.get("method").and_then(Value::as_str) == Some(ROSTER_CALLBACK) {
                recorded.push(message.get("params").cloned().unwrap_or(Value::Null));
            }
            answer_host_callback(gateway, message);
        },
        |message| is_response_with_id(message, id),
    );
    let Some(response) = matched else {
        panic!(
            "[{}] no response to {method} within {:?}\n{}",
            gateway.label,
            deadline,
            gateway.stderr_tail()
        );
    };
    assert!(
        response.get("error").is_none(),
        "[{}] {method} failed: {response}\n{}",
        gateway.label,
        gateway.stderr_tail()
    );
    response
}

/// The `RosterContext` the SDK callback receives, or a panic naming the
/// defect this test guards: the gateway used to serialize the context as the
/// params root, so `params.context` (what Python and TypeScript read) was `{}`.
fn roster_callback_context(params: &Value) -> &Value {
    assert!(
        params.get("mob_definition").is_none() && params.get("previous_identities").is_none(),
        "roster context was flattened into the params root; the SDK dispatchers read \
         `params.context` and would receive an empty context: {params}"
    );
    params
        .get("context")
        .unwrap_or_else(|| panic!("roster callback params carry no `context` envelope: {params}"))
}

/// Regression for the SDK-reported defect that the roster callback hosted by
/// Python/TypeScript received an empty context. Drives the real binary and
/// asserts on the wire, so it proves what an SDK host sees, not what the Rust
/// bridge intends: the bootstrap roster calls carry the mob definition (with
/// nothing registered yet, no previous identities), and a later topology query
/// re-asks the roster with the identity the runtime has since registered.
#[test]
fn the_roster_callback_receives_the_mob_definition_and_previous_identities() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    std::fs::create_dir_all(&state).expect("create state dir");

    let mut gateway = Gateway::start("roster-context", false);
    let mut recorded = Vec::new();
    let response = call_recording_roster_callbacks(
        &mut gateway,
        "init",
        "meta/init",
        json!({
            "persistent_state": state,
            "mob_config": MOB_CONFIG,
            "has_roster_provider": true,
            "runtime_options": {
                "demo_llm": true,
                "identity_bootstrap_mode": { "mode": "eager_materialize" }
            }
        }),
        INIT_TIMEOUT,
        &mut recorded,
    );
    assert!(
        response["result"]["contract_version"].is_string(),
        "[roster-context] identity-first init returned no contract version: {response}"
    );
    assert!(
        !recorded.is_empty(),
        "[roster-context] init never resolved the roster through the host callback"
    );
    for params in &recorded {
        let context = roster_callback_context(params);
        assert_eq!(
            context["mob_definition"]["id"],
            json!("identity-first-subprocess-reboot"),
            "[roster-context] bootstrap roster context lacks the mob definition: {params}"
        );
        assert!(
            context["mob_definition"]["profiles"]["default"].is_object(),
            "[roster-context] mob definition profiles did not reach the host: {params}"
        );
        assert!(
            context["previous_identities"].is_array(),
            "[roster-context] previous_identities must be a list, even when empty: {params}"
        );
    }
    // The first resolve runs before anything is registered, so the runtime has
    // no previous identities to report there; the field is present but empty.
    assert_eq!(
        recorded[0]["context"]["previous_identities"],
        json!([]),
        "[roster-context] the pre-registration roster call must report no previous identities"
    );

    // A topology query re-derives the desired roster with the identities the
    // runtime has registered since bootstrap, so this is where the SDK
    // callback must see the previously known identity.
    recorded.clear();
    call_recording_roster_callbacks(
        &mut gateway,
        "topology",
        "meta/topology/query",
        json!({}),
        RPC_TIMEOUT,
        &mut recorded,
    );
    let params = recorded.last().unwrap_or_else(|| {
        panic!(
            "[roster-context] the identity-first topology query did not consult the roster\n{}",
            gateway.stderr_tail()
        )
    });
    let context = roster_callback_context(params);
    assert_eq!(
        context["mob_definition"]["id"],
        json!("identity-first-subprocess-reboot"),
        "[roster-context] topology roster context lacks the mob definition: {params}"
    );
    assert_eq!(
        context["previous_identities"],
        json!([IDENTITY]),
        "[roster-context] the registered identity did not reach the roster callback: {params}"
    );

    gateway.shutdown_and_reap();
}

// ---------------------------------------------------------------------------
// First boot of a migrated state directory
// ---------------------------------------------------------------------------

/// Remove Core's mob store from `state`, with its sidecars: the shape of a
/// directory `storage-migrate-native` converted, whose mob journal 0.9.3
/// kept in memory. Core's session store keeps every session; no member
/// binding names one.
fn remove_mob_store(state: &Path) {
    let mob_store = MetaStorageLayout::with_injected_roots(state.to_path_buf(), None)
        .event_log_db()
        .file_name()
        .and_then(|name| name.to_str())
        .expect("the mob store has a file name")
        .to_string();
    let mut removed = 0;
    for entry in std::fs::read_dir(state).expect("read the state directory") {
        let entry = entry.expect("a state entry");
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(&mob_store))
        {
            std::fs::remove_file(entry.path()).expect("remove the mob store");
            removed += 1;
        }
    }
    assert!(
        removed > 0,
        "the persistent launch kept no mob store in {}",
        state.display()
    );
}

/// A gateway-shaped state directory `storage-migrate-native` converted: the
/// identity's session is in Core's session store, Core's mob holds no
/// binding of it, and the first-boot manifest's `Resume` row names that
/// session. The stdin gateway's first roster compile carries the row out:
/// the identity resumes the session, keeps its transcript, and the manifest
/// is removed. A gateway that never reads the manifest wakes the identity
/// fresh on a new session, and the migrated transcript stays unbound.
#[test]
fn the_stdin_gateway_carries_out_a_migrated_state_dirs_first_boot_resume_row() {
    const TOKEN: &str = "MARKER-FIRST-BOOT-RESUME-DELTA";
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    std::fs::create_dir_all(&state).expect("create state dir");

    let mut gateway = boot("first-boot/written", &state, false);
    send_turn(&mut gateway, &format!("Please note this token: {TOKEN}"));
    let session_id = bound_session(&mut gateway);
    wait_for_durable_messages(&state, &session_id, MESSAGES_PER_TURN, "the migrated turn");
    gateway.shutdown_and_reap();

    remove_mob_store(&state);
    meerkat_meta::identity_first::FirstBootManifest::write_new(
        &state,
        vec![meerkat_meta::identity_first::FirstBootRow::Resume {
            identity: meerkat_meta::identity_first::AgentIdentity::parse(IDENTITY)
                .expect("the identity parses"),
            session: meerkat_core::types::SessionId::parse(&session_id)
                .expect("the session id parses"),
        }],
    )
    .expect("storage-migrate-native writes the first-boot manifest");

    let mut gateway = boot("first-boot/carried-out", &state, false);
    assert_identity_bound(&mut gateway, &session_id);
    assert!(
        !meerkat_meta::identity_first::FirstBootManifest::path(&state).exists(),
        "the manifest is removed once its last row is carried out"
    );
    let migrated = read_durable_transcript(&state, &session_id, "first-boot resume");
    assert!(
        migrated.joined_json().contains(TOKEN),
        "the resumed session keeps its migrated transcript"
    );
    send_turn(&mut gateway, "Which token did I give you?");
    wait_for_durable_messages(
        &state,
        &session_id,
        migrated.message_count + MESSAGES_PER_TURN,
        "the first turn after the carry-out",
    );
    gateway.shutdown_and_reap();
}

/// A first-boot manifest the gateway cannot read may hold a row for any
/// identity, so every identity is refused typed until it reads. That holds
/// for an identity whose Core member survived the restart too: its delivery
/// would otherwise land on the session the unread row may retire. The
/// gateway stays up and its bootstrap status names the file.
#[test]
fn the_stdin_gateway_refuses_every_identity_while_its_first_boot_manifest_is_unreadable() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    std::fs::create_dir_all(&state).expect("create state dir");

    let mut gateway = boot("unreadable/written", &state, false);
    bound_session(&mut gateway);
    gateway.shutdown_and_reap();

    let manifest = meerkat_meta::identity_first::FirstBootManifest::path(&state);
    std::fs::write(
        &manifest,
        br#"{"format":"meerkat-meta.first-boot-manifest","version":2,"rows_sha256":"00","rows":[]}"#,
    )
    .expect("write a manifest from a later format version");

    let mut gateway = boot("unreadable/refused", &state, false);
    let status = gateway.call(
        "bootstrap",
        "meta/status_identity_bootstrap",
        json!({}),
        RPC_TIMEOUT,
    );
    let error = status["result"]["error"].as_str().unwrap_or_default();
    assert!(
        error.contains(&manifest.display().to_string()),
        "the bootstrap status names the unreadable manifest: {status}"
    );
    let refused = gateway.request(
        "send",
        "meta/send",
        json!({ "identity": IDENTITY, "content": "Are you there?" }),
        RPC_TIMEOUT,
    );
    assert_eq!(
        refused["error"]["code"],
        json!(-32014),
        "a send is refused with the storage-resolution code: {refused}"
    );
    assert_eq!(
        refused["error"]["data"]["kind"],
        json!("first_boot_manifest_unreadable"),
        "the refusal names the unreadable manifest: {refused}"
    );
    gateway.shutdown_and_reap();
}

// ---------------------------------------------------------------------------
// Offline identity reset
// ---------------------------------------------------------------------------

/// Run `meta_gateway identity-reset --json` for the identity over `state`
/// and return its report, asserting the exit code (0 done, 1 refused).
fn identity_reset(state: &Path, apply: bool, exit_code: i32) -> Value {
    identity_reset_of(state, IDENTITY, apply, exit_code)
}

/// [`identity_reset`] for `identity`.
fn identity_reset_of(state: &Path, identity: &str, apply: bool, exit_code: i32) -> Value {
    let mut command = Command::new(env!("CARGO_BIN_EXE_meta_gateway"));
    command
        .arg("identity-reset")
        .arg("--state-dir")
        .arg(state)
        .args(["--identity", identity, "--json"]);
    if apply {
        command.arg("--apply");
    }
    let output = command.output().expect("run meta_gateway identity-reset");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!("identity-reset printed no JSON report ({error}); stderr: {stderr}")
    });
    assert_eq!(
        output.status.code(),
        Some(exit_code),
        "identity-reset exit code; report: {report}; stderr: {stderr}"
    );
    report
}

/// `meta_gateway identity-reset` gives an identity of a stopped directory a
/// fresh life at its next start. It is refused while a gateway runs on the
/// directory, and for an identity Core binds to nothing that owes its host a
/// held completion; a dry run writes nothing and reports a held-completion
/// mark of the bound identity as stale; `--apply` leaves one `Fresh` row
/// naming the session of the identity's live binding, and applying it again
/// finds the row pending. The next boot drops the stale mark and carries the
/// row out: the identity answers on a new session that does not inherit the
/// old transcript, the manifest is removed, and Core keeps the old session,
/// its binding retired.
#[test]
fn the_identity_reset_verb_gives_a_stopped_directorys_identity_a_fresh_session() {
    use sha2::Digest as _;
    const TOKEN: &str = "MARKER-IDENTITY-RESET-OLD-LIFE";
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state = temp.path().join("state");
    std::fs::create_dir_all(&state).expect("create state dir");
    let manifest = meerkat_meta::identity_first::FirstBootManifest::path(&state);

    let mut gateway = boot("identity-reset/old-life", &state, false);
    send_turn(&mut gateway, &format!("Please note this token: {TOKEN}"));
    let old_session = bound_session(&mut gateway);
    wait_for_durable_messages(
        &state,
        &old_session,
        MESSAGES_PER_TURN,
        "the old life's turn",
    );
    let running = identity_reset(&state, true, 1);
    assert_eq!(
        (
            &running["refusal"]["kind"],
            &running["refusal"]["refusal"]["kind"]
        ),
        (&json!("directory"), &json!("runtime_in_use")),
        "the reset is refused while a gateway runs on the directory: {running}"
    );
    gateway.shutdown_and_reap();
    assert!(!manifest.exists(), "a refused reset writes no manifest");

    // A marked identity Core binds to nothing owes its host a held
    // completion: there is no life to reset.
    const OWED: &str = "agent:held-owed";
    let marks = state.join(meerkat_meta::identity_first::HELD_COMPLETION_OWED_DIR);
    std::fs::create_dir_all(&marks).expect("create the held-completion marks");
    let owed_mark = marks.join(format!("{:x}", sha2::Sha256::digest(OWED.as_bytes())));
    std::fs::write(&owed_mark, OWED).expect("mark an unbound identity owed");
    let owed = identity_reset_of(&state, OWED, true, 1);
    assert_eq!(
        owed["refusal"]["kind"],
        json!("held_completion_owed"),
        "an unbound identity owing a held completion is refused: {owed}"
    );
    assert!(!manifest.exists(), "a refused reset writes no manifest");
    std::fs::remove_file(&owed_mark).expect("clear the owed mark");

    // A mark left by a held activation that stopped before clearing it: Core
    // binds the identity, so the mark is stale.
    let mark = marks.join(format!("{:x}", sha2::Sha256::digest(IDENTITY.as_bytes())));
    std::fs::write(&mark, IDENTITY).expect("leave a stale held-completion mark");

    let dry_run = identity_reset(&state, false, 0);
    assert_eq!(
        (
            &dry_run["retired_session"],
            &dry_run["binding_state"],
            &dry_run["written"],
            &dry_run["stale_held_completion_mark"]
        ),
        (
            &json!(old_session),
            &json!("live"),
            &json!(false),
            &json!(true)
        ),
        "the dry run names the session it would retire and the stale mark: {dry_run}"
    );
    assert!(!manifest.exists(), "a dry run writes no manifest");

    let applied = identity_reset(&state, true, 0);
    assert_eq!(
        (&applied["retired_session"], &applied["written"]),
        (&json!(old_session), &json!(true)),
        "the applied reset writes the row: {applied}"
    );
    let again = identity_reset(&state, true, 0);
    assert_eq!(
        (&again["written"], &again["already_pending"]),
        (&json!(false), &json!(true)),
        "a second reset finds the same row pending: {again}"
    );
    let written = meerkat_meta::identity_first::FirstBootManifest::read(&state)
        .expect("the manifest reads")
        .expect("the reset wrote the manifest");
    assert_eq!(
        written.rows(),
        [meerkat_meta::identity_first::FirstBootRow::Fresh {
            identity: meerkat_meta::identity_first::AgentIdentity::parse(IDENTITY)
                .expect("the identity parses"),
            retired_session: meerkat_core::types::SessionId::parse(&old_session)
                .expect("the session id parses"),
        }],
        "the manifest holds one Fresh row naming the old session"
    );

    let mut gateway = boot("identity-reset/fresh-life", &state, false);
    let fresh_session = bound_session(&mut gateway);
    assert_ne!(
        fresh_session, old_session,
        "the reset identity starts on a new session"
    );
    assert!(
        !manifest.exists(),
        "the manifest is removed once its last row is carried out"
    );
    assert!(!mark.exists(), "the next start drops the stale mark");
    send_turn(&mut gateway, "Which token did I give you?");
    wait_for_durable_messages(
        &state,
        &fresh_session,
        MESSAGES_PER_TURN,
        "the fresh life's first turn",
    );
    let fresh = read_durable_transcript(&state, &fresh_session, "fresh life");
    assert!(
        !fresh.joined_json().contains(TOKEN),
        "the fresh life does not inherit the old transcript"
    );
    gateway.shutdown_and_reap();

    let old = read_durable_transcript(&state, &old_session, "old life");
    assert!(
        old.joined_json().contains(TOKEN),
        "Core keeps the old session"
    );
    let history = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime for the history read")
        .block_on(meerkat_mob::MobStorage::read_member_binding_history(
            MetaStorageLayout::with_injected_roots(state, None).event_log_db(),
        ))
        .expect("read Core's member history");
    let core_identity = meerkat_meta::member_comms_id::roster_member_id_for_identity(IDENTITY);
    assert_eq!(
        history
            .last_binding(&core_identity)
            .and_then(|binding| binding.session_id())
            .map(ToString::to_string),
        Some(fresh_session),
        "Core's last binding of the identity is the fresh session"
    );
    assert_eq!(
        history
            .binding_for_session(
                &meerkat_core::types::SessionId::parse(&old_session)
                    .expect("the session id parses")
            )
            .map(meerkat_mob::MemberBinding::state),
        Some(meerkat_mob::MemberBindingState::Retired),
        "the old session is bound to nothing"
    );
}
