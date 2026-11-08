//! Joined store/Controller regression for an absent member's delayed cleanup.
//! SQLite and WorkGraph compute the disposition; the real Python Controller
//! applies it to a held worker. Package outputs remain inert. This does not
//! establish native permission, SDK admission, source cancellation or Ready.

use super::shared_preparation::{acquisition, backend, member};
use super::*;
use serde_json::{Value, json};
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout, Command};

const FRAME_LIMIT: u64 = 2 * 1024 * 1024;

async fn frame(output: &mut BufReader<ChildStdout>) -> Result<Value, String> {
    let mut bytes = Vec::new();
    output
        .take(FRAME_LIMIT + 1)
        .read_until(b'\n', &mut bytes)
        .await
        .map_err(|error| format!("Controller frame read: {error}"))?;
    if bytes.len() as u64 > FRAME_LIMIT || bytes.last() != Some(&b'\n') {
        return Err("missing or oversized Controller frame".into());
    }
    serde_json::from_slice(&bytes).map_err(|error| format!("Controller frame JSON: {error}"))
}

async fn drive(mut input: ChildStdin, mut output: BufReader<ChildStdout>) -> Result<(), String> {
    let selected = frame(&mut output).await?;
    if selected["stage"] != "selected" || selected["a"] != selected["b"] {
        return Err("peers did not retain the same actual Controller selection".into());
    }
    let selection = SharedPreparationSelection::from_controller(selected["a"].clone())
        .map_err(|error| format!("actual Controller selection: {error}"))?;
    let mut fixture = Fixture::new();
    let (backend, parent) = backend(&fixture).await;
    let a = acquisition(&mut fixture, &backend, &parent, "absent-child-a").await;
    let b = acquisition(&mut fixture, &backend, &parent, "late-child-b").await;
    let a = member(&fixture, &a, &selection);
    let b = member(&fixture, &b, &selection);
    if a.key() != b.key() || a.acquisition() == b.acquisition() || a.source() == b.source() {
        return Err("expected distinct members of one exact physical selection".into());
    }
    let acquisitions = (
        fixture
            .store
            .get(a.acquisition())
            .await
            .map_err(|error| error.to_string())?,
        fixture
            .store
            .get(b.acquisition())
            .await
            .map_err(|error| error.to_string())?,
    );
    if fixture
        .store
        .get(a.key())
        .await
        .map_err(|error| error.to_string())?
        .is_some()
    {
        return Err("A must never have joined P".into());
    }
    fixture
        .bridge()
        .begin_shared_preparation_leave(&a)
        .await
        .map_err(|error| error.to_string())?;
    let delayed_disposition = fixture
        .bridge()
        .leave_shared_preparation(&a)
        .await
        .map_err(|error| error.to_string())?;
    if fixture
        .store
        .get(a.key())
        .await
        .map_err(|error| error.to_string())?
        .is_some()
    {
        return Err("absent-member cleanup unexpectedly created P".into());
    }
    // Intentionally do not assert the boolean here. Pass the actual result to
    // the physical callback after B joins and starts work. The prior predicate
    // must fail because that callback requests a stop, not by a map-only check.
    fixture
        .bridge()
        .join_shared_preparation(&b, &selection, &backend)
        .await
        .map_err(|error| error.to_string())?;
    let joined = fixture
        .store
        .get(b.key())
        .await
        .map_err(|error| error.to_string())?
        .ok_or("B's committed P is missing")?;
    if joined.data.body["closed"] != false
        || joined.data.body["members"]
            .as_object()
            .map(|members| members.len())
            != Some(1)
    {
        return Err("B must be the sole live joined member".into());
    }
    let events = fixture
        .store
        .read_events(b.key(), 0, 100)
        .await
        .map_err(|error| error.to_string())?;
    let mut command = serde_json::to_vec(&json!({
        "preparation": b.key().id, "last": delayed_disposition,
    }))
    .map_err(|error| error.to_string())?;
    command.push(b'\n');
    input
        .write_all(&command)
        .await
        .map_err(|error| format!("Controller command: {error}"))?;
    input
        .shutdown()
        .await
        .map_err(|error| format!("Controller input close: {error}"))?;
    drop(input);
    let result = frame(&mut output).await?;
    if result["stage"] != "result"
        || result["ok"] != true
        || result["stop_calls"] != 0
        || result["stop_requested_while_held"] != false
        || result["worker_alive_after_callback"] != true
        || result["b_status"] != "succeeded"
        || result["physical_entries"] != 1
        || result["workers_alive_after_close"] != 0
        || result["pending_waiters_after_close"] != 0
        || result["owner_closed"] != true
    {
        return Err(format!(
            "delayed cleanup harmed B or failed owner settlement: {result}"
        ));
    }
    fixture
        .bridge()
        .check_shared_preparation(&b, &backend)
        .await
        .map_err(|error| error.to_string())?;
    if fixture
        .store
        .get(b.key())
        .await
        .map_err(|error| error.to_string())?
        != Some(joined)
        || fixture
            .store
            .read_events(b.key(), 0, 100)
            .await
            .map_err(|error| error.to_string())?
            != events
        || fixture
            .store
            .get(a.acquisition())
            .await
            .map_err(|error| error.to_string())?
            != acquisitions.0
        || fixture
            .store
            .get(b.acquisition())
            .await
            .map_err(|error| error.to_string())?
            != acquisitions.1
    {
        return Err("delayed callback changed canonical P or acquisition/outbox rows".into());
    }
    Ok(())
}

#[tokio::test]
async fn absent_p_cleanup_cannot_stop_a_member_that_joins_before_callback() {
    let python = PathBuf::from(
        std::env::var_os("TOOLKIT_SHARED_TEST_PYTHON")
            .expect("select TOOLKIT_SHARED_TEST_PYTHON explicitly for this Controller test"),
    );
    assert!(
        python.is_absolute() && python.is_file(),
        "selected Python must be an absolute executable path"
    );
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../sdk/consumer-setup/tests/shared_missing_member.py");
    assert!(
        script.is_file(),
        "the exact physical-control source must be selected"
    );
    let mut child = Command::new(python)
        .args(["-I", "-B"])
        .arg(script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn the selected physical-control owner");
    // The outer test retains Child before any fallible scenario work. A Rust
    // panic/timeout still closes its pipe and joins this exact process owner.
    let input = child.stdin.take().expect("piped stdin");
    let output = BufReader::new(child.stdout.take().expect("piped stdout"));
    let mut task = tokio::spawn(drive(input, output));
    let scenario = match tokio::time::timeout(Duration::from_secs(30), &mut task).await {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => Err(format!("scenario task failed: {error}")),
        Err(_) => {
            task.abort();
            let _ = task.await;
            Err("scenario exceeded its 30-second bound".into())
        }
    };
    let (status, forced) = match tokio::time::timeout(Duration::from_secs(15), child.wait()).await {
        Ok(result) => (result, false),
        Err(_) => {
            let _ = child.start_kill();
            (child.wait().await, true)
        }
    };
    assert!(
        !forced,
        "physical-control owner needed forced termination: {scenario:?}"
    );
    assert!(
        status.as_ref().is_ok_and(|status| status.success()),
        "physical-control owner failed after joining: {status:?}; {scenario:?}"
    );
    assert!(scenario.is_ok(), "{scenario:?}");
}
