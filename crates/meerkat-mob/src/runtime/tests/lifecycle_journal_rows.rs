//! The facts of the member lifecycle journal read
//! (`MobHandle::member_lifecycle_events`): what each retirement and seat row
//! records of what MobMachine decided in the step that wrote it. A retirement
//! names the session its incarnation was bound to at its end, the identity
//! entry it was the member of, why it ended (the declared apply's seal
//! unlisted the identity) and the rotation whose terminal step retired it; a
//! seat names its session and the rotation that seated it: a rotation's
//! terminal step, or a fresh life of an identity no member held. A reader
//! pairs a retirement with the seat that follows it from these facts alone,
//! never from their positions in the journal, and the facts read the same
//! after a restart on every store. The rows run on the combined candidate:
//! the machine, the actor, the journal and the replay together.

use std::collections::BTreeSet;

use super::*;
use crate::machines::mob_machine as dsl;
use crate::runtime::{
    DeclaredEntry, DeclaredRoster, DeclaredRosterOutcome, DormantSeatOutcome, EntryStanding,
    IDENTITY_ENTRY_PAGE_MAX, IdentityEntryOrigin, IdentityEntryRead, MemberLifecycleEvent,
    MemberLifecycleRecord, MemberRespawn, MemberRetirementCause, MobJournalCursor, RetiredRotation,
    RotationTarget, SeatRotation, UnlistedEntryOutcome, UnseatedEntryCause,
};
use crate::store::SpiMobStoreHost;
use meerkat_core::host_store::{AcquireMode, HostStore};
use meerkat_runtime::store::spi::{SpiRealm, SpiRealmConfig};
use meerkat_store_conformance::InMemoryHostStore;

const STEP: Duration = Duration::from_mins(1);

const G0: Generation = Generation::new(0);
const G1: Generation = Generation::new(1);
const G2: Generation = Generation::new(2);

fn id(name: &str) -> AgentIdentity {
    AgentIdentity::from(name)
}

/// A turn-driven worker spec: it owes no kickoff, so a seat of it is
/// Dormant and a fresh life of it is a seat.
fn worker_spec(name: &str) -> SpawnMemberSpec {
    let mut spec = SpawnMemberSpec::new("worker", id(name));
    spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    spec
}

/// A worker spec under the profile's own runtime mode (`AutonomousHost`):
/// a fresh life of it owes a kickoff, so it is spawned live.
fn autonomous_spec(name: &str) -> SpawnMemberSpec {
    SpawnMemberSpec::new("worker", id(name))
}

/// `name` declared as a turn-driven worker.
fn declared(name: &str) -> DeclaredEntry {
    DeclaredEntry::new(worker_spec(name))
}

/// What a `Retired` fact records beyond its identity and generation.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RetiredFacts {
    role: ProfileName,
    session_id: Option<SessionId>,
    identity_entry: Option<IdentityEntryOrigin>,
    cause: Option<MemberRetirementCause>,
    rotation: Option<RetiredRotation>,
}

impl RetiredFacts {
    /// A worker's retirement that ended on `session_id`: no entry, no cause
    /// and no rotation.
    fn plain(session_id: Option<SessionId>) -> Self {
        Self {
            role: ProfileName::from("worker"),
            session_id,
            identity_entry: None,
            cause: None,
            rotation: None,
        }
    }
}

/// What a `Spawned` fact records of its role, its session and the rotation
/// that seated it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SeatFacts {
    role: ProfileName,
    session_id: Option<SessionId>,
    rotation: Option<SeatRotation>,
}

/// The `Retired` fact of `name` at `generation`, with its journal cursor.
fn retired(
    records: &[MemberLifecycleRecord],
    name: &str,
    generation: Generation,
) -> Option<(MobJournalCursor, RetiredFacts)> {
    records.iter().find_map(|record| match &record.event {
        MemberLifecycleEvent::Retired {
            agent_identity,
            generation: retired,
            role,
            session_id,
            identity_entry,
            cause,
            rotation,
            ..
        } if agent_identity.as_str() == name && *retired == generation => Some((
            record.cursor,
            RetiredFacts {
                role: role.clone(),
                session_id: session_id.clone(),
                identity_entry: *identity_entry,
                cause: *cause,
                rotation: rotation.clone(),
            },
        )),
        _ => None,
    })
}

/// The `Spawned` fact of `name` at `generation`, with its journal cursor.
fn seated(
    records: &[MemberLifecycleRecord],
    name: &str,
    generation: Generation,
) -> Option<(MobJournalCursor, SeatFacts)> {
    records.iter().find_map(|record| match &record.event {
        MemberLifecycleEvent::Spawned {
            agent_identity,
            generation: seated,
            role,
            session_id,
            rotation,
            ..
        } if agent_identity.as_str() == name && *seated == generation => Some((
            record.cursor,
            SeatFacts {
                role: role.clone(),
                session_id: session_id.clone(),
                rotation: rotation.clone(),
            },
        )),
        _ => None,
    })
}

/// The identity entry `name`'s `Spawned` fact of `generation` names.
fn seat_entry(
    records: &[MemberLifecycleRecord],
    name: &str,
    generation: Generation,
) -> Option<IdentityEntryOrigin> {
    records.iter().find_map(|record| match &record.event {
        MemberLifecycleEvent::Spawned {
            agent_identity,
            generation: seated,
            identity_entry,
            ..
        } if agent_identity.as_str() == name && *seated == generation => *identity_entry,
        _ => None,
    })
}

/// The link of `name`'s rotation from its first incarnation to its second:
/// the rotation its retirement at G0 names, and the one its seat at G1
/// names.
fn rotation_link_of(
    records: &[MemberLifecycleRecord],
    name: &str,
) -> (Option<RetiredRotation>, Option<SeatRotation>) {
    (
        retired(records, name, G0).and_then(|(_, facts)| facts.rotation),
        seated(records, name, G1).and_then(|(_, seat)| seat.rotation),
    )
}

/// The journal event right after `name`'s `MemberRetired` row of
/// `generation`.
fn event_after_retirement<'a>(
    journal: &'a [MobEvent],
    name: &str,
    generation: Generation,
) -> Option<&'a MobEventKind> {
    let at = journal.iter().position(|event| {
        matches!(
            &event.kind,
            MobEventKind::MemberRetired {
                agent_identity,
                generation: retired,
                ..
            } if agent_identity.as_str() == name && *retired == generation
        )
    })?;
    journal.get(at + 1).map(|event| &event.kind)
}

/// Whether `kind` is `name`'s `MemberSpawned` row of `generation`.
fn is_seat_of(kind: Option<&MobEventKind>, name: &str, generation: Generation) -> bool {
    matches!(
        kind,
        Some(MobEventKind::MemberSpawned(spawned))
            if spawned.agent_identity.as_str() == name && spawned.generation == generation
    )
}

/// Every member lifecycle fact of `handle`'s journal, from its first event.
async fn lifecycle(handle: &MobHandle) -> Vec<MemberLifecycleRecord> {
    let mut records = Vec::new();
    let mut after = MobJournalCursor::START;
    loop {
        let page = handle
            .member_lifecycle_events(after, IDENTITY_ENTRY_PAGE_MAX)
            .await
            .expect("the journal read answers");
        if page.next == after {
            return records;
        }
        records.extend(page.records);
        after = page.next;
    }
}

