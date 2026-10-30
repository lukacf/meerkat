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

/// Wait, within one step, until `name` reads as an identity entry.
async fn entry_settles(mob: &EntryMob, name: &str) -> Option<IdentityEntryOrigin> {
    let deadline = tokio::time::Instant::now() + STEP;
    loop {
        if let Some(origin) = mob.entry(name) {
            return Some(origin);
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// A host respawn of an entry's member and a rotation of a seated entry
/// keep the entry: the successor's `MemberSpawned` row records it, and the
/// entry survives a restart.
///
/// RED: drop the respawn's carry of the entry to its successor
/// (`replacement_spec.entry_custody = snapshot.entry_custody`) or the
/// rotation row's `identity_entry`: the respawned identity reads as a
/// worker at once, the rotated one after the restart.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn respawn_and_rotation_keep_an_entry_across_a_restart() {
    let mut mob = EntryMob::new("entry-successors").await;
    mob.handle
        .spawn_spec(entry_spec("respawned-entry"))
        .await
        .expect("the host declares a spawned entry");
    mob.seat(entry_spec("rotated-entry")).await;

    tokio::time::timeout(STEP, mob.handle.respawn(id("respawned-entry"), None))
        .await
        .expect("the respawn answers")
        .expect("the host respawns its entry");
    tokio::time::timeout(STEP, mob.handle.rotate_member(worker_spec("rotated-entry")))
        .await
        .expect("the rotation answers")
        .expect("the host rotates its entry");
    for name in ["respawned-entry", "rotated-entry"] {
        assert_eq!(
            entry_settles(&mob, name).await,
            Some(IdentityEntryOrigin::Declared),
            "{name} stays an entry"
        );
    }
    let rows = spawned_entries(&mob.lifecycle().await);
    for name in ["respawned-entry", "rotated-entry"] {
        assert_eq!(
            rows.get(&id(name)),
            Some(&Some(IdentityEntryOrigin::Declared)),
            "the row of {name}'s successor records the entry"
        );
    }

    mob.restart().await;
    for name in ["respawned-entry", "rotated-entry"] {
        assert_eq!(
            mob.entry(name),
            Some(IdentityEntryOrigin::Declared),
            "{name} is an entry after the restart"
        );
    }
}

/// A placed spawn never acts for an identity entry: one with entry custody
/// is refused typed at the shell before anything is created (MobMachine's
/// remote ladder has no arm for it either; see the machine test
/// `a_placed_spawn_with_entry_custody_has_no_arm`).
///
/// RED: drop the shell refusal in the placed lane
/// (`enqueue_spawn_remote_inner`): the spawn reaches the remote ladder,
/// which refuses it untyped as a machine rejection.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_placed_spawn_with_entry_custody_is_refused_typed() {
    let mob = EntryMob::new("entry-placed").await;
    let members = mob.handle.list_members().await.len();
    let journal = mob.journal_len().await;
    let mut spec = entry_spec("placed-entry");
    spec.placement = Some(dsl::HostId("entry-placed-host".to_string()));
    let error = mob
        .handle
        .spawn_spec(spec)
        .await
        .expect_err("a placed spawn with entry custody is refused");
    assert!(
        matches!(&error, MobError::WiringError(reason) if reason.contains("identity entry")),
        "{error:?}"
    );
    assert_eq!(mob.handle.list_members().await.len(), members);
    assert_eq!(mob.journal_len().await, journal, "no refusal wrote a row");
    assert_eq!(mob.entry("placed-entry"), None);
}

/// The rest of the agent lane's refusal: it does not unwire, rotate or
/// cancel the work of an identity entry's member either, while it still
/// unwires two workers.
///
/// RED: classify `Unwire`, `PlanMemberRotation` and `CancelAllWork` as
/// addressing no member (`AgentLaneAdmission::Unaddressed`): the agent lane
/// unwires, rotates and cancels the entry.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_agent_lane_unwires_rotates_and_cancels_no_entry() {
    let mob = EntryMob::new("entry-agent-scope").await;
    mob.handle
        .spawn_spec(entry_spec("scope-entry"))
        .await
        .expect("the host declares its entry");
    for name in ["scope-worker", "scope-peer"] {
        mob.handle
            .spawn_spec(worker_spec(name))
            .await
            .expect("a worker");
    }
    mob.handle
        .wire(id("scope-worker"), id("scope-entry"))
        .await
        .expect("the host wires its entry to a worker");
    mob.handle
        .wire(id("scope-worker"), id("scope-peer"))
        .await
        .expect("the host wires two workers");
    let agent = mob.agent();

    let error = agent
        .unwire(id("scope-worker"), id("scope-entry"))
        .await
        .expect_err("the agent lane does not unwire an entry");
    assert!(is_identity_member(&error, "scope-entry"), "{error:?}");
    let error = agent
        .rotate_member(worker_spec("scope-entry"))
        .await
        .expect_err("the agent lane does not rotate an entry");
    assert!(is_identity_member(&error, "scope-entry"), "{error:?}");
    let error = agent
        .cancel_all_work(
            crate::ids::AgentRuntimeId::new(id("scope-entry"), crate::ids::Generation::INITIAL),
            crate::ids::FenceToken::new(1),
        )
        .await
        .expect_err("the agent lane does not cancel an entry's work");
    assert!(is_identity_member(&error, "scope-entry"), "{error:?}");
    assert_eq!(
        mob.entry("scope-entry"),
        Some(IdentityEntryOrigin::Declared)
    );

    agent
        .unwire(id("scope-worker"), id("scope-peer"))
        .await
        .expect("the agent lane unwires two workers");
}

