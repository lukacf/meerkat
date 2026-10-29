//! Identity entries: the entry's name reservation, its exclusion from the
//! worker reconcile, the agent lane's refusal, the rows that restore it after
//! a restart, the legacy adoption and the reads that need no actor command.
//! They run on the combined candidate: the machine, the actor, the journal
//! and the replay together.

use super::*;
use crate::control_policy::CommandAuthority;
use crate::machines::mob_machine as dsl;
use crate::runtime::{
    DormantSeatOutcome, EntryStanding, IDENTITY_ENTRY_PAGE_MAX, IdentityEntryOrigin,
    IdentityEntryRead, LegacyIdentityEntryAdoption, MemberLifecycleEvent, MobJournalCursor,
    ReconcileOptions,
};

const STEP: Duration = Duration::from_mins(1);

fn id(name: &str) -> AgentIdentity {
    AgentIdentity::from(name)
}

/// A turn-driven worker spec: it owes no kickoff, so a seat of it is seated
/// Dormant.
fn worker_spec(name: &str) -> SpawnMemberSpec {
    let mut spec = SpawnMemberSpec::new("worker", id(name));
    spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    spec
}

/// `worker_spec` acting for the identity entry of its name.
fn entry_spec(name: &str) -> SpawnMemberSpec {
    worker_spec(name).with_entry_custody(true)
}

/// `worker_spec` carrying the legacy identity label naming `named`.
fn labelled_spec(name: &str, named: &str) -> SpawnMemberSpec {
    let mut spec = worker_spec(name);
    spec.labels = Some(BTreeMap::from([(
        crate::runtime::identity_entry::LEGACY_IDENTITY_LABEL.to_string(),
        named.to_string(),
    )]));
    spec
}

fn is_identity_member(error: &MobError, name: &str) -> bool {
    matches!(error, MobError::IdentityMember { member_id } if member_id.as_str() == name)
}

/// A running mob over the mock durable session service and in-memory
/// storage, with session-scoped member comms identities. A restart resumes
/// the same storage, its identity/member store included.
struct EntryMob {
    keys: tempfile::TempDir,
    storage: MobStorage,
    handle: MobHandle,
    service: Arc<MockSessionService>,
}

impl EntryMob {
    async fn new(label: &str) -> Self {
        let keys = tempfile::tempdir().expect("a comms identity root");
        let service = Arc::new(MockSessionService::new());
        let _adapter = service.enable_runtime_adapter();
        let storage = MobStorage::in_memory();
        let handle = MobBuilder::new(
            with_unique_mob_id(sample_definition(), label),
            storage.clone(),
        )
        .with_session_service(service.clone())
        .with_member_comms_identity_root(keys.path())
        .create()
        .await
        .expect("create the mob");
        Self {
            keys,
            storage,
            handle,
            service,
        }
    }

    /// The handle on the agent lane: the lane the member-tool dispatcher
    /// sends on.
    fn agent(&self) -> MobHandle {
        self.handle
            .clone()
            .with_command_authority(CommandAuthority::agent_lane())
    }

    /// Crash the mob, restart its session service and resume `storage`.
    async fn restart_into(&mut self, storage: MobStorage) {
        crash_stop_and_release_routes(self.handle.clone()).await;
        let restarted = Arc::new(self.service.cold_restart_preserving_durable_state().await);
        let _ = restarted.enable_runtime_adapter();
        self.service = restarted;
        self.handle = tokio::time::timeout(
            STEP,
            Box::pin(
                MobBuilder::for_resume(storage)
                    .with_session_service(self.service.clone())
                    .with_member_comms_identity_root(self.keys.path())
                    .notify_orchestrator_on_resume(false)
                    .resume(),
            ),
        )
        .await
        .expect("the restart answers")
        .expect("the restart succeeds");
    }

    /// Crash the mob and resume the same storage.
    async fn restart(&mut self) {
        let storage = self.storage.clone();
        self.restart_into(storage).await;
    }

