//! Existing fixed resource verifier process shared by CLI and Console.
use super::*;
use serde_json::Value;

fn readback_unavailable() -> ConnectError {
    ConnectError::InvalidInput("The selected service readback is unconfirmed; its private artifacts are retained.".into())
}

// Fixed labels and elapsed time only. No request, path, native identity or
// error text is emitted, and these observations never establish readiness.
pub(super) fn readback_diagnostic(stage: &'static str, outcome: &'static str) {
    use std::io::Write as _;
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let elapsed = START.get_or_init(std::time::Instant::now).elapsed();
    let diagnostic = serde_json::json!({
        "event": "toolkit_installation_readback",
        "stage": stage,
        "outcome": outcome,
        "monotonic_elapsed_ms": u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
    });
    let _ = writeln!(std::io::stderr().lock(), "{diagnostic}");
}

pub(super) async fn locate_readback_resources(stage: &'static str) -> Result<InstalledResources, ConnectError> {
    observe_readback_resources(stage, InstalledResources::locate).await
}

async fn observe_readback_resources<T: Send + 'static>(
    stage: &'static str,
    observe: impl FnOnce() -> Result<T, ConnectError> + Send + 'static,
) -> Result<T, ConnectError> {
    readback_diagnostic(stage, "entered");
    // Resource verification inventories and hashes the full installed closure.
    // The existing recovery owner joins this future; do not select away an
    // entered observation or move its post-readback currentness checks earlier.
    let result = tokio::task::spawn_blocking(observe).await
        .map_err(|_| readback_unavailable()).and_then(|result| result);
    readback_diagnostic(stage, if result.is_ok() { "ok" } else { "error" });
    result
}