/// A running mob over the mock durable session service, with session-scoped
/// member comms identities. A restart resumes its storage, or a reopened
/// copy of it, over a cold-restarted session service.
struct JournalMob {
    keys: tempfile::TempDir,
    storage: MobStorage,
    handle: MobHandle,
    service: Arc<MockSessionService>,
}

impl JournalMob {
    /// A new mob over in-memory storage.
    async fn new(label: &str) -> Self {
        Self::over(
            with_unique_mob_id(sample_definition(), label),
            MobStorage::in_memory(),
        )
        .await
    }

    /// A new mob of `definition` over `storage`.
    async fn over(definition: MobDefinition, storage: MobStorage) -> Self {
        let keys = tempfile::tempdir().expect("a comms identity root");
        let service = Arc::new(MockSessionService::new());
        let _adapter = service.enable_runtime_adapter();
        let handle = MobBuilder::new(definition, storage.clone())
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

    /// Crash the mob: its session service restarts with the durable state it
    /// held, and nothing the mob did not journal survives.
    async fn crash(&mut self) {
        let restarted = Arc::new(self.service.cold_restart_preserving_durable_state().await);
        let _ = restarted.enable_runtime_adapter();
        crash_stop_and_release_routes(self.handle.clone()).await;
        self.service = restarted;
    }

    /// Resume `storage` (the crashed mob's own, or a reopened copy of it)
    /// over the session service the crash restarted.
    async fn resume(&mut self, storage: MobStorage) {
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
    }

    /// Crash the mob and resume `storage`.
    async fn restart_into(&mut self, storage: MobStorage) {
        self.crash().await;
        self.resume(storage).await;
    }

    async fn session(&self, name: &str) -> SessionId {
        self.handle
            .resolve_bridge_session_id(&id(name))
            .await
            .unwrap_or_else(|| panic!("{name} is bound to a session"))
    }

    /// Spawn `name` as a turn-driven worker.
    async fn spawn_worker(&self, name: &str) {
        tokio::time::timeout(STEP, self.handle.spawn_spec(worker_spec(name)))
            .await
            .expect("the spawn answers")
            .unwrap_or_else(|error| panic!("{name} is spawned: {error}"));
    }

    async fn retire(&self, name: &str) {
        tokio::time::timeout(STEP, self.handle.retire(id(name)))
            .await
            .expect("the retire answers")
            .unwrap_or_else(|error| panic!("{name} is retired: {error}"));
    }

    /// `rotate_member` with `spec`: the session the next incarnation is
    /// seated on.
    async fn rotate(&self, spec: SpawnMemberSpec) -> SessionId {
        tokio::time::timeout(STEP, self.handle.rotate_member(spec))
            .await
            .expect("the rotation answers")
            .expect("the rotation seats the next incarnation")
    }

    async fn respawn(&self, name: &str, request: MemberRespawn) {
        tokio::time::timeout(STEP, self.handle.respawn_member(id(name), request))
            .await
            .expect("the respawn answers")
            .unwrap_or_else(|error| panic!("{name} is respawned: {error}"));
    }

    async fn apply(&self, roster: DeclaredRoster) -> DeclaredRosterOutcome {
        tokio::time::timeout(STEP, self.handle.apply_declared_roster(roster))
            .await
            .expect("the apply answers")
            .expect("the apply runs")
    }

    /// Wake `name`, which succeeds, and wait until MobMachine recorded the
    /// epoch of its registration (its routed binding landed).
    async fn wake(&self, name: &str) {
        tokio::time::timeout(STEP, self.handle.ensure_member_live(&id(name)))
            .await
            .expect("the wake answers")
            .expect("the member wakes");
        let key = dsl::AgentIdentity::from_domain(&id(name));
        let mut state = self.handle.machine_state_watch_rx.clone();
        let landed = tokio::time::timeout(
            STEP,
            state.wait_for(|state| {
                state
                    .member_materialization_epoch
                    .get(&key)
                    .is_some_and(Option::is_some)
            }),
        )
        .await;
        assert!(
            matches!(landed, Ok(Ok(_))),
            "the binding of {name}'s registration never landed"
        );
    }

    async fn journal(&self) -> Vec<MobEvent> {
        self.storage
            .events
            .poll(0, usize::MAX)
            .await
            .expect("read the journal")
    }

    async fn lifecycle(&self) -> Vec<MemberLifecycleRecord> {
        lifecycle(&self.handle).await
    }

    /// The journal read's facts once `done` holds of them, within one step:
    /// `what` names what the row waits for.
    async fn lifecycle_until(
        &self,
        what: &str,
        done: impl Fn(&[MemberLifecycleRecord]) -> bool,
    ) -> Vec<MemberLifecycleRecord> {
        let deadline = tokio::time::Instant::now() + STEP;
        loop {
            let records = self.lifecycle().await;
            if done(&records) {
                return records;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "{what}: not journaled within a step; {records:?}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Crash the mob and resume its own storage.
    async fn restart(&mut self) {
        let storage = self.storage.clone();
        self.restart_into(storage).await;
    }
}

/// A rotation's terminal step journals its link on both of its rows: the
/// retirement names the rotation, its target and the incarnation it seated
/// under its role, and the seat names the incarnation it retired. A
/// rotation onto a fresh session (`rotate_member`) names the old session on
/// the retirement and the new one on the seat; a same-session successor
/// under another role (`respawn_member` naming a profile) names the kept
/// session on both, and the successor's role. Neither records a cause: a
/// rotation keeps the identity's standing.
///
/// RED (before the rows record the facts): both rows read no rotation and
/// no session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rotation_journals_its_link_on_both_rows() {
    let mob = JournalMob::new("rotation-link").await;
    mob.spawn_worker("rotated").await;
    let old = mob.session("rotated").await;
    let new = mob.rotate(worker_spec("rotated")).await;
    assert_ne!(old, new, "the rotation seats a new session");
    let records = mob.lifecycle().await;
    let (retired_at, retirement) =
        retired(&records, "rotated", G0).expect("the rotation's retirement is journaled");
    assert_eq!(
        retirement,
        RetiredFacts {
            rotation: Some(RetiredRotation::new(
                RotationTarget::FreshSession,
                G1,
                ProfileName::from("worker"),
            )),
            ..RetiredFacts::plain(Some(old))
        },
        "the retirement names the rotation that ended it, with its successor"
    );
    let (seated_at, seat) = seated(&records, "rotated", G1).expect("the successor is journaled");
    assert_eq!(
        seat,
        SeatFacts {
            role: ProfileName::from("worker"),
            session_id: Some(new),
            rotation: Some(SeatRotation::successor(G0, RotationTarget::FreshSession)),
        },
        "the seat names the incarnation the same step retired"
    );
    assert!(seated_at > retired_at, "one batch, the retirement first");

    mob.spawn_worker("respawned").await;
    let kept = mob.session("respawned").await;
    mob.respawn("respawned", MemberRespawn::new().with_profile("lead"))
        .await;
    let records = mob.lifecycle().await;
    assert_eq!(
        retired(&records, "respawned", G0).map(|(_, facts)| facts),
        Some(RetiredFacts {
            rotation: Some(RetiredRotation::new(
                RotationTarget::SameSession,
                G1,
                ProfileName::from("lead"),
            )),
            ..RetiredFacts::plain(Some(kept.clone()))
        }),
        "the same-session retirement names its rotation and the successor's role"
    );
    assert_eq!(
        seated(&records, "respawned", G1).map(|(_, seat)| seat),
        Some(SeatFacts {
            role: ProfileName::from("lead"),
            session_id: Some(kept),
            rotation: Some(SeatRotation::successor(G0, RotationTarget::SameSession)),
        }),
        "the successor is seated on the kept session"
    );
}

/// The end of an unlisted retirement journals its cause and the entry its
/// incarnation was the member of, though that same step ended the entry:
/// the declared apply's seal unlisted the identity (MobMachine's mark, read
/// before the step). A host's retire of an identity that stays listed
/// journals the entry and no cause, and the entry outlives it.
///
/// RED (before the rows record the facts): the unlisted retirement reads no
/// cause, no entry and no session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unlisted_retirement_journals_its_cause_and_entry_at_its_end() {
    let mob = JournalMob::new("unlisted-facts").await;
    mob.apply(DeclaredRoster::complete([
        declared("listed"),
        declared("unlisted"),
    ]))
    .await;
    let unlisted = mob.session("unlisted").await;
    let sealed = mob
        .apply(DeclaredRoster::complete([declared("listed")]))
        .await;
    assert!(
        matches!(
            sealed.unlisted.get(&id("unlisted")),
            Some(UnlistedEntryOutcome::Retired)
        ),
        "{sealed:?}"
    );
    assert!(
        matches!(
            mob.handle.identity_entry(&id("unlisted")),
            Ok(IdentityEntryRead::NotAnEntry { worker: None })
        ),
        "the end of the unlisted retirement ended the entry"
    );
    let records = mob.lifecycle().await;
    assert_eq!(
        retired(&records, "unlisted", G0).map(|(_, facts)| facts),
        Some(RetiredFacts {
            identity_entry: Some(IdentityEntryOrigin::Declared),
            cause: Some(MemberRetirementCause::Unlisted),
            ..RetiredFacts::plain(Some(unlisted))
        }),
        "the unlisted retirement names its cause and the entry it ended"
    );

    mob.wake("listed").await;
    let listed = mob.session("listed").await;
    mob.retire("listed").await;
    assert!(
        matches!(
            mob.handle.identity_entry(&id("listed")),
            Ok(IdentityEntryRead::Entry(view))
                if view.standing == EntryStanding::Unseated(UnseatedEntryCause::SeatOwed)
        ),
        "the entry outlives the host's retire"
    );
    let records = mob.lifecycle().await;
    assert_eq!(
        retired(&records, "listed", G0).map(|(_, facts)| facts),
        Some(RetiredFacts {
            identity_entry: Some(IdentityEntryOrigin::Declared),
            ..RetiredFacts::plain(Some(listed))
        }),
        "the host's retirement names the entry and no cause"
    );
}

/// A plain retirement journals the session its incarnation was bound to at
/// its end, and no rotation and no cause; the seat that began the
/// incarnation names the same session and no rotation.
///
/// RED (before the rows record the facts): both rows read no session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_plain_retirement_journals_its_session_and_no_rotation() {
    let mob = JournalMob::new("plain-retirement").await;
    mob.spawn_worker("worker").await;
    let session = mob.session("worker").await;
    mob.retire("worker").await;
    let records = mob.lifecycle().await;
    assert_eq!(
        retired(&records, "worker", G0).map(|(_, facts)| facts),
        Some(RetiredFacts::plain(Some(session.clone()))),
        "the retirement names its session, and no rotation or cause"
    );
    assert_eq!(
        seated(&records, "worker", G0).map(|(_, seat)| seat),
        Some(SeatFacts {
            role: ProfileName::from("worker"),
            session_id: Some(session),
            rotation: None,
        }),
        "the spawn names its session and no rotation"
    );
}

/// A peer-only member is bound to no session of its own: its retirement and
/// its seat name none.
///
/// Guard: holds before and after the rows record the facts; it fails when a
/// writer names a session the retirement's journal effect does not.
#[tokio::test]
async fn a_peer_only_retirement_journals_no_session() {
    let _serial = lock_real_comms_tests();
    let definition = with_unique_mob_id(
        sample_definition_with_external_backend(),
        "peer-only-retired-facts",
    );
    let mob_id = definition.id.clone();
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let owner_bridge_session_id = SessionId::new();
    let handle = MobBuilder::new(definition, MobStorage::in_memory())
        .with_session_service(service)
        .with_owner_bridge_session_create_authority(owner_bridge_session_id.clone(), false, false)
        .create()
        .await
        .expect("create the mob");
    let identity = id("peer-only-worker");
    let external =
        spawn_live_external_peer(&test_comms_name_for(&mob_id, "worker", identity.as_str())).await;
    let mut spec = SpawnMemberSpec::new("worker", identity.clone());
    spec.binding = Some(external.binding());
    handle
        .spawn_spec_receipt_with_generated_owner_context(spec, owner_bridge_session_id)
        .await
        .expect("spawn the peer-only member");
    handle
        .retire(identity)
        .await
        .expect("retire the peer-only member");
    let records = lifecycle(&handle).await;
    assert_eq!(
        retired(&records, "peer-only-worker", G0).map(|(_, facts)| facts),
        Some(RetiredFacts::plain(None)),
        "a peer-only member's retirement names no session"
    );
    assert_eq!(
        seated(&records, "peer-only-worker", G0).map(|(_, seat)| seat),
        Some(SeatFacts {
            role: ProfileName::from("worker"),
            session_id: None,
            rotation: None,
        }),
        "nor does its seat"
    );
}

/// `rotate_member` of an identity no member holds is a fresh life, and its
/// seat names the incarnation it follows: the generation MobMachine retained
/// for the identity, whose own retirement ended it. Both of the request's
/// paths name it: a seat (a spec that owes no kickoff) and a spawn (a spec
/// that owes one, embodied live). Each fresh life follows the identity's
/// last incarnation, and an identity no incarnation preceded since the
/// mob's last reset names none.
///
/// RED (before the seats record it): every fresh life reads no rotation.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fresh_life_of_an_unseated_identity_names_the_incarnation_it_follows() {
    let mob = JournalMob::new("fresh-life").await;
    for name in ["seated-life", "embodied-life"] {
        mob.spawn_worker(name).await;
        mob.retire(name).await;
    }
    let seated_session = mob.rotate(worker_spec("seated-life")).await;
    let embodied_session = mob.rotate(autonomous_spec("embodied-life")).await;
    mob.rotate(worker_spec("never-seated")).await;
    let records = mob.lifecycle().await;
    assert_eq!(
        seated(&records, "seated-life", G1).map(|(_, seat)| seat),
        Some(SeatFacts {
            role: ProfileName::from("worker"),
            session_id: Some(seated_session),
            rotation: Some(SeatRotation::fresh_life_after(G0)),
        }),
        "the seat names the incarnation it follows"
    );
    assert_eq!(
        seated(&records, "embodied-life", G1).map(|(_, seat)| seat),
        Some(SeatFacts {
            role: ProfileName::from("worker"),
            session_id: Some(embodied_session),
            rotation: Some(SeatRotation::fresh_life_after(G0)),
        }),
        "the spawn of a fresh life that owes a kickoff names it too"
    );
    assert_eq!(
        seated(&records, "never-seated", G0).map(|(_, seat)| seat.rotation),
        Some(None),
        "an identity never seated follows no incarnation"
    );

