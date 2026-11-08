//! Native attempt leases for the exact retained setup peers.
//! These leases retire transport only. Canonical P remains the worker owner.
use super::*;
use crate::application_setup::resource::SetupResourceRegistration;
use async_trait::async_trait;
use meerkat_core::{MobMemberBinding, mcp_attachment::McpAttachmentRefusal};
use meerkat_mcp::{
    McpAttemptCancellation, McpAttemptDisposition, McpAttemptGuard, McpAttemptGuardBounds,
    McpAttemptGuardFactory, McpAttemptGuardRequest, McpAttemptGuards, McpAttemptOpenOutcome,
    McpAttemptOperation, McpAttemptProcessEnd, McpGuardCompletion, McpGuardCompletionRecord,
    McpGuardFinishError, McpGuardStage, McpServerCloseOutcome,
};
use std::{
    collections::BTreeSet, fmt::Write as _, io::Write, os::unix::fs::DirBuilderExt, sync::OnceLock,
};

const MAX_NATIVE_DEBUG_BYTES: usize = 1024 * 1024;
const MAX_NATIVE_ARTIFACT_BYTES: usize = MAX_NATIVE_DEBUG_BYTES * 6 + 256;

pub(super) struct Registration {
    member: MobMemberBinding,
    session: String,
    participant: Arc<SetupResourceRegistration>,
}

/// This slot is installed only by the existing selected-destination callback.
/// A later native construction cannot replace the participant of a used peer.
#[derive(Default)]
pub(super) struct PeerAttempt {
    registration: OnceLock<Registration>,
    claim: OnceLock<McpAttemptGuardRequest>,
    joined: OnceLock<()>,
}
impl PeerAttempt {
    pub(super) fn current_registration(&self, member: &MobMemberBinding, session: &str) -> bool {
        self.joined.get().is_none()
            && self.claim.get().is_some()
            && self.registration.get().is_some_and(|selected| {
                &selected.member == member
                    && selected.session == session
                    && selected.participant.current().is_ok()
            })
    }
    pub(super) fn unbound(&self) -> bool {
        self.registration.get().is_none()
            && self.claim.get().is_none()
            && self.joined.get().is_none()
    }
    pub(super) fn renewable(&self, member: &MobMemberBinding, session: &str) -> bool {
        self.joined.get().is_some()
            && self.claim.get().is_some()
            && self.registration.get().is_some_and(|registration| {
                &registration.member == member && registration.session == session
            })
    }
    pub(super) fn closed_registration(&self) -> Option<(MobMemberBinding, String)> {
        let registration = self.registration.get()?;
        self.renewable(&registration.member, &registration.session)
            .then(|| (registration.member.clone(), registration.session.clone()))
    }
    pub(super) fn bind(
        &self,
        member: MobMemberBinding,
        session: String,
        participant: Arc<SetupResourceRegistration>,
    ) -> Result<(), String> {
        participant.current()?;
        self.registration
            .set(Registration {
                member,
                session,
                participant,
            })
            .map_err(|_| "shared peer already has its native registration".into())
    }

    fn claim(
        &self,
        peer: &SharedSetupPeer,
        request: McpAttemptGuardRequest,
    ) -> Result<(), McpAttachmentRefusal> {
        // Descriptor and the actual selected registration are checked before
        // the one-use claim. Nothing fallible or asynchronous follows it.
        let selected = self
            .registration
            .get()
            .ok_or(McpAttachmentRefusal::Unavailable)?;
        if request.server != peer.configuration
            || request.key.server != peer.configuration.name
            || request.key.attempt == 0
            || request.owner.member.as_ref() != Some(&selected.member)
            || request.owner.session_id.to_string() != selected.session
            || !matches!(
                request.operation,
                McpAttemptOperation::Add | McpAttemptOperation::Reload
            )
            || peer.entries.is_closing()
        {
            return Err(McpAttachmentRefusal::Mismatch);
        }
        selected
            .participant
            .current()
            .map_err(|_| McpAttachmentRefusal::Unavailable)?;
        self.claim
            .set(request)
            .map_err(|_| McpAttachmentRefusal::Mismatch)
    }
}

