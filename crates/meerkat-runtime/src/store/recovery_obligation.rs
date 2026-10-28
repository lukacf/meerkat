//! Whether a runtime holds recovery work: the one predicate a boot asks.
//!
//! A boot learns which runtimes recovery must act on without loading their
//! bodies. Three small facts kept beside each runtime decide it: its boot
//! facts ([`BootFactsV1`]), its machine lifecycle record and its session
//! catalog entry. [`runtime_has_recovery_obligation`] reads them. The host
//! store adapter's open asks it to choose the bodies it loads, and every
//! store's obligation scan
//! ([`RuntimeStore::list_runtimes_with_recovery_work`](super::RuntimeStore::list_runtimes_with_recovery_work))
//! asks it to choose the runtimes it lists, so the two cannot disagree.
//!
//! An archived runtime holds no recovery work. Archived is the session
//! owner's rule ([`runtime_is_archived`]): the catalog entry carries the
//! Archived terminal, or the machine lifecycle record names a Retired or
//! Destroyed runtime. A runtime-backed archive (a retire) records only the
//! second: it commits the lifecycle row and never rewrites the session body
//! the catalog terminal is projected from.
//!
//! The work a runtime's ops lifecycle snapshot owes (an undelivered
//! background operation completion, a running detached job wait:
//! `PersistedOpsSnapshot::holds_recovery_work`) is part of the
//! `nonterminal` fact, so the pinned facts carry it in the wire form every
//! released reader decodes. A writer that folds that work pins the facts in
//! their folded form ([`encode_folded_boot_facts`]); facts pinned by an
//! earlier writer may leave such work out, so a head that carries them is
//! read as holding work ([`boot_facts_predate_ops_fold`]) until the
//! runtime's next commit pins the folded form.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::{MachineLifecycleObservation, RuntimeSessionCatalogEntry, RuntimeStoreError};
use crate::runtime_state::RuntimeState;

/// Facts a boot reads from a runtime's head to decide whether recovery must
/// act on it, refreshed by every commit of the runtime.
///
/// The host store adapter pins them in every runtime head under the entry
/// key `boot` (see `memory::journal_image`); the in-memory and SQLite stores
/// compute the same facts from their own rows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BootFactsV1 {
    pub(crate) v: u32,
    /// The runtime has nonterminal inputs, or work its ops lifecycle snapshot
    /// owes (an undelivered background operation completion, a running
    /// detached job wait).
    pub(crate) nonterminal: bool,
    /// The runtime has pending terminal owners.
    pub(crate) pending_owners: bool,
    /// The runtime has a provisional tail (an open run cursor).
    pub(crate) tail: bool,
    /// The runtime has input rows.
    pub(crate) inputs: bool,
}

impl BootFactsV1 {
    /// The format version every writer stamps and every reader requires.
    pub(crate) const VERSION: u32 = 1;
}

/// The rollback-compatibility marker of boot facts pinned by a writer that
/// folds the ops lifecycle's work into `nonterminal`: one trailing newline
/// after the JSON.
///
/// It is a byte, not a field, because releases before the fold decode the
/// pinned facts strictly (`BootFactsV1` with `deny_unknown_fields` in 0.9.6,
/// `store/memory/journal_image.rs:181`, decoded at `:195` and read by the
/// host store open at `store/spi/runtime.rs:193`, where a decode error fails
/// the open): a typed field would make a rollback unable to open the store.
/// Those releases decode both forms to the same facts (trailing whitespace
/// is valid JSON), pin the compact JSON and rewrite it on every commit of
/// the runtime, so the marker never survives an older writer's commit. A
/// separate entry in the `boot` group (which those releases ignore) would
/// survive their commits unchanged and vouch for facts they rewrote. A head
/// that lost the marker, to an older writer or a JSON normaliser, reads as
/// predating the fold: its runtime is listed as holding work (fail closed)
/// until its next commit. One writer ([`encode_folded_boot_facts`]) and one
/// reader ([`boot_facts_predate_ops_fold`]) use it.
const FOLDED_BOOT_FACTS_TERMINATOR: u8 = b'\n';

