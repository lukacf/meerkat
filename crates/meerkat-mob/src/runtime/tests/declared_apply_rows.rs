//! The declared apply ([`MobHandle::apply_declared_roster`]): a host's
//! declarative roster of identity entries, decided by MobMachine
//! (`DeclareIdentityEntry`, `SealDeclaredRoster`) and realized with the mob's
//! own steps. The rows cover the seal that unlists, the typed answer per
//! identity, the role hold, the runtime-mode restamp, held creation and
//! placed identities, first-boot requests, the revival of an unlisted
//! identity on its archived session (and the two causes it can fail for),
//! the candidate composition, the unchanged roster that writes nothing, and
//! the rollback rows. They run on the combined candidate: the machine, the
//! actor, the journal and the replay together.

use super::identity_entry_rows::{predating_reader_journal, write_rollback_fixture};
use super::*;
use crate::machines::mob_machine as dsl;
use crate::runtime::actor::member_wake::MemberWakeFailure;
use crate::runtime::dormant_seat::fresh_seat_session_id;
use crate::runtime::{
    DECLARED_APPLY_CHUNK, DeclaredEntry, DeclaredEntryOutcome, DeclaredEntryRefusal,
    DeclaredRoster, DeclaredRosterOutcome, DormantSeatOutcome, EntryStanding, FirstBootRequest,
    HistoryUnavailable, HostPreparationCause, IDENTITY_ENTRY_PAGE_MAX, IdentityEntryRead,
    MemberIdentityManagement, MemberLifecycleEvent, MemberRetirementCause, MobCompositionAuthority,
    MobJournalCursor, UnlistedEntryOutcome, UnseatedEntryCause,
};

const STEP: Duration = Duration::from_mins(1);

fn id(name: &str) -> AgentIdentity {
    AgentIdentity::from(name)
}

fn dsl_id(name: &str) -> dsl::AgentIdentity {
    dsl::AgentIdentity::from_domain(&id(name))
}

/// A spec of `name` under `role`, with `runtime_mode`.
fn spec_as(name: &str, role: &str, runtime_mode: crate::MobRuntimeMode) -> SpawnMemberSpec {
    let mut spec = SpawnMemberSpec::new(role, id(name));
    spec.runtime_mode = Some(runtime_mode);
    spec
}

/// A turn-driven worker spec: it owes no kickoff, so the apply seats it
/// Dormant.
fn worker_spec(name: &str) -> SpawnMemberSpec {
    spec_as(name, "worker", crate::MobRuntimeMode::TurnDriven)
}

/// `name` declared as a turn-driven worker.
fn declared(name: &str) -> DeclaredEntry {
    DeclaredEntry::new(worker_spec(name))
}

/// `name` declared under `role`, with `runtime_mode`.
fn declared_as(name: &str, role: &str, runtime_mode: crate::MobRuntimeMode) -> DeclaredEntry {
    DeclaredEntry::new(spec_as(name, role, runtime_mode))
}

/// A complete roster that lists no identity.
fn empty_roster() -> DeclaredRoster {
    DeclaredRoster::complete(Vec::<DeclaredEntry>::new())
}

/// `name`'s answer in `outcome`.
fn answer<'a>(outcome: &'a DeclaredRosterOutcome, name: &str) -> &'a DeclaredEntryOutcome {
    outcome
        .entries
        .get(&id(name))
        .unwrap_or_else(|| panic!("{name} is answered: {outcome:?}"))
}

/// The session `name`'s answer seated it on, with the answer's notice, when
/// the answer is a seat.
fn seated_on(
    outcome: &DeclaredRosterOutcome,
    name: &str,
) -> Option<(SessionId, Option<HistoryUnavailable>)> {
    match answer(outcome, name) {
        DeclaredEntryOutcome::Seated {
            session_id,
            history_unavailable,
        } => Some((session_id.clone(), history_unavailable.clone())),
        _ => None,
    }
}

/// The cause each `MemberRetirementStarted` row of `journal` records, by
/// member, in journal order.
fn retirement_causes(journal: &[MobEvent]) -> Vec<(AgentIdentity, Option<MemberRetirementCause>)> {
    journal
        .iter()
        .filter_map(|event| match &event.kind {
            MobEventKind::MemberRetirementStarted { agent_identity, .. } => {
                Some((agent_identity.clone(), event.kind.retirement_cause()))
            }
            _ => None,
        })
        .collect()
}

/// A running mob over the mock durable session service and in-memory
/// storage, with session-scoped member comms identities. A restart resumes
/// the same storage, or a copy of it, over a cold-restarted session service.
struct ApplyMob {
    keys: tempfile::TempDir,
    storage: MobStorage,
    handle: MobHandle,
    service: Arc<MockSessionService>,
}

impl ApplyMob {
    async fn new(label: &str) -> Self {
        Self::host_pending(label, &[]).await
    }

