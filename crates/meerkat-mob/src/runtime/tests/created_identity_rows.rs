//! Created identity entries at a start: the recovery of the identity store's
//! created entry rows (`RecoverCreatedIdentityEntry`), the names they
//! reserve, the declared apply's refusals over them, the seat of an entry
//! owed its seat on its archived session, the reads and the census, and the
//! rollback rows. They run on the combined candidate: the machine, the actor,
//! the identity store, the journal and the replay together.

use super::*;
use crate::control_policy::CommandAuthority;
use crate::identity::{
    CreatedIdentityPresence, CreatedIdentityProvenance, CreatedIdentityRecord,
    IdentityStoredObservation,
};
use crate::runtime::{
    DeclaredEntry, DeclaredEntryOutcome, DeclaredEntryRefusal, DeclaredRoster,
    DeclaredRosterOutcome, DormantSeatOutcome, EntryStanding, IDENTITY_ENTRY_PAGE_MAX,
    IdentityEntryOrigin, IdentityEntryRead, ReconcileOptions, UnseatedEntryCause,
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

/// `name` declared as a turn-driven worker.
fn declared(name: &str) -> DeclaredEntry {
    DeclaredEntry::new(worker_spec(name))
}

fn is_identity_member(error: &MobError, name: &str) -> bool {
    matches!(error, MobError::IdentityMember { member_id } if member_id.as_str() == name)
}

/// A sealed created entry's row of `name` in the mob `mob_id`.
fn created_record(
    mob_id: &MobId,
    name: &str,
    presence: CreatedIdentityPresence,
) -> CreatedIdentityRecord {
    CreatedIdentityRecord::sealed(
        mob_id.clone(),
        id(name),
        1,
        presence,
        CreatedIdentityProvenance::new(format!("create-{name}"), 1),
    )
    .expect("a sealed created row")
}

/// A row a later build wrote, in a schema this build cannot read.
fn later_build_row() -> Vec<u8> {
    br#"{"schema_version":3,"a_later_field":true}"#.to_vec()
}

/// In-memory storage over `events`, with an identity store paired with it as
/// `MobStorage::in_memory` pairs them, which the test writes rows into; the
/// other stores are `previous`'s.
fn storage_over(
    events: InMemoryMobEventStore,
    previous: &MobStorage,
) -> (MobStorage, InMemoryMobIdentityStore) {
    let identity = InMemoryMobIdentityStore::paired_with_event_store(
        &events,
        Arc::new(crate::store::SystemMobIdentityStoreClock),
    );
    let identity_member: Arc<dyn crate::store::MobIdentityMemberStore> = Arc::new(identity.clone());
    let storage = MobStorage {
        events: Arc::new(events),
        identity: Arc::new(identity.clone()),
        identity_member: Some(identity_member),
        ..previous.clone()
    };
    (storage, identity)
}

/// A copy of `events`' journal, each row edited by `edit`, as a build that
/// wrote it differently would have left it.
async fn journal_copy(
    events: &Arc<dyn MobEventStore>,
    mut edit: impl FnMut(&mut MobEvent),
) -> InMemoryMobEventStore {
    use crate::store::private::MobEventStoreSealed as _;
    let creation = events
        .creation_record()
        .await
        .expect("read the creation record")
        .expect("a created mob has a creation record");
    let provenance = creation.provenance().expect("decode the provenance");
    let copy = InMemoryMobEventStore::new();
    for mut event in events.poll(0, usize::MAX).await.expect("read the journal") {
        edit(&mut event);
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

/// A running mob over the mock durable session service and in-memory
/// storage whose identity store the test writes rows into, with
/// session-scoped member comms identities. A restart resumes the same
/// storage, or another one, over a cold-restarted session service.
struct CreatedMob {
    keys: tempfile::TempDir,
    storage: MobStorage,
    identity: InMemoryMobIdentityStore,
    handle: MobHandle,
    service: Arc<MockSessionService>,
}

impl CreatedMob {
    async fn new(label: &str) -> Self {
        let keys = tempfile::tempdir().expect("a comms identity root");
        let service = Arc::new(MockSessionService::new());
        let _adapter = service.enable_runtime_adapter();
        let (storage, identity) =
            storage_over(InMemoryMobEventStore::new(), &MobStorage::in_memory());
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
            identity,
            handle,
            service,
        }
    }

    fn mob_id(&self) -> MobId {
        self.handle.mob_id().clone()
    }

    /// The handle on the agent lane.
    fn agent(&self) -> MobHandle {
        self.handle
            .clone()
            .with_command_authority(CommandAuthority::agent_lane())
    }

    /// Crash the mob, restart its session service and resume `storage`,
    /// whose identity store is `identity`.
    async fn restart_into(&mut self, storage: MobStorage, identity: InMemoryMobIdentityStore) {
        crash_stop_and_release_routes(self.handle.clone()).await;
        let restarted = Arc::new(self.service.cold_restart_preserving_durable_state().await);
        let _ = restarted.enable_runtime_adapter();
        self.service = restarted;
        self.handle = tokio::time::timeout(
            STEP,
            Box::pin(
                MobBuilder::for_resume(storage.clone())
                    .with_session_service(self.service.clone())
                    .with_member_comms_identity_root(self.keys.path())
                    .notify_orchestrator_on_resume(false)
                    .resume(),
            ),
        )
        .await
        .expect("the restart answers")
        .expect("the restart succeeds");
        self.storage = storage;
        self.identity = identity;
    }

    /// Crash the mob and resume the same storage.
    async fn restart(&mut self) {
        let storage = self.storage.clone();
        let identity = self.identity.clone();
        self.restart_into(storage, identity).await;
    }

    /// Spawn `name` as a worker and answer its session.
    async fn spawn_worker(&self, name: &str) -> SessionId {
        self.handle
            .spawn_spec(worker_spec(name))
            .await
            .expect("a worker");
        self.session(name)
            .await
            .unwrap_or_else(|| panic!("{name} is bound to a session"))
    }

    async fn session(&self, name: &str) -> Option<SessionId> {
        self.handle.resolve_bridge_session_id(&id(name)).await
    }

    async fn generation(&self, name: &str) -> Option<Generation> {
        self.handle
            .get_member(&id(name))
            .await
            .expect("roster read")
            .map(|entry| entry.generation)
    }

    async fn wired_to(&self, name: &str) -> BTreeSet<AgentIdentity> {
        self.handle
            .get_member(&id(name))
            .await
            .expect("roster read")
            .map(|entry| entry.wired_to)
            .unwrap_or_default()
    }

    async fn is_member(&self, name: &str) -> bool {
        self.handle
            .list_members()
            .await
            .iter()
            .any(|member| member.agent_identity == id(name))
    }

    /// Wait, within one step, until `name` is no member.
    async fn left(&self, name: &str) {
        let deadline = tokio::time::Instant::now() + STEP;
        while self.is_member(name).await {
            assert!(
                tokio::time::Instant::now() < deadline,
                "{name} never left the roster"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
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

    /// Store `name`'s created entry row with `presence`.
    async fn put_created(&self, name: &str, presence: CreatedIdentityPresence) {
        self.identity
            .put_created_identity(created_record(&self.mob_id(), name, presence))
            .await
            .expect("the identity store keeps the row");
    }

    /// Store `stored` as `name`'s row, as a later build wrote it.
    async fn put_raw(&self, name: &str, stored: Vec<u8>) {
        self.identity
            .put_raw_identity_row(&self.mob_id(), &id(name), stored)
            .await;
    }

    /// Remove `name`'s row from the identity store, as a reader that knows
    /// no created entry finds none.
    async fn hide_row(&self, name: &str) {
        self.identity
            .remove_identity_row_for_test(&self.mob_id(), &id(name))
            .await;
    }

    fn read(&self, name: &str) -> IdentityEntryRead {
        self.handle
            .identity_entry(&id(name))
            .expect("the entry read answers")
    }

    fn origin(&self, name: &str) -> Option<IdentityEntryOrigin> {
        match self.read(name) {
            IdentityEntryRead::Entry(view) => Some(view.origin),
            IdentityEntryRead::NotAnEntry { .. }
            | IdentityEntryRead::Unreadable { .. }
            | IdentityEntryRead::Removed => None,
        }
    }

    fn standing(&self, name: &str) -> Option<EntryStanding> {
        match self.read(name) {
            IdentityEntryRead::Entry(view) => Some(view.standing),
            IdentityEntryRead::NotAnEntry { .. }
            | IdentityEntryRead::Unreadable { .. }
            | IdentityEntryRead::Removed => None,
        }
    }

    /// Apply `roster`; the pass itself succeeds.
    async fn apply(&self, roster: DeclaredRoster) -> DeclaredRosterOutcome {
        tokio::time::timeout(STEP, self.handle.apply_declared_roster(roster))
            .await
            .expect("the apply answers")
            .expect("the apply runs")
    }

    /// The identity reconciler's status for `name`, as its store holds it.
    async fn reconcile_status(
        &self,
        name: &str,
    ) -> IdentityStoredObservation<crate::identity::IdentityConvergenceStatus> {
        self.storage
            .identity_status
            .load_identity_convergence_status(&self.mob_id(), &id(name))
            .await
            .expect("the status store answers")
    }
}

/// The session a seated standing names.
fn seated_session(standing: Option<EntryStanding>) -> Option<SessionId> {
    match standing {
        Some(EntryStanding::Seated(member)) => member.session_id,
        _ => None,
    }
}

/// A member row whose origin token this build does not know (a later
/// release wrote it) reserves the identity's name as unreadable at the
/// replay, never a worker's: the read answers unreadable with the member's
/// standing, the census is incomplete, a worker reconcile neither retires
/// the member nor spawns over it, and neither a spawn nor the agent lane acts
/// on it.
///
/// RED: drop the replay's reservation of an unknown origin token's name (the
/// `reserve_unknown_origin_name` calls of the replay): the member reads as a
/// worker (`NotAnEntry { worker: Some(_) }`) and the reconcile retires it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_stored_origin_token_reads_unreadable_never_a_worker() {
    let mut mob = CreatedMob::new("created-unknown-token").await;
    mob.handle
        .spawn_spec(entry_spec("later-entry"))
        .await
        .expect("the host declares its entry");
    let session = mob.session("later-entry").await;
    // The journal as a later release wrote it: the entry's spawn row names
    // an origin this build does not know.
    let journal = journal_copy(&mob.storage.events, |event| {
        if let Some(spawned) = event.kind.member_spawned_mut()
            && spawned.agent_identity == id("later-entry")
        {
            spawned.identity_entry = None;
            spawned.unknown_identity_entry = Some("a_later_origin".to_string());
        }
    })
    .await;
    let (storage, identity) = storage_over(journal, &mob.storage);
    mob.restart_into(storage, identity).await;

    let read = mob.read("later-entry");
    assert!(
        matches!(
            &read,
            IdentityEntryRead::Unreadable {
                member: Some(EntryStanding::Seated(_))
            }
        ),
        "{read:?}"
    );
    let census = mob.handle.identity_census().expect("the census");
    assert_eq!(census.unreadable, 1);
    assert!(!census.is_complete());

    let report = mob
        .handle
        .reconcile(Vec::new(), ReconcileOptions { retire_stale: true })
        .await
        .expect("the reconcile answers");
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert!(report.retired.is_empty(), "{:?}", report.retired);
    assert_eq!(mob.session("later-entry").await, session);

    let error = mob
        .handle
        .spawn_spec(worker_spec("later-entry"))
        .await
        .expect_err("no spawn takes the unreadable name");
    assert!(is_identity_member(&error, "later-entry"), "{error:?}");
    let error = mob
        .agent()
        .retire(id("later-entry"))
        .await
        .expect_err("the agent lane does not act on the unreadable name");
    assert!(is_identity_member(&error, "later-entry"), "{error:?}");
    assert!(mob.is_member("later-entry").await);
}

/// A worker reconcile never spawns a removed created entry's name, and never
/// retires a member whose name is unreadable: the reconcile helpers skip
/// both, as they skip entries.
///
/// RED: let the reconcile helpers skip entries only
/// (`mob_machine_members_to_spawn` and `mob_machine_members_to_retire` without
/// the tombstone and unreadable checks): the reconcile retires the unreadable
/// member, and its spawn of the removed name fails.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_worker_reconcile_never_spawns_or_retires_an_unreadable_or_tombstoned_name() {
    let mut mob = CreatedMob::new("created-reconcile").await;
    let session = mob.spawn_worker("unreadable-member").await;
    mob.put_raw("unreadable-member", later_build_row()).await;
    mob.put_created("removed", CreatedIdentityPresence::Removed)
        .await;
    mob.restart().await;
    assert!(matches!(
        mob.read("unreadable-member"),
        IdentityEntryRead::Unreadable { member: Some(_) }
    ));
    assert_eq!(mob.read("removed"), IdentityEntryRead::Removed);

    let report = mob
        .handle
        .reconcile(
            vec![worker_spec("removed"), worker_spec("fresh")],
            ReconcileOptions { retire_stale: true },
        )
        .await
        .expect("the reconcile answers");
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert!(report.retired.is_empty(), "{:?}", report.retired);
    assert_eq!(
        report
            .spawned
            .iter()
            .map(|spawned| spawned.agent_identity.clone())
            .collect::<Vec<_>>(),
        vec![id("fresh")]
    );
    assert!(!mob.is_member("removed").await);
    assert_eq!(mob.session("unreadable-member").await, Some(session));
}

/// No spawn and no dormant seat takes a removed created entry's name or one
/// whose entry this build cannot read, with entry custody or without: each
/// is refused `MobError::IdentityMember` before anything is created or
/// written.
///
/// RED: drop `ProbeMemberAdmissionIdentityNameReserved` and the reserved-name
/// guards of the admission probe and of `BeginSpawnExec`: the spawn of the
/// removed name opens its ladder, and the ladder close's own guard refuses
/// it untyped (`MobError::Internal`) once its session was created. Drop the
/// close guards too, and the spawn seats a member under the removed name.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_spawn_never_takes_a_tombstoned_or_unreadable_name() {
    let mut mob = CreatedMob::new("created-spawn").await;
    mob.put_created("removed", CreatedIdentityPresence::Removed)
        .await;
    mob.put_raw("unreadable", later_build_row()).await;
    mob.restart().await;
    let journal = mob.journal_len().await;

    for name in ["removed", "unreadable"] {
        for spec in [worker_spec(name), entry_spec(name)] {
            let error = mob
                .handle
                .spawn_spec(spec)
                .await
                .expect_err("no spawn takes a reserved name");
            assert!(is_identity_member(&error, name), "{name}: {error:?}");
        }
        let outcomes = mob
            .handle
            .seat_dormant_members(vec![worker_spec(name)])
            .await
            .expect("the seat batch answers");
        assert!(
            matches!(
                &outcomes[0],
                DormantSeatOutcome::Refused { error, .. } if is_identity_member(error, name)
            ),
            "{name}: {outcomes:?}"
        );
        assert!(!mob.is_member(name).await, "{name}");
    }
    assert_eq!(mob.journal_len().await, journal, "no refusal wrote a row");
}

/// The declared apply never declares over a created entry, a removed one or
/// an unreadable one: each listing is refused typed for that identity
/// alone, and the pass goes on and seats the other identity. The created
/// entry keeps its seat, session and generation.
///
/// RED: drop `DeclareIdentityEntryRefusedNameOwnedByCreated` and the
/// declared apply's origin guard (`declared_entry_or_no_entry` on the
/// `DeclareIdentityEntry` arms): the listing of the created entry is
/// answered `Kept`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_declared_listing_of_a_created_name_is_refused_for_that_identity_alone() {
    let mut mob = CreatedMob::new("created-declared-listing").await;
    let session = mob.spawn_worker("owner").await;
    mob.put_created("owner", CreatedIdentityPresence::Present)
        .await;
    mob.put_created("removed", CreatedIdentityPresence::Removed)
        .await;
    mob.put_raw("unreadable", later_build_row()).await;
    mob.restart().await;
    assert_eq!(mob.origin("owner"), Some(IdentityEntryOrigin::Created));
    let generation = mob.generation("owner").await;

    let outcome = mob
        .apply(DeclaredRoster::complete([
            declared("owner"),
            declared("removed"),
            declared("unreadable"),
            declared("plain"),
        ]))
        .await;
    for (name, expected) in [
        ("owner", DeclaredEntryRefusal::NameOwnedByCreated),
        ("removed", DeclaredEntryRefusal::NameTombstoned),
        ("unreadable", DeclaredEntryRefusal::NameUnreadable),
    ] {
        assert!(
            matches!(
                outcome.entries.get(&id(name)),
                Some(DeclaredEntryOutcome::Refused { refusal }) if *refusal == expected
            ),
            "{name}: {outcome:?}"
        );
    }
    assert!(
        matches!(
            outcome.entries.get(&id("plain")),
            Some(DeclaredEntryOutcome::Seated { .. })
        ),
        "{outcome:?}"
    );
    assert!(outcome.unlisted.is_empty(), "{outcome:?}");
    assert_eq!(mob.origin("owner"), Some(IdentityEntryOrigin::Created));
    assert_eq!(seated_session(mob.standing("owner")), Some(session));
    assert_eq!(mob.generation("owner").await, generation);
}

/// A complete declared roster that omits a created entry never retires it:
/// the seal unlists Declared entries only, so the created owner keeps its
/// seat, its session and its origin.
///
/// RED: let `SealDeclaredRosterSealed` unlist every entry the roster does not
/// list, whatever its origin (drop its Declared-origin filter): the seal
/// retires the created owner.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_declared_omission_never_retires_a_created_owner() {
    let mut mob = CreatedMob::new("created-omission").await;
    let session = mob.spawn_worker("owner").await;
    mob.put_created("owner", CreatedIdentityPresence::Present)
        .await;
    mob.restart().await;
    assert_eq!(mob.origin("owner"), Some(IdentityEntryOrigin::Created));

    let outcome = mob
        .apply(DeclaredRoster::complete([declared("listed")]))
        .await;
    assert!(outcome.unlisted.is_empty(), "{outcome:?}");
    assert_eq!(mob.origin("owner"), Some(IdentityEntryOrigin::Created));
    assert_eq!(seated_session(mob.standing("owner")), Some(session));
    assert!(mob.is_member("owner").await);
}