/// The boot facts in the form a folding writer pins: the JSON, then the
/// rollback-compatibility marker [`FOLDED_BOOT_FACTS_TERMINATOR`]. The one
/// writer of the marker.
pub(crate) fn encode_folded_boot_facts(
    facts: &BootFactsV1,
) -> Result<Arc<Vec<u8>>, RuntimeStoreError> {
    let mut bytes = serde_json::to_vec(facts).map_err(|error| {
        RuntimeStoreError::WriteFailed(format!("host store encode: boot facts: {error}"))
    })?;
    bytes.push(FOLDED_BOOT_FACTS_TERMINATOR);
    Ok(Arc::new(bytes))
}

/// Whether pinned boot facts lack the rollback-compatibility marker
/// [`FOLDED_BOOT_FACTS_TERMINATOR`]: written before the ops lifecycle's work
/// was folded into them, or normalised since. They may leave out an
/// undelivered background completion or a running detached job wait, so a
/// boot reads their runtime as holding recovery work (fail closed) until its
/// next commit pins the folded form. The one reader of the marker.
pub(crate) fn boot_facts_predate_ops_fold(bytes: &[u8]) -> bool {
    bytes.last() != Some(&FOLDED_BOOT_FACTS_TERMINATOR)
}

/// Whether a lifecycle record names a current run. A record that does not
/// decode counts as one: recovery must look at it.
pub(crate) fn lifecycle_has_current_run(bytes: Option<&[u8]>) -> bool {
    match bytes.map(super::classify_machine_lifecycle_record) {
        None => false,
        Some(MachineLifecycleObservation::Decoded { record, .. }) => {
            record.run().current_run_id().is_some()
        }
        Some(_) => true,
    }
}

/// Whether a runtime's session catalog entry marks it archived. The entry's
/// terminal is projected from the session body, which a runtime-backed
/// archive never rewrites: [`runtime_is_archived`] is the whole rule.
pub(crate) fn catalog_entry_is_archived(catalog: Option<&RuntimeSessionCatalogEntry>) -> bool {
    catalog
        .and_then(RuntimeSessionCatalogEntry::lifecycle_terminal)
        .is_some_and(meerkat_core::SessionLifecycleTerminal::is_archived)
}

/// Whether a runtime's machine lifecycle record names an absorbing terminal:
/// a Retired or Destroyed runtime. A record that does not decode, or names
/// no state, is not taken as one ([`lifecycle_has_current_run`] reads such a
/// record as work: recovery must look at it).
pub(crate) fn lifecycle_is_archived(bytes: Option<&[u8]>) -> bool {
    match bytes.map(super::classify_machine_lifecycle_record) {
        Some(MachineLifecycleObservation::Decoded { record, .. }) => matches!(
            record.runtime_state(),
            Some(RuntimeState::Retired | RuntimeState::Destroyed)
        ),
        _ => false,
    }
}

/// Whether a runtime is archived, by the rule its session owner applies
/// (`PersistentSessionService::session_archived_by_authority`): its catalog
/// entry carries the Archived terminal, or its machine lifecycle record
/// names a Retired or Destroyed runtime. Either committed terminal archives,
/// and a retire commits only the second.
pub(crate) fn runtime_is_archived(
    catalog: Option<&RuntimeSessionCatalogEntry>,
    lifecycle: Option<&[u8]>,
) -> bool {
    catalog_entry_is_archived(catalog) || lifecycle_is_archived(lifecycle)
}

/// Whether recovery must act on a runtime: it holds work recovery owns
/// ([`runtime_holds_recovery_work`]) and it is not archived
/// ([`runtime_is_archived`]).
///
/// `facts` are the runtime's boot facts ([`BootFactsV1::default`] for a
/// runtime that holds nothing), `lifecycle` its machine lifecycle record as
/// stored, `catalog` its session catalog entry. An archived runtime holds no
/// recovery work, whatever its other facts say. A retired runtime's work
/// stays durable and runs once its session is revived (a resume that names
/// it, or the next wake of a mob member still seated on it); a destroyed
/// runtime's session is never revived. The SQLite scan asks the parts
/// itself, so that it decodes a catalog entry or an ops lifecycle snapshot
/// only when the answer needs it.
pub(crate) fn runtime_has_recovery_obligation(
    facts: &BootFactsV1,
    lifecycle: Option<&[u8]>,
    catalog: Option<&RuntimeSessionCatalogEntry>,
) -> bool {
    !runtime_is_archived(catalog, lifecycle) && runtime_holds_recovery_work(facts, lifecycle)
}

