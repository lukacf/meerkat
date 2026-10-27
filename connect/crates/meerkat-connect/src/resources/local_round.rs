//! One live handoff owned by the actual installed preparation invocation.
//!
//! This does not establish installation, authentication, or native readiness.
use super::*;
use futures_util::FutureExt;
use std::{future::Future, io, os::unix::fs::OpenOptionsExt, panic::AssertUnwindSafe};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout},
};

fn unavailable() -> ConnectError {
    ConnectError::InvalidInput("The next local setup round is unconfirmed. Retained requests remain available for readback; no automatic retry was started.".into())
}

/// A move-only owner derived by the same verified resource preparation.
/// It cannot be reconstructed from a model-visible path or saved descriptor.
/// Every new invocation reserves new preparation artifacts. A killed old owner
/// may leave a dead locator there; its old descriptor stays recovery-only.
/// The one process slot retains custody even when a wait or protocol fails.
pub struct LocalRoundOwner {
    resources: InstalledResources,
    artifacts: PathBuf,
    request: serde_json::Value,
    // The ordinary preparer's actual output, captured before any native use.
    // Shared peer and completed-descriptor paths cannot construct this owner.
    #[cfg(feature = "console")]
    handoff_sha256: String,
    poisoned: bool,
    process: Option<RoundProcess>,
    #[cfg(test)]
    test_location: Option<(PathBuf, String)>,
}

/// Held across the actual native setup open. Every ordinary branch must await
/// settle or cancel. Drop closes its pipe and requests direct-child stop; the
/// borrowed owner retains that Child until explicit cleanup observes its exit.
#[must_use = "the entered local round owner must be settled or cancelled and joined"]
pub struct LocalRoundGuard<'a> {
    owner: &'a mut LocalRoundOwner,
}

/// Move-owned form for a caller that retains its begin and finish futures.
/// This owns the same preparation and process slot as the borrowed CLI form.
/// It does not decide when a native connection has opened or closed.
#[must_use = "settle or cancel this guard and retain the returned original owner"]
pub struct OwnedLocalRoundGuard {
    owner: LocalRoundOwner,
}

/// Private fixed-protocol state, always stored in its preparation owner before
/// the first await. Neither a callback panic nor a failed wait discards it.
struct RoundProcess {
    child: Child,
    input: Option<ChildStdin>,
    output: Option<BufReader<ChildStdout>>,
    request_path: PathBuf,
    request_raw: Vec<u8>,
}

fn exact_file(path: &Path) -> Result<Vec<u8>, ConnectError> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| unavailable())?;
    let metadata = file.metadata().map_err(|_| unavailable())?;
    if !metadata.is_file() || metadata.len() > MANIFEST_LIMIT || mode(&metadata) & 0o077 != 0 {
        return Err(unavailable());
    }
    let mut bytes = Vec::new();
    Read::take(file, MANIFEST_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| unavailable())?;
    if bytes.len() as u64 > MANIFEST_LIMIT {
        return Err(unavailable());
    }
    Ok(bytes)
}

fn retain(path: &Path, raw: &[u8]) -> Result<(), ConnectError> {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|_| unavailable())?;
    file.write_all(raw)
        .and_then(|_| file.sync_all())
        .map_err(|_| unavailable())
}

impl InstalledResources {
    /// The ordinary preparation result is unchanged. Only this current caller
    /// receives the live continuation owner before transferring the connection.
    pub async fn prepare_with_local_round_owner(
        self,
        storage: &meerkat_core::ResolvedStorage,
        cancelled: impl Future<Output = io::Result<()>>,
    ) -> Result<(PreparedResources, LocalRoundOwner), ConnectError> {
        let prepared = self.prepare_once(storage, cancelled).await?;
        let directory = prepared.artifact_directory.join("prepared");
        let descriptor = exact_file(&directory.join("product-mcp.json"))?;
        let preparation = exact_file(&directory.join("product-prepared.json"))?;
        let owner = LocalRoundOwner {
            resources: self,
            artifacts: prepared.artifact_directory.clone(),
            request: serde_json::json!({
                "prepared": directory,
                "realm_state_root": storage.locator.state_root,
                "realm": storage.locator.realm.as_str(),
                "descriptor_sha256": digest(&descriptor),
                "preparation_sha256": digest(&preparation),
            }),
            #[cfg(feature = "console")]
            handoff_sha256: digest(&exact_file(&prepared.artifact_directory.join("prepare.stdout"))?),
            poisoned: false,
            process: None,
            #[cfg(test)]
            test_location: None,
        };
        Ok((prepared, owner))
    }
}

