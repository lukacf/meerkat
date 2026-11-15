//! How much of the mob's event log one start decodes.
//!
//! A start reads the log whole once, in the resume's first read. Every later
//! read of the same start (the remote-turn custody reload, the actor's index
//! seed, the launched handle's first member binding history, each rebuilt
//! member's continuity intent) reads only the events appended after that
//! first read. The rows count the reads a start makes through the event
//! store and the identity member store, both over the same journal:
//!
//! - a whole read is a read that starts at the log's first event
//!   (`replay_all`, a `poll` after cursor 0, a member binding journal read
//!   from cursor 0): one per start;
//! - the events all reads return together stay below two log lengths, so no
//!   second pass hides in pages.
//!
//! The correctness rows hold what each of those reads saw against the
//! journal itself: an event another writer appends after the first read is
//! in every later read, in journal order. The rows on the read itself
//! (`LaterLogRead`, `ResumeEventLog`) hold its two forms against a whole
//! replay, a journal rewritten since the first read included. A Stop and a
//! lift check the journal's latest quiesce marker without a whole replay,
//! and journal each marker once.

use super::*;
use crate::machines::mob_machine::PlacedCompletionLifecycleIntentKind;
use crate::runtime::MemberRestorePolicy;
use crate::runtime::builder::{LaterLogRead, ResumeEventLog};
use crate::store::private::MobEventStoreSealed as _;

/// Bound for one step; it only turns a hang into a failure.
const STEP: Duration = Duration::from_mins(1);

/// The members of the mob.
const MEMBERS: usize = 6;

/// Journal events appended on top of the members' own, so the log is long
/// against the handful of events a start appends and rereads.
const HISTORY: usize = 240;

fn worker(index: usize) -> AgentIdentity {
    AgentIdentity::from(format!("log-once-{index:02}"))
}

fn turn_driven(identity: &AgentIdentity) -> SpawnMemberSpec {
    let mut spec = SpawnMemberSpec::new("worker", identity.clone());
    spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    spec
}

fn quiet_event(mob_id: &MobId) -> NewMobEvent {
    NewMobEvent {
        mob_id: mob_id.clone(),
        timestamp: None,
        kind: MobEventKind::TopologyViolation {
            from_role: ProfileName::from("worker"),
            to_role: ProfileName::from("lead"),
        },
    }
}

/// The journal reads of one start.
#[derive(Default)]
struct LogReads {
    /// Each read that started at the log's first event, by name.
    whole: std::sync::Mutex<Vec<&'static str>>,
    /// The events all reads returned together.
    rows: AtomicU64,
}

impl LogReads {
    fn whole(&self) -> Vec<&'static str> {
        self.whole
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn rows(&self) -> u64 {
        self.rows.load(Ordering::SeqCst)
    }

    fn count(&self, whole: Option<&'static str>, rows: usize) {
        if let Some(read) = whole {
            self.whole
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(read);
        }
        self.rows.fetch_add(rows as u64, Ordering::SeqCst);
    }
}

/// A storage's event store, counting its reads. Every other operation is
/// the store's own.
struct CountingEvents {
    inner: Arc<dyn MobEventStore>,
    reads: Arc<LogReads>,
    /// Events another writer appends right after the first whole read
    /// answers, before any later read of the start.
    appended_after_first_read: std::sync::Mutex<Vec<NewMobEvent>>,
    /// The cursors those events were journaled at.
    appended_cursors: std::sync::Mutex<Vec<u64>>,
}

impl CountingEvents {
    /// `inner`, counting its reads from here on.
    fn over(inner: Arc<dyn MobEventStore>) -> (Arc<Self>, Arc<LogReads>) {
        let reads = Arc::new(LogReads::default());
        let events = Arc::new(Self {
            inner,
            reads: Arc::clone(&reads),
            appended_after_first_read: std::sync::Mutex::new(Vec::new()),
            appended_cursors: std::sync::Mutex::new(Vec::new()),
        });
        (events, reads)
    }