    /// A new mob whose host owes the first-boot preparation of every
    /// identity of `pending` ([`MobBuilder::with_host_pending_members`]).
    async fn host_pending(label: &str, pending: &[&str]) -> Self {
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
        .with_host_pending_members(pending.iter().copied().map(id))
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

    /// Crash the mob, restart its session service and resume `storage` as
    /// the composition `authority`.
    async fn restart_into(&mut self, storage: MobStorage, authority: MobCompositionAuthority) {
        self.resume_into(storage, authority, &[]).await;
    }

    /// Crash the mob and resume its storage, authoritative, with the host
    /// owing the first-boot preparation of every identity of `pending`.
    async fn restart_host_pending(&mut self, pending: &[&str]) {
        let storage = self.storage.clone();
        self.resume_into(storage, MobCompositionAuthority::Authoritative, pending)
            .await;
    }

    /// Crash the mob, restart its session service and resume `storage` as
    /// the composition `authority`, with the host owing the first-boot
    /// preparation of every identity of `pending`.
    async fn resume_into(
        &mut self,
        storage: MobStorage,
        authority: MobCompositionAuthority,
        pending: &[&str],
    ) {
        let restarted = Arc::new(self.service.cold_restart_preserving_durable_state().await);
        let _ = restarted.enable_runtime_adapter();
        crash_stop_and_release_routes(self.handle.clone()).await;
        self.service = restarted;
        self.handle = tokio::time::timeout(
            STEP,
            Box::pin(
                MobBuilder::for_resume(storage.clone())
                    .with_session_service(self.service.clone())
                    .with_member_comms_identity_root(self.keys.path())
                    .with_composition_authority(authority)
                    .with_host_pending_members(pending.iter().copied().map(id))
                    .notify_orchestrator_on_resume(false)
                    .resume(),
            ),
        )
        .await
        .expect("the restart answers")
        .expect("the restart succeeds");
        self.storage = storage;
    }

    /// Crash the mob and resume its storage as the composition `authority`.
    async fn restart_as(&mut self, authority: MobCompositionAuthority) {
        let storage = self.storage.clone();
        self.restart_into(storage, authority).await;
    }

    /// Crash the mob and resume its storage, authoritative.
    async fn restart(&mut self) {
        self.restart_as(MobCompositionAuthority::Authoritative)
            .await;
    }

    /// Apply `roster`; the pass itself succeeds.
    async fn apply(&self, roster: DeclaredRoster) -> DeclaredRosterOutcome {
        tokio::time::timeout(STEP, self.handle.apply_declared_roster(roster))
            .await
            .expect("the apply answers")
            .expect("the apply runs")
    }

    /// Apply `roster` again, within one step, while `name`'s answer is
    /// deferred (a member step of its own still in flight): a deferred
    /// identity was left as it was, so the next apply decides it anew.
    async fn apply_settled(&self, roster: DeclaredRoster, name: &str) -> DeclaredRosterOutcome {
        let deadline = tokio::time::Instant::now() + STEP;
        loop {
            let outcome = self.apply(roster.clone()).await;
            if !matches!(
                answer(&outcome, name),
                DeclaredEntryOutcome::Deferred { .. }
            ) {
                return outcome;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "{name} stayed deferred: {outcome:?}"
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

    /// Wake `name`, which succeeds, and wait until MobMachine recorded the
    /// epoch of its registration (its routed binding landed), so a release
    /// of the woken member is honoured.
    async fn wake(&self, name: &str) {
        tokio::time::timeout(STEP, self.handle.ensure_member_live(&id(name)))
            .await
            .expect("the wake answers")
            .expect("the member wakes");
        let key = dsl_id(name);
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

    /// Wait, within one step, until `name` is materialized as `phase`.
    async fn reaches(&self, name: &str, phase: dsl::MemberMaterializationPhase) {
        let deadline = tokio::time::Instant::now() + STEP;
        while self.materialization(name) != Some(phase) {
            assert!(
                tokio::time::Instant::now() < deadline,
                "{name} never reached {phase:?}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
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

    async fn is_member(&self, name: &str) -> bool {
        self.handle
            .list_members()
            .await
            .iter()
            .any(|member| member.agent_identity == id(name))
    }

    /// Where `name`'s identity entry stands, when it is one.
    fn standing(&self, name: &str) -> Option<EntryStanding> {
        match self
            .handle
            .identity_entry(&id(name))
            .expect("the entry read answers")
        {
            IdentityEntryRead::Entry(view) => Some(view.standing),
            IdentityEntryRead::NotAnEntry { .. }
            | IdentityEntryRead::Unreadable { .. }
            | IdentityEntryRead::Removed => None,
        }
    }

    /// The host preparation `name`'s entry owes, when it is an entry.
    fn host_preparation(&self, name: &str) -> Option<HostPreparationCause> {
        match self
            .handle
            .identity_entry(&id(name))
            .expect("the entry read answers")
        {
            IdentityEntryRead::Entry(view) => view.host_preparation,
            IdentityEntryRead::NotAnEntry { .. }
            | IdentityEntryRead::Unreadable { .. }
            | IdentityEntryRead::Removed => None,
        }
    }

    /// The host preparation MobMachine records `name` owing, whether or not
    /// it is an entry yet.
    fn preparation_owed(&self, name: &str) -> Option<dsl::HostPreparationCause> {
        self.handle
            .machine_state_watch_rx
            .borrow()
            .member_host_preparation_causes
            .get(&dsl_id(name))
            .copied()
    }

    /// A host's wake of `name`, refused `HostPreparationPending`, building
    /// nothing.
    async fn wake_refused_for_host_preparation(&self, name: &str) {
        let refused = tokio::time::timeout(STEP, self.handle.ensure_member_live(&id(name)))
            .await
            .expect("the wake answers");
        assert!(
            matches!(
                refused,
                Err(MobError::MemberWakeRefused {
                    refusal: crate::MemberWakeRefusal::HostPreparationPending,
                    ..
                })
            ),
            "{name}: {refused:?}"
        );
        assert_eq!(
            self.materialization(name),
            Some(dsl::MemberMaterializationPhase::Dormant),
            "a refused wake builds nothing"
        );
    }

    fn materialization(&self, name: &str) -> Option<dsl::MemberMaterializationPhase> {
        self.handle.member_materialization_for_test(&id(name))
    }

    /// The archived session MobMachine records for `name`.
    fn archived_record(&self, name: &str) -> Option<dsl::SessionId> {
        self.handle
            .machine_state_watch_rx
            .borrow()
            .identity_entry_archived_sessions
            .get(&dsl_id(name))
            .cloned()
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

/// A complete roster is the host's whole roster: its seal unlists every
/// Declared entry the roster does not list. An unlisted entry whose member
/// is seated is retired, its start row records the cause `Unlisted`, and
/// the end of the retirement ends the entry. A worker the roster does not
/// list is no entry and is never unlisted, and a listed entry keeps its
/// seat.
///
/// RED: let `SealDeclaredRosterSealed` mark every seated member the roster
/// does not list (iterate it over the seated members instead of the identity
/// entries, as the TLC mutant `seal_marks_worker` does): the worker is
/// retired as well. Drop the seal from the apply: the unlisted entry stays
/// seated and is answered nowhere.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unlisted_declared_member_is_retired_and_an_unlisted_worker_is_not() {
    let mob = ApplyMob::new("apply-unlisted").await;
    let seated = mob
        .apply(DeclaredRoster::complete([
            declared("listed"),
            declared("unlisted"),
        ]))
        .await;
    for name in ["listed", "unlisted"] {
        assert!(
            matches!(seated_on(&seated, name), Some((_, None))),
            "{name} is seated: {seated:?}"
        );
    }
    assert!(seated.unlisted.is_empty(), "{seated:?}");
    mob.handle
        .spawn_spec(worker_spec("worker"))
        .await
        .expect("spawn a worker");

    let sealed = mob
        .apply(DeclaredRoster::complete([declared("listed")]))
        .await;
    assert!(
        matches!(answer(&sealed, "listed"), DeclaredEntryOutcome::Kept),
        "{sealed:?}"
    );
    assert!(
        matches!(
            sealed.unlisted.get(&id("unlisted")),
            Some(UnlistedEntryOutcome::Retired)
        ),
        "{sealed:?}"
    );
    assert_eq!(
        sealed.unlisted.len(),
        1,
        "only the entry is unlisted: {sealed:?}"
    );
    assert!(mob.is_member("worker").await, "a worker is never unlisted");
    assert!(mob.is_member("listed").await);
    assert!(
        !mob.is_member("unlisted").await,
        "the unlisted entry's member is retired"
    );
    assert_eq!(
        mob.standing("unlisted"),
        None,
        "the end of the unlisted retirement ends the entry"
    );
    assert!(matches!(
        mob.standing("listed"),
        Some(EntryStanding::Seated(_))
    ));
    assert_eq!(
        retirement_causes(&mob.journal().await),
        vec![(id("unlisted"), Some(MemberRetirementCause::Unlisted))],
        "one retirement, and its start row records the cause"
    );

    let again = mob
        .apply(DeclaredRoster::complete([declared("listed")]))
        .await;
    assert!(again.unlisted.is_empty(), "{again:?}");
    assert!(mob.is_member("worker").await);
}

/// One identity's refusal never fails the pass. A worker (a member that is
/// no identity entry) holding a listed name refuses that identity alone,
/// typed (`NameHeldByWorker`); so do an identity the roster declares twice
/// (`DuplicateDeclaration`) and a first-boot request for an identity the
/// roster does not declare (`UndeclaredFirstBootRequest`). The other listed
/// identity is seated, the worker keeps its seat, its session and its
/// generation, and the seal unlists none of them: an entry declared twice
/// is left as it was, and stays listed.
///
/// RED: let the actor fail the chunk when MobMachine takes no arm for one
/// declaration (return the error from `declare_identity_entries`), with the
/// `RefusedNameHeldByWorker` arm dropped: the pass fails, and the other
/// identity is not seated. With the arm dropped alone, the worker's name is
/// answered `Failed`, not the typed refusal.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_worker_holding_one_listed_name_refuses_that_identity_alone() {
    let mob = ApplyMob::new("apply-worker-name").await;
    mob.handle
        .spawn_spec(worker_spec("worker"))
        .await
        .expect("spawn a worker");
    let session = mob.session("worker").await;
    let seated = mob
        .apply(DeclaredRoster::partial([declared("twice")]))
        .await;
    let (twice_session, _) = seated_on(&seated, "twice").expect("the entry is seated");

    let outcome = mob
        .apply(
            DeclaredRoster::complete([
                declared("worker"),
                declared("declared"),
                declared("twice"),
                declared("twice"),
            ])
            .with_first_boot_request("undeclared", FirstBootRequest::Fresh),
        )
        .await;
    for (name, refusal) in [
        ("worker", DeclaredEntryRefusal::NameHeldByWorker),
        ("twice", DeclaredEntryRefusal::DuplicateDeclaration),
        (
            "undeclared",
            DeclaredEntryRefusal::UndeclaredFirstBootRequest,
        ),
    ] {
        assert!(
            matches!(
                answer(&outcome, name),
                DeclaredEntryOutcome::Refused { refusal: answered } if *answered == refusal
            ),
            "{name}: {outcome:?}"
        );
    }
    assert!(
        matches!(seated_on(&outcome, "declared"), Some((_, None))),
        "the pass goes on: {outcome:?}"
    );
    assert!(
        outcome.unlisted.is_empty(),
        "an entry declared twice stays listed: {outcome:?}"
    );
    assert_eq!(mob.standing("worker"), None, "the worker is no entry");
    assert_eq!(mob.session("worker").await, session);
    assert_eq!(mob.generation("worker").await, Some(Generation::new(0)));
    assert!(
        matches!(mob.standing("twice"), Some(EntryStanding::Seated(_))),
        "an entry declared twice is left as it was"
    );
    assert_eq!(mob.session("twice").await, twice_session);
    assert_eq!(mob.generation("twice").await, Some(Generation::new(0)));
    assert!(!mob.is_member("undeclared").await);
}

/// An undeclared role move holds the member, so nothing builds it under its
/// previous role: an apply that declares a seated entry under another role,
/// with no migration declared from its current role, answers
/// `RoleMigrationHeld` and records the host preparation
/// `RoleMigrationUndeclared`. The idle member is released, and every wake is
/// refused `HostPreparationPending`, a peer delivery's and a host's alike.
/// Only an apply ends the hold: one that declares the migration restamps
/// the member under the new role on its own session, and it wakes.
///
/// RED: decide `Kept` while the declared role differs (drop the role
/// comparison from the verdict helper, as a reload that changes the host's
/// spec alone does): the apply answers `Kept`, no hold is recorded, and the
/// peer delivery wakes the member under its previous role.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn undeclared_reprofile_refuses_a_peer_wake_with_host_preparation_pending() {
    let mob = ApplyMob::new("apply-role-hold").await;
    let seated = mob.apply(DeclaredRoster::partial([declared("held")])).await;
    let (session, _) = seated_on(&seated, "held").expect("the identity is seated");
    mob.wake("held").await;

    let moved = DeclaredRoster::partial([declared_as(
        "held",
        "lead",
        crate::MobRuntimeMode::TurnDriven,
    )]);
    let held = mob.apply(moved.clone()).await;
    assert!(
        matches!(
            answer(&held, "held"),
            DeclaredEntryOutcome::RoleMigrationHeld { from, to }
                if from.as_str() == "worker" && to.as_str() == "lead"
        ),
        "{held:?}"
    );
    assert_eq!(
        mob.host_preparation("held"),
        Some(HostPreparationCause::RoleMigrationUndeclared)
    );
    // The idle member is released.
    mob.reaches("held", dsl::MemberMaterializationPhase::Dormant)
        .await;

    let peer_wake = mob
        .handle
        .enqueue_actor_command_for_test(|reply_tx| crate::runtime::state::MobCommand::WakeMember {
            agent_identity: id("held"),
            cause: dsl::MemberWakeCause::PeerDelivery,
            observed_missing: false,
            input_class: dsl::PeerInputClass::Content,
            sender_waking: false,
            reply_tx,
        })
        .await
        .expect("the peer wake reaches the actor");
    let peer_answer = tokio::time::timeout(STEP, peer_wake)
        .await
        .expect("the peer wake answers")
        .expect("the actor replies");
    assert!(
        matches!(
            peer_answer,
            Err(MemberWakeFailure::Refused(
                crate::MemberWakeRefusal::HostPreparationPending
            ))
        ),
        "{peer_answer:?}"
    );
    let host_wake = tokio::time::timeout(STEP, mob.handle.ensure_member_live(&id("held")))
        .await
        .expect("the wake answers");
    assert!(
        matches!(
            host_wake,
            Err(MobError::MemberWakeRefused {
                refusal: crate::MemberWakeRefusal::HostPreparationPending,
                ..
            })
        ),
        "{host_wake:?}"
    );
    assert_eq!(
        mob.materialization("held"),
        Some(dsl::MemberMaterializationPhase::Dormant),
        "a refused wake builds nothing"
    );
    let still = mob.apply(moved).await;
    assert!(
        matches!(
            answer(&still, "held"),
            DeclaredEntryOutcome::RoleMigrationHeld { .. }
        ),
        "the same move is held again: {still:?}"
    );

    let migrated = mob
        .apply_settled(
            DeclaredRoster::partial([declared_as(
                "held",
                "lead",
                crate::MobRuntimeMode::TurnDriven,
            )])
            .with_role_migration("held", "worker"),
            "held",
        )
        .await;
    assert!(
        matches!(
            answer(&migrated, "held"),
            DeclaredEntryOutcome::RoleMigrated { generation } if *generation == Generation::new(1)
        ),
        "{migrated:?}"
    );
    assert_eq!(
        mob.host_preparation("held"),
        None,
        "the apply ends the hold"
    );
    assert_eq!(
        mob.session("held").await,
        session,
        "the migration keeps the member's session"
    );
    let member = mob
        .handle
        .get_member(&id("held"))
        .await
        .expect("roster read")
        .expect("the member is seated");
    assert_eq!(
        (member.role.as_str(), member.generation),
        ("lead", Generation::new(1))
    );
    mob.wake("held").await;
}

/// A runtime-mode change under the same role is a lifecycle fact: the apply
/// restamps the member with the declared mode on its own session, at its
/// next generation (`Restamped`), and MobMachine records the mode. Once the
/// restamp settled, the same declaration keeps the seat.
///
/// RED: drop the runtime-mode comparison from the verdict helper (decide
/// `Kept` when the role matches): the apply answers `Kept`, and the member
/// stays TurnDriven at generation 0.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_profile_runtime_mode_change_rebuilds_with_the_new_mode() {
    let mob = ApplyMob::new("apply-mode-restamp").await;
    let seated = mob
        .apply(DeclaredRoster::partial([declared("restamped")]))
        .await;
    let (session, _) = seated_on(&seated, "restamped").expect("the identity is seated");
    mob.wake("restamped").await;

    let autonomous = || {
        DeclaredRoster::partial([declared_as(
            "restamped",
            "worker",
            crate::MobRuntimeMode::AutonomousHost,
        )])
    };
    let restamped = mob.apply_settled(autonomous(), "restamped").await;
    assert!(
        matches!(
            answer(&restamped, "restamped"),
            DeclaredEntryOutcome::Restamped { generation } if *generation == Generation::new(1)
        ),
        "{restamped:?}"
    );
    let member = mob
        .handle
        .get_member(&id("restamped"))
        .await
        .expect("roster read")
        .expect("the member is seated");
    assert_eq!(
        (member.generation, member.runtime_mode),
        (Generation::new(1), crate::MobRuntimeMode::AutonomousHost)
    );
    assert_eq!(
        mob.session("restamped").await,
        session,
        "the restamp keeps the member's session"
    );
    assert!(matches!(
        mob.standing("restamped"),
        Some(EntryStanding::Seated(facts))
            if facts.runtime_mode == crate::MobRuntimeMode::AutonomousHost
    ));

    let again = mob.apply_settled(autonomous(), "restamped").await;
    assert!(
        matches!(answer(&again, "restamped"), DeclaredEntryOutcome::Kept),
        "the same declaration keeps the seat: {again:?}"
    );
    assert_eq!(mob.generation("restamped").await, Some(Generation::new(1)));
}

/// Held creation follows the host's hold policy, declared on every apply,
/// so it needs no state directory: an identity whose creation the host
/// holds and that was never created awaits its held creation
/// (`AwaitingHeldCreation`), the apply never seats it, and a dormant seat of
/// it is refused, typed. A restart forgets the cause until the next apply
/// declares it again, and that apply answers the same. The host's creation
/// (a spawn holding the entry's custody, the step the held protocol's
/// activation commits) creates the member, and the next apply keeps it.
///
/// RED: drop `held_creation` from the verdict helper (decide `SeatOwed`):
/// the first apply seats the identity Dormant, outside the held path.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn held_creation_identity_is_never_seated_outside_the_held_path_even_without_a_state_dir() {
    let mut mob = ApplyMob::new("apply-held").await;
    let held = || DeclaredRoster::complete([declared("held").with_held_creation(true)]);
    let first = mob.apply(held()).await;
    assert!(
        matches!(
            answer(&first, "held"),
            DeclaredEntryOutcome::AwaitingHeldCreation
        ),
        "{first:?}"
    );
    assert_eq!(
        mob.standing("held"),
        Some(EntryStanding::Unseated(UnseatedEntryCause::HeldCreation))
    );
    assert!(!mob.is_member("held").await);
    let refused = mob
        .handle
        .seat_dormant_members(vec![worker_spec("held").with_entry_custody(true)])
        .await
        .expect("the seat batch answers");
    assert!(
        matches!(
            &refused[0],
            DormantSeatOutcome::Refused {
                error: MobError::WiringError(_),
                ..
            }
        ),
        "a dormant seat never creates a held identity: {refused:?}"
    );
    assert!(!mob.is_member("held").await);

    mob.restart().await;
    assert_eq!(
        mob.standing("held"),
        None,
        "a restart forgets the cause until the next apply declares it"
    );
    let after_restart = mob.apply(held()).await;
    assert!(
        matches!(
            answer(&after_restart, "held"),
            DeclaredEntryOutcome::AwaitingHeldCreation
        ),
        "{after_restart:?}"
    );
    assert!(!mob.is_member("held").await);
    assert!(
        !mob.journal().await.iter().any(|event| matches!(
            &event.kind,
            MobEventKind::MemberSpawned(spawned) if spawned.agent_identity.as_str() == "held"
        )),
        "nothing seated the identity"
    );

    mob.handle
        .spawn_spec(worker_spec("held").with_entry_custody(true))
        .await
        .expect("the host creates the held identity");
    assert!(matches!(
        mob.standing("held"),
        Some(EntryStanding::Seated(_))
    ));
    let created = mob.apply_settled(held(), "held").await;
    assert!(
        matches!(answer(&created, "held"), DeclaredEntryOutcome::Kept),
        "{created:?}"
    );
}

/// Held creation follows the host's current hold policy, and nothing is
/// kept of an earlier one: an identity declared held awaits its held
/// creation, and the next apply, under a policy that no longer holds it,
/// seats it Dormant. Once created, an identity declared held again keeps
/// its seat: a hold covers only an identity never created.
///
/// RED: keep an earlier apply's hold (let the verdict helper answer
/// `AwaitHeldCreation` for an entry unseated `HeldCreation`, whatever the
/// declaration): the second apply answers `AwaitingHeldCreation` again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn held_creation_follows_the_hosts_current_hold_policy() {
    let mob = ApplyMob::new("apply-hold-policy").await;
    let first = mob
        .apply(DeclaredRoster::partial([
            declared("policy").with_held_creation(true)
        ]))
        .await;
    assert!(
        matches!(
            answer(&first, "policy"),
            DeclaredEntryOutcome::AwaitingHeldCreation
        ),
        "{first:?}"
    );
    let released = mob
        .apply(DeclaredRoster::partial([declared("policy")]))
        .await;
    assert!(
        matches!(seated_on(&released, "policy"), Some((_, None))),
        "{released:?}"
    );
    assert!(matches!(
        mob.standing("policy"),
        Some(EntryStanding::Seated(_))
    ));
    let held_again = mob
        .apply(DeclaredRoster::partial([
            declared("policy").with_held_creation(true)
        ]))
        .await;
    assert!(
        matches!(answer(&held_again, "policy"), DeclaredEntryOutcome::Kept),
        "{held_again:?}"
    );
}

/// A placed identity is hosted on its first use: the apply records the
/// entry unseated (`HostedOnFirstUse`) and seats nothing, and a dormant
/// seat of it is refused, typed: its host creates the member (a placed
/// spawn holding the entry's custody, which MobMachine admits for this
/// cause alone). Nothing is written.
///
/// RED: drop the placement from the verdict helper (decide a placed
/// identity `SeatOwed`): the apply seats it Dormant in this process, and its
/// host's first use finds a local member.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_placed_identity_is_hosted_on_its_first_use_and_never_seated_dormant() {
    let mob = ApplyMob::new("apply-placed").await;
    let journal = mob.journal().await.len();
    let mut spec = worker_spec("placed");
    spec.placement = Some(dsl::HostId("apply-placed-host".to_string()));
    let outcome = mob
        .apply(DeclaredRoster::partial([DeclaredEntry::new(spec)]))
        .await;
    assert!(
        matches!(
            answer(&outcome, "placed"),
            DeclaredEntryOutcome::HostedOnFirstUse
        ),
        "{outcome:?}"
    );
    assert_eq!(
        mob.standing("placed"),
        Some(EntryStanding::Unseated(
            UnseatedEntryCause::HostedOnFirstUse
        ))
    );
    let refused = mob
        .handle
        .seat_dormant_members(vec![worker_spec("placed").with_entry_custody(true)])
        .await
        .expect("the seat batch answers");
    assert!(
        matches!(
            &refused[0],
            DormantSeatOutcome::Refused {
                error: MobError::WiringError(_),
                ..
            }
        ),
        "a dormant seat never creates a placed identity: {refused:?}"
    );
    assert!(!mob.is_member("placed").await);
    assert_eq!(mob.journal().await.len(), journal, "nothing is written");
}

/// A candidate composition (non-authoritative, over the mob's store)
/// declares nothing: every identity of its apply is answered `Refused` with
/// `NonAuthoritative`, typed, its seal unlists nothing, and the journal, the
/// roster and the entries are as they were.
///
/// RED: drop the composition authority from the verdict helper (decide a
/// candidate's declarations as an authoritative composition's): the listed
/// identity is answered `Kept`, and the new one is decided `SeatOwed`, which
/// the candidate's dormant seat refuses untyped. (The seal keeps its own
/// composition guard, so it unlists nothing either way.)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn candidate_apply_writes_nothing_and_answers_every_identity_typed() {
    let mut mob = ApplyMob::new("apply-candidate").await;
    let seated = mob
        .apply(DeclaredRoster::complete([
            declared("listed"),
            declared("left-out"),
        ]))
        .await;
    for name in ["listed", "left-out"] {
        assert!(seated_on(&seated, name).is_some(), "{name}: {seated:?}");
    }
    mob.restart_as(MobCompositionAuthority::NonAuthoritative)
        .await;
    let journal = mob.journal().await.len();

    let outcome = mob
        .apply(DeclaredRoster::complete([
            declared("listed"),
            declared("new"),
        ]))
        .await;
    for name in ["listed", "new"] {
        assert!(
            matches!(
                answer(&outcome, name),
                DeclaredEntryOutcome::Refused {
                    refusal: DeclaredEntryRefusal::NonAuthoritative
                }
            ),
            "{name}: {outcome:?}"
        );
    }
    assert!(
        outcome.unlisted.is_empty(),
        "a candidate unlists nothing: {outcome:?}"
    );
    assert_eq!(
        mob.journal().await.len(),
        journal,
        "a candidate writes no row"
    );
    assert!(mob.is_member("listed").await);
    assert!(mob.is_member("left-out").await);
    assert_eq!(mob.standing("new"), None);
    assert!(matches!(
        mob.standing("left-out"),
        Some(EntryStanding::Seated(_))
    ));
}

/// Apply a complete roster of `members` declared identities, restart, and
/// apply the same roster again: the second apply answers every identity
/// `Kept`, in chunks of [`DECLARED_APPLY_CHUNK`], appends no journal row,
/// creates and reads no session and wakes no member. Returns the time the
/// restart and the second apply took together.
async fn unchanged_roster_appends_no_row_and_wakes_none(members: usize) -> Duration {
    let mut mob = ApplyMob::new("apply-unchanged").await;
    let names: Vec<String> = (0..members)
        .map(|index| format!("declared-{index:05}"))
        .collect();
    let roster = || DeclaredRoster::complete(names.iter().map(|name| declared(name.as_str())));
    let first = mob.apply(roster()).await;
    assert_eq!(first.entries.len(), members);
    assert!(
        first.entries.values().all(|outcome| matches!(
            outcome,
            DeclaredEntryOutcome::Seated {
                history_unavailable: None,
                ..
            }
        )),
        "every identity is seated"
    );

    let started = std::time::Instant::now();
    mob.restart().await;
    let journal = mob.journal().await.len();
    let reads = (
        mob.service.session_read_calls(),
        mob.service.persisted_session_load_call_count(),
    );
    let second = mob.apply(roster()).await;
    let elapsed = started.elapsed();
    assert_eq!(second.entries.len(), members);
    assert!(
        second
            .entries
            .values()
            .all(|outcome| matches!(outcome, DeclaredEntryOutcome::Kept)),
        "every identity keeps its seat"
    );
    assert!(second.unlisted.is_empty());
    assert_eq!(
        mob.journal().await.len(),
        journal,
        "an unchanged roster appends no row"
    );
    assert_eq!(
        (
            mob.service.session_read_calls(),
            mob.service.persisted_session_load_call_count(),
        ),
        reads,
        "an unchanged roster reads no session"
    );
    assert!(
        mob.service.recorded_create_requests().await.is_empty(),
        "no session was created"
    );
    assert_eq!(
        mob.service.active_session_count().await,
        0,
        "no member was woken"
    );
    let dormant = mob
        .handle
        .machine_state_watch_rx
        .borrow()
        .member_materialization
        .values()
        .filter(|phase| matches!(phase, dsl::MemberMaterializationPhase::Dormant))
        .count();
    assert_eq!(dormant, members, "every member stays Dormant");
    elapsed
}

/// An unchanged roster of two chunks and one identity more writes nothing:
/// see [`unchanged_roster_appends_no_row_and_wakes_none`].
///
/// Guard: MobMachine decides each declaration in memory, so an
/// unchanged roster writes no row and wakes no member.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unchanged_roster_of_two_chunks_appends_no_row_and_wakes_none() {
    unchanged_roster_appends_no_row_and_wakes_none(2 * DECLARED_APPLY_CHUNK + 1).await;
}

/// [`unchanged_roster_of_two_chunks_appends_no_row_and_wakes_none`] at
/// 10,000 identities, inside the 60 s restart bound: the restart and the
/// apply of the unchanged roster together. The counters are the assertion;
/// the wall clock is a smoke bound. Run explicitly, in the release profile,
/// as the other scale rows are.
///
/// Guard: the gate of the in-memory decision at the roster sizes hosts run.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "scale row: run explicitly in the release profile"]
async fn unchanged_roster_of_10000_dormant_identities_appends_no_row_and_wakes_none() {
    let elapsed = unchanged_roster_appends_no_row_and_wakes_none(10_000).await;
    assert!(
        elapsed < Duration::from_mins(1),
        "the restart and the unchanged apply took {elapsed:?}"
    );
}

/// A first-boot `Resume` request seats its identity Dormant on the
/// request's session (the identity's committed session from before the
/// upgrade) and wakes nothing: no session is created, loaded or built. The
/// seat carries the request out: the identity's first-boot preparation,
/// owed from the start, ends, and the answer names the request carried out.
/// The next apply, without the request (the host consumed it), keeps the
/// seat and carries nothing out.
///
/// RED: let the dormant seat keep the first-boot preparation (drop its
/// removal from `CommitDormantSeatRunning`): the seated entry still owes
/// `FirstBootRow`, and every wake of it is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_boot_resume_request_seats_on_its_session_and_never_wakes() {
    let mob = ApplyMob::host_pending("apply-first-boot", &["imported"]).await;
    assert_eq!(
        mob.preparation_owed("imported"),
        Some(dsl::HostPreparationCause::FirstBootRow),
        "the host owes the identity's first-boot preparation"
    );
    let committed = SessionId::new();
    mob.service
        .replace_persisted_session(Session::with_id(committed.clone()))
        .await;
    let outcome = mob
        .apply(
            DeclaredRoster::complete([declared("imported")]).with_first_boot_request(
                "imported",
                FirstBootRequest::Resume {
                    session_id: committed.clone(),
                },
            ),
        )
        .await;
    assert_eq!(
        seated_on(&outcome, "imported"),
        Some((committed.clone(), None)),
        "{outcome:?}"
    );
    assert_eq!(
        outcome.first_boot_requests_carried_out,
        BTreeSet::from([id("imported")]),
        "the seat carries the request out"
    );
    assert_eq!(mob.session("imported").await, committed);
    assert_eq!(
        mob.materialization("imported"),
        Some(dsl::MemberMaterializationPhase::Dormant)
    );
    assert!(
        mob.service.recorded_create_requests().await.is_empty(),
        "no session was created"
    );
    assert_eq!(
        mob.service
            .durable_reads_of(&committed)
            .load_persisted_session,
        0,
        "the session was not loaded"
    );
    assert_eq!(
        mob.host_preparation("imported"),
        None,
        "the seat ends the first-boot preparation"
    );
    assert_eq!(mob.preparation_owed("imported"), None);