    /// The entry's origin when `name` is an identity entry.
    fn entry(&self, name: &str) -> Option<IdentityEntryOrigin> {
        match self
            .handle
            .identity_entry(&id(name))
            .expect("the entry read answers")
        {
            IdentityEntryRead::Entry(view) => Some(view.origin),
            IdentityEntryRead::NotAnEntry { .. } => None,
        }
    }

    async fn seat(&self, spec: SpawnMemberSpec) -> SessionId {
        let outcomes = self
            .handle
            .seat_dormant_members(vec![spec])
            .await
            .expect("the seat batch answers");
        match &outcomes[0] {
            DormantSeatOutcome::Seated { session_id, .. } => session_id.clone(),
            other => panic!("the spec is seated: {other:?}"),
        }
    }

    async fn journal_len(&self) -> usize {
        self.storage
            .events
            .poll(0, usize::MAX)
            .await
            .expect("read the journal")
            .len()
    }

    /// Every member lifecycle fact of the journal, from its first event.
    async fn lifecycle(&self) -> Vec<MemberLifecycleEvent> {
        let mut records = Vec::new();
        let mut after = MobJournalCursor::START;
        loop {
            let page = self
                .handle
                .member_lifecycle_events(after, IDENTITY_ENTRY_PAGE_MAX)
                .await
                .expect("the journal read answers");
            if page.next == after {
                return records;
            }
            records.extend(page.records.into_iter().map(|record| record.event));
            after = page.next;
        }
    }
}

/// The identity entry each `Spawned` fact of the journal records, by member
/// (the last one per member).
fn spawned_entries(
    lifecycle: &[MemberLifecycleEvent],
) -> BTreeMap<AgentIdentity, Option<IdentityEntryOrigin>> {
    lifecycle
        .iter()
        .filter_map(|event| match event {
            MemberLifecycleEvent::Spawned {
                agent_identity,
                identity_entry,
                ..
            } => Some((agent_identity.clone(), *identity_entry)),
            _ => None,
        })
        .collect()
}

/// A raw spawn of an identity entry's name is refused typed
/// (`MobError::IdentityMember`) before anything is created, on the host lane
/// and on the agent lane, and so is a dormant seat of it; the agent lane
/// cannot claim entry custody. With custody the seated entry's name is an
/// ordinary duplicate, and a seat with custody of a worker's name is
/// refused.
///
/// RED: revert `ProbeMemberAdmissionIdentityMember` and the
/// `not_an_entry_without_custody` guards in the MobMachine DSL (with the
/// actor's verdict mapping): the probe answers `DuplicateRejected` for the
/// seated entry, and the raw spawn answers `MemberAlreadyExists`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn raw_spawn_of_a_declared_entry_name_is_refused_typed() {
    let mob = EntryMob::new("entry-reservation").await;
    mob.handle
        .spawn_spec(entry_spec("declared"))
        .await
        .expect("the host declares its entry");
    assert_eq!(mob.entry("declared"), Some(IdentityEntryOrigin::Declared));
    let members = mob.handle.list_members().await.len();
    let journal = mob.journal_len().await;

    for (lane, caller) in [("host", mob.handle.clone()), ("agent", mob.agent())] {
        let error = caller
            .spawn_spec(worker_spec("declared"))
            .await
            .expect_err("a spawn without entry custody does not take an entry's name");
        assert!(is_identity_member(&error, "declared"), "{lane}: {error:?}");
    }
    let error = mob
        .agent()
        .spawn_spec(entry_spec("declared"))
        .await
        .expect_err("the agent lane carries no entry custody");
    assert!(is_identity_member(&error, "declared"), "{error:?}");
    let outcomes = mob
        .handle
        .seat_dormant_members(vec![worker_spec("declared")])
        .await
        .expect("the seat batch answers");
    assert!(
        matches!(
            &outcomes[0],
            DormantSeatOutcome::Refused { error, .. } if is_identity_member(error, "declared")
        ),
        "{outcomes:?}"
    );
    assert_eq!(
        mob.handle.list_members().await.len(),
        members,
        "no refusal created a member"
    );
    assert_eq!(mob.journal_len().await, journal, "no refusal wrote a row");
    assert_eq!(mob.entry("declared"), Some(IdentityEntryOrigin::Declared));

    let error = mob
        .handle
        .spawn_spec(entry_spec("declared"))
        .await
        .expect_err("the seated entry's name is taken");
    assert!(
        matches!(&error, MobError::MemberAlreadyExists(member) if member.as_str() == "declared"),
        "{error:?}"
    );
    mob.handle
        .spawn_spec(worker_spec("named-worker"))
        .await
        .expect("a worker");
    let outcomes = mob
        .handle
        .seat_dormant_members(vec![entry_spec("named-worker")])
        .await
        .expect("the seat batch answers");
    assert!(
        matches!(
            &outcomes[0],
            DormantSeatOutcome::Refused {
                error: MobError::MemberAlreadyExists(_),
                ..
            }
        ),
        "a seat with entry custody does not take a worker's name: {outcomes:?}"
    );
    assert_eq!(mob.entry("named-worker"), None);
}