    fn appended_cursors(&self) -> Vec<u64> {
        self.appended_cursors
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[async_trait]
impl private::MobEventStoreSealed for CountingEvents {
    fn definition_resume_gate(&self) -> Arc<tokio::sync::RwLock<()>> {
        self.inner.definition_resume_gate()
    }

    async fn acquire_definition_update_claim(
        &self,
    ) -> Result<private::MobDefinitionUpdateClaim, MobStoreError> {
        self.inner.acquire_definition_update_claim().await
    }

    async fn acquire_definition_resume_claim(
        &self,
    ) -> Result<private::MobDefinitionResumeClaim, MobStoreError> {
        self.inner.acquire_definition_resume_claim().await
    }

    async fn append_definition_epoch(
        &self,
        event: NewMobEvent,
        authority: &private::MobDefinitionEpochPersistenceAuthority,
    ) -> Result<private::MobDefinitionEpochAppendOutcome, MobStoreError> {
        self.inner.append_definition_epoch(event, authority).await
    }

    async fn creation_record(
        &self,
    ) -> Result<Option<crate::store::MobCreationRecord>, MobStoreError> {
        self.inner.creation_record().await
    }

    async fn append_creation(
        &self,
        event: NewMobEvent,
        incarnation: meerkat_core::service::MobIncarnationId,
        provenance: &crate::MobProvenance,
    ) -> Result<MobEvent, MobStoreError> {
        self.inner
            .append_creation(event, incarnation, provenance)
            .await
    }

    async fn assign_legacy_incarnation(
        &self,
        mob_id: &MobId,
        incarnation: meerkat_core::service::MobIncarnationId,
    ) -> Result<crate::store::MobCreationRecord, MobStoreError> {
        self.inner
            .assign_legacy_incarnation(mob_id, incarnation)
            .await
    }
}

#[async_trait]
impl MobEventStore for CountingEvents {
    async fn begin_external_delivery(
        &self,
        intent: &crate::store::MobExternalDeliveryIntent,
    ) -> Result<crate::store::MobExternalDeliveryBeginOutcome, MobStoreError> {
        self.inner.begin_external_delivery(intent).await
    }

    async fn complete_external_delivery(
        &self,
        intent: &crate::store::MobExternalDeliveryIntent,
        terminal: &crate::store::MobExternalDeliveryTerminal,
    ) -> Result<crate::store::MobExternalDeliveryCompleteOutcome, MobStoreError> {
        self.inner
            .complete_external_delivery(intent, terminal)
            .await
    }

    async fn claim_external_delivery_realization(
        &self,
        commit: &crate::store::MobExternalDeliveryRealizationCommit,
    ) -> Result<crate::store::MobExternalDeliveryClaimOutcome, MobStoreError> {
        self.inner.claim_external_delivery_realization(commit).await
    }

    async fn complete_external_delivery_realization(
        &self,
        commit: &crate::store::MobExternalDeliveryRealizerCompletionCommit,
    ) -> Result<crate::store::MobExternalDeliveryCompleteOutcome, MobStoreError> {
        self.inner
            .complete_external_delivery_realization(commit)
            .await
    }

    async fn abandon_external_delivery(
        &self,
        intent: &crate::store::MobExternalDeliveryIntent,
        terminal: &crate::store::MobExternalDeliveryTerminal,
    ) -> Result<crate::store::MobExternalDeliveryAbandonOutcome, MobStoreError> {
        self.inner.abandon_external_delivery(intent, terminal).await
    }

    async fn resolve_external_delivery_after_realizer_fenced(
        &self,
        commit: &crate::store::MobExternalDeliveryFencedResolutionCommit,
    ) -> Result<crate::store::MobExternalDeliveryAbandonOutcome, MobStoreError> {
        self.inner
            .resolve_external_delivery_after_realizer_fenced(commit)
            .await
    }

    async fn schedule_external_delivery_repair(
        &self,
        intent: &crate::store::MobExternalDeliveryIntent,
    ) -> Result<crate::store::MobExternalDeliveryRepairOutcome, MobStoreError> {
        self.inner.schedule_external_delivery_repair(intent).await
    }

    async fn load_external_delivery(
        &self,
        mob_id: &MobId,
        idempotency_key: &str,
    ) -> Result<Option<crate::store::MobExternalDeliveryRecord>, MobStoreError> {
        self.inner
            .load_external_delivery(mob_id, idempotency_key)
            .await
    }

    async fn append(&self, event: NewMobEvent) -> Result<MobEvent, MobStoreError> {
        self.inner.append(event).await
    }

    async fn append_terminal_event_if_absent(
        &self,
        event: NewMobEvent,
    ) -> Result<Option<MobEvent>, MobStoreError> {
        self.inner.append_terminal_event_if_absent(event).await
    }

    async fn append_step_failed_event_if_absent(
        &self,
        event: NewMobEvent,
    ) -> Result<Option<MobEvent>, MobStoreError> {
        self.inner.append_step_failed_event_if_absent(event).await
    }

    async fn append_fork_job_terminal_if_absent(
        &self,
        event: NewMobEvent,
    ) -> Result<Option<MobEvent>, MobStoreError> {
        self.inner.append_fork_job_terminal_if_absent(event).await
    }

    async fn append_batch(&self, events: Vec<NewMobEvent>) -> Result<Vec<MobEvent>, MobStoreError> {
        self.inner.append_batch(events).await
    }

    async fn poll(&self, after_cursor: u64, limit: usize) -> Result<Vec<MobEvent>, MobStoreError> {
        let events = self.inner.poll(after_cursor, limit).await?;
        self.reads.count(
            (after_cursor == 0 && !events.is_empty()).then_some("poll after cursor 0"),
            events.len(),
        );
        Ok(events)
    }

    async fn replay_all(&self) -> Result<Vec<MobEvent>, MobStoreError> {
        let events = self.inner.replay_all().await?;
        self.reads.count(Some("replay_all"), events.len());
        // Another writer's appends land after this read answered and before
        // the start reads the journal again.
        let appended = std::mem::take(
            &mut *self
                .appended_after_first_read
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        for event in appended {
            let stored = self.inner.append(event).await?;
            self.appended_cursors
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(stored.cursor);
        }
        Ok(events)
    }

    async fn latest_cursor(&self) -> Result<u64, MobStoreError> {
        self.inner.latest_cursor().await
    }

    fn subscribe(&self) -> Result<crate::store::MobEventReceiver, MobStoreError> {
        self.inner.subscribe()
    }

    async fn clear(&self) -> Result<(), MobStoreError> {
        self.inner.clear().await
    }

    async fn prune(&self, older_than: chrono::DateTime<Utc>) -> Result<u64, MobStoreError> {
        self.inner.prune(older_than).await
    }
}

/// A storage's identity member store, counting its journal reads. Every
/// other operation is the store's own.
struct CountingJournal {
    inner: Arc<dyn crate::store::MobIdentityMemberStore>,
    reads: Arc<LogReads>,
}

#[async_trait]
impl crate::store::MobIdentityMemberStore for CountingJournal {
    async fn observe_identity_member_target(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<crate::store::IdentityMemberTargetObservation, MobStoreError> {
        self.inner
            .observe_identity_member_target(mob_id, identity)
            .await
    }

    async fn commit_identity_member_spawned(
        &self,
        permit: &crate::identity::IdentityActuationPermit,
        event: &NewMobEvent,
    ) -> Result<crate::store::IdentityMemberEventCommitOutcome, MobStoreError> {
        self.inner
            .commit_identity_member_spawned(permit, event)
            .await
    }

    async fn observe_identity_wiring_target(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<crate::store::IdentityWiringTargetObservation, MobStoreError> {
        self.inner
            .observe_identity_wiring_target(mob_id, identity)
            .await
    }

    async fn commit_identity_wiring_event(
        &self,
        permit: &crate::identity::IdentityActuationPermit,
        event: &NewMobEvent,
    ) -> Result<crate::store::IdentityWiringEventCommitOutcome, MobStoreError> {
        self.inner.commit_identity_wiring_event(permit, event).await
    }

    async fn read_member_binding_journal(
        &self,
        mob_id: &MobId,
    ) -> Result<crate::store::MemberBindingJournalRead, MobStoreError> {
        let read = self.inner.read_member_binding_journal(mob_id).await?;
        self.reads
            .count(Some("member binding journal"), read.events.len());
        Ok(read)
    }

    async fn read_member_binding_journal_from(
        &self,
        mob_id: &MobId,
        from_cursor: u64,
    ) -> Result<crate::store::MemberBindingJournalRead, MobStoreError> {
        let read = self
            .inner
            .read_member_binding_journal_from(mob_id, from_cursor)
            .await?;
        self.reads.count(
            (from_cursor == 0).then_some("member binding journal from cursor 0"),
            read.events.len(),
        );
        Ok(read)
    }

    async fn commit_identity_change(
        &self,
        write: &crate::store::IdentityChangeWrite,
    ) -> Result<crate::store::IdentityChangeCommit, MobStoreError> {
        self.inner.commit_identity_change(write).await
    }

    async fn settle_created_identity_row(
        &self,
        row: &crate::store::CreatedIdentityRowWrite,
    ) -> Result<bool, MobStoreError> {
        self.inner.settle_created_identity_row(row).await
    }

    async fn observe_identity_change_receipt(
        &self,
        mob_id: &MobId,
        operation_id: &str,
    ) -> Result<Option<crate::identity::IdentityChangeReceipt>, MobStoreError> {
        self.inner
            .observe_identity_change_receipt(mob_id, operation_id)
            .await
    }

    async fn scan_created_identity_rows(
        &self,
        mob_id: &MobId,
        after: Option<&crate::store::IdentityIntentScanCursor>,
        limit: usize,
    ) -> Result<crate::store::CreatedIdentityScanPage, MobStoreError> {
        self.inner
            .scan_created_identity_rows(mob_id, after, limit)
            .await
    }

    async fn observe_created_identity(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<Option<crate::identity::CreatedIdentityObservation>, MobStoreError> {
        self.inner.observe_created_identity(mob_id, identity).await
    }

    async fn observe_created_identities(
        &self,
        mob_id: &MobId,
        identities: &[AgentIdentity],
    ) -> Result<BTreeMap<AgentIdentity, crate::identity::CreatedIdentityObservation>, MobStoreError>
    {
        self.inner
            .observe_created_identities(mob_id, identities)
            .await
    }

    async fn purge_created_identity_rows(
        &self,
        write: &crate::store::CreatedRowsPurgeWrite,
    ) -> Result<crate::store::CreatedRowsPurge, MobStoreError> {
        self.inner.purge_created_identity_rows(write).await
    }
}

/// How the process before the start ended.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ended {
    /// A Stop landed first: the start resumes a stopped mob and lifts it.
    Stopped,
    /// The process went with the mob running.
    Running,
}

/// A mob of `MEMBERS` turn-driven workers over a long journal, after its
/// process ended, with what a restart finds: the storage, whose journal
/// reads are counted from here on, and the restarted session service.
struct EndedMob {
    mob_id: MobId,
    storage: MobStorage,
    /// The storage's own event store, uncounted.
    journal: Arc<dyn MobEventStore>,
    /// The counting event store `storage` reads through.
    events: Arc<CountingEvents>,
    reads: Arc<LogReads>,
    service: Arc<MockSessionService>,
    members: Vec<(AgentIdentity, SessionId)>,
    /// The journal's length when the process ended.
    log_len: u64,
}

impl EndedMob {
    async fn new(label: &str, ended: Ended) -> Self {
        let definition = with_unique_mob_id(sample_definition(), label);
        let mob_id = definition.id.clone();
        let service = Arc::new(MockSessionService::new());
        let _adapter = service.enable_runtime_adapter();
        let mut storage = MobStorage::in_memory();
        let handle = MobBuilder::new(definition, storage.clone())
            .with_session_service(service.clone())
            .create()
            .await
            .expect("create the mob");
        let mut members = Vec::with_capacity(MEMBERS);
        for index in 0..MEMBERS {
            let identity = worker(index);
            handle
                .spawn_spec(turn_driven(&identity))
                .await
                .expect("spawn a worker");
            let session = handle
                .resolve_bridge_session_id(&identity)
                .await
                .expect("a session-backed worker");
            members.push((identity, session));
        }
        let journal = storage.events.clone();
        for _ in 0..HISTORY {
            journal
                .append(quiet_event(&mob_id))
                .await
                .expect("append a journal event");
        }
        // Another mob's event in the same journal: a start of this mob
        // carries it wherever it carried the whole journal.
        journal
            .append(quiet_event(&MobId::from(format!("{mob_id}-neighbour"))))
            .await
            .expect("append another mob's event");
        if ended == Ended::Stopped {
            handle.stop().await.expect("stop before the restart");
        }
        let restarted = Arc::new(service.cold_restart_preserving_durable_state().await);
        let _ = restarted.enable_runtime_adapter();
        crash_stop_and_release_routes(handle).await;
        drop(service);
        // The create recorded the reads of its own start.
        let _ = crate::runtime::builder::take_start_log_reads_for_test(&mob_id);

        let log_len = journal.replay_all().await.expect("the journal").len() as u64;
        let (events, reads) = CountingEvents::over(Arc::clone(&journal));
        storage.events = Arc::clone(&events) as Arc<dyn MobEventStore>;
        let identity_member = storage
            .identity_member
            .clone()
            .expect("the in-memory storage has the identity member store");
        storage.identity_member = Some(Arc::new(CountingJournal {
            inner: identity_member,
            reads: Arc::clone(&reads),
        }));
        Self {
            mob_id,
            storage,
            journal,
            events,
            reads,
            service: restarted,
            members,
            log_len,
        }
    }

    /// Start the mob as a host does: resume it, lift it when the resume
    /// hands it back stopped, then read its member binding history once (the
    /// launched handle's first such read).
    async fn start(&self, policy: MemberRestorePolicy) -> MobHandle {
        let handle = tokio::time::timeout(
            STEP,
            Box::pin(
                MobBuilder::for_resume(self.storage.clone())
                    .with_session_service(self.service.clone())
                    .notify_orchestrator_on_resume(false)
                    .with_member_restore_policy(policy)
                    .resume(),
            ),
        )
        .await
        .expect("the resume answers")
        .expect("the resume succeeds");
        if handle.status().await.expect("status") == MobState::Stopped {
            tokio::time::timeout(STEP, handle.resume())
                .await
                .expect("the lift answers")
                .expect("the lift succeeds");
        }
        assert_eq!(handle.status().await.expect("status"), MobState::Running);
        handle
            .member_binding_history()
            .await
            .expect("the member binding history");
        handle
    }

    /// The sessions the restarted service built an agent for.
    async fn materialized(&self) -> BTreeSet<SessionId> {
        self.service
            .recorded_create_requests()
            .await
            .into_iter()
            .map(|record| record.session_id)
            .collect()
    }

    fn sessions(&self) -> BTreeSet<SessionId> {
        self.members
            .iter()
            .map(|(_, session)| session.clone())
            .collect()
    }

    /// One start read the journal whole once, and all its reads together
    /// returned less than two journal lengths.
    fn assert_one_whole_read(&self) {
        let whole = self.reads.whole();
        let rows = self.reads.rows();
        assert_eq!(
            whole,
            vec!["replay_all"],
            "the start read the mob's event log whole once, in the resume's first read \
             ({} journal events; all reads returned {rows})",
            self.log_len
        );
        assert!(
            rows < 2 * self.log_len,
            "the start's reads returned {rows} events of a journal of {}: less than two passes",
            self.log_len
        );
    }
}

/// A start under the default restore policy after a clean stop (the resume
/// hands the mob back stopped, the lift materializes no member that holds
/// no work) decodes the journal once.
///
/// RED on the base: four whole reads (the resume's first read, the
/// remote-turn custody reload, the actor's index seed, the first member
/// binding history read).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_default_start_after_a_clean_stop_decodes_the_event_log_once() {
    let mob = EndedMob::new("log-once-default", Ended::Stopped).await;
    let _handle = mob.start(MemberRestorePolicy::OnDemand).await;
    assert!(
        mob.materialized().await.is_empty(),
        "the default start built no member"
    );
    mob.assert_one_whole_read();
}

/// An eager start after a clean stop rebuilds every member in the lift, and
/// still decodes the journal once: no member's rebuild replays it.
///
/// RED on the base: four whole reads and one more per rebuilt member (its
/// continuity intent).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_eager_start_after_a_clean_stop_decodes_the_event_log_once_for_all_its_members() {
    let mob = EndedMob::new("log-once-eager-lift", Ended::Stopped).await;
    let handle = mob.start(MemberRestorePolicy::Eager).await;
    assert_eq!(
        mob.materialized().await,
        mob.sessions(),
        "the eager start rebuilt every member"
    );
    for (identity, _) in &mob.members {
        let snapshot = handle
            .member_status(identity)
            .await
            .unwrap_or_else(|error| panic!("{identity}: member status: {error}"));
        assert_eq!(
            snapshot.status,
            MobMemberStatus::Active,
            "{identity} is rebuilt: {snapshot:?}"
        );
    }
    mob.assert_one_whole_read();
}

/// A start under the default restore policy after an unclean stop (the mob
/// was running) decodes the journal once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_default_start_after_an_unclean_stop_decodes_the_event_log_once() {
    let mob = EndedMob::new("log-once-default-unclean", Ended::Running).await;
    let _handle = mob.start(MemberRestorePolicy::OnDemand).await;
    assert!(
        mob.materialized().await.is_empty(),
        "the default start built no member"
    );
    mob.assert_one_whole_read();
}

/// An eager start after an unclean stop restores every member inside the
/// resume, and decodes the journal once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_eager_start_after_an_unclean_stop_decodes_the_event_log_once() {
    let mob = EndedMob::new("log-once-eager-unclean", Ended::Running).await;
    let _handle = mob.start(MemberRestorePolicy::Eager).await;
    assert_eq!(
        mob.materialized().await,
        mob.sessions(),
        "the eager start restored every member"
    );
    mob.assert_one_whole_read();
}

/// The journal cursors of `events`.
fn cursors(events: &[MobEvent]) -> Vec<u64> {
    events.iter().map(|event| event.cursor).collect()
}

/// An event another writer appends after the resume's first read is in the
/// remote-turn custody reload and in the actor's index seed, each of which
/// saw the journal as it was then: the same events in the same order, the
/// other mob's event included where the whole journal was read.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_event_appended_after_the_first_read_is_in_every_later_read_of_the_start() {
    let mob = EndedMob::new("log-once-appended", Ended::Stopped).await;
    mob.events
        .appended_after_first_read
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(quiet_event(&mob.mob_id));
    let _handle = mob.start(MemberRestorePolicy::OnDemand).await;