pub(super) struct Factory {
    peers: Vec<Arc<SharedSetupPeer>>,
}
impl Factory {
    pub(super) fn new(peers: Vec<Arc<SharedSetupPeer>>) -> Self {
        Self { peers }
    }
}
#[async_trait]
impl McpAttemptGuardFactory for Factory {
    async fn begin(
        &self,
        request: McpAttemptGuardRequest,
        cancel: McpAttemptCancellation,
    ) -> Result<Option<Box<dyn McpAttemptGuard>>, McpAttachmentRefusal> {
        let mut exact = self
            .peers
            .iter()
            .filter(|peer| peer.configuration == request.server);
        let Some(peer) = exact.next() else {
            // Reserve known setup names. Another peer or inherited provider
            // cannot turn a changed descriptor into an unguarded setup launch.
            return if self
                .peers
                .iter()
                .any(|peer| peer.configuration.name == request.server.name)
            {
                Err(McpAttachmentRefusal::Mismatch)
            } else {
                Ok(None)
            };
        };
        if exact.next().is_some() {
            return Err(McpAttachmentRefusal::Mismatch);
        }
        if cancel.is_requested() {
            return Err(McpAttachmentRefusal::Unavailable);
        }
        let guard = Guard {
            peer: peer.clone(),
            request: request.clone(),
        };
        peer.attempt.claim(peer, request)?;
        // Core retains begin and cancels a late returned guard if the caller
        // disappeared. There is no await or error exit between claim and return.
        Ok(Some(Box::new(guard)))
    }
}

struct Guard {
    peer: Arc<SharedSetupPeer>,
    request: McpAttemptGuardRequest,
}
impl Guard {
    async fn finish(
        self,
        disposition: McpAttemptDisposition,
        settled: bool,
    ) -> Result<(), McpGuardFinishError> {
        let open_matches = match disposition.open {
            McpAttemptOpenOutcome::Installed { .. } => settled,
            McpAttemptOpenOutcome::SetupFailed { .. }
            | McpAttemptOpenOutcome::Refused { .. }
            | McpAttemptOpenOutcome::NotInstalled { .. }
            | McpAttemptOpenOutcome::Cancelled { .. } => !settled,
            _ => false,
        };
        let exact = disposition.key == self.request.key && open_matches;
        let mut state = self.peer.close_peer().await.map_err(|_| finish_error())?;
        loop {
            match state {
                control::PeerRetirement::Joined => break,
                control::PeerRetirement::Pending => {
                    // Core's bound limits its wait, not this consuming future.
                    // The same peer close task survives and the channel remains
                    // available until the physical owner's guard join ends.
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                    state = self.peer.observe_peer().await.map_err(|_| finish_error())?;
                }
                _ => return Err(finish_error()),
            }
        }
        if exact {
            self.peer
                .attempt
                .joined
                .set(())
                .map_err(|_| finish_error())?;
            Ok(())
        } else {
            Err(finish_error())
        }
    }
}
fn finish_error() -> McpGuardFinishError {
    McpGuardFinishError::new(
        "The exact shared setup peer did not establish its native attempt drain",
    )
}
#[async_trait]
impl McpAttemptGuard for Guard {
    async fn settle(
        self: Box<Self>,
        disposition: McpAttemptDisposition,
    ) -> Result<(), McpGuardFinishError> {
        self.finish(disposition, true).await
    }
    async fn cancel(
        self: Box<Self>,
        disposition: McpAttemptDisposition,
    ) -> Result<(), McpGuardFinishError> {
        self.finish(disposition, false).await
    }
}