/// Whether a runtime holds work recovery owns, archived or not: nonterminal
/// inputs or work its ops lifecycle snapshot owes, pending terminal owners,
/// a provisional tail (an open run cursor), a `lifecycle` record that names
/// a current run or does not decode, or input rows with no lifecycle record.
/// The facts are read first: the lifecycle record is decoded only when they
/// name no work.
pub(crate) fn runtime_holds_recovery_work(facts: &BootFactsV1, lifecycle: Option<&[u8]>) -> bool {
    facts.nonterminal
        || facts.pending_owners
        || facts.tail
        || lifecycle_has_current_run(lifecycle)
        || (facts.inputs && lifecycle.is_none())
}

/// A contract every [`super::RuntimeStore`] obligation scan meets, seeded
/// through the store's own trait methods: one runtime for each kind of
/// recovery work, and runtimes the scan must leave out.
#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
pub(crate) mod scan_contract {
    use std::collections::HashSet;
    use std::sync::Arc;

    use meerkat_core::lifecycle::{InputId, RunId};
    use meerkat_core::ops_lifecycle::{
        CompletionCursorConsumer, OperationId, OperationKind, OperationResult, OperationSource,
        OperationSpec, OpsLifecycleRegistry,
    };
    use meerkat_core::types::{Message, SessionId, UserMessage};
    use meerkat_core::{EpochCursorState, RuntimeEpochId};

    use crate::identifiers::LogicalRuntimeId;
    use crate::input_state::{
        InputLifecycleState, InputStatePersistenceRecord, InputTerminalOutcome, StoredInputState,
    };
    use crate::ops_lifecycle::{PersistedOpsSnapshot, RuntimeOpsLifecycleRegistry};
    use crate::runtime_state::RuntimeState;
    use crate::store::{
        MachineLifecycleBindingFacts, MachineLifecycleCommit, MachineLifecyclePreRunPhase,
        MachineLifecycleRunFacts, PreparedWholeBlobProvisionalTail, RuntimeStore,
        SerializedSessionSnapshot, SupervisorAuthoritySnapshot,
    };

    /// The runtimes [`seed`] wrote, by whether a scan lists them.
    pub(crate) struct ScanFixture {
        /// One runtime for each kind of recovery work.
        pub(crate) listed: HashSet<LogicalRuntimeId>,
        /// An idle runtime, one whose ops lifecycle snapshot's completion
        /// was delivered, and archived runtimes that would otherwise hold
        /// work: a document archived with a nonterminal input, a runtime
        /// retired with an owed completion, an archived document with an
        /// owed completion, and a runtime destroyed with a nonterminal
        /// input.
        pub(crate) unlisted: HashSet<LogicalRuntimeId>,
        /// The listed runtime whose only work is the background operation
        /// completion its ops lifecycle snapshot owes.
        pub(crate) owed_completion: LogicalRuntimeId,
        /// A runtime retired as 0.9.x retires one: its lifecycle names
        /// Retired with everything its unregister clears cleared, its
        /// session body carries no terminal, and it keeps an ops lifecycle
        /// snapshot that owes the same kind of completion.
        pub(crate) retired_owed_completion: LogicalRuntimeId,
        /// A runtime whose session document carries the Archived terminal
        /// (as an imported archived document does), keeping an ops
        /// lifecycle snapshot that owes the same kind of completion.
        pub(crate) document_archived_owed_completion: LogicalRuntimeId,
    }

    impl ScanFixture {
        /// Every runtime the fixture wrote.
        pub(crate) fn runtimes(&self) -> impl Iterator<Item = &LogicalRuntimeId> {
            self.listed.iter().chain(&self.unlisted)
        }
    }

    fn persistable(state: StoredInputState) -> InputStatePersistenceRecord {
        InputStatePersistenceRecord::from_machine_snapshot(state).expect("machine snapshot")
    }

    /// An accepted input: nonterminal.
    fn accepted() -> InputStatePersistenceRecord {
        persistable(StoredInputState::new_accepted(InputId::new()))
    }

    /// `state`, consumed: terminal.
    fn consumed(mut state: StoredInputState) -> InputStatePersistenceRecord {
        state.seed.phase = InputLifecycleState::Consumed;
        state.seed.terminal_outcome = Some(InputTerminalOutcome::Consumed);
        state.seed.recovery_lane = None;
        persistable(state)
    }