/// A worker reconcile retires stale workers and spawns desired ones, and
/// never retires or spawns an identity entry: the entry is neither stale nor
/// desired for it.
///
/// RED: revert `mob_machine_members_to_retire` (the DSL helper, its TLA twin
/// and the kernel's evaluation) to range over every bound identity: the
/// reconcile retires the entry.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_reconcile_never_retires_or_spawns_an_entry() {
    let mob = EntryMob::new("entry-reconcile").await;
    mob.handle
        .spawn_spec(entry_spec("kept-entry"))
        .await
        .expect("the host declares its entry");
    mob.handle
        .spawn_spec(worker_spec("stale-worker"))
        .await
        .expect("a worker");

    let report = mob
        .handle
        .reconcile(
            vec![worker_spec("fresh-worker")],
            ReconcileOptions { retire_stale: true },
        )
        .await
        .expect("the reconcile answers");
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert_eq!(report.retired, vec![id("stale-worker")]);
    assert_eq!(
        report
            .spawned
            .iter()
            .map(|spawned| spawned.agent_identity.clone())
            .collect::<Vec<_>>(),
        vec![id("fresh-worker")]
    );
    assert_eq!(mob.entry("kept-entry"), Some(IdentityEntryOrigin::Declared));
    assert!(
        mob.handle
            .list_members()
            .await
            .iter()
            .any(|member| member.agent_identity == id("kept-entry")),
        "the entry's member is still a member"
    );

    let report = mob
        .handle
        .reconcile(
            vec![worker_spec("kept-entry"), worker_spec("fresh-worker")],
            ReconcileOptions { retire_stale: true },
        )
        .await
        .expect("the reconcile naming the entry answers");
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert!(report.spawned.is_empty(), "{:?}", report.spawned);
    assert!(report.retired.is_empty(), "{:?}", report.retired);
    assert!(report.retained.contains(&id("kept-entry")));
    assert_eq!(mob.entry("kept-entry"), Some(IdentityEntryOrigin::Declared));
    assert!(matches!(
        mob.handle
            .ensure_member(worker_spec("kept-entry"))
            .await
            .expect("the ensure answers"),
        EnsureMemberOutcome::Existed(_)
    ));
}