impl LocalRoundOwner {
    #[cfg(feature = "console")]
    pub(crate) fn matches_preparation(
        &self, resources: &crate::application_setup::PreparedApplicationResources,
    ) -> Result<(), String> {
        if self.poisoned || self.has_pending_round()
            || self.artifacts != resources.artifact_directory
            || resources.handoff.path != self.artifacts.join("prepare.stdout")
            || resources.handoff.sha256 != self.handoff_sha256
            || digest(&exact_file(&resources.handoff.path).map_err(|e| e.to_string())?) != self.handoff_sha256
        {
            return Err("local round owner differs from the ordinary preparation".into());
        }
        self.check_resources().map_err(|e| e.to_string())
    }
    /// Call only after the previous NativeSetup has actually closed and joined.
    /// Lock availability is an additional overlap check, not settlement proof.
    /// Retain this future and signal cancellation instead of dropping it.
    pub async fn begin(
        &mut self,
        cancelled: impl Future<Output = io::Result<()>>,
    ) -> Result<LocalRoundGuard<'_>, ConnectError> {
        self.enter(cancelled).await?;
        Ok(LocalRoundGuard { owner: self })
    }

    /// The same round, owning the preparation across asynchronous custody.
    /// Call only after the previous NativeSetup has actually closed and joined.
    /// Lock availability is an additional overlap check, not settlement proof.
    /// On failure the original, poisoned owner is returned, including a child
    /// whose OS wait failed. It must not be replaced by a reconstructed owner.
    /// The caller must retain this future until it returns.
    pub async fn begin_owned(
        mut self,
        cancelled: impl Future<Output = io::Result<()>>,
    ) -> Result<OwnedLocalRoundGuard, (Self, ConnectError)> {
        match self.enter(cancelled).await {
            Ok(()) => Ok(OwnedLocalRoundGuard { owner: self }),
            Err(error) => Err((self, error)),
        }
    }

    /// Whether this owner still holds an unjoined protocol child. This is
    /// custody only, not a statement about a native server or its descendants.
    pub fn has_pending_round(&self) -> bool {
        self.process.is_some()
    }

    /// Close and join the retained child after an unsuccessful operation.
    /// A failed OS wait leaves the same child here for a later explicit join.
    /// Cancellation never clears poison or authorizes another setup round.
    pub async fn cancel_pending_round(&mut self) -> Result<(), ConnectError> {
        self.finish(false).await
    }

    async fn enter(
        &mut self,
        cancelled: impl Future<Output = io::Result<()>>,
    ) -> Result<(), ConnectError> {
        if self.poisoned || self.process.is_some() {
            return Err(unavailable());
        }
        // Invocation-local custody, not a persisted denial record.
        self.poisoned = true;
        // The child stays in self, outside the future caught here. In
        // particular a panicking cancellation future cannot drop the child.
        let result = AssertUnwindSafe(async {
            self.start_process()?;
            let Some(output) = self.process.as_mut().and_then(|p| p.output.as_mut()) else {
                return Err(unavailable());
            };
            tokio::pin!(cancelled);
            let admitted = tokio::select! {
                result = read_ready(output) => result,
                _ = &mut cancelled => false,
                _ = tokio::time::sleep(std::time::Duration::from_secs(60)) => false,
            };
            if admitted { Ok(()) } else { Err(unavailable()) }
        })
        .catch_unwind()
        .await
        .unwrap_or_else(|_| Err(unavailable()));
        match result {
            Ok(()) => Ok(()),
            Err(primary) => {
                // The fixed owner wakes on input EOF. A failed OS wait keeps
                // its actual Child in self rather than turning Drop into join.
                let cleanup = self.join_child(false).await;
                Err(with_cleanup(primary, cleanup))
            }
        }
    }

    fn start_process(&mut self) -> Result<(), ConnectError> {
        self.check_resources()?;
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::Builder::new()
            .prefix("round-")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(&self.artifacts)
            .map_err(|_| unavailable())?
            .keep();
        let request_path = directory.join("request.json");
        let request_raw = serde_json::to_vec(&self.request).map_err(|_| unavailable())?;
        retain(&request_path, &request_raw)?;
        let stderr = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(directory.join("owner.stderr"))
            .map_err(|_| unavailable())?;
        let mut child = tokio::process::Command::new(&self.resources.python)
            .args(["-I", "-B"])
            .arg(&self.resources.prepare)
            .arg("--destination")
            .arg(directory.join("unused"))
            .arg("--local-round-owner")
            .arg(&request_path)
            .current_dir(&self.resources.root)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(stderr))
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| unavailable())?;
        let input = child.stdin.take();
        let output = child.stdout.take().map(BufReader::new);
        // Publish custody before checking pipes or waiting for Ready.
        self.process = Some(RoundProcess {
            child,
            input,
            output,
            request_path,
            request_raw,
        });
        if self
            .process
            .as_ref()
            .is_none_or(|p| p.input.is_none() || p.output.is_none())
        {
            return Err(unavailable());
        }
        Ok(())
    }

    async fn finish(&mut self, settle: bool) -> Result<(), ConnectError> {
        let result = AssertUnwindSafe(async {
            if settle {
                self.settle_protocol().await
            } else {
                self.join_child(false).await
            }
        })
        .catch_unwind()
        .await
        .unwrap_or_else(|_| Err(unavailable()));
        match result {
            Ok(()) => Ok(()),
            Err(primary) => {
                // Broken output can block the coordinator on its pipe. It
                // owns no descendants; kill and join only this retained child.
                let cleanup = self.join_child(true).await;
                Err(with_cleanup(primary, cleanup))
            }
        }
    }

    async fn settle_protocol(&mut self) -> Result<(), ConnectError> {
        let process = self.process.as_mut().ok_or_else(unavailable)?;
        let sent = match process.input.take() {
            Some(mut input) => input.write_all(b"{\"settle\":true}\n").await.is_ok(),
            None => false,
        };
        let mut raw = Vec::new();
        let output = process.output.as_mut().ok_or_else(unavailable)?;
        let read = output.take(4097).read_to_end(&mut raw).await;
        if read.is_err() || raw.len() > 4096 {
            return Err(unavailable());
        }
        let status = process.child.wait().await.map_err(|_| unavailable())?;
        // Only a successful wait releases the process slot, even when the
        // following protocol/resource checks refuse the round.
        let process = self.process.take().ok_or_else(unavailable)?;
        if !sent
            || !status.success()
            || raw != b"{\"consumed\":true}\n{\"settled\":true}\n"
            || exact_file(&process.request_path)? != process.request_raw
        {
            return Err(unavailable());
        }
        self.check_resources()?;
        self.poisoned = false;
        Ok(())
    }

    async fn join_child(&mut self, kill: bool) -> Result<(), ConnectError> {
        let Some(process) = self.process.as_mut() else {
            return Ok(());
        };
        process.input.take();
        if kill {
            let _ = process.child.start_kill();
        }
        process.child.wait().await.map_err(|_| unavailable())?;
        self.process.take();
        Ok(())
    }

    fn check_resources(&self) -> Result<(), ConnectError> {
        #[cfg(not(test))]
        let current = InstalledResources::locate()?;
        #[cfg(test)]
        let current = match &self.test_location {
            Some((executable, pin)) => InstalledResources::from_executable(executable, pin)?,
            None => InstalledResources::locate()?,
        };
        if current.root != self.resources.root
            || current.prepare != self.resources.prepare
            || current.python != self.resources.python
        {
            return Err(unavailable());
        }
        Ok(())
    }
}

