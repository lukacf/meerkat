//! Rotation targets: the rotation a retirement's start row records (so a
//! restarted mob finishes an interrupted rotation itself), the same-session
//! successor (`respawn_member`, and `respawn` of an identity entry's member)
//! that seats the next incarnation on the member's own session in one machine
//! step, the declared role migration it carries, and the scoped release
//! MobMachine refuses for a stale scope. They run on the combined candidate:
//! the machine, the actor, the journal and the replay together. The queued
//! input handoff of a same-session successor is a row of the stop fixture
//! (`host_human_input`), which has a runtime whose sessions commit each
//! round.

use super::*;
use crate::machines::mob_machine as dsl;
use crate::runtime::dormant_seat::fresh_seat_session_id;
use crate::runtime::{IdentityEntryOrigin, IdentityEntryRead, MemberRespawn, ReleaseOutcome};

const STEP: Duration = Duration::from_mins(1);

fn id(name: &str) -> AgentIdentity {
    AgentIdentity::from(name)
}

/// A turn-driven worker spec: it owes no kickoff.
fn worker_spec(name: &str) -> SpawnMemberSpec {
    let mut spec = SpawnMemberSpec::new("worker", id(name));
    spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    spec
}

/// `worker_spec` acting for the identity entry of its name.
fn entry_spec(name: &str) -> SpawnMemberSpec {
    worker_spec(name).with_entry_custody(true)
}

/// A running mob over the mock durable session service and in-memory
/// storage, with session-scoped member comms identities. A restart resumes
/// the same storage.
struct RotationMob {
    keys: tempfile::TempDir,
    storage: MobStorage,
    handle: MobHandle,
    service: Arc<MockSessionService>,
}

impl RotationMob {
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