/// The identity reconciler never takes up a created entry's row: its
/// startup discovery reads the host-adopted intents' rows only, so a created
/// row and a later build's created row get no reconcile status, while an
/// intent row this build cannot read is recorded blocked as before.
///
/// RED: let the in-memory intent scan keep the created entries' rows this
/// build cannot read (drop its `created_domain` filter): the startup
/// discovery reads the later build's row as an unsupported intent and
/// records that identity blocked.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_adoption_reconciler_skips_created_rows() {
    let mut mob = CreatedMob::new("created-reconciler").await;
    mob.spawn_worker("owner").await;
    mob.put_created("owner", CreatedIdentityPresence::Present)
        .await;
    mob.put_raw("later", later_build_row()).await;
    // An intent row this build cannot read, ordered after every other row:
    // the discovery records a page's rows in identity order, so once this
    // one is recorded, every row before it was read.
    mob.put_raw(
        "zz-control",
        br#"{"schema_version":1,"future_shape":true}"#.to_vec(),
    )
    .await;
    mob.restart().await;

    let deadline = tokio::time::Instant::now() + STEP;
    while !matches!(
        mob.reconcile_status("zz-control").await,
        IdentityStoredObservation::Valid(_)
    ) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the unreadable intent row is never recorded"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    for name in ["owner", "later"] {
        assert!(
            matches!(
                mob.reconcile_status(name).await,
                IdentityStoredObservation::Missing
            ),
            "{name} has no reconcile status"
        );
    }
    assert_eq!(mob.origin("owner"), Some(IdentityEntryOrigin::Created));
}