    let appended = mob.events.appended_cursors();
    assert_eq!(appended.len(), 1, "the writer appended its event");
    let appended = appended[0];
    let journal = mob.journal.replay_all().await.expect("the journal");
    let whole = cursors(&journal);
    let of_mob = journal
        .iter()
        .filter(|event| event.mob_id == mob.mob_id)
        .map(|event| event.cursor)
        .collect::<Vec<_>>();
    let neighbour = journal
        .iter()
        .find(|event| event.mob_id != mob.mob_id)
        .map(|event| event.cursor)
        .expect("the other mob's event");

    let reads = crate::runtime::builder::take_start_log_reads_for_test(&mob.mob_id);
    let read = |name: &str| {
        reads
            .iter()
            .find(|(read, _)| *read == name)
            .map(|(_, cursors)| cursors.clone())
            .unwrap_or_else(|| panic!("the start made the {name} read: {reads:?}"))
    };

    // No reset in this journal: the mob's current epoch is all its events.
    let custody = read("remote_turn_custody");
    assert!(
        custody.contains(&appended),
        "the custody reload read the appended event"
    );
    assert!(
        of_mob.starts_with(&custody),
        "the custody reload read the mob's events as the journal holds them, in order"
    );

    let actor_start = read("actor_start");
    assert!(
        actor_start.contains(&appended),
        "the actor's index seed read the appended event"
    );
    assert!(
        actor_start.contains(&neighbour),
        "the actor's index seed read the whole journal, the other mob's event included"
    );
    assert!(
        whole.starts_with(&actor_start),
        "the actor's index seed read the journal as it holds it, in order"
    );
}