    mob.retire("seated-life").await;
    mob.rotate(worker_spec("seated-life")).await;
    let records = mob.lifecycle().await;
    assert_eq!(
        seated(&records, "seated-life", G2).and_then(|(_, seat)| seat.rotation),
        Some(SeatRotation::fresh_life_after(G1)),
        "the next fresh life follows the identity's last incarnation"
    );
}

/// A raw retirement of an identity followed, at the very next journal event,
/// by a reset of the now-unseated identity (`rotate_member`) is not one
/// rotation, and the journal's facts say so: the retirement names no
/// rotation (no rotation's terminal step ended it), and the seat names a
/// fresh life after it, not a successor. A rotation of a seated identity
/// sits at the same positions and names its link on both rows. A reader
/// tells the two apart from the facts alone.
///
/// RED (before the rows record the facts): the reset's seat reads no
/// rotation, and the rotation's rows no link: only their positions remain,
/// and they are the same for both.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_retire_then_a_reset_of_the_unseated_identity_is_not_one_rotation() {
    let mob = JournalMob::new("retire-then-reset").await;
    mob.spawn_worker("reset").await;
    mob.retire("reset").await;
    mob.rotate(worker_spec("reset")).await;
    mob.spawn_worker("rotated").await;
    mob.rotate(worker_spec("rotated")).await;
    let journal = mob.journal().await;
    for name in ["reset", "rotated"] {
        assert!(
            is_seat_of(event_after_retirement(&journal, name, G0), name, G1),
            "{name}'s seat is the journal event right after its retirement"
        );
    }
    let records = mob.lifecycle().await;
    let link = |name: &str| {
        (
            retired(&records, name, G0).and_then(|(_, facts)| facts.rotation),
            seated(&records, name, G1).and_then(|(_, seat)| seat.rotation),
        )
    };
    assert_eq!(
        link("reset"),
        (None, Some(SeatRotation::fresh_life_after(G0))),
        "the raw retirement names no rotation, and the reset's seat a fresh life after it"
    );
    assert_eq!(
        link("rotated"),
        (
            Some(RetiredRotation::new(
                RotationTarget::FreshSession,
                G1,
                ProfileName::from("worker"),
            )),
            Some(SeatRotation::successor(G0, RotationTarget::FreshSession)),
        ),
        "the rotation names its link on both rows"
    );
}