    /// A lifecycle bound to `runtime_id`, naming `current_run` or no run.
    fn lifecycle(
        runtime_id: &LogicalRuntimeId,
        current_run: Option<RunId>,
    ) -> MachineLifecycleCommit {
        let state = if current_run.is_some() {
            RuntimeState::Running
        } else {
            RuntimeState::Idle
        };
        MachineLifecycleCommit::new_with_binding_run_and_unregister_progress(
            state,
            MachineLifecycleBindingFacts::new(
                Some(runtime_id.0.clone()),
                Some(1),
                Some(1),
                Some("epoch-1".to_string()),
            ),
            MachineLifecycleRunFacts::new(current_run, Some(MachineLifecyclePreRunPhase::Idle)),
            SupervisorAuthoritySnapshot::UnboundNoReceipt,
            None,
        )
    }

    /// The lifecycle a runtime keeps once it ended in `state` (Retired or
    /// Destroyed) and its unregister completed: no binding, no run, no
    /// unregister progress, as the generated final image records it.
    pub(crate) fn ended_lifecycle(state: RuntimeState) -> MachineLifecycleCommit {
        MachineLifecycleCommit::new_with_binding_run_and_unregister_progress(
            state,
            MachineLifecycleBindingFacts::default(),
            MachineLifecycleRunFacts::default(),
            SupervisorAuthoritySnapshot::UnboundNoReceipt,
            None,
        )
    }

    /// The ops lifecycle snapshot of `registry`, as its runtime persists it.
    pub(crate) fn ops_snapshot(registry: &RuntimeOpsLifecycleRegistry) -> PersistedOpsSnapshot {
        registry
            .capture_persistence_snapshot(RuntimeEpochId::new(), &EpochCursorState::new())
            .expect("the ops lifecycle snapshot")
    }

    /// An ops lifecycle snapshot holding one completed background tool
    /// operation, its completion taken by `delivered_to` or by no consumer.
    pub(crate) fn completed_background_operation(
        delivered_to: Option<CompletionCursorConsumer>,
    ) -> PersistedOpsSnapshot {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let spec = OperationSpec {
            id: OperationId::new(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: SessionId::new(),
            display_name: "scan background operation".into(),
            source_label: "scan".into(),
            operation_source: None,
            child_session_id: None,
            expect_peer_channel: false,
        };
        let operation_id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&operation_id).unwrap();
        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .unwrap();
        if let Some(consumer) = delivered_to {
            let completion = ops_snapshot(&registry)
                .completion_entries
                .iter()
                .map(|entry| entry.seq)
                .max()
                .expect("the completion is in the feed");
            registry
                .advance_completion_cursor(consumer, completion, None)
                .unwrap();
        }
        ops_snapshot(&registry)
    }