/// The legacy adoption never adopts a created entry's name, nor a removed
/// or unreadable one, a declared entry's name that a created row the journal
/// contradicts reserves included: each is answered `NotAdoptable`, typed,
/// and nothing is written.
///
/// RED: answer `AlreadyAnEntry` for an entry of any origin before MobMachine
/// decides (the handle's shortcut over every origin, the shape before this
/// fix): the created owner is answered `AlreadyAnEntry`, as if a host had
/// adopted it. Answer it for a declared entry whatever its name's
/// reservation (the shortcut without its unreadable check): the reserved
/// declared name is answered `AlreadyAnEntry`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_legacy_adoption_answers_a_created_or_reserved_name_not_adoptable() {
    let mut mob = CreatedMob::new("created-adoption").await;
    mob.spawn_worker("owner").await;
    mob.handle
        .spawn_spec(entry_spec("declared"))
        .await
        .expect("a declared entry");
    mob.put_created("owner", CreatedIdentityPresence::Present)
        .await;
    // A created row of a declared entry's name: the journal contradicts it,
    // so the name is reserved unreadable and keeps its declared entry.
    mob.put_created("declared", CreatedIdentityPresence::Present)
        .await;
    mob.put_created("removed", CreatedIdentityPresence::Removed)
        .await;
    mob.put_raw("later", later_build_row()).await;
    mob.restart().await;
    assert_eq!(mob.origin("owner"), Some(IdentityEntryOrigin::Created));
    assert!(matches!(
        mob.read("declared"),
        IdentityEntryRead::Unreadable { member: Some(_) }
    ));

    let journal = mob.journal_len().await;
    for name in ["owner", "declared", "removed", "later"] {
        let answer = tokio::time::timeout(STEP, mob.handle.adopt_legacy_identity_entry(&id(name)))
            .await
            .expect("the adoption answers")
            .expect("the adoption is decided");
        assert_eq!(
            answer,
            crate::runtime::LegacyIdentityEntryAdoption::NotAdoptable,
            "{name}"
        );
    }
    assert_eq!(mob.journal_len().await, journal, "nothing is journaled");
    assert_eq!(mob.origin("owner"), Some(IdentityEntryOrigin::Created));
}