/// Separate appends carry no rotation link, wherever they sit in the
/// journal: a retire, then a seat of the same identity
/// (`seat_dormant_members`) at the very next journal event, names no
/// rotation on either row. (A respawn's seat names the retirement it
/// follows from MobMachine's respawn hold, never from its position: see
/// `a_respawn_names_the_incarnation_it_follows`.)
///
/// Guard: holds before and after the rows record the facts; it fails when a
/// writer links rows by their positions instead of the machine step that
/// wrote them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn separate_appends_carry_no_rotation_link() {
    let mob = JournalMob::new("separate-appends").await;
    mob.spawn_worker("reseated").await;
    mob.retire("reseated").await;
    let outcomes = tokio::time::timeout(
        STEP,
        mob.handle
            .seat_dormant_members(vec![worker_spec("reseated")]),
    )
    .await
    .expect("the seat answers")
    .expect("the seat batch runs");
    assert!(
        matches!(&outcomes[0], DormantSeatOutcome::Seated { .. }),
        "{outcomes:?}"
    );
    assert!(
        is_seat_of(
            event_after_retirement(&mob.journal().await, "reseated", G0),
            "reseated",
            G1
        ),
        "the seat is the journal event right after the retirement"
    );

    let records = mob.lifecycle().await;
    assert_eq!(
        retired(&records, "reseated", G0).map(|(_, facts)| facts.rotation),
        Some(None),
        "the retirement names no rotation"
    );
    assert_eq!(
        seated(&records, "reseated", G1).map(|(_, seat)| seat.rotation),
        Some(None),
        "the next seat names no rotation"
    );
}

/// A host's respawn of a member no identity entry holds is two machine
/// steps, a retirement and then a seat, so its retirement names no
/// rotation; its seat names the incarnation the respawn retired
/// (`SeatRotation::RespawnAfter`): MobMachine's respawn hold, which the
/// retirement left for its successor and the seat's step took. A reader
/// tells a respawn from a retire followed by an unrelated seat from the
/// facts alone.
///
/// RED (before the seat records the hold): the respawn's seat names no
/// rotation.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_respawn_names_the_incarnation_it_follows() {
    let mob = JournalMob::new("respawn-after").await;
    mob.spawn_worker("respawned").await;
    tokio::time::timeout(STEP, mob.handle.respawn(id("respawned"), None))
        .await
        .expect("the respawn answers")
        .expect("the worker is respawned");
    let records = mob.lifecycle().await;
    assert_eq!(
        rotation_link_of(&records, "respawned"),
        (None, Some(SeatRotation::respawn_after(G0))),
        "the respawn's seat names the incarnation it follows, and its retirement no rotation"
    );
}

/// A placed member's seat and its retirement name the same session: the
/// one its member host keeps, which the seat's journal effect named (the
/// row's member ref is the host peer's, with no session) and the
/// retirement's journal effect names.
///
/// RED (before the seat row records the effect's session): the placed
/// seat reads no session, though its retirement names one.
#[tokio::test(flavor = "multi_thread")]
async fn a_placed_member_journals_one_session_on_its_seat_and_its_retirement() {
    use super::placement_support as placement;
    let _guard = placement::REAL_COMMS_TEST_LOCK.lock().await;
    let scripted = placement::spawn_scripted_host_peer("journal-placed-host").await;
    let member =
        Arc::new(placement::spawn_peer_comms_endpoint("journal-placed-member", true, None).await);
    scripted.script_member_identity("placed", placement::member_identity_of(&member));
    scripted.bind_member_endpoint("placed", Arc::clone(&member));
    let responder = placement::spawn_scripted_member_turn_responder(member);
    let controlling = placement::create_controlling_mob("journal-placed").await;
    let report = controlling.bind_scripted(&scripted).await;
    controlling
        .spawn_placed("worker", "placed", &report.host_id)
        .await
        .expect("the placed spawn commits");
    tokio::time::timeout(STEP, controlling.handle.retire(id("placed")))
        .await
        .expect("the retire answers")
        .expect("the placed member is retired");
    let records = lifecycle(&controlling.handle).await;
    let (_, retirement) =
        retired(&records, "placed", G0).expect("the placed retirement is journaled");
    let (_, seat) = seated(&records, "placed", G0).expect("the placed seat is journaled");
    assert!(
        retirement.session_id.is_some(),
        "the placed retirement names its member host's session"
    );
    assert_eq!(
        seat.session_id, retirement.session_id,
        "the placed seat names the session its retirement names"
    );
    responder.shutdown();
    scripted.shutdown();
}

