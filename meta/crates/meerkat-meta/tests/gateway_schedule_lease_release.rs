#![cfg(unix)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args
)]

//! Regression: a graceful gateway exit must RELEASE the schedule executor
//! lease, in BOTH gateway binaries.
//!
//! Both binaries bound `_schedule_host` and relied on `Drop`. Dropping a
//! `ScheduleHostHandle` only SIGNALS the supervisor; only
//! `ScheduleHostHandle::shutdown().await` calls
//! `driver.release_executor_lease()`, and tokio gives no guarantee the woken
//! supervisor is polled between main's completion and runtime teardown. The
//! lease row was measured still held (owner_id, lease_token, acquired_at_ms and
//! expires_at_ms all set) 57s past a clean exit on a persistent store.
//!
//! Cost: the replacement process gets
//! `AcquireScheduleExecutorLeaseOutcome::Busy`, its tick returns without
//! claiming due occurrences, and schedules do not fire for up to the 60s lease
//! duration after every restart. The claim watchdog cannot see it - its overdue
//! threshold is longer than the window.
//!
//! Both binaries are covered on purpose. `rpc_gateway` is the one with a live
//! consumer, and the two differ in store layout, init params and composition
//! order, so a fix verified on one proves nothing about the other. The first
//! draft of this test exercised `meta_gateway` alone.
//!
//! Nothing in the type system enforces the call: the previous code compiled
//! clean and warned nothing. This test observes the SQLite row, which is the
//! only place the defect was ever visible.
//!
//! WHAT EACH LEG IS AND IS NOT PROVEN TO CATCH. Reverting the fix in both
//! binaries and re-running:
//!
//!     rpc_gateway       FAILS, on the exact assertion, first try
//!     meta_gateway    PASSES, 7 runs out of 7
//!
//! Fixed, it is green on repeats rather than on a sample: 6 local runs out of 6,
//! plus both tests inside the full CI suite (0.8s and 2.4s there, against a 60s
//! acquisition budget). A separate measurement put the neighbouring shutdown race at 4 hangs
//! in 6 runs on identical Rust, so a single green run on any gateway teardown
//! path is worth about as much as a coin flip; repeat, do not sample.
//!
//! So only the `rpc_gateway` leg is a mutation-proven guard for this defect.
//! Under `meta_gateway`'s teardown the signalled supervisor evidently does
//! get polled in time, and `Drop` releases the lease anyway - which is luck
//! this harness cannot take away, not a property the code states. The
//! `meta_gateway` leg is kept because the assertion is true and cheap and
//! would still catch a hard regression (a lease never released at all), but it
//! must not be read as evidence that the `shutdown().await` call is what makes
//! that binary correct. Do not delete the `rpc_gateway` leg on the theory that
//! the two are redundant. They are not.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tempfile::TempDir;

const MOB_CONFIG: &str = r#"
[mob]
id = "schedule-lease-release-test"

[profiles.default]
model = "gpt-5.5"
external_addressable = true
"#;