/// A spawn under way when its name is reserved seats no member under it:
/// MobMachine refuses the ladder's close, the shell answers the spawn with
/// the name's admission refusal and aborts the ladder, and the name stays
/// reserved with no member. No production path reaches this order (a
/// ladder opens only through BeginSpawnExec, which refuses a reserved name,
/// and the boot scan runs before any ladder opens), so a test-only state
/// injection reserves the name, as the boot scan's Unreadable arm would,
/// under the worker spawn's open ladder. A local spawn opens its ladder
/// after its session is created and prepares the ladder's close in the same
/// actor step, so no command lands between the two: the injection is armed
/// before the spawn and fires the moment the ladder opens.
///
/// RED: drop `name_not_reserved` from the ladder close arms (as before the
/// close guard): the worker is seated under the reserved name and the spawn
/// succeeds. Drop the shell's typed refusal (`reserved_name_refusal`): the
/// spawn answers the machine's untyped refusal, `MobError::Internal`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_spawn_under_way_when_its_name_is_reserved_is_aborted_and_the_name_not_taken() {
    let mob = CreatedMob::new("created-reserved-mid-spawn").await;
    let name = "reserved-mid-spawn";
    let (injected_tx, mut injected_rx) = tokio::sync::oneshot::channel();
    let armed = mob
        .handle
        .enqueue_actor_command_for_test(|reply_tx| {
            MobCommand::InjectUnreadableNameAtLadderOpenForTest {
                agent_identity: id(name),
                injected_tx,
                reply_tx,
            }
        })
        .await
        .expect("enqueue the state injection");
    tokio::time::timeout(STEP, armed)
        .await
        .expect("the injection is armed in time")
        .expect("the actor replies")
        .expect("the injection is armed");

    let answer = tokio::time::timeout(STEP, mob.handle.spawn_spec(worker_spec(name)))
        .await
        .expect("the spawn answers");
    // The injection fires inside the spawn's actor step, before the spawn
    // answers.
    injected_rx
        .try_recv()
        .expect("the injection fired under the worker spawn's open ladder")
        .expect("the name is reserved unreadable");
    assert!(
        matches!(&answer, Err(error) if is_identity_member(error, name)),
        "the spawn is refused typed: {answer:?}"
    );
    assert!(!mob.is_member(name).await, "the name takes no member");
    let dsl_name = crate::machines::mob_machine::AgentIdentity::from_domain(&id(name));
    assert!(
        !mob.handle
            .machine_state_watch_rx
            .borrow()
            .spawn_exec_phase
            .contains_key(&dsl_name),
        "the refused ladder is aborted"
    );
    assert!(
        matches!(
            mob.read(name),
            IdentityEntryRead::Unreadable { member: None }
        ),
        "the name stays reserved with no member"
    );
}