/// A mob reset ends every incarnation's standing, and with it the
/// generation MobMachine retains for an identity: a fresh life after a
/// reset follows no pre-reset incarnation, right after the reset as after
/// a restart, which replays only the reset's epoch. Either way the fresh
/// life mints above the identity's last generation (the reset folds it
/// into the identity's high-water).
///
/// RED (before the reset folds the retained generation into the
/// high-water): right after the reset, the fresh life names the pre-reset
/// incarnation (`FreshLifeAfter(G0)`), while after a restart it names none.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fresh_life_after_a_mob_reset_follows_no_pre_reset_incarnation() {
    for (label, restart) in [
        ("reset-fresh-life", false),
        ("reset-restart-fresh-life", true),
    ] {
        let leg = if restart {
            "with a restart between the reset and the fresh life"
        } else {
            "right after the reset"
        };
        let mut mob = JournalMob::new(label).await;
        mob.spawn_worker("reset-member").await;
        mob.retire("reset-member").await;
        tokio::time::timeout(STEP, mob.handle.reset())
            .await
            .expect("the reset answers")
            .expect("the reset succeeds");
        if restart {
            mob.restart().await;
        }
        mob.rotate(worker_spec("reset-member")).await;
        let records = mob.lifecycle().await;
        assert_eq!(
            seated(&records, "reset-member", G1).map(|(_, seat)| seat.rotation),
            Some(None),
            "{leg}: the fresh life mints above the pre-reset generation and follows no \
             pre-reset incarnation"
        );
    }
}

/// A rotation interrupted between its durable start and its terminal step
/// is finished by the restarted mob, whose terminal journals the link on
/// both rows from the state the replay rebuilt: the target the start row's
/// rotation record recovers (`RecoverMemberRotationIntent`), and the
/// incarnation and role the restarted terminal seats.
///
/// Pins the facts a restarted mob writes from replayed machine state.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_interrupted_rotation_finished_after_a_restart_journals_its_link() {
    let mut mob = JournalMob::new("restart-rotation-link").await;
    mob.spawn_worker("rotated").await;
    let hold = crate::runtime::actor::retirement_stage_hold::install(
        &id("rotated"),
        "retire-runtime-quiesce",
    );
    let rotating = {
        let handle = mob.handle.clone();
        tokio::spawn(async move { handle.rotate_member(worker_spec("rotated")).await })
    };
    tokio::time::timeout(STEP, hold.entered())
        .await
        .expect("the rotation's retirement starts");
    mob.restart().await;
    hold.release();
    let _ = tokio::time::timeout(STEP, rotating).await;
    let records = mob
        .lifecycle_until("the restarted mob seats the successor", |records| {
            seated(records, "rotated", G1).is_some()
        })
        .await;
    assert_eq!(
        rotation_link_of(&records, "rotated"),
        (
            Some(RetiredRotation::new(
                RotationTarget::FreshSession,
                G1,
                ProfileName::from("worker"),
            )),
            Some(SeatRotation::successor(G0, RotationTarget::FreshSession)),
        ),
        "the restarted terminal journals the rotation's link on both rows"
    );
}

/// An unlisted retirement interrupted between its durable start and its
/// end is finished by the restarted mob, whose terminal names the cause
/// `Unlisted` (the mark the start row's cause recovers,
/// `RecoverIdentityEntryRetiring`), the entry it ended and the session the
/// incarnation was bound to.
///
/// Pins the facts a restarted mob writes from replayed machine state.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_interrupted_unlisted_retirement_finished_after_a_restart_journals_its_cause() {
    let mut mob = JournalMob::new("restart-unlisted-facts").await;
    mob.apply(DeclaredRoster::complete([
        declared("listed"),
        declared("unlisted"),
    ]))
    .await;
    let session = mob.session("unlisted").await;
    let hold = crate::runtime::actor::retirement_stage_hold::install(
        &id("unlisted"),
        "retire-runtime-quiesce",
    );
    let sealing = {
        let handle = mob.handle.clone();
        tokio::spawn(async move {
            handle
                .apply_declared_roster(DeclaredRoster::complete([declared("listed")]))
                .await
        })
    };
    tokio::time::timeout(STEP, hold.entered())
        .await
        .expect("the unlisted retirement starts");
    mob.restart().await;
    hold.release();
    let _ = tokio::time::timeout(STEP, sealing).await;
    let records = mob
        .lifecycle_until(
            "the restarted mob ends the unlisted retirement",
            |records| retired(records, "unlisted", G0).is_some(),
        )
        .await;
    assert_eq!(
        retired(&records, "unlisted", G0).map(|(_, facts)| facts),
        Some(RetiredFacts {
            identity_entry: Some(IdentityEntryOrigin::Declared),
            cause: Some(MemberRetirementCause::Unlisted),
            ..RetiredFacts::plain(Some(session))
        }),
        "the restarted terminal names the recovered cause, the entry it ended and the session"
    );
    assert!(
        matches!(
            mob.handle.identity_entry(&id("unlisted")),
            Ok(IdentityEntryRead::NotAnEntry { worker: None })
        ),
        "the end of the unlisted retirement ended the entry"
    );
}

/// A complete declared apply that does not list an identity entry whose
/// member's spawn is rolling back unlists nothing: MobMachine's seal leaves
/// a member whose spawn is still in flight to a later seal
/// (`marked_entry_has_no_spawn_in_flight`). The rollback's end names no
/// cause and keeps the entry, unseated, and the next complete apply
/// removes it. The row: a host's resumed spawn of an identity entry's
/// member fails after its membership step (its role wiring fan-out is
/// refused), and the apply runs while the rollback is held after its
/// durable start row, before its Retire step. Two legs: the rollback ends
/// live, or the mob restarts there and the restarted mob ends it.
///
/// RED (before the seal's guard): the seal marks the identity between the
/// rollback's start row and its Retire step.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_seal_during_an_entry_members_spawn_rollback_defers_its_unlisting() {
    seal_during_a_spawn_rollback("deferred", false).await;
    seal_during_a_spawn_rollback("deferred-restart", true).await;
}