/// The agent lane does not act on an identity entry as a member: its
/// retire, respawn, cancellation and wiring addressed to the entry are
/// refused `MobError::IdentityMember`, and the entry is untouched. The agent
/// lane still acts on a worker, and the host still acts on its entry.
///
/// RED: revert the agent-lane arm of chokepoint (a) (`admit_command_scope`
/// admits every agent-lane command): the agent lane retires the entry.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_lane_retire_of_an_entry_is_refused_identity_member() {
    let mob = EntryMob::new("entry-agent-lane").await;
    mob.handle
        .spawn_spec(entry_spec("lane-entry"))
        .await
        .expect("the host declares its entry");
    mob.handle
        .spawn_spec(worker_spec("lane-worker"))
        .await
        .expect("a worker");
    let agent = mob.agent();

    let error = agent
        .retire(id("lane-entry"))
        .await
        .expect_err("the agent lane does not retire an entry");
    assert!(is_identity_member(&error, "lane-entry"), "{error:?}");
    let error = agent
        .respawn(id("lane-entry"), None)
        .await
        .expect_err("the agent lane does not respawn an entry");
    assert!(
        matches!(&error, MobRespawnError::Mob(error) if is_identity_member(error, "lane-entry")),
        "{error:?}"
    );
    let error = agent
        .force_cancel_member(id("lane-entry"))
        .await
        .expect_err("the agent lane does not cancel an entry's work");
    assert!(is_identity_member(&error, "lane-entry"), "{error:?}");
    let error = agent
        .wire(id("lane-worker"), id("lane-entry"))
        .await
        .expect_err("the agent lane does not wire an entry");
    assert!(is_identity_member(&error, "lane-entry"), "{error:?}");
    assert_eq!(mob.entry("lane-entry"), Some(IdentityEntryOrigin::Declared));
    assert!(
        mob.handle
            .list_members()
            .await
            .iter()
            .any(|member| member.agent_identity == id("lane-entry"))
    );

    agent
        .retire(id("lane-worker"))
        .await
        .expect("the agent lane retires a worker");
    mob.handle
        .retire(id("lane-entry"))
        .await
        .expect("the host retires its own entry");
    assert_eq!(
        mob.entry("lane-entry"),
        None,
        "the retirement ends the entry"
    );
}

/// An entry's origin rides on its seat's or spawn's `MemberSpawned` row, so
/// a restart restores the entry from the journal alone, and the restored
/// entry keeps its name reserved. The entries page in identity order.
///
/// RED: drop the `identity_entry` the seat's and the spawn's rows carry (or
/// the replay's `RecoverIdentityEntry`): after the restart the identities
/// read as workers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn entry_origin_survives_a_restart_from_its_seat_row() {
    let mut mob = EntryMob::new("entry-restart").await;
    mob.seat(entry_spec("seated-entry")).await;
    mob.handle
        .spawn_spec(entry_spec("spawned-entry"))
        .await
        .expect("the host declares a spawned entry");
    mob.handle
        .spawn_spec(worker_spec("plain-worker"))
        .await
        .expect("a worker");
    let rows = spawned_entries(&mob.lifecycle().await);
    assert_eq!(
        rows.get(&id("seated-entry")),
        Some(&Some(IdentityEntryOrigin::Declared)),
        "the seat's row records the entry"
    );
    assert_eq!(
        rows.get(&id("spawned-entry")),
        Some(&Some(IdentityEntryOrigin::Declared)),
        "the spawn's row records the entry"
    );
    assert_eq!(rows.get(&id("plain-worker")), Some(&None));

    mob.restart().await;
    assert_eq!(
        mob.entry("seated-entry"),
        Some(IdentityEntryOrigin::Declared)
    );
    assert_eq!(
        mob.entry("spawned-entry"),
        Some(IdentityEntryOrigin::Declared)
    );
    assert_eq!(mob.entry("plain-worker"), None);
    assert_eq!(
        mob.handle
            .identity_census()
            .expect("the census answers")
            .declared,
        2
    );
    let first = mob
        .handle
        .identity_entries(None, 1)
        .expect("the first page answers");
    assert_eq!(
        first
            .entries
            .iter()
            .map(|entry| entry.identity.clone())
            .collect::<Vec<_>>(),
        vec![id("seated-entry")]
    );
    let next = first.next.expect("a second page");
    let second = mob
        .handle
        .identity_entries(Some(&next), 1)
        .expect("the second page answers");
    assert_eq!(
        second
            .entries
            .iter()
            .map(|entry| entry.identity.clone())
            .collect::<Vec<_>>(),
        vec![id("spawned-entry")]
    );
    assert_eq!(second.next, None);
    let error = mob
        .handle
        .spawn_spec(worker_spec("seated-entry"))
        .await
        .expect_err("the restored entry keeps its name");
    assert!(is_identity_member(&error, "seated-entry"), "{error:?}");
}