/// A fresh-life respawn of a worker whose name is reserved is refused before
/// anything is retired: its successor's spawn would be refused by the
/// reservation, so the respawn answers the name's admission refusal,
/// `MobError::IdentityMember`, and the member keeps its seat, generation
/// and session.
///
/// RED: let the respawn retire the member before its successor's admission
/// is checked (the shape before this fix): the member is retired, the
/// successor is refused (`MobRespawnError::SpawnAfterRetireWithCause`), and
/// the name keeps no member.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_respawn_under_a_reserved_name_is_refused_before_the_member_is_retired() {
    let mut mob = CreatedMob::new("created-reserved-respawn").await;
    let session = mob.spawn_worker("held").await;
    mob.put_raw("held", later_build_row()).await;
    mob.restart().await;
    assert!(matches!(
        mob.read("held"),
        IdentityEntryRead::Unreadable { member: Some(_) }
    ));
    let generation = mob.generation("held").await;
    let journal = mob.journal_len().await;

    let error = tokio::time::timeout(STEP, mob.handle.respawn(id("held"), None))
        .await
        .expect("the respawn answers")
        .expect_err("the respawn of a reserved name's member is refused");
    assert!(
        matches!(
            &error,
            crate::runtime::handle::MobRespawnError::Mob(error)
                if is_identity_member(error, "held")
        ),
        "{error:?}"
    );
    assert!(mob.is_member("held").await, "the member is kept");
    assert_eq!(mob.session("held").await, Some(session));
    assert_eq!(mob.generation("held").await, generation);
    assert_eq!(mob.journal_len().await, journal, "nothing is journaled");
}