/// The legacy adoption observes a member again when MobMachine answers its
/// observation stale, a bounded number of times, and answers a retryable
/// refusal when the incarnation keeps changing. The agent lane runs no
/// adoption at all, of a worker or of an entry.
///
/// RED: observe once (no loop): the first stale observation is the answer,
/// and the member observed once is not adopted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stale_legacy_observation_is_observed_again() {
    use crate::runtime::handle::legacy_observation_hold;

    let mob = EntryMob::new("entry-stale-observation").await;
    for name in ["observed-once", "observed-stale"] {
        mob.handle
            .spawn_spec(labelled_spec(name, name))
            .await
            .expect("a legacy member");
    }

    legacy_observation_hold::force_stale(&id("observed-once"), 1);
    assert_eq!(
        mob.handle
            .adopt_legacy_identity_entry(&id("observed-once"))
            .await
            .expect("the adoption answers"),
        LegacyIdentityEntryAdoption::Adopted,
        "one stale observation is observed again"
    );

    legacy_observation_hold::force_stale(&id("observed-stale"), 3);
    let error = mob
        .handle
        .adopt_legacy_identity_entry(&id("observed-stale"))
        .await
        .expect_err("an incarnation that keeps changing is refused");
    assert!(
        matches!(&error, MobError::LifecycleOperationPending { .. }),
        "{error:?}"
    );
    assert_eq!(
        error
            .structured_data()
            .and_then(|data| data.get("retryable").and_then(serde_json::Value::as_bool)),
        Some(true),
        "the refusal is retryable"
    );
    assert_eq!(mob.entry("observed-stale"), None);

    for name in ["observed-stale", "observed-once"] {
        let error = mob
            .agent()
            .adopt_legacy_identity_entry(&id(name))
            .await
            .expect_err("the agent lane adopts nothing");
        assert!(
            matches!(error, MobError::ScopeDenied(_)),
            "{name}: {error:?}"
        );
    }
}