    let kept = mob
        .apply(DeclaredRoster::complete([declared("imported")]))
        .await;
    assert!(
        matches!(answer(&kept, "imported"), DeclaredEntryOutcome::Kept),
        "{kept:?}"
    );
    assert!(kept.first_boot_requests_carried_out.is_empty(), "{kept:?}");
    assert_eq!(
        mob.materialization("imported"),
        Some(dsl::MemberMaterializationPhase::Dormant)
    );
}

/// A first-boot `Fresh` request starts its identity's fresh life on a new
/// session, answered `FreshLife`: an unseated identity lets its archived
/// session record go and is seated fresh, and a seated member is rotated
/// onto a fresh session. Until then the first-boot preparation the host owes
/// both refuses the seated member's wakes; the fresh life carries the
/// request out and ends it.
///
/// RED: let the rotation's commit keep the first-boot preparation (drop the
/// `FirstBootRow` removal from the `CommitMemberRotation` arms): the rotated
/// member still owes `FirstBootRow` after the apply that carried its request
/// out.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fresh_first_boot_request_starts_a_fresh_life() {
    let mut mob = ApplyMob::new("apply-fresh-life").await;
    let seated = mob
        .apply(DeclaredRoster::complete([
            declared("reset-unseated"),
            declared("reset-seated"),
        ]))
        .await;
    let (unseated_session, _) = seated_on(&seated, "reset-unseated").expect("seated");
    let (seated_session, _) = seated_on(&seated, "reset-seated").expect("seated");
    mob.wake("reset-unseated").await;
    mob.apply(DeclaredRoster::complete([declared("reset-seated")]))
        .await;
    assert_eq!(
        mob.archived_record("reset-unseated"),
        Some(dsl::SessionId::from_domain(&unseated_session))
    );
    mob.restart_host_pending(&["reset-unseated", "reset-seated"])
        .await;
    for name in ["reset-unseated", "reset-seated"] {
        assert_eq!(
            mob.preparation_owed(name),
            Some(dsl::HostPreparationCause::FirstBootRow),
            "{name}"
        );
    }
    mob.wake_refused_for_host_preparation("reset-seated").await;

    let reset = mob
        .apply(
            DeclaredRoster::complete([declared("reset-unseated"), declared("reset-seated")])
                .with_first_boot_request("reset-unseated", FirstBootRequest::Fresh)
                .with_first_boot_request("reset-seated", FirstBootRequest::Fresh),
        )
        .await;
    for (name, previous) in [
        ("reset-unseated", &unseated_session),
        ("reset-seated", &seated_session),
    ] {
        let fresh = match answer(&reset, name) {
            DeclaredEntryOutcome::FreshLife { session_id } => session_id.clone(),
            other => panic!("{name} starts a fresh life: {other:?}"),
        };
        assert_ne!(&fresh, previous, "{name}");
        assert_eq!(mob.session(name).await, fresh, "{name}");
        assert_eq!(
            mob.preparation_owed(name),
            None,
            "{name}: the fresh life ends the first-boot preparation"
        );
    }
    assert_eq!(
        reset.first_boot_requests_carried_out,
        BTreeSet::from([id("reset-unseated"), id("reset-seated")]),
        "{reset:?}"
    );
    assert_eq!(
        mob.archived_record("reset-unseated"),
        None,
        "the record is let go"
    );
}