/// The census counts created entries apart from declared ones, by standing
/// and cause, counts removed and unreadable names apart, and is incomplete
/// while a name is unreadable; the reads list an unreadable name as such and
/// a removed one as removed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_census_reports_incomplete_over_an_unreadable_created_entry() {
    let mut mob = CreatedMob::new("created-census").await;
    mob.spawn_worker("owner").await;
    mob.put_created("owner", CreatedIdentityPresence::Present)
        .await;
    mob.put_created(
        "preparing",
        CreatedIdentityPresence::PendingPreparation { attempt: 1 },
    )
    .await;
    mob.put_created("removed", CreatedIdentityPresence::Removed)
        .await;
    mob.put_raw("later", later_build_row()).await;
    mob.restart().await;

    let census = mob.handle.identity_census().expect("the census");
    assert_eq!(
        (
            census.declared,
            census.created,
            census.seated,
            census.tombstoned,
            census.unreadable
        ),
        (0, 2, 1, 1, 1)
    );
    assert_eq!(census.unseated_by_cause.template_preparation, 1);
    assert!(!census.is_complete());
    assert_eq!(
        mob.standing("preparing"),
        Some(EntryStanding::Unseated(
            UnseatedEntryCause::TemplatePreparation
        ))
    );
    assert!(matches!(
        mob.read("later"),
        IdentityEntryRead::Unreadable { member: None }
    ));
    assert_eq!(mob.read("removed"), IdentityEntryRead::Removed);
    let page = mob
        .handle
        .identity_entries(None, IDENTITY_ENTRY_PAGE_MAX)
        .expect("the page");
    assert_eq!(
        page.entries
            .iter()
            .map(|entry| entry.identity.clone())
            .collect::<Vec<_>>(),
        vec![id("owner"), id("preparing")]
    );
    assert_eq!(page.unreadable, vec![id("later")]);
}

/// A resume of a SQLite mob recovers every created entry row of its identity
/// store when host-adopted intents' rows sit between them in key order: the
/// created scan leaves out the intents' rows and no created row. Each created
/// row reads as it is stored (a created entry, a removed one, one pending its
/// preparation, a later build's row as unreadable), the census counts them,
/// and no host-adopted identity reads as an entry.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sqlite_resume_recovers_every_created_row_among_host_adopted_intents() {
    let dir = tempfile::tempdir().expect("a mob directory");
    let path = dir.path().join("mob.sqlite");
    let service = Arc::new(MockSessionService::new());
    let _adapter = service.enable_runtime_adapter();
    let created = MobBuilder::new(
        with_unique_mob_id(sample_definition(), "created-sqlite-resume"),
        MobStorage::persistent(&path).expect("open the mob store"),
    )
    .with_session_service(service.clone())
    .allow_ephemeral_sessions(true)
    .create()
    .await
    .expect("create the mob");
    let mob_id = created.mob_id().clone();
    created.shutdown().await.expect("stop the mob");
    drop(created);

    {
        let stores =
            crate::store::SqliteMobStores::open(&path).expect("open the mob's identity store");
        let store = stores.identity_store();
        for name in ["a-adopted", "c-adopted", "e-adopted", "h-adopted"] {
            let (request, compiled) = crate::identity::identity_adoption_fixture(
                mob_id.clone(),
                id(name),
                &format!("adopt-{name}"),
                SessionId::new(),
            );
            let outcome = store
                .adopt_member_identity_declaration(&request, &compiled)
                .await
                .expect("the host adopts the identity");
            assert!(
                matches!(
                    outcome,
                    crate::identity::IdentityAdoptionOutcome::Adopted { .. }
                ),
                "{name}: {outcome:?}"
            );
        }
        for (name, presence) in [
            ("b-created", CreatedIdentityPresence::Present),
            ("d-removed", CreatedIdentityPresence::Removed),
            (
                "f-preparing",
                CreatedIdentityPresence::PendingPreparation { attempt: 1 },
            ),
        ] {
            store
                .put_identity_row_for_test(
                    &mob_id,
                    &id(name),
                    serde_json::to_vec(&created_record(&mob_id, name, presence))
                        .expect("encode the created row"),
                )
                .await
                .expect("store the created row");
        }
        store
            .put_identity_row_for_test(&mob_id, &id("g-later"), later_build_row())
            .await
            .expect("store the later build's row");
    }

    let resumed = tokio::time::timeout(
        STEP,
        Box::pin(
            MobBuilder::for_resume(MobStorage::persistent(&path).expect("reopen the mob store"))
                .with_session_service(service)
                .allow_ephemeral_sessions(true)
                .resume(),
        ),
    )
    .await
    .expect("the resume answers")
    .expect("the resume succeeds");
    let read = |name: &str| {
        resumed
            .identity_entry(&id(name))
            .expect("the entry read answers")
    };
    assert!(
        matches!(
            read("b-created"),
            IdentityEntryRead::Entry(view) if view.origin == IdentityEntryOrigin::Created
        ),
        "{:?}",
        read("b-created")
    );
    assert_eq!(read("d-removed"), IdentityEntryRead::Removed);
    assert!(
        matches!(
            read("f-preparing"),
            IdentityEntryRead::Entry(view)
                if view.origin == IdentityEntryOrigin::Created
                    && view.standing
                        == EntryStanding::Unseated(UnseatedEntryCause::TemplatePreparation)
        ),
        "{:?}",
        read("f-preparing")
    );
    assert!(
        matches!(
            read("g-later"),
            IdentityEntryRead::Unreadable { member: None }
        ),
        "{:?}",
        read("g-later")
    );
    for name in ["a-adopted", "c-adopted", "e-adopted", "h-adopted"] {
        assert!(
            matches!(read(name), IdentityEntryRead::NotAnEntry { .. }),
            "{name}: {:?}",
            read(name)
        );
    }
    let census = resumed.identity_census().expect("the census");
    assert_eq!(
        (census.created, census.tombstoned, census.unreadable),
        (2, 1, 1)
    );
    tokio::time::timeout(STEP, resumed.shutdown())
        .await
        .expect("the shutdown answers")
        .expect("stop the resumed mob");
}