    /// Crash the mob, restart its session service and resume its storage,
    /// with no host record of anything in flight.
    async fn crash_and_restart(&mut self) {
        let restarted = Arc::new(self.service.cold_restart_preserving_durable_state().await);
        let _ = restarted.enable_runtime_adapter();
        crash_stop_and_release_routes(self.handle.clone()).await;
        self.service = restarted;
        self.handle = tokio::time::timeout(
            STEP,
            Box::pin(
                MobBuilder::for_resume(self.storage.clone())
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

    async fn session(&self, name: &str) -> SessionId {
        self.handle
            .resolve_bridge_session_id(&id(name))
            .await
            .unwrap_or_else(|| panic!("{name} is bound to a session"))
    }

    async fn generation(&self, name: &str) -> Option<Generation> {
        self.handle
            .get_member(&id(name))
            .await
            .expect("roster read")
            .map(|entry| entry.generation)
    }

    /// Wait, within one step, until `name` is seated at `generation`.
    async fn seated_at(&self, name: &str, generation: Generation) {
        let deadline = tokio::time::Instant::now() + STEP;
        while self.generation(name).await != Some(generation) {
            assert!(
                tokio::time::Instant::now() < deadline,
                "{name} never reached generation {generation}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn journal(&self) -> Vec<MobEvent> {
        self.storage
            .events
            .poll(0, usize::MAX)
            .await
            .expect("read the journal")
    }
}

/// The rotation `name`'s retirement start row records for `generation`.
fn recorded_rotation(
    journal: &[MobEvent],
    name: &str,
    generation: Generation,
) -> Option<crate::runtime::MemberRotationRecord> {
    journal.iter().find_map(|event| match &event.kind {
        MobEventKind::MemberRetirementStarted {
            agent_identity,
            generation: started,
            ..
        } if agent_identity.as_str() == name && *started == generation => {
            event.kind.retirement_rotation().cloned()
        }
        _ => None,
    })
}

/// The incarnation the row right after `name`'s `MemberRetired` row of
/// `generation` seats, with the session it names: the rotation's terminal
/// batch holds both rows, in that order.
fn successor_in_the_terminal_batch(
    journal: &[MobEvent],
    name: &str,
    generation: Generation,
) -> Option<(Generation, ProfileName, Option<SessionId>)> {
    let retired = journal.iter().position(|event| {
        matches!(
            &event.kind,
            MobEventKind::MemberRetired {
                agent_identity,
                generation: retired,
                ..
            } if agent_identity.as_str() == name && *retired == generation
        )
    })?;
    match &journal.get(retired + 1)?.kind {
        MobEventKind::MemberSpawned(spawned) if spawned.agent_identity.as_str() == name => Some((
            spawned.generation,
            spawned.role.clone(),
            spawned
                .bridge_member_ref
                .as_ref()
                .and_then(MemberRef::bridge_session_id)
                .cloned(),
        )),
        _ => None,
    }
}

/// How many `MemberRetired` rows the journal holds for `name`.
fn retirements_of(journal: &[MobEvent], name: &str) -> usize {
    journal
        .iter()
        .filter(|event| {
            matches!(
                &event.kind,
                MobEventKind::MemberRetired { agent_identity, .. } if agent_identity.as_str() == name
            )
        })
        .count()
}

/// A crash between a rotation's durable start and its terminal step: the
/// start row records the rotation (its target, next session and what its
/// next incarnation is seated with), so the restarted mob records the intent
/// again (`RecoverMemberRotationIntent`) and its own re-drive seats the next
/// incarnation on the name-based session the rotation targeted. No host
/// record and no re-issued call: the identity is rotated, never retired, and
/// the journal holds the retirement only in the batch that seats its
/// successor.
///
/// RED: drop the replay's `RecoverMemberRotationIntent` (builder,
/// `recover_pending_member_retirements`) or the start row's rotation record
/// (`append_retirement_started_event_for_entry`): the restart replays the
/// start as a plain retirement and its re-drive retires the identity, which
/// is then no member at all and never reaches generation 1.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crash_between_rotation_start_and_terminal_finishes_the_rotation_with_no_host_record() {
    let mut mob = RotationMob::new("rotation-recorded").await;
    mob.handle
        .spawn_spec(worker_spec("rotated"))
        .await
        .expect("spawn the member");
    let target = fresh_seat_session_id(
        mob.handle.mob_id(),
        mob.handle.incarnation(),
        &id("rotated"),
        Generation::new(1),
    );
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
    let started = recorded_rotation(&mob.journal().await, "rotated", Generation::new(0))
        .expect("the start row records the rotation");
    assert_eq!(
        (
            started.target(),
            started.session_id(),
            started.next_generation()
        ),
        (
            crate::runtime::RotationTarget::FreshSession,
            &target,
            Generation::new(1)
        )
    );

    mob.crash_and_restart().await;
    hold.release();
    let _ = tokio::time::timeout(STEP, rotating).await;

    mob.seated_at("rotated", Generation::new(1)).await;
    assert_eq!(mob.session("rotated").await, target);
    let journal = mob.journal().await;
    assert_eq!(retirements_of(&journal, "rotated"), 1, "rotated once");
    assert_eq!(
        successor_in_the_terminal_batch(&journal, "rotated", Generation::new(0)),
        Some((
            Generation::new(1),
            ProfileName::from("worker"),
            Some(target)
        )),
        "the retirement is journaled only with its successor"
    );
}

/// `respawn` of an identity entry's member is a same-session successor: one
/// machine step whose `MemberRetired` and `MemberSpawned` rows are one batch
/// naming the member's own session, so the identity is never unseated. While
/// the respawn's retirement is held at the session release, the member is
/// still seated, Retiring, on its kept binding; the successor keeps the
/// entry. A crash while a second entry's respawn is held there is finished
/// by the restarted mob on the same session, from the start row's record.
///
/// RED: route `respawn` of an entry back through the retire-then-spawn
/// helper (drop the entry dispatch in `handle_respawn`): the respawn retires
/// the member and spawns its successor on a new session, the retirement row
/// stands alone, and the hold at the session release is never reached.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn respawn_is_one_machine_step_and_never_leaves_the_identity_unseated() {
    let mut mob = RotationMob::new("same-session-respawn").await;
    for name in ["respawned-entry", "crashed-entry"] {
        mob.handle
            .spawn_spec(entry_spec(name))
            .await
            .expect("the host declares a spawned entry");
    }
    let session = mob.session("respawned-entry").await;
    let dsl_identity = dsl::AgentIdentity::from_domain(&id("respawned-entry"));

    let hold = crate::runtime::actor::retirement_stage_hold::install(
        &id("respawned-entry"),
        "retire-session-release",
    );
    let respawning = {
        let handle = mob.handle.clone();
        tokio::spawn(async move { handle.respawn(id("respawned-entry"), None).await })
    };
    tokio::time::timeout(STEP, hold.entered())
        .await
        .expect("the respawn's retirement reaches the session release");
    {
        let state = mob.handle.machine_state_watch_rx.borrow();
        assert!(
            state.identity_to_runtime.contains_key(&dsl_identity),
            "the identity stays seated while its successor is pending"
        );
        assert_eq!(
            state.member_session_bindings.get(&dsl_identity),
            Some(&dsl::SessionId::from_domain(&session)),
            "the binding is kept"
        );
        assert_eq!(
            state.member_rotation_targets.get(&dsl_identity),
            Some(&dsl::RotationTarget::SameSession)
        );
    }
    hold.release();
    let receipt = tokio::time::timeout(STEP, respawning)
        .await
        .expect("the respawn answers")
        .expect("the respawn task")
        .expect("the host respawns its entry");
    assert_eq!(receipt.identity, id("respawned-entry"));
    assert_eq!(
        mob.generation("respawned-entry").await,
        Some(Generation::new(1))
    );
    assert_eq!(mob.session("respawned-entry").await, session);
    assert!(
        matches!(
            mob.handle.identity_entry(&id("respawned-entry")),
            Ok(IdentityEntryRead::Entry(view)) if view.origin == IdentityEntryOrigin::Declared
        ),
        "the successor keeps the entry"
    );
    let journal = mob.journal().await;
    assert_eq!(
        successor_in_the_terminal_batch(&journal, "respawned-entry", Generation::new(0)),
        Some((
            Generation::new(1),
            ProfileName::from("worker"),
            Some(session.clone())
        )),
        "one batch seats the successor on the same session"
    );
    assert_eq!(
        recorded_rotation(&journal, "respawned-entry", Generation::new(0))
            .map(|record| record.target()),
        Some(crate::runtime::RotationTarget::SameSession)
    );

    // A crash with the second entry's respawn held at the session release.
    let crashed_session = mob.session("crashed-entry").await;
    let hold = crate::runtime::actor::retirement_stage_hold::install(
        &id("crashed-entry"),
        "retire-session-release",
    );
    let crashed = {
        let handle = mob.handle.clone();
        tokio::spawn(async move { handle.respawn(id("crashed-entry"), None).await })
    };
    tokio::time::timeout(STEP, hold.entered())
        .await
        .expect("the second respawn reaches the session release");
    mob.crash_and_restart().await;
    hold.release();
    let _ = tokio::time::timeout(STEP, crashed).await;
    mob.seated_at("crashed-entry", Generation::new(1)).await;
    assert_eq!(mob.session("crashed-entry").await, crashed_session);
    let journal = mob.journal().await;
    assert_eq!(retirements_of(&journal, "crashed-entry"), 1);
    assert_eq!(
        successor_in_the_terminal_batch(&journal, "crashed-entry", Generation::new(0)),
        Some((
            Generation::new(1),
            ProfileName::from("worker"),
            Some(crashed_session)
        ))
    );
}

/// A release scoped to a member incarnation that is no longer current is
/// refused by Core: after a same-session successor replaced the incarnation
/// a scope was captured at (same session, newer runtime id and fence),
/// `release_member_at_scope` answers `Stale` and the live successor stays
/// live; the successor's own scope releases it.
///
/// RED: drop the `scope_current` guard of MobMachine's
/// `ClassifyMemberRelease` (and its Stale twin): the stale scope is
/// classified a release and closes the successor's registration, so the
/// successor is Dormant after the stale call.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scoped_release_of_a_stale_scope_is_refused_by_core() {
    let mob = RotationMob::new("scoped-release").await;
    mob.handle
        .spawn_spec(worker_spec("scoped"))
        .await
        .expect("spawn the member");
    let stale = mob
        .handle
        .capture_member_delivery_scope(&id("scoped"))
        .await
        .expect("capture the member's scope");
    tokio::time::timeout(
        STEP,
        mob.handle
            .respawn_member(id("scoped"), MemberRespawn::new().with_wake(true)),
    )
    .await
    .expect("the respawn answers")
    .expect("the member is respawned and woken");
    assert_eq!(
        mob.handle.member_materialization_for_test(&id("scoped")),
        Some(dsl::MemberMaterializationPhase::Live)
    );
    assert_eq!(stale.session_id(), &mob.session("scoped").await);

    assert_eq!(
        mob.handle
            .release_member_at_scope(&stale)
            .await
            .expect("the stale release answers"),
        ReleaseOutcome::Stale
    );
    assert_eq!(
        mob.handle.member_materialization_for_test(&id("scoped")),
        Some(dsl::MemberMaterializationPhase::Live),
        "a stale scope releases nothing"
    );

    let current = mob
        .handle
        .capture_member_delivery_scope(&id("scoped"))
        .await
        .expect("capture the successor's scope");
    assert_eq!(
        mob.handle
            .release_member_at_scope(&current)
            .await
            .expect("the current release answers"),
        ReleaseOutcome::Released
    );
    let deadline = tokio::time::Instant::now() + STEP;
    while mob.handle.member_materialization_for_test(&id("scoped"))
        != Some(dsl::MemberMaterializationPhase::Dormant)
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the current scope's release never reached Dormant"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// A declared role migration is one same-session step: `respawn_member`
/// naming another profile seats the next incarnation under that profile on
/// the member's own session, its retirement and its successor journaled in
/// one batch, and the successor wakes under the new profile.
///
/// RED: drop `MemberRespawn::profile` from the plan
/// (`plan_member_respawn` seats the member's own role): the successor keeps
/// the previous profile.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn declared_migration_restamps_on_the_same_session_in_one_step() {
    let mob = RotationMob::new("declared-migration").await;
    mob.handle
        .spawn_spec(worker_spec("migrating"))
        .await
        .expect("spawn the member");
    let session = mob.session("migrating").await;
    tokio::time::timeout(
        STEP,
        mob.handle.respawn_member(
            id("migrating"),
            MemberRespawn::new().with_profile("lead").with_wake(true),
        ),
    )
    .await
    .expect("the migration answers")
    .expect("the member migrates");
    let entry = mob
        .handle
        .get_member(&id("migrating"))
        .await
        .expect("roster read")
        .expect("the member is seated");
    assert_eq!(
        (entry.role.clone(), entry.generation, entry.runtime_mode),
        (
            ProfileName::from("lead"),
            Generation::new(1),
            crate::MobRuntimeMode::TurnDriven
        ),
        "the successor takes the declared role and keeps the member's runtime mode"
    );
    assert_eq!(mob.session("migrating").await, session);
    assert_eq!(
        mob.handle.member_materialization_for_test(&id("migrating")),
        Some(dsl::MemberMaterializationPhase::Live),
        "the successor wakes under the new role"
    );
    assert_eq!(
        successor_in_the_terminal_batch(&mob.journal().await, "migrating", Generation::new(0)),
        Some((Generation::new(1), ProfileName::from("lead"), Some(session))),
        "the retirement and the restamped successor are one batch on the same session"
    );
}

/// Rollback of an interrupted rotation, lane-run leg: needs the published
/// Core 0.9.7 (`meerkat-mob` 0.9.7). The Meta 0.9.6 leg needs the rotation
/// mirrored into Meta 0.9.6's own rotation record, which a later change adds;
/// until then that leg is not run.
///
/// This build writes the fixture: a persistent mob store (SQLite) at
/// `$MEERKAT_ROLLBACK_FIXTURE_DIR/interrupted-rotation/mob.sqlite` holding a
/// member whose rotation was interrupted after its retirement's durable
/// start: the start row carries the rotation record, and no terminal batch
/// follows. The lane resumes it with Core 0.9.7, which ignores the record
/// (its decoder drops the unknown key) and so ends the retirement as a plain
/// one; it checks that the resume succeeds and that a host re-issue of
/// `rotate_member` with the identity host-pending (Meta 0.9.6's record
/// re-drive) seats the next incarnation on the name-based session the
/// interrupted rotation targeted.
///
/// RED: a record the 0.9.7 decoder cannot ignore (a stored key on a public
/// field, or a new event kind) fails the 0.9.7 resume of this fixture.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "lane:rollback - writes the fixture the published Core 0.9.7 resumes"]
async fn interrupted_rotation_under_097_is_completed_by_096() {
    let dir = rollback_fixture_dir("interrupted-rotation");
    let (handle, _service) = open_rotation_fixture(&dir, "rollback-interrupted-rotation").await;
    handle
        .spawn_spec(worker_spec("interrupted"))
        .await
        .expect("spawn the member");
    let hold = crate::runtime::actor::retirement_stage_hold::install(
        &id("interrupted"),
        "retire-runtime-quiesce",
    );
    let rotating = {
        let handle = handle.clone();
        tokio::spawn(async move { handle.rotate_member(worker_spec("interrupted")).await })
    };
    tokio::time::timeout(STEP, hold.entered())
        .await
        .expect("the rotation's retirement starts");
    crash_stop_and_release_routes(handle).await;
    hold.release();
    let _ = tokio::time::timeout(STEP, rotating).await;
}

/// Rollback of same-session successor rows, lane-run leg: needs the
/// published Core 0.9.7 (`meerkat-mob` 0.9.7).
///
/// This build writes the fixture: a persistent mob store (SQLite) at
/// `$MEERKAT_ROLLBACK_FIXTURE_DIR/same-session-successor/mob.sqlite` holding
/// a member respawned on its own session (`respawn_member`): its preserving
/// retirement start row with the rotation record, and its `MemberRetired`
/// and `MemberSpawned` rows naming the same session. The lane resumes it with
/// Core 0.9.7 and checks: the member is listed `active` at generation 1 on
/// the same session, a delivery wakes it, and a reset (`rotate_member`)
/// re-seats it.
///
/// RED: a successor row 0.9.7 cannot replay (a retirement and a spawn of the
/// same session that 0.9.7 reads as a collision, or a stored key it
/// refuses) fails the resume or leaves the member unlisted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "lane:rollback - writes the fixture the published Core 0.9.7 resumes"]
async fn same_session_successor_rows_replay_under_097() {
    let dir = rollback_fixture_dir("same-session-successor");
    let (handle, _service) = open_rotation_fixture(&dir, "rollback-same-session-successor").await;
    handle
        .spawn_spec(worker_spec("respawned"))
        .await
        .expect("spawn the member");
    tokio::time::timeout(
        STEP,
        handle.respawn_member(id("respawned"), MemberRespawn::new()),
    )
    .await
    .expect("the respawn answers")
    .expect("the member is respawned on its own session");
    handle.shutdown().await.expect("shut the fixture mob down");
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

/// Create the fixture mob `mob_id` on a persistent store in `dir`: what the
/// published binaries resume.
async fn open_rotation_fixture(
    dir: &std::path::Path,
    mob_id: &str,
) -> (MobHandle, Arc<MockSessionService>) {
    let service = Arc::new(MockSessionService::new());
    let _adapter = service.enable_runtime_adapter();
    let mut definition = sample_definition();
    definition.id = MobId::from(mob_id);
    let handle = MobBuilder::new(
        definition,
        MobStorage::persistent(dir.join("mob.sqlite")).expect("open the fixture store"),
    )
    .with_session_service(service.clone())
    .with_member_comms_identity_root(dir.join("comms-keys"))
    .create()
    .await
    .expect("create the fixture mob");
    (handle, service)
}