/// A seated member's first-boot `Resume` request, carried with a role move
/// the roster does not declare, is carried out in the declaration's own
/// step: the member is answered `RoleMigrationHeld`, the answer names the
/// request carried out, and the role hold takes the place of the first-boot
/// preparation in that step, so no wake finds the member owing neither.
/// Before the apply its first-boot preparation refuses its wakes; after it,
/// its role hold does.
///
/// RED: declare the identity without its request's kind (pass
/// `FirstBootRequestKind::None` with every declaration): the step keeps the
/// first-boot preparation, which masks the role hold, and the entry owes
/// `FirstBootRow` after an apply whose request the host consumes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_seated_members_first_boot_resume_with_a_role_move_holds_it_in_one_step() {
    let mut mob = ApplyMob::new("apply-first-boot-held").await;
    let seated = mob.apply(DeclaredRoster::partial([declared("held")])).await;
    let (session, _) = seated_on(&seated, "held").expect("the identity is seated");
    mob.restart_host_pending(&["held"]).await;
    assert_eq!(
        mob.host_preparation("held"),
        Some(HostPreparationCause::FirstBootRow)
    );
    mob.wake_refused_for_host_preparation("held").await;

    let moved = mob
        .apply(
            DeclaredRoster::partial([declared_as(
                "held",
                "lead",
                crate::MobRuntimeMode::TurnDriven,
            )])
            .with_first_boot_request(
                "held",
                FirstBootRequest::Resume {
                    session_id: session.clone(),
                },
            ),
        )
        .await;
    assert!(
        matches!(
            answer(&moved, "held"),
            DeclaredEntryOutcome::RoleMigrationHeld { from, to }
                if from.as_str() == "worker" && to.as_str() == "lead"
        ),
        "{moved:?}"
    );
    assert_eq!(
        moved.first_boot_requests_carried_out,
        BTreeSet::from([id("held")]),
        "the seated member carries the request out"
    );
    assert_eq!(
        mob.host_preparation("held"),
        Some(HostPreparationCause::RoleMigrationUndeclared),
        "the role hold takes the first-boot preparation's place"
    );
    mob.wake_refused_for_host_preparation("held").await;
    assert_eq!(mob.session("held").await, session);
}

