//! Live resource invocation for separate native helper connections.
//!
//! The shared owner supplies transport only. Each returned connection must go
//! through the normal member construction, attachment and call authorization.
use super::*;
use std::{future::Future, io, os::unix::fs::OpenOptionsExt};
use tokio::{
    io::{AsyncReadExt, BufReader},
    process::{Child, ChildStdin, ChildStdout},
};
#[cfg(feature = "console")]
mod attempt;
mod control;
mod entry;
mod lifetime;
mod physical;
pub use physical::{ColdPhysicalNoEntry, PhysicalSelection};
#[cfg(feature = "console")]
mod abandonment;
#[cfg(feature = "console")]
pub use abandonment::AbandonedPhysicalDisposition;
#[cfg(feature = "console")]
mod continuity;
#[cfg(all(test, feature = "console"))]
mod native_case;
#[cfg(all(test, feature = "console"))]
mod native_case_artifacts;
#[cfg(all(test, feature = "console"))]
pub(crate) mod sdk_host;
#[cfg(feature = "console")]
pub use attempt::SharedSetupAttemptReport;
use control::Channel;
pub use control::SharedSetupPeer;
pub use entry::SharedSetupCallContext;
use std::sync::Arc;

fn unavailable() -> ConnectError {
    ConnectError::InvalidInput(
        "Shared setup ownership is unavailable. Retained work was not retried.".into(),
    )
}

/// Connections from this one live resource invocation, never a persisted
/// connection pool. A connection is consumed by its normal native helper.
pub struct PreparedSharedSetup {
    pub connections: Vec<PreparedSharedConnection>,
    pub artifact_directory: PathBuf,
    pub owner: SharedSetupOwner,
}

/// A native connection and its matching private selection observation owner.
/// Neither component can be reconstructed from a saved MCP configuration.
pub struct PreparedSharedConnection {
    pub connection: PreparedSetupConnection,
    pub peer: SharedSetupPeer,
}

/// Closing the inherited pipe fences all peers and joins their calls plus the
/// Controller's workers before this owner reports success. Dropping is only a
/// last-resort direct-child stop and does not establish joined cleanup.
#[must_use = "close and join the shared setup owner after native helpers close"]
pub struct SharedSetupOwner {
    child: Child,
    channel: Arc<tokio::sync::Mutex<Channel>>,
    entries: Arc<entry::EntryTasks>,
    #[cfg(feature = "console")]
    attempts: std::sync::OnceLock<Arc<attempt::Installation>>,
    #[cfg(feature = "console")]
    artifact_directory: PathBuf,
    #[cfg(feature = "console")]
    realm: meerkat_core::RealmLocator,
    #[cfg(feature = "console")]
    close_identity: Option<String>,
    #[cfg(feature = "console")]
    lifetime_sha256: Option<String>,
    #[cfg_attr(
        not(feature = "console"),
        allow(
            dead_code,
            reason = "retains the physical lease with this owner; only console code reads it"
        )
    )]
    physical: Option<Arc<physical::PhysicalLease>>,
}