/// The launched handle's member binding history after a start is the fold of
/// the whole journal, and stays it once a member joins after the start: the
/// fold kept from the start is advanced over the events appended since.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_member_binding_history_after_a_start_folds_the_events_appended_since() {
    let mob = EndedMob::new("log-once-history", Ended::Running).await;
    mob.events
        .appended_after_first_read
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(quiet_event(&mob.mob_id));
    let handle = mob.start(MemberRestorePolicy::OnDemand).await;
    let reference = |whole: crate::store::MemberBindingJournalRead| {
        format!(
            "{:?}",
            MemberBindingHistory::from_journal(&whole.events, whole.intent_rows, Some(&mob.mob_id))
        )
    };
    let journal_store = mob
        .storage
        .identity_member
        .clone()
        .expect("the identity member store");

    let first = handle
        .member_binding_history()
        .await
        .expect("the history after the start");
    let whole = journal_store
        .read_member_binding_journal(&mob.mob_id)
        .await
        .expect("the whole journal");
    assert_eq!(format!("{first:?}"), reference(whole));

    let late = AgentIdentity::from("log-once-late");
    handle
        .spawn_spec(turn_driven(&late))
        .await
        .expect("spawn a member after the start");
    let second = handle
        .member_binding_history()
        .await
        .expect("the history after the spawn");
    assert!(
        second.journal_cursor() > first.journal_cursor(),
        "the spawn appended journal events"
    );
    let whole = journal_store
        .read_member_binding_journal(&mob.mob_id)
        .await
        .expect("the whole journal");
    assert_eq!(format!("{second:?}"), reference(whole));
}