/// An unlisted identity listed again comes back as the same identity on its
/// own archived session: the end of its unlisted retirement records the
/// session it archived (live, and again on the replay after a restart), and
/// the apply that lists it again seats it Dormant on that session, at its
/// next generation, with no wake, no new session and no fresh life. Its
/// history stays in the session's durable body, which its first wake
/// revives.
///
/// RED: drop the record from the retirement's end (or seat every SeatOwed
/// identity fresh in `owed_seat`): the identity is seated on its fresh
/// generation-1 session, and its history stays behind on the archived one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unlisted_identity_listed_again_revives_on_its_archived_session() {
    let mut mob = ApplyMob::new("apply-revive").await;
    let seated = mob
        .apply(DeclaredRoster::complete([declared("revived")]))
        .await;
    let (session, _) = seated_on(&seated, "revived").expect("the identity is seated");
    mob.wake("revived").await;

    let unlisted = mob.apply(empty_roster()).await;
    assert!(
        matches!(
            unlisted.unlisted.get(&id("revived")),
            Some(UnlistedEntryOutcome::Retired)
        ),
        "{unlisted:?}"
    );
    assert_eq!(
        mob.standing("revived"),
        None,
        "the unlisted identity is no entry"
    );
    assert!(mob.service.test_is_session_archived(&session).await);
    assert_eq!(
        mob.archived_record("revived"),
        Some(dsl::SessionId::from_domain(&session))
    );

    mob.restart().await;
    assert_eq!(
        mob.standing("revived"),
        None,
        "the replay of the retirement's end ends the entry again"
    );
    assert_eq!(
        mob.archived_record("revived"),
        Some(dsl::SessionId::from_domain(&session)),
        "and records the session again"
    );

    let listed = mob
        .apply(DeclaredRoster::complete([declared("revived")]))
        .await;
    assert_eq!(
        seated_on(&listed, "revived"),
        Some((session.clone(), None)),
        "{listed:?}"
    );
    assert_eq!(mob.session("revived").await, session);
    assert_eq!(mob.generation("revived").await, Some(Generation::new(1)));
    assert_eq!(
        mob.materialization("revived"),
        Some(dsl::MemberMaterializationPhase::Dormant),
        "no wake"
    );
    assert_eq!(
        mob.archived_record("revived"),
        None,
        "the seat drops the record"
    );
    assert!(
        mob.service.recorded_create_requests().await.is_empty(),
        "no session was created"
    );
    assert!(
        mob.service
            .persisted_session_clone(&session)
            .await
            .is_some(),
        "its history is where it was"
    );
    assert!(matches!(
        mob.standing("revived"),
        Some(EntryStanding::Seated(facts)) if facts.session_id.as_ref() == Some(&session)
    ));
}

/// An unlisted retirement a crash interrupted after its start row is
/// finished by the restart as it would have finished live: the replay marks
/// the entry again from the start row's cause (`Unlisted`), the start
/// re-drives the retirement, and its end ends the entry and records the
/// session it archived.
///
/// RED: let the replay of a pending retirement's start row leave its mark
/// out (drop its `recover_unlisted_retirement_mark`): the re-driven
/// retirement ends as a plain one, which keeps the entry, unseated and owed
/// its seat.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_interrupted_unlisted_retirement_ends_its_entry_after_a_restart() {
    let mut mob = ApplyMob::new("apply-unlisted-restart").await;
    let seated = mob
        .apply(DeclaredRoster::complete([
            declared("listed"),
            declared("unlisted"),
        ]))
        .await;
    let (session, _) = seated_on(&seated, "unlisted").expect("the identity is seated");
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
    assert_eq!(
        retirement_causes(&mob.journal().await),
        vec![(id("unlisted"), Some(MemberRetirementCause::Unlisted))],
        "the start row records the cause"
    );

    mob.restart().await;
    hold.release();
    let _ = tokio::time::timeout(STEP, sealing).await;

    mob.left("unlisted").await;
    assert_eq!(
        mob.standing("unlisted"),
        None,
        "the re-driven retirement's end ends the entry"
    );
    assert_eq!(
        mob.archived_record("unlisted"),
        Some(dsl::SessionId::from_domain(&session)),
        "and records the session it archived"
    );
    assert!(matches!(
        mob.standing("listed"),
        Some(EntryStanding::Seated(_))
    ));
}

/// One declared apply of a mob runs at a time: an apply called while
/// another one runs is refused, typed and retryable
/// (`LifecycleOperationPending`), and declares and writes nothing. Once the
/// running apply ends, the next one runs.
///
/// RED: let `OpenDeclaredApply` open an apply while one is open (drop the
/// `OpenDeclaredApplyBusy` arm and `OpenDeclaredApplyOpened`'s
/// `no_apply_open` guard): the second apply runs inside the first one's
/// pass and seats its identity.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_apply_while_another_runs_is_refused_retryable() {
    let mob = ApplyMob::new("apply-serialized").await;
    mob.apply(DeclaredRoster::complete([
        declared("listed"),
        declared("unlisted"),
    ]))
    .await;
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
        .expect("the running apply's seal retires the unlisted entry's member");
    let journal = mob.journal().await.len();

    let refused = tokio::time::timeout(
        STEP,
        mob.handle
            .apply_declared_roster(DeclaredRoster::partial([declared("other")])),
    )
    .await
    .expect("the second apply answers");
    assert!(
        matches!(refused, Err(MobError::LifecycleOperationPending { .. })),
        "{refused:?}"
    );
    assert_eq!(
        mob.standing("other"),
        None,
        "the refused apply declares nothing"
    );
    assert_eq!(mob.journal().await.len(), journal, "and writes nothing");

    hold.release();
    let sealed = tokio::time::timeout(STEP, sealing)
        .await
        .expect("the running apply answers")
        .expect("the running apply's task")
        .expect("the running apply runs");
    assert!(
        matches!(
            sealed.unlisted.get(&id("unlisted")),
            Some(UnlistedEntryOutcome::Retired)
        ),
        "{sealed:?}"
    );
    let next = mob
        .apply(DeclaredRoster::partial([declared("other")]))
        .await;
    assert!(
        matches!(seated_on(&next, "other"), Some((_, None))),
        "the next apply runs: {next:?}"
    );
}