/// A legacy member (its spawn row carries the legacy identity label naming
/// its own id) is adopted as an identity entry once: the adoption is
/// journaled, a second adoption changes nothing, a worker labelled for
/// another id keeps the name, and an identity no member holds adopts
/// nothing. After a restart the entry is restored from the adoption's row,
/// and the label is never read for it again: an adoption against a storage
/// composition whose binding read refuses answers from the entry, while the
/// same adoption of a worker needs that read.
///
/// RED: drop the adoption's row (`MemberSessionBindingRecovered` carrying
/// `identity_entry`) and the handle's entry check before its observation:
/// after the restart the member reads as a worker, and the adoption reads
/// the label again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_member_is_adopted_once_and_its_label_is_never_read_again() {
    let mut mob = EntryMob::new("entry-legacy").await;
    mob.handle
        .spawn_spec(labelled_spec("legacy-member", "legacy-member"))
        .await
        .expect("a member a legacy host labelled with its own id");
    mob.handle
        .spawn_spec(labelled_spec("other-worker", "someone-else"))
        .await
        .expect("a worker labelled for another id");

    let adopt = |handle: MobHandle, name: &'static str| async move {
        handle.adopt_legacy_identity_entry(&id(name)).await
    };
    assert_eq!(
        adopt(mob.handle.clone(), "legacy-member")
            .await
            .expect("the adoption answers"),
        LegacyIdentityEntryAdoption::Adopted
    );
    assert_eq!(
        mob.entry("legacy-member"),
        Some(IdentityEntryOrigin::Declared)
    );
    assert_eq!(
        adopt(mob.handle.clone(), "legacy-member")
            .await
            .expect("the second adoption answers"),
        LegacyIdentityEntryAdoption::AlreadyAnEntry
    );
    assert_eq!(
        adopt(mob.handle.clone(), "other-worker")
            .await
            .expect("the worker's adoption answers"),
        LegacyIdentityEntryAdoption::NameHeldByWorker
    );
    assert_eq!(mob.entry("other-worker"), None);
    assert_eq!(
        adopt(mob.handle.clone(), "never-seated")
            .await
            .expect("an unseated adoption answers"),
        LegacyIdentityEntryAdoption::NotSeated
    );
    assert!(
        mob.lifecycle().await.iter().any(|event| matches!(
            event,
            MemberLifecycleEvent::EntryAdopted { agent_identity, .. }
                if agent_identity == &id("legacy-member")
        )),
        "the adoption is journaled"
    );

    mob.restart().await;
    assert_eq!(
        mob.entry("legacy-member"),
        Some(IdentityEntryOrigin::Declared),
        "the adoption's row restores the entry"
    );

    // A storage composition without the built-in identity/member store
    // refuses the binding read the observation makes.
    let events = mob.storage.events.clone();
    let runtime_metadata = mob.storage.runtime_metadata.clone();
    mob.restart_into(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .await;
    assert_eq!(
        adopt(mob.handle.clone(), "legacy-member")
            .await
            .expect("an entry's adoption reads nothing"),
        LegacyIdentityEntryAdoption::AlreadyAnEntry
    );
    let error = adopt(mob.handle.clone(), "other-worker")
        .await
        .expect_err("a worker's adoption reads its binding");
    assert!(matches!(error, MobError::StorageError(_)), "{error:?}");
}