/// A member spawned after the start is rebuilt by a later lift with the
/// continuity intent its own spawn recorded: the lift reads the events
/// appended since the actor's start, not only what the start replayed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lift_rebuilds_a_member_spawned_after_the_start() {
    let mob = EndedMob::new("log-once-late-member", Ended::Running).await;
    let handle = mob.start(MemberRestorePolicy::OnDemand).await;
    let late = AgentIdentity::from("log-once-late");
    handle
        .spawn_spec(turn_driven(&late))
        .await
        .expect("spawn a member after the start");
    let session = handle
        .resolve_bridge_session_id(&late)
        .await
        .expect("a session-backed worker");
    handle.stop().await.expect("stop the mob");
    // Its live session goes while the mob is stopped, its durable session
    // kept: the lift rebuilds the member.
    MobSessionService::discard_live_session(mob.service.as_ref(), &session)
        .await
        .expect("discard the live session");
    let whole_before = mob.reads.whole().len();

    let mut lifted = tokio::time::timeout(STEP, handle.resume())
        .await
        .expect("the lift answers");
    if matches!(
        lifted,
        Err(MobError::LifecycleOperationProgressStalled { .. })
    ) {
        // The unit build's lift observer reports a stall after two seconds
        // without progress; the lift stays joinable.
        lifted = tokio::time::timeout(STEP, handle.resume())
            .await
            .expect("the joined lift answers");
    }
    lifted.expect("the lift succeeds");
    let snapshot = handle
        .member_status(&late)
        .await
        .expect("the late member's status");
    assert_eq!(
        snapshot.status,
        MobMemberStatus::Active,
        "the member spawned after the start is rebuilt: {snapshot:?}"
    );
    assert_eq!(
        mob.reads.whole().len(),
        whole_before,
        "the lift read the journal whole for no member: {:?}",
        mob.reads.whole()
    );
}