/// A host retire of an entry beside an agent retire of it retires the
/// entry: the agent's retire is refused at its scope admission, before it
/// elects the retirement slot the host's retire would join.
///
/// RED: admit the retire's scope without naming the member
/// (`ScopeAdmissionSubject::Mob` in `join_or_start_retirement_step`): both
/// retires elect one slot, so either the host answers the agent's refusal or
/// the agent's retire answers the host's success.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_host_retire_beside_an_agent_retire_of_an_entry_retires_it() {
    let mob = EntryMob::new("entry-retire-slot").await;
    mob.handle
        .spawn_spec(entry_spec("slot-entry"))
        .await
        .expect("the host declares its entry");

    // The actor parks, so both retires' scope admissions queue behind it;
    // a second park keeps their retirements from running until both
    // elected their slots.
    let (first_entered_tx, first_entered_rx) = tokio::sync::oneshot::channel();
    let (first_release_tx, first_release_rx) = tokio::sync::oneshot::channel();
    let first = mob
        .handle
        .enqueue_actor_command_for_test(|reply_tx| MobCommand::ParkActorForObservationTest {
            entered_tx: first_entered_tx,
            release_rx: first_release_rx,
            reply_tx,
        })
        .await
        .expect("park the actor");
    first_entered_rx.await.expect("the actor parks");
    let agent = tokio::spawn({
        let agent = mob.agent();
        async move { agent.retire(id("slot-entry")).await }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let host = tokio::spawn({
        let host = mob.handle.clone();
        async move { host.retire(id("slot-entry")).await }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let (second_entered_tx, second_entered_rx) = tokio::sync::oneshot::channel();
    let (second_release_tx, second_release_rx) = tokio::sync::oneshot::channel();
    let second = mob
        .handle
        .enqueue_actor_command_for_test(|reply_tx| MobCommand::ParkActorForObservationTest {
            entered_tx: second_entered_tx,
            release_rx: second_release_rx,
            reply_tx,
        })
        .await
        .expect("park the actor again");
    first_release_tx.send(()).expect("release the first park");
    first
        .await
        .expect("the first park answers")
        .expect("the first park succeeds");
    second_entered_rx
        .await
        .expect("the actor parks after both admissions");
    tokio::time::sleep(Duration::from_millis(100)).await;
    second_release_tx.send(()).expect("release the second park");
    second
        .await
        .expect("the second park answers")
        .expect("the second park succeeds");

    let agent = tokio::time::timeout(STEP, agent)
        .await
        .expect("the agent's retire answers")
        .expect("the agent's retire task");
    let host = tokio::time::timeout(STEP, host)
        .await
        .expect("the host's retire answers")
        .expect("the host's retire task");
    assert!(
        matches!(&agent, Err(error) if is_identity_member(error, "slot-entry")),
        "{agent:?}"
    );
    host.expect("the host retires its own entry");
    assert_eq!(mob.entry("slot-entry"), None);
    assert!(
        !mob.handle
            .list_members()
            .await
            .iter()
            .any(|member| member.agent_identity == id("slot-entry")),
        "the entry's member is retired"
    );
}

/// The worker reconcile and ensure spawn workers only: a spec that carries
/// entry custody is refused typed before anything is created (an identity
/// entry is declared with `spawn_spec` or `seat_dormant_members`).
///
/// RED: drop the refusal in `handle_reconcile` and `handle_ensure_member`:
/// the reconcile and the ensure seat identity entries.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_worker_reconcile_and_ensure_refuse_entry_custody() {
    let mob = EntryMob::new("entry-reconcile-custody").await;
    let members = mob.handle.list_members().await.len();
    let journal = mob.journal_len().await;
    let error = mob
        .handle
        .reconcile(
            vec![
                worker_spec("reconciled-worker"),
                entry_spec("reconciled-entry"),
            ],
            ReconcileOptions {
                retire_stale: false,
            },
        )
        .await
        .expect_err("a reconcile of an entry custody spec is refused");
    assert!(
        matches!(&error, MobError::WiringError(reason) if reason.contains("entry custody")),
        "{error:?}"
    );
    let error = mob
        .handle
        .ensure_member(entry_spec("ensured-entry"))
        .await
        .expect_err("an ensure of an entry custody spec is refused");
    assert!(
        matches!(&error, MobError::WiringError(reason) if reason.contains("entry custody")),
        "{error:?}"
    );
    assert_eq!(
        mob.handle.list_members().await.len(),
        members,
        "nothing was created"
    );
    assert_eq!(mob.journal_len().await, journal, "no refusal wrote a row");
    for name in ["reconciled-worker", "reconciled-entry", "ensured-entry"] {
        assert_eq!(mob.entry(name), None, "{name}");
    }
}

// ---------------------------------------------------------------------------
// The legacy label's one reader (source guard)
// ---------------------------------------------------------------------------

/// The constant the legacy identity label is read through.
const LEGACY_LABEL_CONSTANT: &str = "LEGACY_IDENTITY_LABEL";

/// The calls a label lookup makes: on a map named for labels, or the
/// roster's label query.
const LABEL_LOOKUPS: &[&str] = &[".get(", ".contains_key(", ".remove(", ".get_key_value("];

/// The label lookups in `code` (comments and literals blanked) whose key is
/// a literal, as `line: text`: a lookup on a map whose receiver names labels,
/// or the roster's `find_by_label`, whose key argument is all blank (a
/// blanked literal).
fn literal_label_lookups(code: &str) -> Vec<String> {
    let line = |at: usize| {
        let start = code[..at].rfind('\n').map_or(0, |index| index + 1);
        let end = code[at..].find('\n').map_or(code.len(), |index| at + index);
        format!(
            "{}: {}",
            super::fail_stop_cause::line_of(code, at),
            code[start..end].trim()
        )
    };
    let literal_key = |open: usize| {
        let rest = &code[open..];
        let end = rest.find([',', ')']).unwrap_or(rest.len());
        let key = &rest[..end];
        !key.is_empty() && key.chars().all(char::is_whitespace)
    };
    let mut lookups = Vec::new();
    for call in LABEL_LOOKUPS {
        for (at, _) in code.match_indices(call) {
            let receiver_start = code[..at]
                .char_indices()
                .rev()
                .find(|(_, character)| {
                    !(character.is_ascii_alphanumeric() || "_.()".contains(*character))
                })
                .map_or(0, |(index, character)| index + character.len_utf8());
            let receiver = code[receiver_start..at].to_ascii_lowercase();
            if receiver.contains("label") && literal_key(at + call.len()) {
                lookups.push(line(at));
            }
        }
    }
    for (at, name) in code.match_indices("find_by_label(") {
        if literal_key(at + name.len()) {
            lookups.push(line(at));
        }
    }
    lookups
}

/// The uses of the legacy label constant in `code` other than its
/// definition and the reads inside `reader` (a byte range), as `line: text`,
/// with the counts of definitions and reads.
fn legacy_label_constant_uses(
    code: &str,
    reader: Option<&std::ops::Range<usize>>,
) -> (Vec<String>, usize, usize) {
    let mut others = Vec::new();
    let (mut definitions, mut reads) = (0, 0);
    for (at, name) in code.match_indices(LEGACY_LABEL_CONSTANT) {
        if !super::fail_stop_cause::whole_identifier(code, at, name) {
            continue;
        }
        if code[..at].trim_end().ends_with("const") {
            definitions += 1;
        } else if reader.is_some_and(|body| body.contains(&at)) {
            reads += 1;
        } else {
            let start = code[..at].rfind('\n').map_or(0, |index| index + 1);
            let end = code[at..].find('\n').map_or(code.len(), |index| at + index);
            others.push(format!(
                "{}: {}",
                super::fail_stop_cause::line_of(code, at),
                code[start..end].trim()
            ));
        }
    }
    (others, definitions, reads)
}

/// The legacy identity label is read in one place, the legacy adoption's
/// observation: shipped code names its constant only where it is defined
/// and in `legacy_roster_member`, and makes no label lookup by a literal
/// key anywhere (a lookup of the label by its literal would bypass the
/// constant).
///
/// RED: plant a second reader in shipped code, a lookup of
/// `LEGACY_IDENTITY_LABEL` outside `legacy_roster_member` or of the label
/// by its literal key: the guard names it (see
/// `the_legacy_label_scans_see_a_planted_second_reader`).
#[test]
fn the_legacy_identity_label_is_read_only_by_the_adoption_observation() {
    let mut violations = Vec::new();
    let (mut definitions, mut reads) = (0, 0);
    for (path, code) in super::fail_stop_cause::shipped_sources() {
        let reader = if path == "runtime/identity_entry.rs" {
            super::fail_stop_cause::function_body(&code, "fn legacy_roster_member")
        } else {
            None
        };
        let (others, defined, read) = legacy_label_constant_uses(&code, reader.as_ref());
        definitions += defined;
        reads += read;
        violations.extend(others.into_iter().map(|use_| format!("{path}:{use_}")));
        violations.extend(
            literal_label_lookups(&code)
                .into_iter()
                .map(|lookup| format!("{path}:{lookup}")),
        );
    }
    assert!(
        violations.is_empty(),
        "the legacy identity label has readers outside the adoption's observation:\n{}",
        violations.join("\n")
    );
    assert_eq!(
        (definitions, reads),
        (1, 1),
        "the label constant is defined once and read once, by legacy_roster_member"
    );
}

#[test]
fn the_legacy_label_scans_see_a_planted_second_reader() {
    let code = super::fail_stop_cause::shipped_code(concat!(
        "pub(crate) const LEGACY_IDENTITY_LABEL: &str = \"agent_identity\";\n",
        "fn legacy_roster_member(b: &B) -> bool { b.labels().get(LEGACY_IDENTITY_LABEL).is_some() }\n",
        "fn second(b: &B) -> bool { b.labels().contains_key(LEGACY_IDENTITY_LABEL) }\n",
        "fn literal(b: &B) -> bool { b.labels().get(\"agent_identity\").is_some() }\n",
        "fn roster(r: &R) -> bool { r.find_by_label(\"agent_identity\", \"x\").is_some() }\n",
        "fn variable(b: &B, key: &str) -> bool { b.labels().get(key).is_some() }\n",
        "fn other(m: &M) -> bool { m.get(\"agent_identity\").is_some() }\n",
        "// b.labels().get(\"agent_identity\")\n",
        "#[cfg(test)]\nfn seam(b: &B) -> bool { b.labels().get(\"agent_identity\").is_some() }\n",
    ));
    let reader = super::fail_stop_cause::function_body(&code, "fn legacy_roster_member");
    let (others, definitions, reads) = legacy_label_constant_uses(&code, reader.as_ref());
    assert_eq!((definitions, reads), (1, 1));
    assert_eq!(
        others.iter().map(|use_| &use_[..2]).collect::<Vec<_>>(),
        vec!["3:"],
        "the second reader of the constant: {others:?}"
    );
    let lookups = literal_label_lookups(&code);
    assert_eq!(
        lookups
            .iter()
            .map(|lookup| &lookup[..2])
            .collect::<Vec<_>>(),
        vec!["4:", "5:"],
        "the literal lookups, not the variable key, the other map, the comment or the \
         test item: {lookups:?}"
    );
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
/// published Core is `members_seated_with_entry_fields_are_served_by_097`
/// below.
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
/// journal that ends before the row, and the member wakes after the
/// restart over the whole journal (the row carries the member's durable
/// endpoint, so the reader keeps its endpoint check for it). The lane-run
/// leg against the published Core is
/// `legacy_adoption_row_replays_as_a_no_op_under_097` below.
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
    tokio::time::timeout(STEP, mob.handle.ensure_member_live(&id("legacy-member")))
        .await
        .expect("the wake answers")
        .expect("the member wakes after the restart over the adoption's row");
}

/// Rollback, lane-run leg: needs the published Core 0.9.7 (`meerkat-mob`
/// 0.9.7). A Meta rollback leg belongs with the Meta change that writes a
/// Meta state directory.
///
/// This build writes the fixture: a persistent mob store (SQLite) at
/// `$MEERKAT_ROLLBACK_FIXTURE_DIR/served-by-097/mob.sqlite` holding a
/// member seated with entry custody (its session unborn, so its first wake
/// creates it) and its `MemberSpawned` row's `identity_entry`. The lane then
/// resumes that store with the published Core (`MobBuilder::for_resume`)
/// and checks: the member is listed `active`, a delivery wakes it, and a
/// reset (`rotate_member`) re-seats it. The harness that resumes the
/// fixtures with the published crates is a release gate.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "lane:rollback - writes the fixture the published Core 0.9.7 resumes"]
async fn members_seated_with_entry_fields_are_served_by_097() {
    let dir = std::path::PathBuf::from(
        std::env::var_os("MEERKAT_ROLLBACK_FIXTURE_DIR")
            .expect("MEERKAT_ROLLBACK_FIXTURE_DIR names the fixture directory"),
    )
    .join("served-by-097");
    std::fs::create_dir_all(&dir).expect("create the fixture directory");
    write_rollback_fixture(&dir, "rollback-served-by-097", |handle| async move {
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
/// on the same session, a delivery wakes it, and a copy of the store whose
/// journal ends before the row resumes to the same members. The adoption's
/// row carries the member's durable endpoint when MobMachine held one; a
/// Dormant seat may hold none, and the lane's wake shows Core 0.9.7 serves
/// the member then too.
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