/// The entry reads (one entry, a page of entries, the census), the
/// run-start hold read and the member lifecycle journal read answer while
/// the actor is parked behind a command that does not finish, and none of
/// them wakes a member: the seated entry stays Dormant and nothing is built.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn entry_read_needs_no_actor_command_and_never_wakes() {
    let mob = EntryMob::new("entry-read").await;
    mob.seat(entry_spec("dormant-entry")).await;
    mob.seat(worker_spec("dormant-worker")).await;
    let builds = mob.service.recorded_create_requests().await.len();

    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let parked = mob
        .handle
        .enqueue_actor_command_for_test(|reply_tx| MobCommand::ParkActorForObservationTest {
            entered_tx,
            release_rx,
            reply_tx,
        })
        .await
        .expect("enqueue the parking command");
    entered_rx.await.expect("the actor parks");
    let mut queued = mob
        .handle
        .enqueue_actor_command_for_test(|reply_tx| MobCommand::QueryPhase { reply_tx })
        .await
        .expect("enqueue a command behind the parked one");
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut queued)
            .await
            .is_err(),
        "control: an actor command waits while the actor is parked"
    );

    match mob
        .handle
        .identity_entry(&id("dormant-entry"))
        .expect("the entry read answers")
    {
        IdentityEntryRead::Entry(view) => {
            assert_eq!(view.origin, IdentityEntryOrigin::Declared);
            assert!(
                matches!(view.standing, EntryStanding::Seated(_)),
                "{view:?}"
            );
            assert!(view.broken.is_none());
        }
        other => panic!("the seated entry reads as an entry: {other:?}"),
    }
    assert!(
        matches!(
            mob.handle
                .identity_entry(&id("dormant-worker"))
                .expect("the entry read answers for a worker"),
            IdentityEntryRead::NotAnEntry { worker: Some(_) }
        ),
        "a seated worker reads as a worker"
    );
    let page = mob
        .handle
        .identity_entries(None, IDENTITY_ENTRY_PAGE_MAX)
        .expect("the entry page answers");
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.next, None);
    let census = mob.handle.identity_census().expect("the census answers");
    assert_eq!((census.declared, census.seated), (1, 1));
    assert!(
        mob.handle
            .members_holding_run_starts(crate::HostRunStartHoldReason::ToolsNotPublished)
            .expect("the run-start hold read answers")
            .is_empty()
    );
    let lifecycle = tokio::time::timeout(
        STEP,
        mob.handle
            .member_lifecycle_events(MobJournalCursor::START, IDENTITY_ENTRY_PAGE_MAX),
    )
    .await
    .expect("the journal read answers while the actor is parked")
    .expect("the journal read succeeds");
    assert_eq!(
        spawned_entries(
            &lifecycle
                .records
                .into_iter()
                .map(|record| record.event)
                .collect::<Vec<_>>()
        )
        .get(&id("dormant-entry")),
        Some(&Some(IdentityEntryOrigin::Declared))
    );

    assert_eq!(
        mob.handle
            .member_materialization_for_test(&id("dormant-entry")),
        Some(dsl::MemberMaterializationPhase::Dormant),
        "no read woke the entry's member"
    );
    assert_eq!(
        mob.service.recorded_create_requests().await.len(),
        builds,
        "no read built a member"
    );
    release_tx.send(()).expect("release the actor");
    parked
        .await
        .expect("the parked command answers")
        .expect("the parked command succeeds");
    queued
        .await
        .expect("the queued command answers")
        .expect("the queued command succeeds");
}

/// The journal a reader that predates identity entries reads: every row as
/// written, with the internal `identity_entry` fields absent (its decoder
/// ignores the keys it does not know).
async fn predating_reader_journal(
    events: &Arc<dyn MobEventStore>,
    through: Option<u64>,
) -> Arc<dyn MobEventStore> {
    use crate::store::private::MobEventStoreSealed as _;
    // The copy keeps the mob's creation record (its incarnation), as the
    // predating reader's store does: only the entry fields are left out.
    let creation = events
        .creation_record()
        .await
        .expect("read the creation record")
        .expect("a created mob has a creation record");
    let provenance = creation.provenance().expect("decode the provenance");
    let copy: Arc<dyn MobEventStore> = Arc::new(InMemoryMobEventStore::new());
    for mut event in events.poll(0, usize::MAX).await.expect("read the journal") {
        if through.is_some_and(|through| event.cursor > through) {
            break;
        }
        if let Some(spawned) = event.kind.member_spawned_mut() {
            spawned.identity_entry = None;
        }
        if let Some(recovered) = event.kind.member_session_binding_recovered_mut() {
            recovered.identity_entry = None;
        }
        let row = crate::event::NewMobEvent {
            mob_id: event.mob_id,
            timestamp: Some(event.timestamp),
            kind: event.kind,
        };
        if matches!(row.kind, MobEventKind::MobCreated { .. }) {
            copy.append_creation(row, creation.incarnation, &provenance)
                .await
                .expect("copy the creation row");
        } else {
            copy.append(row).await.expect("copy a row");
        }
    }
    copy
}