/// The quiesce markers of `mob_id` in `journal`, in journal order: a start
/// by its intent, an end as `None`.
async fn quiesce_markers(
    journal: &Arc<dyn MobEventStore>,
    mob_id: &MobId,
) -> Vec<Option<PlacedCompletionLifecycleIntentKind>> {
    journal
        .replay_all()
        .await
        .expect("the journal")
        .iter()
        .filter(|event| event.mob_id == *mob_id)
        .filter_map(|event| match &event.kind {
            MobEventKind::PlacedCompletionLifecycleQuiesceStarted { intent } => Some(Some(*intent)),
            MobEventKind::PlacedCompletionLifecycleQuiesceEnded { .. } => Some(None),
            _ => None,
        })
        .collect()
}

/// A Stop and a lift check the journal's latest quiesce marker before they
/// write their own. Neither replays the journal for it, in the process that
/// started the mob or later, and each marker is journaled once: the Stop's
/// start, the lift's end. A check answered from anything but the journal as
/// it is then would journal a marker twice or leave one out.
///
/// RED on the base: every Stop and every lift replays the journal once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stop_and_a_lift_never_replay_the_journal_and_journal_each_quiesce_marker_once() {
    let stop = Some(PlacedCompletionLifecycleIntentKind::Stop);
    let mob = EndedMob::new("log-once-stop-lift", Ended::Stopped).await;
    assert_eq!(
        quiesce_markers(&mob.journal, &mob.mob_id).await,
        vec![stop],
        "the process before the start ended with its Stop's marker open"
    );
    let handle = mob.start(MemberRestorePolicy::OnDemand).await;
    assert_eq!(
        quiesce_markers(&mob.journal, &mob.mob_id).await,
        vec![stop, None],
        "the start's lift closed it"
    );
    let whole_after_start = mob.reads.whole();
    for round in 0..2 {
        tokio::time::timeout(STEP, handle.stop())
            .await
            .expect("the Stop answers")
            .expect("the Stop succeeds");
        assert_eq!(
            mob.reads.whole(),
            whole_after_start,
            "Stop {round} replayed the journal for no marker check"
        );
        tokio::time::timeout(STEP, handle.resume())
            .await
            .expect("the lift answers")
            .expect("the lift succeeds");
        assert_eq!(
            mob.reads.whole(),
            whole_after_start,
            "lift {round} replayed the journal for no marker check"
        );
    }
    assert_eq!(
        quiesce_markers(&mob.journal, &mob.mob_id).await,
        vec![stop, None, stop, None, stop, None],
        "each Stop journaled its start once and each lift its end once"
    );
}