/// Cumulative local cleanup evidence from the one installed native owner.
/// Current native drain and terminal evidence coverage are distinct. Earlier
/// failures survive later empty native joins.
#[derive(Clone, Debug)]
#[must_use]
pub struct SharedSetupAttemptReport {
    pub recorded_batches: u64,
    pub recorded_calls: u64,
    /// Held/TimedOut calls in the latest native join, not historical counts.
    pub pending_calls: usize,
    /// Claims without saved terminal evidence or a current pending row.
    /// Missing or evicted evidence does not mean work is still running.
    pub missing_terminal_receipts: usize,
    /// The latest closed-installation join returned strictly before its
    /// deadline with no current pending/unknown completion. Immediate or
    /// expired observations do not establish this drain.
    pub native_drained: bool,
    pub failed: bool,
}
impl SharedSetupAttemptReport {
    pub fn is_complete(&self) -> bool {
        self.native_drained
            && self.pending_calls == 0
            && self.missing_terminal_receipts == 0
            && !self.failed
    }
}

pub(super) struct Installation {
    // The same factory owns the peer claim slots. A native history bound or
    // foreign consuming join cannot make a claimed peer disappear from coverage.
    factory: Arc<Factory>,
    guards: McpAttemptGuards,
    directory: PathBuf,
    state: tokio::sync::Mutex<JoinState>,
}
#[derive(Default)]
struct JoinState {
    batches: u64,
    calls: u64,
    // Replaced from each actual native batch. A prior Held receipt cannot
    // turn missing later terminal evidence into invented active work.
    pending: BTreeSet<(u64, String, u64, u8)>,
    native_drained: bool,
    // Saved terminal evidence only for this factory's at most eight claims.
    // Required keys are always read from their original one-use peer slots.
    terminal: BTreeSet<(u64, String, u64)>,
    failed: bool,
    // Core forgets ended records before returning them. Retain its actual
    // returned vector before any fallible save or next call to that owner.
    unsaved: Option<Vec<McpGuardCompletionRecord>>,
    // One bounded byte-identical all-pending observation, not an authority
    // cache. Ended records are never suppressed, even if their bytes repeat.
    last_pending: Option<String>,
}
impl Installation {
    pub(super) fn new(
        peers: Vec<Arc<SharedSetupPeer>>,
        directory: PathBuf,
    ) -> Result<Arc<Self>, ConnectError> {
        // Existing private invocation artifacts hold evidence, not another
        // permission, membership or recovery store. No reopen API is provided.
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .map_err(|_| unavailable())?;
        let factory = Arc::new(Factory::new(peers));
        Ok(Arc::new(Self {
            guards: McpAttemptGuards::new(factory.clone(), McpAttemptGuardBounds::default()),
            factory,
            directory,
            state: tokio::sync::Mutex::new(JoinState::default()),
        }))
    }
    pub(super) fn guards(&self) -> McpAttemptGuards {
        self.guards.clone()
    }

    fn report(&self, state: &JoinState) -> SharedSetupAttemptReport {
        let missing = self
            .factory
            .peers
            .iter()
            .filter_map(|peer| peer.attempt.claim.get())
            .map(|request| {
                (
                    request.key.construction.get(),
                    request.key.server.clone(),
                    request.key.attempt,
                )
            })
            .filter(|key| !state.terminal.contains(key))
            .filter(|key| {
                !state
                    .pending
                    .iter()
                    .any(|p| (p.0, &p.1, p.2) == (key.0, &key.1, key.2))
            })
            .count();
        SharedSetupAttemptReport {
            recorded_batches: state.batches,
            recorded_calls: state.calls,
            pending_calls: state.pending.len(),
            missing_terminal_receipts: missing,
            native_drained: state.native_drained,
            failed: state.failed,
        }
    }