/// Rollback, in-process leg: members seated and spawned with entry fields are
/// served by a reader that predates them: listed active, woken by a
/// delivery and reset, as ordinary members. The lane-run leg against the
/// published binaries is
/// `members_seated_with_entry_fields_are_served_by_096` below.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn members_seated_with_entry_fields_are_served_without_them() {
    let mut mob = EntryMob::new("entry-rollback-serve").await;
    mob.seat(entry_spec("seated-entry")).await;
    mob.handle
        .spawn_spec(entry_spec("spawned-entry"))
        .await
        .expect("a spawned entry");
    let predating = predating_reader_journal(&mob.storage.events, None).await;
    mob.restart_into(MobStorage::with_events_and_runtime_metadata(
        predating,
        mob.storage.runtime_metadata.clone(),
    ))
    .await;
    let members = mob.handle.list_members().await;
    for name in ["seated-entry", "spawned-entry"] {
        let member = members
            .iter()
            .find(|member| member.agent_identity == id(name))
            .unwrap_or_else(|| panic!("{name} is listed"));
        assert_eq!(member.status, MobMemberStatus::Active, "{name}");
        assert_eq!(mob.entry(name), None, "{name} reads as an ordinary member");
    }
    tokio::time::timeout(STEP, mob.handle.ensure_member_live(&id("seated-entry")))
        .await
        .expect("the wake answers")
        .expect("the seated member wakes");
    tokio::time::timeout(STEP, mob.handle.rotate_member(worker_spec("spawned-entry")))
        .await
        .expect("the reset answers")
        .expect("the member is reset");
}

/// Rollback of an adoption, in-process leg: the adoption's row, read by a
/// reader that predates identity entries, replays as the member's current
/// binding and changes nothing: the restored members equal those of the
/// journal that ends before the row. The lane-run leg against the published
/// binaries is `legacy_adoption_row_replays_as_a_no_op_under_097` below.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_adoption_row_replays_as_a_no_op_without_its_entry() {
    let mut mob = EntryMob::new("entry-rollback-adoption").await;
    mob.handle
        .spawn_spec(labelled_spec("legacy-member", "legacy-member"))
        .await
        .expect("a legacy member");
    let before_adoption = mob
        .storage
        .events
        .poll(0, usize::MAX)
        .await
        .expect("read the journal")
        .last()
        .map(|event| event.cursor)
        .expect("a journal");
    assert_eq!(
        mob.handle
            .adopt_legacy_identity_entry(&id("legacy-member"))
            .await
            .expect("the adoption answers"),
        LegacyIdentityEntryAdoption::Adopted
    );
    let session = mob
        .handle
        .resolve_bridge_session_id(&id("legacy-member"))
        .await
        .expect("the member's session");

    let mut restored = Vec::new();
    for through in [Some(before_adoption), None] {
        let journal = predating_reader_journal(&mob.storage.events, through).await;
        mob.restart_into(MobStorage::with_events_and_runtime_metadata(
            journal,
            mob.storage.runtime_metadata.clone(),
        ))
        .await;
        let member = mob
            .handle
            .list_members()
            .await
            .into_iter()
            .find(|member| member.agent_identity == id("legacy-member"))
            .expect("the legacy member is listed");
        assert_eq!(mob.entry("legacy-member"), None);
        restored.push((
            member.status,
            member.runtime_mode,
            member.labels,
            mob.handle
                .resolve_bridge_session_id(&id("legacy-member"))
                .await,
        ));
    }
    assert_eq!(restored[0], restored[1], "the adoption's row moves nothing");
    assert_eq!(restored[1].3, Some(session));
}