impl SharedSetupOwner {
    #[cfg(feature = "console")]
    pub(crate) fn physical_selection(&self) -> Option<PhysicalSelection> {
        self.physical.as_ref().map(|physical| physical.selection())
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Ready {
    shared_setup: u8,
    peers: Vec<PathBuf>,
}

impl InstalledResources {
    /// Prepare once, then open a single physical Controller for 1-8 explicitly
    /// selected native helpers. The caller retains the owner through their real
    /// runtime lifetime; saved peer configurations cannot restart that owner.
    /// Retain and await this preparation future through cancellation. Signal
    /// cancellation through `cancelled` so its existing cleanup can join the
    /// child. Dropping the whole future is not a settled cleanup operation.
    pub async fn prepare_with_shared_setup(
        self,
        storage: &meerkat_core::ResolvedStorage,
        helpers: usize,
        cancelled: impl Future<Output = io::Result<()>>,
    ) -> Result<PreparedSharedSetup, ConnectError> {
        if !(1..=8).contains(&helpers) {
            return Err(unavailable());
        }
        tokio::pin!(cancelled);
        let prepared = self.prepare_once(storage, &mut cancelled).await?;
        self.launch_shared_setup(storage, prepared, helpers, None, &mut cancelled)
            .await
    }

    async fn launch_shared_setup(
        &self,
        storage: &meerkat_core::ResolvedStorage,
        prepared: PreparedResources,
        helpers: usize,
        restored: Option<&str>,
        cancelled: impl Future<Output = io::Result<()>>,
    ) -> Result<PreparedSharedSetup, ConnectError> {
        tokio::pin!(cancelled);
        let config = prepared.connection.into_server();
        let meerkat_core::mcp_config::McpTransportConfig::Stdio(launch) = config.transport else {
            return Err(unavailable());
        };
        // This launch came from our verified preparer, not an external command.
        // Retain its exact interpreter/entry selection for every peer bridge.
        if launch.command != "/usr/bin/env"
            || !launch.env.is_empty()
            || !launch.secret_env.is_empty()
            || launch.args.len() < 8
            || launch.args[..3] != ["-i", "PATH=/usr/bin:/bin", "PYTHONDONTWRITEBYTECODE=1"]
            || launch.args[4..6] != ["-I", "-B"]
            || !Path::new(&launch.args[3]).is_absolute()
            || !Path::new(&launch.args[6]).is_absolute()
            || Path::new(&launch.args[6])
                .file_name()
                .and_then(|name| name.to_str())
                != Some("local_session.py")
            || launch.args.iter().any(|arg| arg.starts_with("--shared-"))
        {
            return Err(unavailable());
        }
        let stderr_name = restored
            .map(|_| format!("shared-owner-{}.stderr", uuid::Uuid::new_v4()))
            .unwrap_or_else(|| "shared-owner.stderr".into());
        let stderr = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(prepared.artifact_directory.join(stderr_name))
            .map_err(|_| unavailable())?;
        let close_identity = digest(uuid::Uuid::new_v4().as_bytes());
        let (lifetime_path, lifetime_sha256) = lifetime::PhysicalLifetime::retain(
            &prepared.artifact_directory,
            &self.root,
            &storage.locator,
            &close_identity,
            &launch.command,
            &launch.args,
            helpers,
            restored,
        )?;
        // Fence before spawning. A delayed child must independently acquire
        // this same lease and reject a cold reader's closed-admission seal.
        let physical = physical::PhysicalLease::join(&lifetime_path, &lifetime_sha256)?;
        let mut command = tokio::process::Command::new(&launch.command);
        command
            .args(&launch.args)
            .arg("--shared-peers")
            .arg(helpers.to_string());
        command.arg("--shared-close-id").arg(&close_identity);
        command
            .arg("--shared-lifetime")
            .arg(&lifetime_path)
            .arg("--shared-lifetime-sha256")
            .arg(&lifetime_sha256);
        if let Some(snapshot) = restored {
            command.arg("--shared-restore-sha256").arg(snapshot);
        }
        let mut child = command
            .env_clear()
            .current_dir(&self.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(stderr))
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| unavailable())?;
        let (Some(input), Some(output)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(unavailable());
        };
        let mut output = BufReader::new(output);
        let selected = tokio::select! {
            ready = read_ready(&mut output, helpers) => ready,
            _ = &mut cancelled => Err(unavailable()),
            _ = tokio::time::sleep(std::time::Duration::from_secs(60)) => Err(unavailable()),
        };
        let owner = SharedSetupOwner {
            child,
            channel: Arc::new(tokio::sync::Mutex::new(Channel::new(input, output))),
            entries: Arc::new(entry::EntryTasks::default()),
            #[cfg(feature = "console")]
            attempts: std::sync::OnceLock::new(),
            #[cfg(feature = "console")]
            artifact_directory: prepared.artifact_directory.clone(),
            #[cfg(feature = "console")]
            realm: storage.locator.clone(),
            #[cfg(feature = "console")]
            close_identity: Some(close_identity),
            #[cfg(feature = "console")]
            lifetime_sha256: Some(lifetime_sha256.clone()),
            physical: Some(physical.clone()),
        };
        let peers = match selected {
            Ok(peers) => peers,
            Err(primary) => {
                return match owner.close().await {
                    Ok(()) => Err(primary),
                    Err(cleanup) => Err(ConnectError::Cleanup {
                        primary: Box::new(primary),
                        cleanup: Box::new(cleanup),
                    }),
                };
            }
        };
        let connections = peers
            .into_iter()
            .enumerate()
            .map(|(index, path)| {
                let mut args = launch.args[..7].to_vec();
                args.push("--shared-peer".into());
                args.push(path.to_string_lossy().into_owned());
                args.extend([
                    "--shared-lifetime".into(),
                    lifetime_path.to_string_lossy().into_owned(),
                    "--shared-lifetime-sha256".into(),
                    lifetime_sha256.clone(),
                ]);
                // The native configuration parser is shared with ordinary setup.
                let bytes =
                    serde_json::to_vec(&serde_json::json!({"mcpServers": {"toolkit-setup": {
                        "command": launch.command, "args": args,
                    }}}))
                    .map_err(|_| unavailable())?;
                let connection = PreparedSetupConnection::from_preparer_output(&bytes)?;
                let configuration =
                    PreparedSetupConnection::from_preparer_output(&bytes)?.into_server();
                Ok(PreparedSharedConnection {
                    connection,
                    peer: SharedSetupPeer::new(
                        owner.channel.clone(),
                        index,
                        configuration,
                        owner.entries.clone(),
                    )
                    .with_physical(physical.clone()),
                })
            })
            .collect::<Result<Vec<_>, _>>();
        let connections = match connections {
            Ok(connections) => connections,
            Err(primary) => {
                return match owner.close().await {
                    Ok(()) => Err(primary),
                    Err(cleanup) => Err(ConnectError::Cleanup {
                        primary: Box::new(primary),
                        cleanup: Box::new(cleanup),
                    }),
                };
            }
        };
        Ok(PreparedSharedSetup {
            connections,
            artifact_directory: prepared.artifact_directory,
            owner,
        })
    }
}

impl SharedSetupOwner {
    /// Check the same physical owner and the actual preparer's selected realm.
    /// Legitimate state-root aliases retain the native locator contract.
    #[cfg(feature = "console")]
    pub(crate) fn selects_peer(
        &self,
        peer: &SharedSetupPeer,
        realm: &meerkat_core::RealmLocator,
    ) -> bool {
        self.realm.realm == realm.realm
            && self
                .realm
                .state_root
                .canonicalize()
                .ok()
                .zip(realm.state_root.canonicalize().ok())
                .is_some_and(|(prepared, selected)| prepared == selected)
            && Arc::ptr_eq(&peer.channel, &self.channel)
            && Arc::ptr_eq(&peer.entries, &self.entries)
            && !self.entries.is_closing()
    }