/// A journal of `own` and `other` events: `own`'s, with one of `other`
/// after every second.
async fn two_mob_journal(label: &str, own: usize) -> (Arc<dyn MobEventStore>, MobId, MobId) {
    let mob_id = MobId::from(format!("{label}-own"));
    let other = MobId::from(format!("{label}-other"));
    let journal: Arc<dyn MobEventStore> = Arc::new(InMemoryMobEventStore::new());
    for index in 0..own {
        journal
            .append(quiet_event(&mob_id))
            .await
            .expect("append the mob's event");
        if index % 2 == 1 {
            journal
                .append(quiet_event(&other))
                .await
                .expect("append the other mob's event");
        }
    }
    (journal, mob_id, other)
}

/// Each event as its `Debug` text: journal order, cursors, timestamps, mobs
/// and kinds at once.
fn rendered<'a>(events: impl IntoIterator<Item = &'a MobEvent>) -> Vec<String> {
    events
        .into_iter()
        .map(|event| format!("{event:?}"))
        .collect()
}

/// Both forms of a later read against a whole replay of `journal`: the
/// mob's events of the log now, and the whole log now.
async fn assert_later_read_is_the_journal(
    journal: &Arc<dyn MobEventStore>,
    log: ResumeEventLog,
    mob_events: Vec<MobEvent>,
    later: LaterLogRead,
    mob_id: &MobId,
) {
    let replay = journal.replay_all().await.expect("the journal");
    assert_eq!(
        rendered(log.mob_events_now(&mob_events, &later, mob_id)),
        rendered(replay.iter().filter(|event| event.mob_id == *mob_id)),
        "the mob's events of the later read are the whole replay's"
    );
    assert_eq!(
        rendered(&log.whole_log_now(mob_events, later)),
        rendered(&replay),
        "the whole log of the later read is the whole replay"
    );
}

/// A later read answers the first read's decode followed by the events
/// appended since, as a whole replay does: the events the start itself
/// appended (which it also pushed onto its own vector), another writer's
/// event between them, and another mob's. It reads the store from the first
/// read's last event on, never whole.
#[tokio::test]
async fn a_later_read_is_the_first_reads_decode_and_the_events_appended_since() {
    let (journal, mob_id, other) = two_mob_journal("later-read", 6).await;
    let (events, reads) = CountingEvents::over(Arc::clone(&journal));
    let first_read = events.replay_all().await.expect("the first read");
    let first_len = first_read.len() as u64;
    let (log, mut mob_events) = ResumeEventLog::of_first_read(first_read, &mob_id);
    assert_eq!(mob_events.len(), 6, "the resume keeps its own mob's events");

    // The start appends an event and keeps it, another writer appends one
    // of this mob and one of the other, and the start appends again.
    let pushed = journal
        .append(quiet_event(&mob_id))
        .await
        .expect("the start's own append");
    mob_events.push(pushed);
    journal
        .append(quiet_event(&mob_id))
        .await
        .expect("another writer's append");
    journal
        .append(quiet_event(&other))
        .await
        .expect("another mob's append");
    let pushed = journal
        .append(quiet_event(&mob_id))
        .await
        .expect("the start's own append");
    mob_events.push(pushed);

    let later = log
        .read_later(events.as_ref())
        .await
        .expect("the later read");
    assert!(
        matches!(&later, LaterLogRead::Appended(appended) if appended.len() == 4),
        "the later read answers the four events appended since"
    );
    assert_eq!(
        reads.whole(),
        vec!["replay_all"],
        "only the first read read the journal whole"
    );
    assert_eq!(
        reads.rows(),
        first_len + 1 + 4,
        "the later read returned the first read's last event and the four after it"
    );
    assert_later_read_is_the_journal(&journal, log, mob_events, later, &mob_id).await;
}

/// A later read pages through a long tail and loses none of it.
#[tokio::test]
async fn a_later_read_pages_through_everything_appended_since() {
    let (journal, mob_id, other) = two_mob_journal("later-read-pages", 4).await;
    let first_read = journal.replay_all().await.expect("the first read");
    let (log, mob_events) = ResumeEventLog::of_first_read(first_read, &mob_id);
    // Three full pages with the first read's last event at their head: the
    // read ends on an empty fourth page.
    for index in 0..767 {
        let mob = if index % 3 == 0 { &other } else { &mob_id };
        journal.append(quiet_event(mob)).await.expect("append");
    }
    let later = log
        .read_later(journal.as_ref())
        .await
        .expect("the later read");
    assert!(
        matches!(&later, LaterLogRead::Appended(appended) if appended.len() == 767),
        "the later read answers every event appended since"
    );
    assert_later_read_is_the_journal(&journal, log, mob_events, later, &mob_id).await;
}