/// Rollback, lane-run leg: needs the published Meta 0.9.6 and Core 0.9.7.
///
/// This build writes the fixture: a persistent mob store (SQLite) at
/// `$MEERKAT_ROLLBACK_FIXTURE_DIR/served-by-096/mob.sqlite` holding a
/// member seated with entry custody (its session unborn, so its first wake
/// creates it) and its `MemberSpawned` row's `identity_entry`. The lane then
/// resumes that store with Meta 0.9.6 (which pins Core 0.9.7) and checks:
/// the member is listed `active`, a peer delivery wakes it, and a reset
/// (`rotate_member`) re-seats it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "lane:rollback - writes the fixture the published Meta 0.9.6 and Core 0.9.7 resume"]
async fn members_seated_with_entry_fields_are_served_by_096() {
    let dir = std::path::PathBuf::from(
        std::env::var_os("MEERKAT_ROLLBACK_FIXTURE_DIR")
            .expect("MEERKAT_ROLLBACK_FIXTURE_DIR names the fixture directory"),
    )
    .join("served-by-096");
    std::fs::create_dir_all(&dir).expect("create the fixture directory");
    write_rollback_fixture(&dir, "rollback-served-by-096", |handle| async move {
        let outcomes = handle
            .seat_dormant_members(vec![entry_spec("seated-entry")])
            .await
            .expect("the seat answers");
        assert!(
            matches!(&outcomes[0], DormantSeatOutcome::Seated { .. }),
            "{outcomes:?}"
        );
    })
    .await;
}

/// Rollback of an adoption, lane-run leg: needs the published Core 0.9.7.
///
/// This build writes the fixture: a persistent mob store (SQLite) at
/// `$MEERKAT_ROLLBACK_FIXTURE_DIR/adoption-under-097/mob.sqlite` holding a
/// legacy member seated Dormant with the legacy identity label naming its
/// own id, and the row of its adoption as an identity entry. The lane then
/// resumes that store with Core 0.9.7 and checks that the adoption's row
/// replays as the member's current binding: the member is listed `active`
/// on the same session, and a copy of the store whose journal ends before
/// the row resumes to the same members.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "lane:rollback - writes the fixture the published Core 0.9.7 resumes"]
async fn legacy_adoption_row_replays_as_a_no_op_under_097() {
    let dir = std::path::PathBuf::from(
        std::env::var_os("MEERKAT_ROLLBACK_FIXTURE_DIR")
            .expect("MEERKAT_ROLLBACK_FIXTURE_DIR names the fixture directory"),
    )
    .join("adoption-under-097");
    std::fs::create_dir_all(&dir).expect("create the fixture directory");
    write_rollback_fixture(&dir, "rollback-adoption-under-097", |handle| async move {
        let outcomes = handle
            .seat_dormant_members(vec![labelled_spec("legacy-member", "legacy-member")])
            .await
            .expect("the seat answers");
        assert!(
            matches!(&outcomes[0], DormantSeatOutcome::Seated { .. }),
            "{outcomes:?}"
        );
        assert_eq!(
            handle
                .adopt_legacy_identity_entry(&id("legacy-member"))
                .await
                .expect("the adoption answers"),
            LegacyIdentityEntryAdoption::Adopted
        );
    })
    .await;
}

/// Create the fixture mob `mob_id` on a persistent store in `dir`, let
/// `populate` seat its members, and shut it down: what the published
/// binaries resume.
async fn write_rollback_fixture<F, Fut>(dir: &std::path::Path, mob_id: &str, populate: F)
where
    F: FnOnce(MobHandle) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let service = Arc::new(MockSessionService::new());
    let _adapter = service.enable_runtime_adapter();
    let mut definition = sample_definition();
    definition.id = MobId::from(mob_id);
    let handle = MobBuilder::new(
        definition,
        MobStorage::persistent(dir.join("mob.sqlite")).expect("open the fixture store"),
    )
    .with_session_service(service)
    .with_member_comms_identity_root(dir.join("comms-keys"))
    .create()
    .await
    .expect("create the fixture mob");
    populate(handle.clone()).await;
    handle.shutdown().await.expect("shut the fixture mob down");
}