    #[cfg(feature = "console")]
    pub(crate) fn install_attempt_guards(
        &self,
        factory: meerkat::AgentFactory,
        contexts: &[Arc<SharedSetupCallContext>],
    ) -> Result<meerkat::AgentFactory, ConnectError> {
        // The getter is the only supported inspection of an opaque native
        // installation. Never call the overwriting setter on an existing one.
        if factory.mcp_attempt_guards().is_some()
            || self.attempts.get().is_some()
            || self.entries.is_closing()
            || !(1..=8).contains(&contexts.len())
        {
            return Err(unavailable());
        }
        let peers: Vec<_> = contexts
            .iter()
            .map(|context| context.attempt_peer())
            .collect();
        let mut indices = BTreeSet::new();
        if peers.iter().any(|peer| {
            !Arc::ptr_eq(&peer.channel, &self.channel)
                || !Arc::ptr_eq(&peer.entries, &self.entries)
                || !indices.insert(peer.index)
        }) {
            return Err(unavailable());
        }
        let installation =
            attempt::Installation::new(peers, self.artifact_directory.join("attempt-guards"))?;
        let guards = installation.guards();
        self.attempts.set(installation).map_err(|_| unavailable())?;
        Ok(factory.with_mcp_attempt_guards(guards))
    }