    pub(super) async fn join(
        &self,
        deadline: tokio::time::Instant,
    ) -> Result<SharedSetupAttemptReport, ConnectError> {
        // Serializes only native report collection/saving, never a peer command
        // or dispatch. The enclosing EntryTasks task owns this whole operation.
        let mut state = tokio::time::timeout_at(deadline, self.state.lock())
            .await
            .map_err(|_| unavailable())?;
        if state.unsaved.is_some() {
            self.save(&mut state)?;
        }
        state.native_drained = false;
        state.unsaved = Some(self.guards.close_and_join(deadline).await);
        let before_deadline = tokio::time::Instant::now() < deadline;
        let records = state.unsaved.as_ref().ok_or_else(unavailable)?;
        let known_completions = records.iter().all(|record| {
            matches!(
                record.completion,
                McpGuardCompletion::Held
                    | McpGuardCompletion::TimedOut
                    | McpGuardCompletion::Completed
                    | McpGuardCompletion::Cancelled
                    | McpGuardCompletion::Failed
                    | McpGuardCompletion::Panicked
            )
        });
        let pending = records
            .iter()
            .filter(|record| {
                matches!(
                    record.completion,
                    McpGuardCompletion::Held | McpGuardCompletion::TimedOut
                )
            })
            .map(|record| {
                (
                    record.key.construction.get(),
                    record.key.server.clone(),
                    record.key.attempt,
                    match record.stage {
                        McpGuardStage::Begin => 0,
                        McpGuardStage::Settle => 1,
                        McpGuardStage::Cancel => 2,
                        _ => 255,
                    },
                )
            })
            .collect();
        state.pending = pending;
        // Exact Core join can omit late tracked work at/past its deadline.
        // A before-deadline return instead requires no late tasks, with every
        // guard held or tracked. The installation is already closed to begins.
        // Preserve this native fact even if saving the retained batch fails.
        // Persistence failure cannot turn drained native work into active work.
        state.native_drained = before_deadline && known_completions && state.pending.is_empty();
        self.save(&mut state)?;
        if state.native_drained && self.report(&state).missing_terminal_receipts != 0 {
            // Evidence loss prevents success but does not invent running work.
            state.failed = true;
        }
        Ok(self.report(&state))
    }

    pub(super) async fn native_drained(&self) -> bool {
        self.state.lock().await.native_drained
    }

    // Called by the same retained resource-close future after it has proved
    // native drain and joined the physical owner. This never consumes another
    // native batch or changes the native drain observation.
    pub(super) async fn retry_persistence(&self) -> Result<SharedSetupAttemptReport, ConnectError> {
        let mut state = self.state.lock().await;
        self.save(&mut state)?;
        Ok(self.report(&state))
    }