/// An unlisted identity whose archived session is gone from the store (its
/// retention purged it) is seated on a fresh session when it is listed
/// again, never left unseatable: the apply's answer and the seat's journal
/// row carry the typed notice `HistoryUnavailable`, naming the lost
/// session, and the record is let go. An identity whose member never woke
/// had no history to lose: it is seated again with no notice.
///
/// RED: answer the lost session's refusal (`SessionUnavailableForResume`,
/// `Absent`) as the identity's failure (drop the fallback in
/// `realize_seats`): the identity is answered `Failed`, stays unseated, and
/// every later apply fails it again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_identity_whose_archived_session_is_gone_is_seated_fresh_with_a_notice() {
    let mob = ApplyMob::new("apply-history-lost").await;
    let roster = || DeclaredRoster::complete([declared("purged"), declared("never-woken")]);
    let seated = mob.apply(roster()).await;
    let (purged, _) = seated_on(&seated, "purged").expect("the identity is seated");
    mob.wake("purged").await;
    let unlisted = mob.apply(empty_roster()).await;
    assert_eq!(unlisted.unlisted.len(), 2, "{unlisted:?}");
    assert_eq!(
        mob.archived_record("purged"),
        Some(dsl::SessionId::from_domain(&purged))
    );
    // The store's retention purges the archived session.
    mob.service.delete_persisted_session(&purged).await;

    let listed = mob.apply(roster()).await;
    let fresh = fresh_seat_session_id(
        mob.handle.mob_id(),
        mob.handle.incarnation(),
        &id("purged"),
        Generation::new(1),
    );
    assert_eq!(
        seated_on(&listed, "purged"),
        Some((fresh.clone(), Some(HistoryUnavailable::new(purged.clone())))),
        "{listed:?}"
    );
    assert_eq!(mob.session("purged").await, fresh);
    assert_eq!(mob.archived_record("purged"), None, "the record is let go");
    let seats: Vec<(Generation, Option<HistoryUnavailable>)> = mob
        .lifecycle()
        .await
        .into_iter()
        .filter_map(|event| match event {
            MemberLifecycleEvent::Spawned {
                agent_identity,
                generation,
                history_unavailable,
                ..
            } if agent_identity == id("purged") => Some((generation, history_unavailable)),
            _ => None,
        })
        .collect();
    assert_eq!(
        seats,
        vec![
            (Generation::new(0), None),
            (Generation::new(1), Some(HistoryUnavailable::new(purged))),
        ],
        "the seat's row carries the notice"
    );
    assert!(
        matches!(seated_on(&listed, "never-woken"), Some((_, None))),
        "{listed:?}"
    );
}

/// Q1 (a) for an identity that owes a kickoff: an autonomous identity whose
/// archived session is gone from the store is embodied on a fresh session
/// (its kickoff runs once there), and the notice rides both its answer and
/// its spawn's journal row.
///
/// RED: let the spawn path drop the spec's notice and the apply answer
/// `Embodied` without one (the embodied seat before this fix): neither the
/// answer nor the row carries it, and the host is never told that the
/// identity's history was lost.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_autonomous_identity_whose_archived_session_is_gone_is_embodied_fresh_with_a_notice() {
    let mob = ApplyMob::new("apply-history-lost-autonomous").await;
    let roster = || {
        DeclaredRoster::complete([declared_as(
            "autonomous",
            "worker",
            crate::MobRuntimeMode::AutonomousHost,
        )])
    };
    let embodied = mob.apply(roster()).await;
    let purged = match answer(&embodied, "autonomous") {
        DeclaredEntryOutcome::Embodied {
            session_id,
            history_unavailable: None,
        } => session_id.clone(),
        other => panic!("the autonomous identity is embodied: {other:?}"),
    };
    let unlisted = mob.apply(empty_roster()).await;
    assert!(
        matches!(
            unlisted.unlisted.get(&id("autonomous")),
            Some(UnlistedEntryOutcome::Retired)
        ),
        "{unlisted:?}"
    );
    assert_eq!(
        mob.archived_record("autonomous"),
        Some(dsl::SessionId::from_domain(&purged))
    );
    // The store's retention purges the archived session.
    mob.service.delete_persisted_session(&purged).await;

    let listed = mob.apply(roster()).await;
    let fresh = fresh_seat_session_id(
        mob.handle.mob_id(),
        mob.handle.incarnation(),
        &id("autonomous"),
        Generation::new(1),
    );
    assert!(
        matches!(
            answer(&listed, "autonomous"),
            DeclaredEntryOutcome::Embodied {
                session_id,
                history_unavailable: Some(notice),
            } if session_id == &fresh && notice == &HistoryUnavailable::new(purged.clone())
        ),
        "the identity is embodied fresh with the notice: {listed:?}"
    );
    assert_eq!(
        mob.archived_record("autonomous"),
        None,
        "the record is let go"
    );
    let seats: Vec<(Generation, Option<HistoryUnavailable>)> = mob
        .lifecycle()
        .await
        .into_iter()
        .filter_map(|event| match event {
            MemberLifecycleEvent::Spawned {
                agent_identity,
                generation,
                history_unavailable,
                ..
            } if agent_identity == id("autonomous") => Some((generation, history_unavailable)),
            _ => None,
        })
        .collect();
    assert_eq!(
        seats,
        vec![
            (Generation::new(0), None),
            (Generation::new(1), Some(HistoryUnavailable::new(purged))),
        ],
        "the spawn's row carries the notice"
    );
}

/// An unlisted identity listed again while the store cannot read its
/// archived session now is deferred, typed and retryable, for that identity
/// alone: it is never seated on a fresh session (an unreadable session is
/// not an absent one), its record is kept, and the next apply, with the
/// store readable, seats it on its archived session with its history.
///
/// RED: let the fallback in `realize_seats` take every refused revival seat
/// (any refusal read as the session's loss): the identity is seated on a
/// fresh session with `HistoryUnavailable` while its history is intact.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unreadable_archived_session_defers_its_identity_and_never_seats_it_fresh() {
    let mob = ApplyMob::new("apply-history-unreadable").await;
    let roster = || DeclaredRoster::complete([declared("revived"), declared("bystander")]);
    let seated = mob.apply(roster()).await;
    let (session, _) = seated_on(&seated, "revived").expect("the identity is seated");
    mob.wake("revived").await;
    let unlisted = mob
        .apply(DeclaredRoster::complete([declared("bystander")]))
        .await;
    assert!(
        matches!(
            unlisted.unlisted.get(&id("revived")),
            Some(UnlistedEntryOutcome::Retired)
        ),
        "{unlisted:?}"
    );

    mob.service
        .fail_next_resume_authority_observes_for(&session, 1);
    let deferred = mob.apply(roster()).await;
    assert!(
        matches!(
            answer(&deferred, "revived"),
            DeclaredEntryOutcome::Deferred {
                reason: MobError::SessionError(meerkat_core::service::SessionError::Store(_))
            }
        ),
        "{deferred:?}"
    );
    assert!(
        matches!(answer(&deferred, "bystander"), DeclaredEntryOutcome::Kept),
        "the pass goes on: {deferred:?}"
    );
    assert!(
        !mob.is_member("revived").await,
        "never seated on a fresh session"
    );
    assert_eq!(
        mob.standing("revived"),
        Some(EntryStanding::Unseated(UnseatedEntryCause::SeatOwed))
    );
    assert_eq!(
        mob.archived_record("revived"),
        Some(dsl::SessionId::from_domain(&session)),
        "the record is kept"
    );

    let revived = mob.apply(roster()).await;
    assert_eq!(
        seated_on(&revived, "revived"),
        Some((session.clone(), None)),
        "{revived:?}"
    );
    assert!(
        mob.service
            .persisted_session_clone(&session)
            .await
            .is_some(),
        "its history is where it was"
    );
}

/// Retiring a declared identity that stays listed does not remove it: the
/// host's retire ends the member, and the entry outlives the retirement,
/// unseated and owed its seat, with its archived session recorded. The next
/// apply of the roster seats it again on that session. A host removes an
/// identity by unlisting it.
///
/// RED: end the entry at the end of every retirement (as identity entries
/// did before the declared apply): the entry is gone after the retire, and
/// the next apply seats the identity on a fresh session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_retired_identity_that_stays_listed_is_seated_again_on_its_session() {
    let mob = ApplyMob::new("apply-retire-listed").await;
    let roster = || DeclaredRoster::complete([declared("retired")]);
    let seated = mob.apply(roster()).await;
    let (session, _) = seated_on(&seated, "retired").expect("the identity is seated");
    mob.wake("retired").await;
    tokio::time::timeout(STEP, mob.handle.retire(id("retired")))
        .await
        .expect("the retire answers")
        .expect("the host retires the member");
    assert!(!mob.is_member("retired").await);
    assert_eq!(
        mob.standing("retired"),
        Some(EntryStanding::Unseated(UnseatedEntryCause::SeatOwed)),
        "the entry outlives the retirement"
    );
    assert_eq!(
        mob.archived_record("retired"),
        Some(dsl::SessionId::from_domain(&session))
    );
    assert_eq!(
        retirement_causes(&mob.journal().await),
        vec![(id("retired"), None)],
        "the host's retirement records no cause"
    );

    let reseated = mob.apply(roster()).await;
    assert_eq!(
        seated_on(&reseated, "retired"),
        Some((session, None)),
        "{reseated:?}"
    );
    assert_eq!(mob.generation("retired").await, Some(Generation::new(1)));
}