impl InstalledResources {
    /// One bounded package-owned verifier call within existing joined recovery.
    /// No helper/runtime is started and no installation or auth is replayed.
    pub(crate) async fn readback(self, storage: &meerkat_core::ResolvedStorage, request: &Value)
        -> Result<Value, ConnectError>
    {
        use std::{io::Write, os::unix::fs::OpenOptionsExt};
        use nix::{sys::signal::{kill, Signal}, unistd::Pid};
        readback_diagnostic("process_prepare", "entered");
        let directory = reserve_setup_artifacts(storage)?.canonicalize().map_err(|_| readback_unavailable())?;
        let request_path = directory.join("readback-request.json");
        let raw = serde_json::to_vec(request).map_err(|_| readback_unavailable())?;
        if raw.len() > 1024 * 1024 { return Err(readback_unavailable()); }
        let mut file = fs::OpenOptions::new().write(true).create_new(true).mode(0o600)
            .open(&request_path).map_err(|_| readback_unavailable())?;
        file.write_all(&raw).map_err(|_| readback_unavailable())?;
        file.sync_all().map_err(|_| readback_unavailable())?;
        drop(file);
        let stdout_path = directory.join("readback.stdout");
        let stdout = fs::OpenOptions::new().create_new(true).write(true).mode(0o600)
            .open(&stdout_path).map_err(|_| readback_unavailable())?;
        let stderr = fs::OpenOptions::new().create_new(true).write(true).mode(0o600)
            .open(directory.join("readback.stderr")).map_err(|_| readback_unavailable())?;
        let mut child = tokio::process::Command::new(&self.python)
            .args(["-I", "-B"]).arg(&self.prepare)
            .arg("--destination").arg(directory.join("readback"))
            .arg("--installation-readback").arg(&request_path)
            .current_dir(&self.root).env_clear().env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::null()).stdout(Stdio::from(stdout)).stderr(Stdio::from(stderr))
            .kill_on_drop(true).spawn().map_err(|_| readback_unavailable())?;
        readback_diagnostic("process_prepare", "ok");
        readback_diagnostic("child_wait", "entered");
        let status = tokio::select! {
            result = child.wait() => result.map_err(|_| {
                readback_diagnostic("child_wait", "error");
                readback_unavailable()
            })?,
            _ = tokio::time::sleep(std::time::Duration::from_secs(60)) => {
                readback_diagnostic("child_wait", "deadline");
                // The fixed owner handles SIGTERM and joins its verifier child.
                // Keep this entered future until that actual owner has exited.
                if let Some(pid) = child.id().and_then(|p| i32::try_from(p).ok()) {
                    let _ = kill(Pid::from_raw(pid), Signal::SIGTERM);
                }
                let _ = child.wait().await;
                readback_diagnostic("child_wait", "deadline_wait_returned");
                return Err(readback_unavailable());
            }
        };
        readback_diagnostic("child_wait", if status.success() { "ok" } else { "exit_unsuccessful" });
        if !status.success() || fs::read(&request_path).map_err(|_| readback_unavailable())? != raw {
            return Err(readback_unavailable());
        }
        // Native verification uses the binary's original manifest pin again.
        let current = locate_readback_resources("resources_after").await?;
        if current.root != self.root || current.prepare != self.prepare || current.python != self.python {
            return Err(readback_unavailable());
        }
        let metadata = fs::symlink_metadata(&stdout_path).map_err(|_| readback_unavailable())?;
        if !metadata.is_file() || metadata.len() > 1024 * 1024 { return Err(readback_unavailable()); }
        let mut bytes = Vec::new();
        fs::File::open(stdout_path).map_err(|_| readback_unavailable())?.take(1024 * 1024 + 1)
            .read_to_end(&mut bytes).map_err(|_| readback_unavailable())?;
        if bytes.len() > 1024 * 1024 { return Err(readback_unavailable()); }
        let value = serde_json::from_slice(&bytes).map_err(|_| readback_unavailable());
        readback_diagnostic("retained_output", if value.is_ok() { "ok" } else { "error" });
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, atomic::{AtomicBool, Ordering}, mpsc};
    use std::time::Duration;

    #[tokio::test(flavor = "current_thread")]
    async fn resource_observation_keeps_async_controls_responsive_and_joins_entered_work() {
        let (entered, waiting) = tokio::sync::oneshot::channel();
        let (release, held) = mpsc::channel();
        let finished = Arc::new(AtomicBool::new(false));
        let worker_finished = finished.clone();
        let work = tokio::spawn(observe_readback_resources("resources_before", move || {
            let _ = entered.send(());
            let released = held.recv_timeout(Duration::from_secs(5)).is_ok();
            worker_finished.store(true, Ordering::SeqCst);
            if released { Ok(41_u8) } else { Err(ConnectError::ReviewCancelled) }
        }));
        let entered_result = tokio::time::timeout(Duration::from_secs(2), waiting).await;
        // This timer must run while the synchronous observation is held. An
        // inline implementation reaches it only after the bounded hold ends.
        tokio::time::sleep(Duration::from_millis(10)).await;
        let pending_during_control = !work.is_finished() && !finished.load(Ordering::SeqCst);
        let release_result = release.send(());
        let joined = work.await;
        // Release and join before assertions, including on a failed probe.
        assert!(matches!(entered_result, Ok(Ok(()))));
        assert!(pending_during_control);
        assert!(release_result.is_ok());
        assert!(matches!(joined, Ok(Ok(41))));
        assert!(finished.load(Ordering::SeqCst));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn resource_observation_preserves_refusal_and_reports_worker_failure() {
        let refusal = observe_readback_resources("resources_after", || {
            Err::<(), _>(ConnectError::ReviewCancelled)
        }).await;
        assert!(matches!(refusal, Err(ConnectError::ReviewCancelled)));
        let failure = observe_readback_resources("resources_after", || -> Result<(), ConnectError> {
            panic!("controlled resource observation failure")
        }).await;
        assert!(matches!(failure, Err(ConnectError::InvalidInput(ref message))
            if message == &readback_unavailable().to_string()));
    }
}