/// A created entry whose member a build that predates created entries
/// retired (its worker reconcile with `retire_stale` retires any member it
/// does not list, and its retirement row records no archived session) is
/// seated again at the next start, Dormant, on the session the journal last
/// bound it to, which the retirement archived: the same identity, a created
/// entry, on its own session, and its first wake revives that session.
///
/// Also the in-process leg of the rollback row of a retire-stale reconcile:
/// this build stands in for the predecessor with the created row hidden, as
/// a reader that knows no created entry finds none, and the journal it
/// leaves is edited to the predecessor's rows.
///
/// The seat carries no roster labels, whatever the member's last spawn row
/// carried, so no label-based authority reaches a created identity.
///
/// RED: drop the start's seat of the created entries owed their seat
/// (`reseat_owed_created_entries`): the entry stays unseated, owed its seat,
/// and no member holds it. Drop `RecoverCreatedIdentityEntryRetired`: the
/// start refuses the present row of an identity the journal left without a
/// member. Copy the last spawn row's labels into the seat (`reseat_spec`):
/// the seated entry carries the owner's label.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_present_created_entry_whose_member_an_older_binary_retired_is_reseated_on_its_archived_session()
 {
    let mut mob = CreatedMob::new("created-reseat").await;
    let mut owner = worker_spec("owner");
    owner.labels = Some(BTreeMap::from([("team".to_string(), "alpha".to_string())]));
    mob.handle
        .spawn_spec(owner)
        .await
        .expect("the labelled owner");
    let session = mob
        .session("owner")
        .await
        .expect("the owner is bound to a session");
    mob.put_created("owner", CreatedIdentityPresence::Present)
        .await;
    mob.restart().await;
    assert_eq!(mob.origin("owner"), Some(IdentityEntryOrigin::Created));

    // The predecessor: no created row, so its reconcile retires the member.
    mob.hide_row("owner").await;
    mob.restart().await;
    assert!(matches!(
        mob.read("owner"),
        IdentityEntryRead::NotAnEntry { worker: Some(_) }
    ));
    let report = mob
        .handle
        .reconcile(Vec::new(), ReconcileOptions { retire_stale: true })
        .await
        .expect("the reconcile answers");
    assert_eq!(report.retired, vec![id("owner")]);
    mob.left("owner").await;
    let retired_generation = Generation::new(0);

    // The journal as the predecessor left it: the seat row of the created
    // entry recorded by this build, and a retirement row with no archived
    // session.
    let journal = journal_copy(&mob.storage.events, |event| match &mut event.kind {
        MobEventKind::MemberSpawned(spawned) if spawned.agent_identity == id("owner") => {
            spawned.identity_entry = Some(IdentityEntryOrigin::Created);
        }
        MobEventKind::MemberRetired {
            agent_identity,
            session_archived,
            ..
        } if *agent_identity == id("owner") => {
            *session_archived = false;
        }
        _ => {}
    })
    .await;
    // The roll-forward: this build over the predecessor's journal, the
    // created row present as it was all along.
    let (storage, identity) = storage_over(journal, &mob.storage);
    identity
        .put_created_identity(created_record(
            &mob.mob_id(),
            "owner",
            CreatedIdentityPresence::Present,
        ))
        .await
        .expect("the identity store keeps the row");
    mob.restart_into(storage, identity).await;

    assert_eq!(mob.origin("owner"), Some(IdentityEntryOrigin::Created));
    assert_eq!(seated_session(mob.standing("owner")), Some(session.clone()));
    assert!(
        mob.generation("owner")
            .await
            .is_some_and(|generation| generation > retired_generation),
        "the entry is seated at its next generation"
    );
    assert!(
        mob.handle
            .get_member(&id("owner"))
            .await
            .expect("roster read")
            .expect("the seated owner")
            .labels
            .is_empty(),
        "a created seat carries no roster labels"
    );
    tokio::time::timeout(STEP, mob.handle.ensure_member_live(&id("owner")))
        .await
        .expect("the wake answers")
        .expect("the wake revives the archived session");
    assert_eq!(mob.session("owner").await, Some(session));
}

/// Rollback of a created owner, in-process leg: a created owner served by a
/// reader that knows no created entry (this build with the created row
/// hidden) is the same member on the same session with its wiring; after the
/// roll-forward it is the created entry again, on the same session, with the
/// same edge. Nothing is retired or recreated either way.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn roll_forward_reprojects_on_the_same_session_in_process() {
    let mut mob = CreatedMob::new("created-roll-forward").await;
    let session = mob.spawn_worker("owner").await;
    mob.spawn_worker("peer").await;
    mob.handle
        .wire(id("owner"), id("peer"))
        .await
        .expect("wire the owner to its peer");
    mob.put_created("owner", CreatedIdentityPresence::Present)
        .await;
    mob.restart().await;
    assert_eq!(mob.origin("owner"), Some(IdentityEntryOrigin::Created));
    let generation = mob.generation("owner").await;

    mob.hide_row("owner").await;
    mob.restart().await;
    assert!(matches!(
        mob.read("owner"),
        IdentityEntryRead::NotAnEntry { worker: Some(_) }
    ));
    assert_eq!(mob.session("owner").await, Some(session.clone()));
    assert_eq!(mob.wired_to("owner").await, BTreeSet::from([id("peer")]));

    mob.put_created("owner", CreatedIdentityPresence::Present)
        .await;
    mob.restart().await;
    assert_eq!(mob.origin("owner"), Some(IdentityEntryOrigin::Created));
    assert_eq!(seated_session(mob.standing("owner")), Some(session));
    assert_eq!(mob.generation("owner").await, generation);
    assert_eq!(mob.wired_to("owner").await, BTreeSet::from([id("peer")]));
}