/// One leg of the row above for the identity `name`, restarting the mob
/// in the rollback's window when `restart` is set.
async fn seal_during_a_spawn_rollback(name: &str, restart: bool) {
    let definition = with_unique_mob_id(sample_definition_with_role_wiring(), name);
    let mob_id = definition.id.clone();
    let mut mob = JournalMob::over(definition, MobStorage::in_memory()).await;
    mob.spawn_worker("w-peer").await;
    let seeded = mob
        .service
        .create_session(CreateSessionRequest {
            injected_context: Vec::new(),
            model: "claude-sonnet-4-5".to_string(),
            prompt: "seed".to_string().into(),
            system_prompt: meerkat_core::SystemPromptOverride::Inherit,
            max_tokens: Some(4096),
            event_tx: None,
            build: Some(meerkat_core::service::SessionBuildOptions {
                comms_name: Some(test_comms_name_for(&mob_id, "worker", name)),
                ..Default::default()
            }),
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            labels: None,
        })
        .await
        .expect("seed the durable member session");
    let session = seeded.session_id.clone();
    MobSessionService::discard_live_session(mob.service.as_ref(), &session)
        .await
        .expect("keep only the durable snapshot");
    // The wiring fan-out to the peer is refused after the membership step.
    mob.service
        .set_comms_behavior(
            &test_comms_name_for(&mob_id, "worker", "w-peer"),
            MockCommsBehavior {
                fail_add_trust: true,
                ..MockCommsBehavior::default()
            },
        )
        .await;

    let resume_held =
        crate::runtime::actor::retirement_stage_hold::install(&id(name), "spawn-rollback");
    // The spawn's answer waits for its rollback: the actor keeps the failed
    // spawn's reply with the rollback it retained and sends it when the
    // rollback attempt settles. The row holds that rollback, so it reads the
    // answer only once the rollback has ended.
    let spawning = {
        let handle = mob.handle.clone();
        let spec = worker_spec(name)
            .with_entry_custody(true)
            .with_resume_bridge_session_id(session);
        tokio::spawn(async move { handle.spawn_spec(spec).await })
    };
    tokio::time::timeout(STEP, resume_held.entered())
        .await
        .expect("the spawn's rollback starts");
    // The rollback's start row is appended, and its next step held: the
    // window before its Retire step.
    let start_held =
        crate::runtime::actor::retirement_stage_hold::install(&id(name), "spawn-rollback");
    resume_held.release();
    tokio::time::timeout(STEP, start_held.entered())
        .await
        .expect("the rollback reaches its start row");
    let window_held =
        crate::runtime::actor::retirement_stage_hold::install(&id(name), "spawn-rollback");
    start_held.release();
    tokio::time::timeout(STEP, window_held.entered())
        .await
        .expect("the rollback's start row is durable");
    let key = dsl::AgentIdentity::from_domain(&id(name));
    {
        let state = mob.handle.machine_state_watch_rx.borrow();
        assert!(
            state.spawn_exec_phase.contains_key(&key)
                && state.identity_to_runtime.get(&key).is_some_and(|runtime| {
                    state.member_state_markers.get(runtime) != Some(&dsl::MobMemberState::Retiring)
                }),
            "the rollback is held before its Retire step: {name} is seated, not retiring, and \
             its spawn is in flight"
        );
    }

    // A complete apply that does not list the identity leaves it to a later
    // seal.
    let mut state = mob.handle.machine_state_watch_rx.clone();
    let mut applying = {
        let handle = mob.handle.clone();
        tokio::spawn(async move {
            handle
                .apply_declared_roster(DeclaredRoster::complete(Vec::<DeclaredEntry>::new()))
                .await
        })
    };
    let outcome = tokio::time::timeout(STEP, async {
        tokio::select! {
            outcome = &mut applying => outcome
                .expect("the apply task ends")
                .expect("the apply runs"),
            _ = state.wait_for(|state| state.identity_entry_retiring.contains(&key)) => {
                panic!("the seal marked {name}, whose spawn is still in flight")
            }
        }
    })
    .await
    .expect("the apply answers while the rollback is held");
    assert!(
        !outcome.unlisted.contains_key(&id(name)),
        "the apply leaves {name} to a later seal: {outcome:?}"
    );

    if restart {
        mob.restart().await;
    }
    window_held.release();
    let records = mob
        .lifecycle_until("the end of the rolled-back incarnation", |records| {
            retired(records, name, G0).is_some()
        })
        .await;
    assert_eq!(
        retired(&records, name, G0).and_then(|(_, facts)| facts.cause),
        None,
        "the end of {name}'s rollback names no cause"
    );
    // The rollback has ended, so the spawn has its answer: the failure after
    // its membership step, or, when the mob restarted in the window, the
    // crashed mob's.
    let spawned = tokio::time::timeout(STEP, spawning)
        .await
        .expect("the spawn answers once its rollback has ended")
        .expect("the spawn task ends");
    assert!(
        spawned.is_err(),
        "the resumed spawn fails after its membership step: {spawned:?}"
    );
    let mut state = mob.handle.machine_state_watch_rx.clone();
    tokio::time::timeout(
        STEP,
        state.wait_for(|state| !state.identity_to_runtime.contains_key(&key)),
    )
    .await
    .expect("the end of the rollback unseats the member")
    .expect("the state watch outlives the row");
    assert!(
        matches!(
            mob.handle.identity_entry(&id(name)),
            Ok(IdentityEntryRead::Entry(view))
                if view.standing == EntryStanding::Unseated(UnseatedEntryCause::SeatOwed)
        ),
        "the end of the rollback keeps {name}'s entry, unseated"
    );
    let removed = mob
        .apply(DeclaredRoster::complete(Vec::<DeclaredEntry>::new()))
        .await;
    assert!(
        matches!(
            removed.unlisted.get(&id(name)),
            Some(UnlistedEntryOutcome::Removed)
        ),
        "the next complete apply removes {name}'s entry: {removed:?}"
    );
    assert!(
        matches!(
            mob.handle.identity_entry(&id(name)),
            Ok(IdentityEntryRead::NotAnEntry { worker: None })
        ),
        "{name} is no entry"
    );
}

/// A rotation of a seated member that owes a kickoff (an `AutonomousHost`
/// member: MobMachine's `CommitMemberRotation*Kickoff` arms, which record
/// the kickoff the next incarnation owes in the same step) journals its
/// link on both rows, as a rotation that owes none does.
///
/// Pins the kickoff arms' rows.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rotation_that_owes_a_kickoff_journals_its_link() {
    let mob = JournalMob::new("kickoff-rotation-link").await;
    tokio::time::timeout(STEP, mob.handle.spawn_spec(autonomous_spec("kicked")))
        .await
        .expect("the spawn answers")
        .expect("the autonomous member is spawned");
    mob.rotate(autonomous_spec("kicked")).await;
    let records = mob.lifecycle().await;
    assert_eq!(
        rotation_link_of(&records, "kicked"),
        (
            Some(RetiredRotation::new(
                RotationTarget::FreshSession,
                G1,
                ProfileName::from("worker"),
            )),
            Some(SeatRotation::successor(G0, RotationTarget::FreshSession)),
        ),
        "the rotation that owes a kickoff journals its link on both rows"
    );
}