/// `owner_id` of the singleton executor lease row, or `None` when released.
///
/// Opened read-only against a live writer: the store runs in WAL mode, so this
/// never blocks the gateway and never creates the file. A plain
/// `Connection::open` would manufacture an empty database and turn "the gateway
/// never wrote a lease" into a silent pass.
fn lease_owner(schedule_db: &Path) -> Option<String> {
    if !schedule_db.exists() {
        return None;
    }
    let conn = rusqlite::Connection::open_with_flags(
        schedule_db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open schedule store read-only");
    conn.query_row(
        "SELECT owner_id FROM schedule_executor_lease WHERE singleton=1",
        [],
        |row| row.get::<_, Option<String>>(0),
    )
    .expect("read executor lease row")
}

fn poll_until<F: FnMut() -> bool>(deadline: Duration, mut predicate: F) -> bool {
    let started = Instant::now();
    while started.elapsed() < deadline {
        if predicate() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    predicate()
}

/// One gateway binary's boot recipe. The two binaries take different init
/// params and resolve the schedule store through different layout
/// constructors, so the recipe is data rather than a second copy of the test.
struct GatewayCase {
    label: &'static str,
    bin: &'static str,
    persistent_flag: bool,
}

impl GatewayCase {
    fn meta_gateway() -> Self {
        Self {
            label: "meta_gateway",
            bin: env!("CARGO_BIN_EXE_meta_gateway"),
            persistent_flag: false,
        }
    }

    fn rpc_gateway() -> Self {
        Self {
            label: "rpc_gateway",
            bin: env!("CARGO_BIN_EXE_rpc_gateway"),
            persistent_flag: true,
        }
    }

    /// Init params plus the schedule store the gateway will resolve for them.
    ///
    /// The paths come from Meta's own layout authority, not a second
    /// spelling of "schedule.sqlite": a hardcoded filename here would keep
    /// passing after a rename while observing a file nobody writes.
    fn init_params(&self, workspace: &Path, state: &Path) -> (Value, PathBuf) {
        match self.label {
            "meta_gateway" => {
                let store_path = state.join("store");
                let schedule_db = meerkat_meta::MetaStorageLayout::standalone_from_store_path(
                    &store_path,
                    state.join("gateway-home"),
                )
                .schedule_db();
                (
                    json!({
                        "workspace_root": workspace.to_string_lossy(),
                        "store_path": store_path.to_string_lossy(),
                        // The schedule host is only built on the persistent
                        // path; ephemeral has no schedule store to leak.
                        "persistent_sessions": true,
                    }),
                    schedule_db,
                )
            }
            _ => {
                let schedule_db =
                    meerkat_meta::MetaStorageLayout::with_injected_roots(state.to_path_buf(), None)
                        .schedule_db();
                (
                    json!({
                        "persistent_state": state.to_string_lossy(),
                        "mob_config": MOB_CONFIG,
                    }),
                    schedule_db,
                )
            }
        }
    }
}

/// Boot one gateway, prove it holds the lease, SIGTERM it, prove it released.
///
/// Every bound below is sized against nextest's hard ceiling for this profile
/// (`slow-timeout = { period = "60s", terminate-after = 5 }`, so 300s), not
/// picked for comfort. A test killed at 300s reports a nextest timeout, which
/// REPLACES this test's own self-describing message with one that names
/// nothing. Worst-case path: 90 + 60 + 60 + 20 = 230s. Do not raise one of
/// these without subtracting from another or adding a per-binary
/// `slow-timeout` override - a bigger poll that gets guillotined buys nothing.
/// See the `gateway_schedule_lease_release` note in `.config/nextest.toml`.
fn assert_releases_lease_on_graceful_exit(case: GatewayCase) {
    let workspace = TempDir::new().expect("workspace tempdir");
    let state = TempDir::new().expect("state tempdir");
    let (params, schedule_db) = case.init_params(workspace.path(), state.path());

    let init = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "meta/init",
        "params": params,
    });

    let mut command = Command::new(case.bin);
    if case.persistent_flag {
        command.arg("--persistent");
    }
    let mut child = command
        .current_dir(workspace.path())
        // Created at bootstrap, never called: bootstrap precedes any provider
        // request, so placeholders keep this key-independent and CI-safe.
        .env("ANTHROPIC_API_KEY", "sk-ant-regression-test")
        .env("OPENAI_API_KEY", "sk-regression-test")
        // The default ~/.local/state/meerkat-meta is shared across concurrent
        // test processes and intermittently fails the peer-key mint.
        .env("XDG_STATE_HOME", workspace.path().join("xdg-state"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap_or_else(|error| panic!("[{}] spawn: {}", case.label, error));

    let mut stdin = child.stdin.take().expect("stdin");
    writeln!(stdin, "{}", serde_json::to_string(&init).unwrap()).expect("write init");
    stdin.flush().expect("flush");

    let stdout = child.stdout.take().expect("stdout");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let _ = reader.read_line(&mut line);
        let _ = tx.send(line);
    });
    let line = match rx.recv_timeout(Duration::from_secs(90)) {
        Ok(line) => line,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            panic!("[{}] no meta/init answer within 90s: {}", case.label, error);
        }
    };
    let response: Value = serde_json::from_str(line.trim())
        .unwrap_or_else(|error| panic!("[{}] non-JSON init {:?}: {}", case.label, line, error));
    assert!(
        response.get("result").is_some(),
        "[{}] meta/init failed, so no schedule host ran: {}",
        case.label,
        response
    );

    // Leg 1. Without this the test is unfalsifiable: a gateway that never
    // acquired a lease also reads owner_id NULL at the end, and leg 2 would
    // pass while observing nothing.
    let acquired = poll_until(Duration::from_mins(1), || {
        lease_owner(&schedule_db).is_some()
    });
    if !acquired {
        let _ = child.kill();
        let _ = child.wait();
        panic!(
            "[{}] the running gateway never held the schedule executor lease at {} - this \
             test no longer observes the schedule host and cannot detect the leak it guards",
            case.label,
            schedule_db.display()
        );
    }

    // Leg 2. SIGTERM is the production trigger (a container stop), and both
    // binaries select on `shutdown_signal`, which covers SIGINT and SIGTERM.
    // A `kill()` would skip the graceful path and prove nothing.
    let pid = child.id().to_string();
    let signalled = Command::new("kill")
        .args(["-TERM", &pid])
        .status()
        .expect("send SIGTERM");
    assert!(
        signalled.success(),
        "[{}] SIGTERM to {} failed",
        case.label,
        pid
    );
    drop(stdin);

    // The lease clearing is the observable under test; process exit is a
    // separate fact. Polling the row rather than gating on exit keeps a
    // shutdown wedge (an open, unrelated investigation) from masquerading as a
    // lease leak, and vice versa.
    let released = poll_until(Duration::from_mins(1), || {
        lease_owner(&schedule_db).is_none()
    });
    let exited = poll_until(Duration::from_secs(20), || {
        matches!(child.try_wait(), Ok(Some(_)))
    });
    let owner = lease_owner(&schedule_db);
    reap(&mut child);

    // `lease_owner` reports None for a missing file, so prove the subject of
    // the assertion still exists rather than reading a released lease off a
    // database that went away.
    assert!(
        schedule_db.exists(),
        "[{}] schedule store vanished during shutdown: {}",
        case.label,
        schedule_db.display()
    );
    assert!(
        released && owner.is_none(),
        "[{}] schedule executor lease still held by {:?} after a graceful exit (process \
         exited: {}): the binary dropped the ScheduleHostHandle instead of awaiting \
         shutdown(), so the next process is refused the lease and schedules stop firing \
         for up to 60s",
        case.label,
        owner,
        exited
    );
}

fn reap(child: &mut Child) {
    if matches!(child.try_wait(), Ok(None)) {
        let _ = child.kill();
    }
    let _ = child.wait();
}

#[test]
fn meta_gateway_releases_the_schedule_executor_lease() {
    assert_releases_lease_on_graceful_exit(GatewayCase::meta_gateway());
}

/// The binary with a live consumer.
#[test]
fn rpc_gateway_releases_the_schedule_executor_lease() {
    assert_releases_lease_on_graceful_exit(GatewayCase::rpc_gateway());
}