/// A retirement that only released a host-owned session archives nothing,
/// live or replayed: the identity's next seat is the same after a restart
/// as without one, a fresh session, never the other host's live session.
/// The live end records an archived session only for an Archived disposal,
/// and the terminal row's replay-only `session_archived` lets the replay
/// apply the same condition.
///
/// RED: let the replay record the session whatever the disposal (the replay
/// before this fix, which the row did not tell): after the restart the
/// identity holds an archived record of the host's session, and the apply
/// seats it there.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_host_owned_retirement_records_no_archived_session_live_or_replayed() {
    let mut mob = ApplyMob::new("apply-host-owned-retirement").await;
    let roster = || DeclaredRoster::complete([declared("hosted-session")]);
    let seated = mob.apply(roster()).await;
    let (session, _) = seated_on(&seated, "hosted-session").expect("the identity is seated");
    mob.wake("hosted-session").await;
    // The member's durable session belongs to another host: its retirement
    // releases the runtime only.
    mob.service.set_archive_authority_unknown(&session).await;
    mob.service.set_archive_not_found(&session).await;
    tokio::time::timeout(STEP, mob.handle.retire(id("hosted-session")))
        .await
        .expect("the retire answers")
        .expect("the retire succeeds");
    assert_eq!(
        mob.standing("hosted-session"),
        Some(EntryStanding::Unseated(UnseatedEntryCause::SeatOwed)),
        "the listed entry outlives the retirement"
    );
    assert_eq!(
        mob.archived_record("hosted-session"),
        None,
        "a host-owned session is not archived here"
    );

    mob.restart().await;
    assert_eq!(
        mob.standing("hosted-session"),
        Some(EntryStanding::Unseated(UnseatedEntryCause::SeatOwed)),
        "the replay restores the entry unseated"
    );
    assert_eq!(
        mob.archived_record("hosted-session"),
        None,
        "the replay records no archived session either"
    );
    let listed = mob.apply(roster()).await;
    let fresh = fresh_seat_session_id(
        mob.handle.mob_id(),
        mob.handle.incarnation(),
        &id("hosted-session"),
        Generation::new(1),
    );
    assert_eq!(
        seated_on(&listed, "hosted-session"),
        Some((fresh, None)),
        "seated as without the restart, on a fresh session: {listed:?}"
    );
}

/// Where `name`'s identity entry stands on `handle`, when it is one.
fn entry_standing(handle: &MobHandle, name: &str) -> Option<EntryStanding> {
    match handle
        .identity_entry(&id(name))
        .expect("the entry read answers")
    {
        IdentityEntryRead::Entry(view) => Some(view.standing),
        IdentityEntryRead::NotAnEntry { .. }
        | IdentityEntryRead::Unreadable { .. }
        | IdentityEntryRead::Removed => None,
    }
}

/// An externally bound identity entry's retirement replays as its live end
/// did. Declared hosted on its first use, created by its host's spawn with
/// entry custody (bound to its external peer, with no session of its own),
/// retired, then restarted, it stays hosted on its first use, never owed a
/// local seat: the replay reads the incarnation's spawn row (a peer-only
/// member reference) as hosted elsewhere.
///
/// RED: let the replay read a hosted incarnation from a placed spawn id
/// alone (no peer-only member reference): after the restart the entry reads
/// `SeatOwed`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_declared_external_identity_is_hosted_on_its_first_use_across_restarts() {
    let _serial = lock_real_comms_tests();
    let definition = with_unique_mob_id(
        sample_definition_with_external_backend(),
        "apply-external-entry",
    );
    let mob_id = definition.id.clone();
    let storage = MobStorage::in_memory();
    let (handle, _service) =
        create_test_mob_with_real_comms_and_storage(definition, storage.clone()).await;
    let external =
        spawn_live_external_peer(&test_comms_name_for(&mob_id, "worker", "external-entry")).await;
    let mut spec = SpawnMemberSpec::host_root("worker", id("external-entry"));
    spec.binding = Some(external.binding());
    let declared_outcome = tokio::time::timeout(
        STEP,
        handle.apply_declared_roster(DeclaredRoster::complete([DeclaredEntry::new(spec.clone())])),
    )
    .await
    .expect("the apply answers")
    .expect("the apply runs");
    assert!(
        matches!(
            declared_outcome.entries.get(&id("external-entry")),
            Some(DeclaredEntryOutcome::HostedOnFirstUse)
        ),
        "{declared_outcome:?}"
    );

    // The host creates the member: its spawn holds the entry's custody.
    let owner_context = handle
        .create_generated_ops_owner_context_for_test()
        .await
        .expect("an owner context");
    handle
        .spawn_spec_receipt_with_owner_context(spec.with_entry_custody(true), owner_context)
        .await
        .expect("the host's spawn creates the external member");
    assert!(
        matches!(
            entry_standing(&handle, "external-entry"),
            Some(EntryStanding::Seated(_))
        ),
        "the external member is the entry's"
    );

    tokio::time::timeout(STEP, handle.retire(id("external-entry")))
        .await
        .expect("the retire answers")
        .expect("the retire succeeds");
    assert_eq!(
        entry_standing(&handle, "external-entry"),
        Some(EntryStanding::Unseated(
            UnseatedEntryCause::HostedOnFirstUse
        )),
        "the live end keeps the entry hosted on its first use"
    );

    crash_stop_and_release_routes(handle).await;
    let (handle, _service) = resume_test_mob_with_real_comms(storage).await;
    assert_eq!(
        entry_standing(&handle, "external-entry"),
        Some(EntryStanding::Unseated(
            UnseatedEntryCause::HostedOnFirstUse
        )),
        "the replay keeps the entry hosted on its first use"
    );
    drop(external);
}

/// A descriptive facet change is no lifecycle fact: an apply that changes
/// only the declared display name, labels, context or instructions of a
/// seated identity answers `Kept` and writes nothing. The member keeps its
/// session, its generation and its live materialization: no rotation, no
/// restamp and no new life. The host keeps the facets it declares, and its
/// next build of the member applies them.
///
/// Guard: `DeclareIdentityEntry` takes no facet, so no verdict reads one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_descriptive_facet_change_is_never_a_new_life() {
    let mob = ApplyMob::new("apply-facets").await;
    let faceted = |display_name: &str, instruction: &str| {
        let mut spec = worker_spec("faceted");
        spec.labels = Some(BTreeMap::from([(
            "display_name".to_string(),
            display_name.to_string(),
        )]));
        spec.context = Some(serde_json::json!({ "title": display_name }));
        spec.additional_instructions = Some(vec![instruction.to_string()]);
        DeclaredRoster::complete([DeclaredEntry::new(spec)])
    };
    let seated = mob.apply(faceted("Analyst", "Answer briefly.")).await;
    let (session, _) = seated_on(&seated, "faceted").expect("the identity is seated");
    mob.wake("faceted").await;
    let journal = mob.journal().await.len();
    let creates = mob.service.recorded_create_requests().await.len();

    let renamed = mob
        .apply(faceted("Senior analyst", "Answer in detail."))
        .await;
    assert!(
        matches!(answer(&renamed, "faceted"), DeclaredEntryOutcome::Kept),
        "{renamed:?}"
    );
    assert_eq!(
        mob.journal().await.len(),
        journal,
        "a facet change writes no row"
    );
    assert_eq!(
        mob.service.recorded_create_requests().await.len(),
        creates,
        "and creates no session"
    );
    assert_eq!(mob.session("faceted").await, session);
    assert_eq!(mob.generation("faceted").await, Some(Generation::new(0)));
    assert_eq!(
        mob.materialization("faceted"),
        Some(dsl::MemberMaterializationPhase::Live),
        "the live member keeps serving"
    );
}

/// Rollback of an unlisted retirement, in-process leg: a retirement start
/// row that records the cause `Unlisted`, read by a reader that predates
/// the cause (its decoder ignores the key), replays as a plain retirement.
/// A restart over the journal with the cause left out finishes the
/// retirement, the member leaves the roster, and the listed member is
/// served. The lane-run leg against the published Core is
/// `an_unlisted_retirement_start_row_is_read_by_097` below.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unlisted_retirement_start_row_replays_as_a_plain_retirement_without_its_cause() {
    let mut mob = ApplyMob::new("apply-rollback-cause").await;
    mob.apply(DeclaredRoster::complete([
        declared("listed"),
        declared("unlisted"),
    ]))
    .await;
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
    assert_eq!(
        retirement_causes(&mob.journal().await),
        vec![(id("unlisted"), Some(MemberRetirementCause::Unlisted))],
        "the start row records the cause"
    );

    let predating = predating_reader_journal(&mob.storage.events, None).await;
    let storage = MobStorage::with_events_and_runtime_metadata(
        predating,
        mob.storage.runtime_metadata.clone(),
    );
    mob.restart_into(storage, MobCompositionAuthority::Authoritative)
        .await;
    hold.release();
    let _ = tokio::time::timeout(STEP, sealing).await;

    mob.left("unlisted").await;
    assert_eq!(mob.standing("unlisted"), None);
    let member = mob
        .handle
        .list_members()
        .await
        .into_iter()
        .find(|member| member.agent_identity == id("listed"))
        .expect("the listed member is served");
    assert_eq!(member.status, MobMemberStatus::Active);
    mob.wake("listed").await;
}