/// A rotation of an identity entry's member under another role (the
/// rotation a declared apply's `RoleRotate` makes: `rotate_member` with the
/// entry's custody) journals the entry on both rows, and the role the same
/// step seated, which is not the retired incarnation's, on the
/// retirement's link and on the seat.
///
/// Pins the entry and the role of a rotation's rows.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rotation_of_an_entrys_member_journals_the_entry_and_its_new_role() {
    let mut definition = with_unique_mob_id(sample_definition(), "entry-role-rotation");
    let worker = definition
        .profiles
        .get(&ProfileName::from("worker"))
        .cloned()
        .expect("the worker profile");
    definition
        .profiles
        .insert(ProfileName::from("reviewer"), worker);
    let mob = JournalMob::over(definition, MobStorage::in_memory()).await;
    tokio::time::timeout(
        STEP,
        mob.handle
            .spawn_spec(worker_spec("migrated").with_entry_custody(true)),
    )
    .await
    .expect("the spawn answers")
    .expect("the host declares a spawned entry");
    mob.rotate(
        SpawnMemberSpec::new("reviewer", id("migrated"))
            .with_runtime_mode(crate::MobRuntimeMode::TurnDriven)
            .with_entry_custody(true),
    )
    .await;
    let records = mob.lifecycle().await;
    let (_, retirement) =
        retired(&records, "migrated", G0).expect("the rotation's retirement is journaled");
    assert_eq!(
        (
            retirement.role,
            retirement.identity_entry,
            retirement.rotation
        ),
        (
            ProfileName::from("worker"),
            Some(IdentityEntryOrigin::Declared),
            Some(RetiredRotation::new(
                RotationTarget::FreshSession,
                G1,
                ProfileName::from("reviewer"),
            )),
        ),
        "the retirement names the entry and the successor's role"
    );
    let (_, seat) = seated(&records, "migrated", G1).expect("the successor is journaled");
    assert_eq!(
        (seat.role, seat.rotation),
        (
            ProfileName::from("reviewer"),
            Some(SeatRotation::successor(G0, RotationTarget::FreshSession)),
        ),
        "the seat names its new role and the incarnation it succeeds"
    );
    assert_eq!(
        seat_entry(&records, "migrated", G1),
        Some(IdentityEntryOrigin::Declared),
        "the seat keeps the entry"
    );
}

/// Every kind of fact the journal read records, written on `mob`: a
/// rotation onto a fresh session and a same-session successor under another
/// role (the link on both rows), a plain retirement and the fresh life that
/// follows it, and an unlisted retirement (its cause and its entry). Answers
/// the facts read back, each checked.
async fn write_every_fact(mob: &JournalMob) -> Vec<MemberLifecycleRecord> {
    mob.spawn_worker("rotated").await;
    mob.rotate(worker_spec("rotated")).await;
    mob.spawn_worker("respawned").await;
    mob.respawn("respawned", MemberRespawn::new().with_profile("lead"))
        .await;
    mob.spawn_worker("revived").await;
    mob.retire("revived").await;
    mob.rotate(worker_spec("revived")).await;
    mob.apply(DeclaredRoster::complete([declared("unlisted")]))
        .await;
    let sealed = mob
        .apply(DeclaredRoster::complete(Vec::<DeclaredEntry>::new()))
        .await;
    assert!(
        matches!(
            sealed.unlisted.get(&id("unlisted")),
            Some(UnlistedEntryOutcome::Retired)
        ),
        "{sealed:?}"
    );

    let records = mob.lifecycle().await;
    let link = |name: &str| {
        (
            retired(&records, name, G0).and_then(|(_, facts)| facts.rotation),
            seated(&records, name, G1).and_then(|(_, seat)| seat.rotation),
        )
    };
    assert_eq!(
        link("rotated"),
        (
            Some(RetiredRotation::new(
                RotationTarget::FreshSession,
                G1,
                ProfileName::from("worker"),
            )),
            Some(SeatRotation::successor(G0, RotationTarget::FreshSession)),
        )
    );
    assert_eq!(
        link("respawned"),
        (
            Some(RetiredRotation::new(
                RotationTarget::SameSession,
                G1,
                ProfileName::from("lead"),
            )),
            Some(SeatRotation::successor(G0, RotationTarget::SameSession)),
        )
    );
    assert_eq!(
        link("revived"),
        (None, Some(SeatRotation::fresh_life_after(G0)))
    );
    let revived = retired(&records, "revived", G0)
        .map(|(_, facts)| facts)
        .expect("the plain retirement is journaled");
    assert!(revived.session_id.is_some(), "{revived:?}");
    let unlisted = retired(&records, "unlisted", G0)
        .map(|(_, facts)| facts)
        .expect("the unlisted retirement is journaled");
    assert_eq!(
        (unlisted.cause, unlisted.identity_entry),
        (
            Some(MemberRetirementCause::Unlisted),
            Some(IdentityEntryOrigin::Declared)
        )
    );
    records
}

/// The journal read of `mob` answers `before` again: every fact written
/// before its restart, unchanged.
async fn assert_read_the_same(mob: &JournalMob, before: &[MemberLifecycleRecord]) {
    let last = before
        .last()
        .map(|record| record.cursor)
        .expect("facts were written");
    let after = mob
        .lifecycle()
        .await
        .into_iter()
        .filter(|record| record.cursor <= last)
        .collect::<Vec<_>>();
    assert_eq!(
        after, before,
        "the journal read answers the same facts after a restart"
    );
}

/// The journal read's facts read the same after a restart of a mob over
/// in-memory storage.
///
/// RED (before the rows record the facts): the facts read before the
/// restart are absent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lifecycle_facts_read_the_same_after_a_restart_in_memory() {
    let mut mob = JournalMob::new("facts-restart-memory").await;
    let before = write_every_fact(&mob).await;
    let storage = mob.storage.clone();
    mob.restart_into(storage).await;
    assert_read_the_same(&mob, &before).await;
}

/// The journal read's facts read the same after a restart of a mob over a
/// SQLite store, which keeps them through the stored event codec.
///
/// RED (before the rows record the facts): the facts read before the
/// restart are absent; with the rows written but the codec not keeping
/// them, the restart loses them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lifecycle_facts_read_the_same_after_a_restart_on_sqlite() {
    let directory = tempfile::tempdir().expect("a store directory");
    let path = directory.path().join("mob.sqlite");
    let mut mob = JournalMob::over(
        with_unique_mob_id(sample_definition(), "facts-restart-sqlite"),
        MobStorage::persistent(&path).expect("open the store"),
    )
    .await;
    let before = write_every_fact(&mob).await;
    mob.crash().await;
    mob.resume(MobStorage::persistent(&path).expect("reopen the store"))
        .await;
    assert_read_the_same(&mob, &before).await;
}

/// A host store realm over `host`, as one process opens it: it takes over
/// from a predecessor process that may still renew its lease.
async fn open_realm(host: &Arc<InMemoryHostStore>) -> Arc<SpiRealm> {
    SpiRealm::open(
        Arc::clone(host) as Arc<dyn HostStore>,
        SpiRealmConfig {
            acquire: AcquireMode::Takeover,
            ..SpiRealmConfig::default()
        },
    )
    .await
    .expect("open the realm")
}

/// The journal read's facts read the same after a restart of a mob over a
/// host store, whose mob storage keeps them through the stored event codec:
/// the restarted process opens a new realm over the same host.
///
/// RED (before the rows record the facts): the facts read before the
/// restart are absent; with the rows written but the codec not keeping
/// them, the restart loses them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lifecycle_facts_read_the_same_after_a_restart_on_a_host_store() {
    let host = Arc::new(InMemoryHostStore::new());
    let definition = with_unique_mob_id(sample_definition(), "facts-restart-host");
    let storage_key = definition.id.to_string();
    let realm = open_realm(&host).await;
    let mobs = SpiMobStoreHost::for_realm(&realm)
        .await
        .expect("claim the mob families");
    let storage = mobs
        .open(&storage_key)
        .await
        .expect("open the mob's storage")
        .storage();
    let mut mob = JournalMob::over(definition, storage).await;
    let before = write_every_fact(&mob).await;
    mob.crash().await;
    drop((mobs, realm));

    host.lapse_fence_leases();
    let realm = open_realm(&host).await;
    let mobs = SpiMobStoreHost::for_realm(&realm)
        .await
        .expect("claim the mob families again");
    let reopened = mobs
        .open(&storage_key)
        .await
        .expect("reopen the mob's storage")
        .storage();
    mob.resume(reopened).await;
    assert_read_the_same(&mob, &before).await;
    mob.crash().await;
    drop((mobs, realm));
}