    /// Fence native attempt admission and observe bounded cleanup. The existing
    /// command owner retains the entire join-and-save task before any await.
    /// A dropped receiver cannot discard a native returned batch. Calling this
    /// closes the native installation; use it during native shutdown, not as a
    /// readiness or permission check. Pending reports require later observation.
    /// Native immediate observation deadlines remain valid. The existing
    /// command-task owner bounds concurrent caller admission.
    /// A save failure is observable here. After native drain is established,
    /// `close` joins physical resources before retrying retained unsaved batches.
    /// Evidence settlement is not bounded when storage remains unavailable.
    /// An earlier unsaved pending batch can still prevent a fresh native drain
    /// observation; this path does not collect over that retained batch.
    #[cfg(feature = "console")]
    pub fn join_attempt_guards(
        &self,
        deadline: tokio::time::Instant,
    ) -> impl Future<Output = Result<SharedSetupAttemptReport, ConnectError>> + Send + 'static {
        let installation = self.attempts.get().cloned();
        let (send, receive) = tokio::sync::oneshot::channel();
        let retained = match installation {
            Some(installation) => self.entries.spawn_command(
                async move {
                    let result = installation.join(deadline).await;
                    // Saving owns sticky failure and the unsaved vector.
                    // A bounded observation refusal does not poison unrelated
                    // command cleanup merely because its receiver gets Err.
                    let _ = send.send(result);
                    Ok(())
                },
                true,
            ),
            None => Err(unavailable()),
        };
        async move {
            retained?;
            receive.await.map_err(|_| unavailable())?
        }
    }