    /// An ops lifecycle snapshot holding one detached job wait: running when
    /// `running`, still provisioning otherwise.
    pub(crate) fn detached_job_wait(running: bool) -> PersistedOpsSnapshot {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let owner_session_id = SessionId::new();
        let operation_id =
            OperationId::for_detached_job_wait(&owner_session_id, "scan-realm", "scan-job");
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::DetachedJobWait,
                owner_session_id,
                display_name: "await detached job".into(),
                source_label: "await_job".into(),
                operation_source: Some(OperationSource::detached_job("scan-realm", "scan-job")),
                child_session_id: None,
                expect_peer_channel: false,
            })
            .unwrap();
        if running {
            registry.provisioning_succeeded(&operation_id).unwrap();
        }
        ops_snapshot(&registry)
    }

    /// Commit `session` as its runtime's committed document.
    async fn commit_session(
        store: &dyn RuntimeStore,
        session: &meerkat_core::Session,
    ) -> LogicalRuntimeId {
        let runtime_id = LogicalRuntimeId::for_session(session.id());
        store
            .commit_session_snapshot(
                &runtime_id,
                SerializedSessionSnapshot {
                    session_snapshot: Arc::new(serde_json::to_vec(session).unwrap()),
                },
            )
            .await
            .expect("the committed document");
        runtime_id
    }

    /// Write the fixture into an empty `store`.
    pub(crate) async fn seed(store: &dyn RuntimeStore) -> ScanFixture {
        let mut listed = HashSet::new();
        let mut unlisted = HashSet::new();

        // Nonterminal inputs, beside a lifecycle with no run.
        let nonterminal = LogicalRuntimeId::new("scan-nonterminal-inputs");
        store
            .commit_machine_lifecycle(&nonterminal, lifecycle(&nonterminal, None), &[])
            .await
            .unwrap();
        store
            .persist_input_state(&nonterminal, &accepted())
            .await
            .unwrap();
        listed.insert(nonterminal);

        // A pending terminal owner whose own input is terminal, beside a
        // lifecycle with no run.
        let owner = LogicalRuntimeId::new("scan-pending-terminal-owner");
        store
            .commit_machine_lifecycle(&owner, lifecycle(&owner, None), &[])
            .await
            .unwrap();
        let (pending, _) = crate::store::pending_terminal_owner_fixture(InputId::new(), false);
        store
            .persist_input_state(&owner, &consumed(pending))
            .await
            .unwrap();
        listed.insert(owner);

        // A provisional tail: a run's candidate on a committed base, with
        // no input and no lifecycle.
        let mut session = meerkat_core::Session::new();
        session.push(Message::User(UserMessage::text("the committed base")));
        let tail = commit_session(store, &session).await;
        let base = store
            .load_whole_blob_store_authority(&tail)
            .await
            .unwrap()
            .expect("a committed base");
        session.push(Message::User(UserMessage::text("the run's first round")));
        store
            .write_prepared_whole_blob_provisional_tail(
                &tail,
                PreparedWholeBlobProvisionalTail::prepare_from_session(
                    base,
                    RunId::new(),
                    1,
                    &session,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        listed.insert(tail);

        // A lifecycle that names a current run, and nothing else.
        let running = LogicalRuntimeId::new("scan-current-run");
        store
            .commit_machine_lifecycle(&running, lifecycle(&running, Some(RunId::new())), &[])
            .await
            .unwrap();
        listed.insert(running);

        // Terminal input rows and no lifecycle record.
        let orphan = LogicalRuntimeId::new("scan-inputs-without-lifecycle");
        store
            .persist_input_state(
                &orphan,
                &consumed(StoredInputState::new_accepted(InputId::new())),
            )
            .await
            .unwrap();
        listed.insert(orphan);

        // Idle: terminal inputs, a lifecycle with no run, no tail.
        let idle = LogicalRuntimeId::new("scan-idle");
        store
            .commit_machine_lifecycle(&idle, lifecycle(&idle, None), &[])
            .await
            .unwrap();
        store
            .persist_input_state(
                &idle,
                &consumed(StoredInputState::new_accepted(InputId::new())),
            )
            .await
            .unwrap();
        unlisted.insert(idle);

        // Archived, though it still holds a nonterminal input.
        let mut archived_session = meerkat_core::Session::new();
        archived_session.push(Message::User(UserMessage::text("an archived session")));
        let archived = LogicalRuntimeId::for_session(archived_session.id());
        store
            .persist_input_state(&archived, &accepted())
            .await
            .unwrap();
        archived_session
            .set_lifecycle_terminal(meerkat_core::SessionLifecycleTerminal::Archived)
            .unwrap();
        commit_session(store, &archived_session).await;
        unlisted.insert(archived);

        // An ops lifecycle snapshot holding a background operation
        // completion its session was not woken for, and nothing else.
        let undelivered = LogicalRuntimeId::new("scan-ops-undelivered-completion");
        store
            .persist_ops_lifecycle(&undelivered, &completed_background_operation(None))
            .await
            .unwrap();
        listed.insert(undelivered.clone());

        // Retired as 0.9.x retires a runtime, keeping an ops lifecycle
        // snapshot that owes the same kind of completion: the retire commits
        // the lifecycle row, its unregister keeps the snapshot, and the
        // session body is never rewritten. An archived runtime holds no
        // recovery work, whatever its snapshot says.
        let mut retired_session = meerkat_core::Session::new();
        retired_session.push(Message::User(UserMessage::text("a retired session")));
        let retired_owed = commit_session(store, &retired_session).await;
        store
            .persist_ops_lifecycle(&retired_owed, &completed_background_operation(None))
            .await
            .unwrap();
        store
            .commit_machine_lifecycle(&retired_owed, ended_lifecycle(RuntimeState::Retired), &[])
            .await
            .unwrap();
        unlisted.insert(retired_owed.clone());

        // The same owed completion beside a session document that carries
        // the Archived terminal itself, as an imported archived document
        // does.
        let mut archived_document = meerkat_core::Session::new();
        archived_document.push(Message::User(UserMessage::text("an archived document")));
        let document_archived_owed = LogicalRuntimeId::for_session(archived_document.id());
        store
            .persist_ops_lifecycle(
                &document_archived_owed,
                &completed_background_operation(None),
            )
            .await
            .unwrap();
        archived_document
            .set_lifecycle_terminal(meerkat_core::SessionLifecycleTerminal::Archived)
            .unwrap();
        commit_session(store, &archived_document).await;
        unlisted.insert(document_archived_owed.clone());

        // Destroyed, though it still holds a nonterminal input.
        let destroyed = LogicalRuntimeId::new("scan-destroyed-nonterminal-input");
        store
            .persist_input_state(&destroyed, &accepted())
            .await
            .unwrap();
        store
            .commit_machine_lifecycle(&destroyed, ended_lifecycle(RuntimeState::Destroyed), &[])
            .await
            .unwrap();
        unlisted.insert(destroyed);

        // An ops lifecycle snapshot holding a running detached job wait, and
        // nothing else.
        let waiting = LogicalRuntimeId::new("scan-ops-running-detached-job-wait");
        store
            .persist_ops_lifecycle(&waiting, &detached_job_wait(true))
            .await
            .unwrap();
        listed.insert(waiting);

        // An ops lifecycle snapshot whose completion the runtime injected,
        // beside a lifecycle with no run.
        let delivered = LogicalRuntimeId::new("scan-ops-delivered-completion");
        store
            .commit_machine_lifecycle(&delivered, lifecycle(&delivered, None), &[])
            .await
            .unwrap();
        store
            .persist_ops_lifecycle(
                &delivered,
                &completed_background_operation(Some(CompletionCursorConsumer::RuntimeInjected)),
            )
            .await
            .unwrap();
        unlisted.insert(delivered);

        ScanFixture {
            listed,
            unlisted,
            owed_completion: undelivered,
            retired_owed_completion: retired_owed,
            document_archived_owed_completion: document_archived_owed,
        }
    }

    /// The scan of a store that holds exactly `fixture` lists each runtime
    /// with recovery work once, and no other runtime.
    pub(crate) async fn assert_lists_exactly(store: &dyn RuntimeStore, fixture: &ScanFixture) {
        let listed = store
            .list_runtimes_with_recovery_work()
            .await
            .expect("the scan answers");
        let unique: HashSet<LogicalRuntimeId> = listed.iter().cloned().collect();
        assert_eq!(
            unique.len(),
            listed.len(),
            "each runtime is listed once: {listed:?}"
        );
        assert!(
            unique.contains(&fixture.owed_completion),
            "a completion its ops lifecycle snapshot owes is recovery work"
        );
        assert!(
            !unique.contains(&fixture.retired_owed_completion),
            "a retired runtime holds no recovery work, whatever its kept snapshot owes"
        );
        assert!(
            !unique.contains(&fixture.document_archived_owed_completion),
            "an archived document holds no recovery work, whatever its kept snapshot owes"
        );
        assert_eq!(
            unique, fixture.listed,
            "the scan lists exactly the runtimes with recovery work"
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use meerkat_core::SessionLifecycleTerminal;
    use meerkat_core::lifecycle::RunId;

    use super::*;
    use crate::runtime_state::RuntimeState;
    use crate::store::{
        MachineLifecycleBindingFacts, MachineLifecycleCommit, MachineLifecyclePreRunPhase,
        MachineLifecycleRunFacts, RuntimeSessionPersistenceProfile, SupervisorAuthoritySnapshot,
    };

    fn facts(nonterminal: bool, pending_owners: bool, tail: bool, inputs: bool) -> BootFactsV1 {
        BootFactsV1 {
            v: BootFactsV1::VERSION,
            nonterminal,
            pending_owners,
            tail,
            inputs,
        }
    }

    /// The stored bytes of a lifecycle naming `current_run` or no run.
    fn lifecycle(current_run: Option<RunId>) -> Vec<u8> {
        let state = if current_run.is_some() {
            RuntimeState::Running
        } else {
            RuntimeState::Idle
        };
        MachineLifecycleCommit::new_with_binding_run_and_unregister_progress(
            state,
            MachineLifecycleBindingFacts::new(
                Some("predicate-runtime".to_string()),
                Some(1),
                Some(1),
                Some("epoch-1".to_string()),
            ),
            MachineLifecycleRunFacts::new(current_run, Some(MachineLifecyclePreRunPhase::Idle)),
            SupervisorAuthoritySnapshot::UnboundNoReceipt,
            None,
        )
        .store_record()
        .encode()
        .unwrap()
    }

    /// The stored bytes of a lifecycle in `state`, with no binding and no
    /// run.
    fn lifecycle_in(state: RuntimeState) -> Vec<u8> {
        MachineLifecycleCommit::new_with_binding_run_and_unregister_progress(
            state,
            MachineLifecycleBindingFacts::default(),
            MachineLifecycleRunFacts::default(),
            SupervisorAuthoritySnapshot::UnboundNoReceipt,
            None,
        )
        .store_record()
        .encode()
        .unwrap()
    }

    fn catalog(terminal: Option<SessionLifecycleTerminal>) -> RuntimeSessionCatalogEntry {
        let mut session = meerkat_core::Session::new();
        if let Some(terminal) = terminal {
            session.set_lifecycle_terminal(terminal).unwrap();
        }
        RuntimeSessionCatalogEntry::from_session(
            &session,
            RuntimeSessionPersistenceProfile::WholeBlobV1,
            None,
        )
        .unwrap()
    }

    /// Boot facts pinned in the folded form decode as the compact form an
    /// earlier writer pinned (a reader of either version reads both), and
    /// only the folded form carries the marker: an earlier writer's compact
    /// form and a JSON normaliser's rewrite of the folded form both read as
    /// predating the ops fold.
    #[test]
    fn boot_obligation_scan_folded_boot_facts_are_told_apart_from_an_earlier_writers() {
        let pinned = facts(false, true, false, true);
        let folded = encode_folded_boot_facts(&pinned).unwrap();
        let compact = serde_json::to_vec(&pinned).unwrap();
        let normalised =
            serde_json::to_vec(&serde_json::from_slice::<serde_json::Value>(&folded).unwrap())
                .unwrap();
        for bytes in [folded.as_slice(), compact.as_slice(), normalised.as_slice()] {
            assert_eq!(
                serde_json::from_slice::<BootFactsV1>(bytes).unwrap(),
                pinned
            );
        }
        assert!(!boot_facts_predate_ops_fold(&folded));
        assert!(boot_facts_predate_ops_fold(&compact));
        assert!(boot_facts_predate_ops_fold(&normalised));
    }

    /// An ops lifecycle snapshot holds work while a completion has reached
    /// no consumer or a detached job wait that recovery keeps is running.
    #[test]
    fn boot_obligation_scan_ops_snapshot_holds_work_until_its_completion_is_delivered() {
        use meerkat_core::ops_lifecycle::CompletionCursorConsumer;

        use super::scan_contract::{
            completed_background_operation, detached_job_wait, ops_snapshot,
        };
        use crate::ops_lifecycle::RuntimeOpsLifecycleRegistry;

        let rows = [
            (
                "no operation",
                ops_snapshot(&RuntimeOpsLifecycleRegistry::new()),
                false,
            ),
            (
                "a background operation completion no consumer took",
                completed_background_operation(None),
                true,
            ),
            (
                "a completion the runtime injected",
                completed_background_operation(Some(CompletionCursorConsumer::RuntimeInjected)),
                false,
            ),
            (
                "a completion the agent applied",
                completed_background_operation(Some(CompletionCursorConsumer::AgentApplied)),
                false,
            ),
            ("a running detached job wait", detached_job_wait(true), true),
            (
                "a detached job wait still provisioning, which recovery discards",
                detached_job_wait(false),
                false,
            ),
        ];
        for (name, snapshot, expected) in rows {
            assert_eq!(snapshot.holds_recovery_work(), expected, "{name}");
        }
    }

    /// Each kind of work, alone, makes an obligation; no work makes none;
    /// and an archived catalog entry makes none whatever the facts say.
    #[test]
    fn boot_obligation_scan_predicate_names_each_kind_of_work_and_no_archived_runtime() {
        let idle = lifecycle(None);
        let running = lifecycle(Some(RunId::new()));
        let undecodable = b"not a lifecycle record".to_vec();
        let rows: [(&str, BootFactsV1, Option<&[u8]>, bool); 10] = [
            ("nothing", facts(false, false, false, false), None, false),
            (
                "a lifecycle with no run",
                facts(false, false, false, false),
                Some(idle.as_slice()),
                false,
            ),
            (
                "terminal inputs beside a lifecycle with no run",
                facts(false, false, false, true),
                Some(idle.as_slice()),
                false,
            ),
            (
                "nonterminal inputs",
                facts(true, false, false, true),
                Some(idle.as_slice()),
                true,
            ),
            (
                "pending terminal owners",
                facts(false, true, false, true),
                Some(idle.as_slice()),
                true,
            ),
            (
                "a provisional tail",
                facts(false, false, true, false),
                None,
                true,
            ),
            (
                "a lifecycle that names a current run",
                facts(false, false, false, false),
                Some(running.as_slice()),
                true,
            ),
            (
                "a lifecycle that does not decode",
                facts(false, false, false, false),
                Some(undecodable.as_slice()),
                true,
            ),
            (
                "input rows with no lifecycle",
                facts(false, false, false, true),
                None,
                true,
            ),
            (
                "every kind of work",
                facts(true, true, true, true),
                Some(running.as_slice()),
                true,
            ),
        ];
        let unmarked = catalog(None);
        let active = catalog(Some(SessionLifecycleTerminal::Active));
        let archived = catalog(Some(SessionLifecycleTerminal::Archived));
        for (name, facts, lifecycle, expected) in &rows {
            for (entry_name, entry) in [
                ("no catalog entry", None),
                ("a catalog entry with no terminal", Some(&unmarked)),
                ("an active catalog entry", Some(&active)),
            ] {
                assert_eq!(
                    runtime_has_recovery_obligation(facts, *lifecycle, entry),
                    *expected,
                    "{name} with {entry_name}"
                );
            }
            assert!(
                !runtime_has_recovery_obligation(facts, *lifecycle, Some(&archived)),
                "{name}: an archived runtime holds no recovery work"
            );
        }
    }

    /// A Retired or Destroyed runtime is archived by its lifecycle record
    /// alone (a retire never writes the catalog terminal), and holds no
    /// recovery work whatever its facts and catalog entry say. A runtime
    /// stopped by runtime control, or one whose record does not decode, is
    /// not archived.
    #[test]
    fn boot_obligation_scan_predicate_reads_a_retired_or_destroyed_lifecycle_as_archived() {
        let retired = lifecycle_in(RuntimeState::Retired);
        let destroyed = lifecycle_in(RuntimeState::Destroyed);
        let stopped = lifecycle_in(RuntimeState::Stopped);
        let idle = lifecycle(None);
        let undecodable = b"not a lifecycle record".to_vec();
        let unmarked = catalog(None);
        let active = catalog(Some(SessionLifecycleTerminal::Active));
        let rows: [(&str, Option<&[u8]>, bool); 6] = [
            ("no lifecycle record", None, false),
            ("an idle lifecycle", Some(idle.as_slice()), false),
            ("a stopped lifecycle", Some(stopped.as_slice()), false),
            (
                "a lifecycle that does not decode",
                Some(undecodable.as_slice()),
                false,
            ),
            ("a retired lifecycle", Some(retired.as_slice()), true),
            ("a destroyed lifecycle", Some(destroyed.as_slice()), true),
        ];
        for (name, lifecycle, archived) in rows {
            assert_eq!(lifecycle_is_archived(lifecycle), archived, "{name}");
            for entry in [None, Some(&unmarked), Some(&active)] {
                assert_eq!(
                    runtime_is_archived(entry, lifecycle),
                    archived,
                    "{name} beside {entry:?}"
                );
            }
        }
        let every_kind_of_work = facts(true, true, true, true);
        for (name, lifecycle) in [("retired", &retired), ("destroyed", &destroyed)] {
            for entry in [None, Some(&unmarked), Some(&active)] {
                assert!(
                    !runtime_has_recovery_obligation(
                        &every_kind_of_work,
                        Some(lifecycle.as_slice()),
                        entry
                    ),
                    "a {name} runtime holds no recovery work, beside {entry:?}"
                );
            }
        }
    }
}