    fn save(&self, state: &mut JoinState) -> Result<(), ConnectError> {
        let Some(records) = state.unsaved.as_ref() else {
            return Ok(());
        };
        if records.is_empty() {
            state.unsaved.take();
            return Ok(());
        }
        let only_pending = records.iter().all(|row| {
            matches!(
                row.completion,
                McpGuardCompletion::Held | McpGuardCompletion::TimedOut
            )
        });
        let debug = match bounded_debug(records) {
            Ok(value) => value,
            Err(error) => {
                state.failed = true;
                return Err(error);
            }
        };
        if only_pending && state.last_pending.as_ref() == Some(&debug) {
            // Core forgets only ended tasks. These are the same complete
            // pending bytes already saved, so an observation cannot generate
            // unlimited copies while no native fact changes.
            state.unsaved.take();
            return Ok(());
        }
        let encoded = (|| {
            let number = state.batches.checked_add(1).ok_or_else(unavailable)?;
            let calls = state
                .calls
                .checked_add(records.len() as u64)
                .ok_or_else(unavailable)?;
            // The native vector is retained without truncation. Formatting has
            // a fixed byte bound; oversize remains unsaved and unresolved. The
            // native API bounds retained ended calls, not every active begin.
            let bytes = serde_json::to_vec_pretty(&serde_json::json!({
                "schema":1, "batch":number, "native_records_debug":debug,
            }))
            .map_err(|_| unavailable())?;
            if bytes.len() > MAX_NATIVE_ARTIFACT_BYTES {
                return Err(unavailable());
            }
            write_batch(&self.directory, number, &bytes)?;
            Ok((number, calls))
        })();
        let (number, calls) = match encoded {
            Ok(value) => value,
            Err(error) => {
                state.failed = true;
                return Err(error);
            }
        };
        state.last_pending = only_pending.then_some(debug);
        let records = state.unsaved.take().ok_or_else(unavailable)?;
        state.batches = number;
        state.calls = calls;
        for record in records {
            let stage = match record.stage {
                McpGuardStage::Begin => 0,
                McpGuardStage::Settle => 1,
                McpGuardStage::Cancel => 2,
                _ => {
                    state.failed = true;
                    continue;
                }
            };
            let key = (
                record.key.construction.get(),
                record.key.server.clone(),
                record.key.attempt,
                stage,
            );
            state.failed |= !matches!(
                record.completion,
                McpGuardCompletion::Held
                    | McpGuardCompletion::TimedOut
                    | McpGuardCompletion::Completed
                    | McpGuardCompletion::Cancelled
                    | McpGuardCompletion::Failed
                    | McpGuardCompletion::Panicked
            );
            state.failed |= matches!(
                record.completion,
                McpGuardCompletion::Failed | McpGuardCompletion::Panicked
            );
            if let Some(disposition) = record.disposition {
                state.failed |= disposition.key != record.key;
                if stage != 0
                    && disposition.key == record.key
                    && matches!(
                        record.completion,
                        McpGuardCompletion::Completed
                            | McpGuardCompletion::Cancelled
                            | McpGuardCompletion::Failed
                            | McpGuardCompletion::Panicked
                    )
                    && self.factory.peers.iter().any(|peer| {
                        peer.attempt
                            .claim
                            .get()
                            .is_some_and(|request| request.key == record.key)
                    })
                {
                    state.terminal.insert((key.0, key.1.clone(), key.2));
                }
                match disposition.process {
                    McpAttemptProcessEnd::NotSpawned => (),
                    McpAttemptProcessEnd::Closed(receipt) => {
                        // Use the native final-fact predicate. Phase timing
                        // (`settled_in`) is evidence, not settlement authority.
                        state.failed |= !McpServerCloseOutcome::Stdio(receipt).is_settled();
                    }
                    McpAttemptProcessEnd::Abandoned => state.failed = true,
                    _ => state.failed = true,
                }
            }
        }
        Ok(())
    }
}

// Debug is evidence only, never decoded for authority or terminality. The
// buffer stops growing before the bound, even for a future oversized field.
pub(super) fn bounded_debug(value: &impl std::fmt::Debug) -> Result<String, ConnectError> {
    struct Limited(String);
    impl std::fmt::Write for Limited {
        fn write_str(&mut self, text: &str) -> std::fmt::Result {
            if self.0.len().saturating_add(text.len()) > MAX_NATIVE_DEBUG_BYTES {
                return Err(std::fmt::Error);
            }
            self.0.push_str(text);
            Ok(())
        }
    }
    let mut result = Limited(String::new());
    write!(&mut result, "{value:#?}").map_err(|_| unavailable())?;
    Ok(result.0)
}

fn write_batch(directory: &Path, number: u64, bytes: &[u8]) -> Result<(), ConnectError> {
    let path = directory.join(format!("native-attempt-join-{number}.json"));
    match fs::symlink_metadata(&path) {
        Ok(metadata) => {
            // A prior rename followed by an uncertain directory sync is retried
            // only as exact evidence readback, never another native join.
            if !metadata.is_file()
                || metadata.len() > MAX_NATIVE_ARTIFACT_BYTES as u64
                || fs::read(&path).map_err(|_| unavailable())? != bytes
            {
                return Err(unavailable());
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let mut file = tempfile::NamedTempFile::new_in(directory).map_err(|_| unavailable())?;
            file.write_all(bytes).map_err(|_| unavailable())?;
            file.as_file().sync_all().map_err(|_| unavailable())?;
            file.persist_noclobber(&path).map_err(|_| unavailable())?;
        }
        Err(_) => return Err(unavailable()),
    }
    fs::File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(|_| unavailable())
}

// The shared owner controls use Unix pipes, process groups and filesystem
// primitives. Only the /proc census control inside the module is Linux-only.
#[cfg(all(test, unix))]
mod tests;