/// A journal cleared and written again since the first read does not hold
/// the first read's last event where it was read: the later read reads the
/// journal whole and answers it, nothing of the first read's decode.
#[tokio::test]
async fn a_later_read_of_a_journal_rewritten_since_reads_it_whole() {
    let (journal, mob_id, other) = two_mob_journal("later-read-rewritten", 6).await;
    let (events, reads) = CountingEvents::over(Arc::clone(&journal));
    let first_read = events.replay_all().await.expect("the first read");
    let (log, mob_events) = ResumeEventLog::of_first_read(first_read, &mob_id);

    journal.clear().await.expect("clear the journal");
    // Longer than the first journal, so an event of another kind sits at
    // every cursor the first read saw, its last one included.
    for index in 0..12 {
        let mob = if index % 4 == 0 { &other } else { &mob_id };
        journal
            .append(NewMobEvent {
                mob_id: mob.clone(),
                timestamp: None,
                kind: MobEventKind::PlacedCompletionLifecycleQuiesceEnded {
                    intent: PlacedCompletionLifecycleIntentKind::Stop,
                },
            })
            .await
            .expect("append to the new journal");
    }
    let later = log
        .read_later(events.as_ref())
        .await
        .expect("the later read");
    assert!(
        matches!(&later, LaterLogRead::Whole(whole) if whole.len() == 12),
        "the later read answers the new journal whole"
    );
    assert_eq!(
        reads.whole().len(),
        2,
        "the later read read the journal whole: {:?}",
        reads.whole()
    );
    assert_later_read_is_the_journal(&journal, log, mob_events, later, &mob_id).await;
}

/// A first read of an empty log leaves nothing to read from: the later read
/// reads the journal whole.
#[tokio::test]
async fn a_later_read_after_an_empty_first_read_reads_the_journal_whole() {
    let journal: Arc<dyn MobEventStore> = Arc::new(InMemoryMobEventStore::new());
    let mob_id = MobId::from("later-read-empty");
    let (log, mob_events) = ResumeEventLog::of_first_read(Vec::new(), &mob_id);
    for _ in 0..3 {
        journal.append(quiet_event(&mob_id)).await.expect("append");
    }
    let later = log
        .read_later(journal.as_ref())
        .await
        .expect("the later read");
    assert!(matches!(&later, LaterLogRead::Whole(whole) if whole.len() == 3));
    assert_later_read_is_the_journal(&journal, log, mob_events, later, &mob_id).await;
}

/// The same over a store file: a later read of a SQLite journal answers
/// what its whole replay answers, for a tail longer than a page and for a
/// journal rewritten since the first read.
#[tokio::test]
async fn a_later_read_of_a_sqlite_journal_is_its_whole_replay() {
    let dir = tempfile::tempdir().expect("a directory for the store file");
    let journal: Arc<dyn MobEventStore> = Arc::new(
        SqliteMobStores::open(dir.path().join("mob.sqlite"))
            .expect("open the mob stores")
            .event_store(),
    );
    let mob_id = MobId::from("later-read-sqlite-own");
    let other = MobId::from("later-read-sqlite-other");
    for index in 0..9 {
        let mob = if index % 3 == 2 { &other } else { &mob_id };
        journal.append(quiet_event(mob)).await.expect("append");
    }
    let first_read = journal.replay_all().await.expect("the first read");
    let (log, mob_events) = ResumeEventLog::of_first_read(first_read, &mob_id);
    for index in 0..300 {
        let mob = if index % 5 == 0 { &other } else { &mob_id };
        journal.append(quiet_event(mob)).await.expect("append");
    }
    let later = log
        .read_later(journal.as_ref())
        .await
        .expect("the later read");
    assert!(
        matches!(&later, LaterLogRead::Appended(appended) if appended.len() == 300),
        "the later read answers every event appended since"
    );
    assert_later_read_is_the_journal(&journal, log, mob_events, later, &mob_id).await;

    let first_read = journal.replay_all().await.expect("the first read");
    let (log, mob_events) = ResumeEventLog::of_first_read(first_read, &mob_id);
    journal.clear().await.expect("clear the journal");
    for _ in 0..4 {
        journal
            .append(quiet_event(&mob_id))
            .await
            .expect("append to the new journal");
    }
    let later = log
        .read_later(journal.as_ref())
        .await
        .expect("the later read");
    assert!(
        matches!(&later, LaterLogRead::Whole(whole) if whole.len() == 4),
        "the later read answers the new journal whole"
    );
    assert_later_read_is_the_journal(&journal, log, mob_events, later, &mob_id).await;
}