    /// Retain and join the physical resource's existing cleanup. Once called,
    /// cancellation of this wait cannot drop the child or its guard evidence.
    /// Call this inside the still-live Tokio runtime. Native helpers must be shut down first; an unresolved native attempt
    /// keeps this task and its exact Controller channel owned for later drain.
    /// Once native drain is known, failed persistence retains this task and its
    /// exact evidence after physical cleanup, without keeping the Controller live.
    pub fn close(self) -> impl Future<Output = Result<(), ConnectError>> + Send + 'static {
        let entries = self.entries.clone();
        let retained = entries.retain_resource_close(async move { self.close_owned().await });
        async move {
            let retained = retained?;
            (&mut *retained.lock().await)
                .await
                .map_err(|_| unavailable())?
        }
    }

    async fn close_owned(mut self) -> Result<(), ConnectError> {
        #[allow(unused_mut)]
        let mut attempt_failed = false;
        #[cfg(feature = "console")]
        if let Some(installation) = self.attempts.get() {
            loop {
                match installation
                    .join(tokio::time::Instant::now() + std::time::Duration::from_secs(1))
                    .await
                {
                    Ok(report) => {
                        attempt_failed |= report.failed;
                        if report.native_drained {
                            attempt_failed |= report.missing_terminal_receipts != 0;
                            break;
                        }
                    }
                    Err(_) => {
                        attempt_failed = true;
                        if installation.native_drained().await {
                            break;
                        }
                    }
                }
                // Native bounds end its wait, never its consuming finish.
                // Keep the resource/channel live while current drain is
                // unresolved. An older unsaved pending batch still blocks a
                // new collection. Once drain is known, missing or unsaved
                // evidence remains owned through physical cleanup below.
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        }
        // First settle ephemeral initial-entry decisions while the inherited
        // owner channel is still available, then close the physical Controller.
        let entries = self.entries.close().await;
        let mut channel = self.channel.lock().await;
        channel.input.take();
        let normal = tokio::time::timeout(std::time::Duration::from_secs(60), async {
            let mut trailing = Vec::new();
            (&mut channel.output)
                .take(4097)
                .read_to_end(&mut trailing)
                .await
                .map_err(|_| unavailable())?;
            if !trailing.is_empty() {
                return Err(unavailable());
            }
            let status = self.child.wait().await.map_err(|_| unavailable())?;
            if !status.success() {
                return Err(unavailable());
            }
            Ok(())
        })
        .await;
        let physical_failed = !matches!(normal, Ok(Ok(())));
        if physical_failed {
            let _ = self.child.start_kill();
            let _status =
                reap_owned_child(&mut self.child, std::time::Duration::from_secs(30)).await;
            // Forced shutdown remains a failed close even after actual reap.
            // An observation timeout or wait error cannot drop this task's
            // child owner or its native evidence before the join is known.
        }
        drop(channel);
        #[cfg(feature = "console")]
        if let Some(installation) = self.attempts.get() {
            loop {
                match installation.retry_persistence().await {
                    Ok(report) => {
                        attempt_failed |= report.failed || report.missing_terminal_receipts != 0;
                        break;
                    }
                    Err(_) => attempt_failed = true,
                }
                // The same resource-close task retains self and Installation,
                // including the exact unsaved Vec, across caller loss. A disk
                // failure keeps evidence settlement pending after child join.
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        }
        if physical_failed || attempt_failed {
            Err(unavailable())
        } else {
            entries
        }
    }
}

// An observation bound is not a termination receipt. The already retained
// resource-close task owns this child through every timeout/error and returns
// only an actual wait status. Runtime teardown remains the last-resort owner.
async fn reap_owned_child(
    child: &mut Child,
    observation: std::time::Duration,
) -> std::process::ExitStatus {
    reap_owned_child_observed(child, observation, || {}).await
}
async fn reap_owned_child_observed(
    child: &mut Child,
    observation: std::time::Duration,
    mut unconfirmed: impl FnMut(),
) -> std::process::ExitStatus {
    loop {
        if let Ok(Ok(status)) = tokio::time::timeout(observation, child.wait()).await {
            return status;
        }
        // Private observation seam only. Production supplies an empty closure;
        // no callback decides child state, cleanup classification or ownership.
        unconfirmed();
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

async fn read_ready(
    reader: &mut (impl tokio::io::AsyncRead + Unpin),
    count: usize,
) -> Result<Vec<PathBuf>, ConnectError> {
    let mut raw = Vec::new();
    for _ in 0..4096 {
        let byte = reader.read_u8().await.map_err(|_| unavailable())?;
        raw.push(byte);
        if byte == b'\n' {
            let ready: Ready = serde_json::from_slice(&raw).map_err(|_| unavailable())?;
            if ready.shared_setup != 1 || ready.peers.len() != count {
                return Err(unavailable());
            }
            let mut paths = BTreeSet::new();
            let mut parent = None;
            for path in &ready.peers {
                if !path.is_absolute() || !paths.insert(path.clone()) {
                    return Err(unavailable());
                }
                if path.to_str().is_none()
                    || path
                        .components()
                        .any(|part| matches!(part, Component::ParentDir))
                {
                    return Err(unavailable());
                }
                let directory = path.parent().ok_or_else(unavailable)?;
                if parent.as_ref().is_some_and(|old| old != &directory) {
                    return Err(unavailable());
                }
                parent = Some(directory);
            }
            return Ok(ready.peers);
        }
    }
    Err(unavailable())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shared_ready_requires_exact_bounded_distinct_host_peers() {
        let valid =
            b"{\"shared_setup\":1,\"peers\":[\"/tmp/owner/0.sock\",\"/tmp/owner/1.sock\"]}\n";
        assert_eq!(read_ready(&mut &valid[..], 2).await.unwrap().len(), 2);
        for bytes in [
            b"{\"shared_setup\":1,\"peers\":[\"/tmp/owner/0.sock\",\"/tmp/owner/0.sock\"]}\n"
                .as_slice(),
            b"{\"shared_setup\":1,\"peers\":[\"/tmp/owner/0.sock\",\"/tmp/other/1.sock\"]}\n",
            b"{\"shared_setup\":1,\"peers\":[\"relative\",\"/tmp/owner/1.sock\"]}\n",
            b"{\"shared_setup\":1,\"shared_setup\":1,\"peers\":[]}\n",
        ] {
            assert!(read_ready(&mut &bytes[..], 2).await.is_err());
        }
        let (mut writer, mut reader) = tokio::io::duplex(4096);
        use tokio::io::AsyncWriteExt;
        writer.write_all(&vec![b'x'; 4096]).await.unwrap();
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_secs(1),
                read_ready(&mut reader, 2)
            )
            .await
            .expect("bounded host readiness")
            .is_err()
        );
    }
}

#[cfg(test)]
mod retained_reap_tests {
    use super::*;
    use futures_util::FutureExt;
    use std::{
        panic::{AssertUnwindSafe, resume_unwind},
        process::Stdio,
    };
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn retained_close_keeps_real_child_after_observation_timeout_and_caller_loss() {
        let mut child = Some(
            tokio::process::Command::new("/bin/sh")
                .args(["-c", "printf 'ready\n'; IFS= read -r value; exit 7"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .kill_on_drop(true)
                .spawn()
                .unwrap(),
        );
        let mut input = child.as_mut().unwrap().stdin.take();
        let output = child.as_mut().unwrap().stdout.take().unwrap();
        let entries = Arc::new(entry::EntryTasks::default());
        let joined = Arc::new(std::sync::Mutex::new(None));
        let mut close = None;
        let caught = AssertUnwindSafe(async {
            let mut output = BufReader::new(output);
            let mut line = String::new();
            tokio::time::timeout(
                std::time::Duration::from_secs(5),
                output.read_line(&mut line),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(line, "ready\n");
            let mut owned = child.take().unwrap();
            let status = joined.clone();
            let (unconfirmed, mut observations) = tokio::sync::mpsc::channel(2);
            close = Some(
                entries
                    .retain_resource_close(async move {
                        let actual = reap_owned_child_observed(
                            &mut owned,
                            std::time::Duration::from_millis(20),
                            || {
                                let _ = unconfirmed.try_send(());
                            },
                        )
                        .await;
                        *status.lock().unwrap() = Some(actual);
                        if actual.success() {
                            Ok(())
                        } else {
                            Err(unavailable())
                        }
                    })
                    .unwrap(),
            );
            for _ in 0..2 {
                assert_eq!(
                    tokio::time::timeout(std::time::Duration::from_secs(5), observations.recv())
                        .await
                        .unwrap(),
                    Some(()),
                    "the real child wait must remain inconclusive across both observation deadlines"
                );
            }
            let handle = close.as_ref().unwrap();
            let mut observation = Box::pin(async { (&mut *handle.lock().await).await });
            assert!(futures_util::poll!(observation.as_mut()).is_pending());
            drop(observation);
            assert!(Arc::ptr_eq(
                handle,
                &entries.retained_resource_close().unwrap()
            ));
            assert!(!handle.lock().await.is_finished());
            assert!(
                joined.lock().unwrap().is_none(),
                "there is no invented child status"
            );
        })
        .catch_unwind()
        .await;
        // Release and join after assertion/timeout failure too. The retained
        // task remains the child owner when its disposable observer vanishes.
        if let Some(mut stdin) = input.take() {
            let _ = stdin.write_all(b"release\n").await;
        }
        let cleanup = if let Some(handle) = close {
            Some((&mut *handle.lock().await).await)
        } else {
            let status =
                reap_owned_child(child.as_mut().unwrap(), std::time::Duration::from_secs(5)).await;
            *joined.lock().unwrap() = Some(status);
            None
        };
        if let Err(panic) = caught {
            resume_unwind(panic);
        }
        assert!(
            cleanup.unwrap().unwrap().is_err(),
            "actual nonzero exit remains failed after reap"
        );
        assert_eq!(joined.lock().unwrap().as_ref().unwrap().code(), Some(7));
    }
}