/// The constructions of a `MemberRetired` row in `code` (shipped code, its
/// comments and literals blanked): each one's byte index and the text
/// between its braces. A pattern is none: its braces end in `..`.
fn member_retired_constructions(code: &str) -> Vec<(usize, &str)> {
    const NAME: &str = "MemberRetired";
    code.match_indices(NAME)
        .filter(|(at, _)| super::fail_stop_cause::whole_identifier(code, *at, NAME))
        .filter(|(at, _)| code[..*at].trim_end().ends_with("::"))
        .filter_map(|(at, _)| {
            let after = &code[at + NAME.len()..];
            let open = at + NAME.len() + (after.len() - after.trim_start().len());
            if code.as_bytes().get(open) != Some(&b'{') {
                return None;
            }
            let close = brace_group_close(code, open);
            let inner = &code[open + 1..close];
            (!inner.trim_end().ends_with("..")).then_some((at, inner))
        })
        .collect()
}

/// The index of the brace that closes the group opening at `open`.
fn brace_group_close(code: &str, open: usize) -> usize {
    let mut depth = 0usize;
    for (offset, byte) in code.as_bytes()[open..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return open + offset;
                }
            }
            _ => {}
        }
    }
    code.len()
}

/// The name of the function whose body holds byte `at` of `code`: the last
/// `fn` before it.
fn enclosing_fn(code: &str, at: usize) -> &str {
    let before = &code[..at];
    let start = before
        .rmatch_indices("fn ")
        .find(|(index, _)| super::fail_stop_cause::whole_identifier(before, *index, "fn"))
        .map(|(index, keyword)| index + keyword.len())
        .expect("a function holds the construction");
    let name = &code[start..];
    let end = name
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(name.len());
    &name[..end]
}

/// Every writer of a `MemberRetired` row is accounted for. The terminal
/// steps (a plain retirement's and the destroy's, through one writer, and a
/// rotation's) record what MobMachine decided; the two rows that close a
/// failed spawn (its membership commit refused after its spawn row was
/// durable, and a resumed spawn's rollback) record none, and read
/// unannotated, as rows written before the facts do. A new writer fails
/// here until it decides what its row records.
///
/// RED (before the writers record the facts): the terminal writers write
/// the default facts too.
#[test]
fn every_member_retired_writer_records_its_facts_but_the_spawn_compensations() {
    let mut recorded = BTreeSet::new();
    let mut unannotated = BTreeSet::new();
    for (path, code) in super::fail_stop_cause::shipped_sources() {
        for (at, inner) in member_retired_constructions(&code) {
            let writer = format!("{path}::{}", enclosing_fn(&code, at));
            if inner.contains("MemberRetiredFacts::default()") {
                unannotated.insert(writer);
            } else {
                recorded.insert(writer);
            }
        }
    }
    assert_eq!(
        recorded,
        BTreeSet::from([
            "runtime/actor.rs::append_retire_event_for_entry".to_string(),
            "runtime/actor/dormant_seat.rs::commit_member_rotation".to_string(),
        ]),
        "the terminal writers record the facts"
    );
    assert_eq!(
        unannotated,
        BTreeSet::from([
            "runtime/actor.rs::finalize_spawn_admit".to_string(),
            "runtime/actor/retirement_io.rs::prepare_spawn_rollback_step".to_string(),
        ]),
        "the spawn compensations record none"
    );
}

/// The journal read maps exactly the six member lifecycle row kinds
/// (`member_lifecycle_event`), read with comments and literals blanked, as
/// the writer guard above reads its writers: a kind named only in a comment
/// or a string does not count. A row kind dropped from the mapping, or one
/// added to it without a decision on what it records, fails here.
#[test]
fn the_lifecycle_mapping_names_the_six_row_kinds() {
    let code = super::fail_stop_cause::shipped_sources()
        .into_iter()
        .find_map(|(path, code)| (path == "runtime/identity_entry.rs").then_some(code))
        .expect("the journal read's source");
    let at = code
        .find("fn member_lifecycle_event(")
        .expect("the mapping");
    let open = at + code[at..].find('{').expect("the mapping's body");
    let body = &code[open..brace_group_close(&code, open)];
    let mut kinds = body
        .match_indices("MobEventKind::")
        .map(|(at, prefix)| {
            let rest = &body[at + prefix.len()..];
            let end = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(rest.len());
            &rest[..end]
        })
        .collect::<Vec<_>>();
    kinds.sort_unstable();
    kinds.dedup();
    assert_eq!(
        kinds,
        [
            "MemberReset",
            "MemberRetired",
            "MemberRetirementStarted",
            "MemberSessionBindingRecovered",
            "MemberSpawned",
            "MobReset",
        ],
        "the journal read's mapping names exactly the six row kinds"
    );
}

/// Rollback of the journal read's facts, lane-run leg: needs the published
/// Core 0.9.7 (`meerkat-mob` 0.9.7).
///
/// This build writes the fixture: a persistent mob store (SQLite) at
/// `$MEERKAT_ROLLBACK_FIXTURE_DIR/lifecycle-facts/mob.sqlite` holding a
/// member retired by a plain retire, whose `MemberRetired` row records its
/// session (`retired_session`), and the fresh life `rotate_member` seated
/// for it afterwards, whose `MemberSpawned` row records the incarnation it
/// follows (`seat_rotation`); and a member rotated onto a fresh session,
/// whose terminal batch records the link on both rows (`retired_rotation`,
/// `seat_rotation`). The lane resumes it with Core 0.9.7, whose event
/// decoder ignores the keys it does not know, and checks: the resume
/// succeeds, both members are listed `active` at generation 1, and a
/// delivery wakes each.
///
/// RED: a fact the 0.9.7 decoder cannot ignore (a stored key on a public
/// field, or a new event kind) fails the 0.9.7 resume of this fixture.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "lane:rollback - writes the fixture the published Core 0.9.7 resumes"]
async fn lifecycle_fact_rows_are_read_by_097() {
    let dir = std::path::PathBuf::from(
        std::env::var_os("MEERKAT_ROLLBACK_FIXTURE_DIR")
            .expect("MEERKAT_ROLLBACK_FIXTURE_DIR names the fixture directory"),
    )
    .join("lifecycle-facts");
    std::fs::create_dir_all(&dir).expect("create the fixture directory");
    super::identity_entry_rows::write_rollback_fixture(
        &dir,
        "rollback-lifecycle-facts",
        |handle| async move {
            handle
                .spawn_spec(worker_spec("revived"))
                .await
                .expect("spawn the member");
            handle
                .retire(id("revived"))
                .await
                .expect("retire the member");
            handle
                .rotate_member(worker_spec("revived"))
                .await
                .expect("seat its fresh life");
            handle
                .spawn_spec(worker_spec("rotated"))
                .await
                .expect("spawn the member");
            handle
                .rotate_member(worker_spec("rotated"))
                .await
                .expect("rotate the member");
        },
    )
    .await;
}