/// A dropped apply never leaves the mob's applies refused. An opening whose
/// call is gone before the actor answers it is closed by the actor at once;
/// an apply dropped in its body is closed by the lease of its open fact. The
/// next apply opens either way.
///
/// RED: answer the opening with a bare verdict and arm the close guard in
/// the call once the answer arrives (the shape before this fix): the
/// opening whose call is gone leaves the open fact set, and the next apply
/// is refused `LifecycleOperationPending` for the life of the process.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dropped_apply_never_refuses_the_next_one() {
    let mob = ApplyMob::new("apply-dropped").await;

    // An opening whose call is gone before the parked actor answers it.
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
    let opening = mob
        .handle
        .enqueue_actor_command_for_test(|reply_tx| MobCommand::OpenDeclaredApply {
            close_authority: mob.handle.command_authority.clone(),
            reply_tx,
        })
        .await
        .expect("enqueue an opening behind the parked actor");
    drop(opening);
    let _ = release_tx.send(());
    let _ = parked.await;
    let opened = mob
        .apply(DeclaredRoster::partial([declared("after-dropped-opening")]))
        .await;
    assert!(
        matches!(
            answer(&opened, "after-dropped-opening"),
            DeclaredEntryOutcome::Seated { .. }
        ),
        "{opened:?}"
    );

    // An apply dropped in its body: it waits on the role read of an unseated
    // identity's archived session.
    mob.apply(DeclaredRoster::partial([declared("revived")]))
        .await;
    mob.wake("revived").await;
    tokio::time::timeout(STEP, mob.handle.retire(id("revived")))
        .await
        .expect("the retire answers")
        .expect("the retire succeeds");
    assert!(
        mob.archived_record("revived").is_some(),
        "the retirement archived the session"
    );
    let reads = mob.service.persisted_session_metadata_load_call_count();
    mob.service
        .set_persisted_session_metadata_delay_ms(3_600_000);
    let reading = tokio::spawn({
        let handle = mob.handle.clone();
        async move {
            handle
                .apply_declared_roster(DeclaredRoster::partial([declared("revived")]))
                .await
        }
    });
    let deadline = tokio::time::Instant::now() + STEP;
    while mob.service.persisted_session_metadata_load_call_count() == reads {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the apply never reached its role read"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    reading.abort();
    let _ = reading.await;
    mob.service.set_persisted_session_metadata_delay_ms(0);
    let opened = mob
        .apply(DeclaredRoster::partial([declared("after-dropped-body")]))
        .await;
    assert!(
        matches!(
            answer(&opened, "after-dropped-body"),
            DeclaredEntryOutcome::Seated { .. }
        ),
        "{opened:?}"
    );
}

/// Rollback of an unlisted retirement, lane-run leg: needs the published
/// Core 0.9.7 (`meerkat-mob` 0.9.7).
///
/// This build writes the fixture: a persistent mob store (SQLite) at
/// `$MEERKAT_ROLLBACK_FIXTURE_DIR/unlisted-retirement/mob.sqlite` holding
/// two identities the declared apply seated, and a complete apply that
/// unlisted one, interrupted after its retirement's durable start: the
/// start row carries the cause `unlisted`, and no terminal follows. The
/// lane resumes it with Core 0.9.7, whose event decoder ignores a key it
/// does not know (it rejects no unknown field), and checks: the resume
/// succeeds, the unlisted member's retirement is finished as a plain one,
/// and the listed member is listed `active` and woken by a delivery.
///
/// RED: a cause the 0.9.7 decoder cannot ignore (a stored key on a public
/// field, or a new event kind) fails the 0.9.7 resume of this fixture.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "lane:rollback - writes the fixture the published Core 0.9.7 resumes"]
async fn an_unlisted_retirement_start_row_is_read_by_097() {
    let dir = rollback_fixture_dir("unlisted-retirement");
    let handle = open_apply_fixture(&dir, "rollback-unlisted-retirement").await;
    handle
        .apply_declared_roster(DeclaredRoster::complete([
            declared("listed"),
            declared("unlisted"),
        ]))
        .await
        .expect("the apply runs");
    let hold = crate::runtime::actor::retirement_stage_hold::install(
        &id("unlisted"),
        "retire-runtime-quiesce",
    );
    let sealing = {
        let handle = handle.clone();
        tokio::spawn(async move {
            handle
                .apply_declared_roster(DeclaredRoster::complete([declared("listed")]))
                .await
        })
    };
    tokio::time::timeout(STEP, hold.entered())
        .await
        .expect("the unlisted retirement starts");
    crash_stop_and_release_routes(handle).await;
    hold.release();
    let _ = tokio::time::timeout(STEP, sealing).await;
}

/// Rollback of the declared apply's rows, in-process leg: they are plain
/// member rows. The apply writes no new event kind and no identity intent
/// row: its seats are `MemberSpawned` rows and its unlisted retirement a
/// `MemberRetirementStarted` and a `MemberRetired` row, whose internal
/// fields (the entry, the cause, the history notice) a predating reader
/// ignores. Read so, the members are served as ordinary members: listed
/// active and woken by a delivery. The lane-run leg against the published
/// Core is `declared_apply_rows_are_served_by_097` below; the leg of Meta
/// 0.9.6's roster pass over them belongs with the Meta change that moves
/// the roster pass onto the declared apply.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn declared_apply_rows_are_plain_members_to_096() {
    let mut mob = ApplyMob::new("apply-rollback-plain").await;
    let mut written = Vec::new();
    let before = mob.journal().await.len();
    mob.apply(DeclaredRoster::complete([
        declared("plain-a"),
        declared("plain-b"),
        declared("unlisted"),
    ]))
    .await;
    written.extend(mob.journal().await.into_iter().skip(before));
    mob.wake("plain-a").await;
    let before = mob.journal().await.len();
    let sealed = mob
        .apply(DeclaredRoster::complete([
            declared("plain-a"),
            declared("plain-b"),
        ]))
        .await;
    assert!(
        matches!(
            sealed.unlisted.get(&id("unlisted")),
            Some(UnlistedEntryOutcome::Retired)
        ),
        "{sealed:?}"
    );
    written.extend(mob.journal().await.into_iter().skip(before));
    let kinds: BTreeSet<String> = written
        .iter()
        .map(|event| {
            serde_json::to_value(&event.kind).expect("encode the event kind")["type"]
                .as_str()
                .expect("a tagged event kind")
                .to_string()
        })
        .collect();
    assert_eq!(
        kinds,
        BTreeSet::from([
            "member_retired".to_string(),
            "member_retirement_started".to_string(),
            "member_spawned".to_string(),
        ]),
        "the apply writes plain member rows only"
    );
    for name in ["plain-a", "plain-b"] {
        assert_eq!(
            mob.handle
                .identity_management(&id(name))
                .await
                .expect("the identity store answers"),
            MemberIdentityManagement::CommandDriven,
            "{name} has no identity intent row"
        );
    }

    let predating = predating_reader_journal(&mob.storage.events, None).await;
    let storage = MobStorage::with_events_and_runtime_metadata(
        predating,
        mob.storage.runtime_metadata.clone(),
    );
    mob.restart_into(storage, MobCompositionAuthority::Authoritative)
        .await;
    let members = mob.handle.list_members().await;
    for name in ["plain-a", "plain-b"] {
        let member = members
            .iter()
            .find(|member| member.agent_identity == id(name))
            .unwrap_or_else(|| panic!("{name} is listed"));
        assert_eq!(member.status, MobMemberStatus::Active, "{name}");
        assert_eq!(
            mob.standing(name),
            None,
            "{name} reads as an ordinary member"
        );
    }
    assert!(!mob.is_member("unlisted").await);
    mob.wake("plain-b").await;
}

/// Rollback of the declared apply's rows, lane-run leg: needs the
/// published Core 0.9.7 (`meerkat-mob` 0.9.7).
///
/// This build writes the fixture: a persistent mob store (SQLite) at
/// `$MEERKAT_ROLLBACK_FIXTURE_DIR/declared-apply/mob.sqlite` holding three
/// identities the declared apply seated and the complete apply that
/// unlisted one, retired. The lane resumes it with Core 0.9.7 and checks:
/// the two listed members are listed `active` and woken by a delivery, and
/// the unlisted one is no member.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "lane:rollback - writes the fixture the published Core 0.9.7 resumes"]
async fn declared_apply_rows_are_served_by_097() {
    let dir = rollback_fixture_dir("declared-apply");
    write_rollback_fixture(&dir, "rollback-declared-apply", |handle| async move {
        handle
            .apply_declared_roster(DeclaredRoster::complete([
                declared("plain-a"),
                declared("plain-b"),
                declared("unlisted"),
            ]))
            .await
            .expect("the apply runs");
        let sealed = handle
            .apply_declared_roster(DeclaredRoster::complete([
                declared("plain-a"),
                declared("plain-b"),
            ]))
            .await
            .expect("the apply runs");
        assert!(
            matches!(
                sealed.unlisted.get(&id("unlisted")),
                Some(UnlistedEntryOutcome::Retired)
            ),
            "{sealed:?}"
        );
    })
    .await;
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
async fn open_apply_fixture(dir: &std::path::Path, mob_id: &str) -> MobHandle {
    let service = Arc::new(MockSessionService::new());
    let _adapter = service.enable_runtime_adapter();
    let mut definition = sample_definition();
    definition.id = MobId::from(mob_id);
    MobBuilder::new(
        definition,
        MobStorage::persistent(dir.join("mob.sqlite")).expect("open the fixture store"),
    )
    .with_session_service(service)
    .with_member_comms_identity_root(dir.join("comms-keys"))
    .create()
    .await
    .expect("create the fixture mob")
}