fn with_cleanup(primary: ConnectError, cleanup: Result<(), ConnectError>) -> ConnectError {
    match cleanup {
        Ok(()) => primary,
        Err(cleanup) => ConnectError::Cleanup {
            primary: Box::new(primary),
            cleanup: Box::new(cleanup),
        },
    }
}

impl LocalRoundGuard<'_> {
    /// A native object may already exist when this fails. Its actual owner must
    /// close and join it; an unconfirmed handoff is not an inferred rollback.
    pub async fn settle(self) -> Result<(), ConnectError> {
        self.owner.finish(true).await
    }

    /// Join the fixed owner without asserting consumption or clearing poison.
    pub async fn cancel(self) -> Result<(), ConnectError> {
        self.owner.finish(false).await
    }
}

impl Drop for LocalRoundGuard<'_> {
    fn drop(&mut self) {
        // Preserve emergency cancellation without discarding the actual Child.
        // Consuming settle/cancel already cleared the slot after a good wait.
        if let Some(process) = self.owner.process.as_mut() {
            self.owner.poisoned = true;
            process.input.take();
            let _ = process.child.start_kill();
        }
    }
}

impl OwnedLocalRoundGuard {
    /// The caller supplies the native open/close ordering. This only settles
    /// the round protocol and returns the original preparation plus its result.
    /// On error retain the owner; has_pending_round identifies an unjoined child.
    pub async fn settle(mut self) -> (LocalRoundOwner, Result<(), ConnectError>) {
        let result = self.owner.finish(true).await;
        (self.owner, result)
    }

    /// Return the same poisoned owner after cancellation. A wait error retains
    /// its actual child for cancel_pending_round; it does not claim a join.
    pub async fn cancel(mut self) -> (LocalRoundOwner, Result<(), ConnectError>) {
        let result = self.owner.finish(false).await;
        (self.owner, result)
    }
}

async fn read_ready(reader: &mut (impl tokio::io::AsyncRead + Unpin)) -> bool {
    let mut ready = [0_u8; 15];
    reader.read_exact(&mut ready).await.is_ok() && ready == *b"{\"ready\":true}\n"
}

#[cfg(test)]
mod tests;

#[cfg(feature = "console")]
mod attempt;
#[cfg(feature = "console")]
pub use attempt::LocalRoundAttempts;