/// The member a created owner's rollback fixture holds beside the owner.
#[derive(Clone, Copy)]
enum FixturePeer {
    /// None.
    Absent,
    /// `peer`, unwired: an identity a predecessor's identity-first host can
    /// try to adopt in the same mob.
    Unwired,
    /// `peer`, wired to the owner.
    Wired,
}

/// The created owner's fixture of the rollback rows: a persistent mob store
/// (SQLite) at `$MEERKAT_ROLLBACK_FIXTURE_DIR/<name>/mob.sqlite` holding the
/// member `owner`, the member `peer` as `peer` says, and `owner`'s created
/// entry row (present), as this build leaves a created entry.
async fn write_created_owner_fixture(name: &str, peer: FixturePeer) {
    let dir = rollback_fixture_dir(name);
    let mob_id = format!("rollback-{name}");
    super::identity_entry_rows::write_rollback_fixture(&dir, &mob_id, |handle| async move {
        handle
            .spawn_spec(worker_spec("owner"))
            .await
            .expect("spawn the owner");
        if !matches!(peer, FixturePeer::Absent) {
            handle
                .spawn_spec(worker_spec("peer"))
                .await
                .expect("spawn the peer");
        }
        if matches!(peer, FixturePeer::Wired) {
            handle
                .wire(id("owner"), id("peer"))
                .await
                .expect("wire the owner to its peer");
        }
    })
    .await;
    let mob_id = MobId::from(mob_id.as_str());
    let stores = crate::store::SqliteMobStores::open(dir.join("mob.sqlite"))
        .expect("open the fixture store");
    stores
        .identity_store()
        .put_identity_row_for_test(
            &mob_id,
            &id("owner"),
            serde_json::to_vec(&created_record(
                &mob_id,
                "owner",
                CreatedIdentityPresence::Present,
            ))
            .expect("encode the created row"),
        )
        .await
        .expect("store the created row");
}

fn rollback_fixture_dir(name: &str) -> std::path::PathBuf {
    let dir = std::path::PathBuf::from(
        std::env::var_os("MEERKAT_ROLLBACK_FIXTURE_DIR")
            .expect("MEERKAT_ROLLBACK_FIXTURE_DIR names the fixture directory"),
    )
    .join(name);
    std::fs::create_dir_all(&dir).expect("create the fixture directory");
    dir
}

/// Rollback of a fresh created owner under an identity-first host, lane-run leg:
/// `fresh_created_owner_is_not_retired_by_the_identity_first_pass_under_<pair>`
/// for each published pair it runs against, with the pair's recorded
/// artifacts.
///
/// This build writes the fixture (`write_created_owner_fixture`): the
/// created owner and an unwired member `peer`. The lane runs the pair's
/// identity-first host over it and checks: the created row reads unreadable
/// to the predecessor (its member binding history reads `IntentUnreadable`),
/// its identity-first roster pass and verbs refuse the identity, nothing
/// retires the member, and nothing is deleted (the row is byte-identical
/// afterwards). It then adopts `peer` under the predecessor and asserts the
/// whole-mob refusal the downgrade guide states: on SQLite a published 0.9.7
/// or 0.9.8 refuses every new identity-first adoption in a mob that holds a
/// created entry's row, while its reads stay per identity.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "lane:rollback - writes the fixture the published pairs run"]
async fn fresh_created_owner_is_not_retired_by_the_identity_first_pass_fixture() {
    write_created_owner_fixture("fresh-created-owner", FixturePeer::Unwired).await;
}

/// Rollback of a created owner rolled forward, lane-run leg:
/// `roll_forward_reprojects_on_the_same_session_under_<pair>` for each pair.
///
/// This build writes the fixture: the created owner wired to its peer. The
/// lane serves it with the pair, rolls it forward to this build and checks:
/// the same member on the same session, with its edge, a created entry
/// again; nothing retired or recreated.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "lane:rollback - writes the fixture the published pairs run"]
async fn roll_forward_reprojects_on_the_same_session_fixture() {
    write_created_owner_fixture("created-owner-roll-forward", FixturePeer::Wired).await;
}

/// Rollback of a created owner under a retire-stale reconcile, lane-run leg:
/// `a_retire_stale_reconcile_under_<pair>_archives_and_roll_forward_reseats_on_the_same_session`
/// for each pair.
///
/// This build writes the fixture: the created owner. The lane runs the
/// pair's worker reconcile with `retire_stale` over it (the predecessor
/// retires the member and archives its session), rolls it forward to this
/// build and checks: the created entry is seated again, Dormant, on that
/// session at its next generation, and its first wake revives it. Lifecycle
/// only.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "lane:rollback - writes the fixture the published pairs run"]
async fn a_retire_stale_reconcile_archives_and_roll_forward_reseats_on_the_same_session_fixture() {
    write_created_owner_fixture("created-owner-retire-stale", FixturePeer::Absent).await;
}
