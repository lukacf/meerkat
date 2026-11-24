//! Focused behavioural tests for the canonical `MobStoreRestoreLifecycleMachine`.
//!
//! The machine is record-scoped: every test drives ONE mob store's record
//! through its stay in a host's set. It reads no clock and never loads,
//! re-attempts or unloads on its own: the shell asks, and reports how each
//! load, re-attempt or unload ended. The machine decides which requests it
//! admits, and whether the mob's handle may be handed out.
//!
//! [`every_input_in_every_phase_is_total_and_only_mutates_where_intended`]
//! drives every input against every phase and asserts the one intended
//! transition or a byte-identical refusal.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use meerkat_machine_schema::catalog::dsl::mob_store_restore_lifecycle::{
    MobQuarantineClass, MobStoreCreateOutcome, MobStoreLoadTrigger, MobStoreOrigin,
    MobStoreRestoreLifecycleEffect, MobStoreRestoreLifecycleInput,
    MobStoreRestoreLifecycleMachineAuthority, MobStoreRestoreLifecycleMachineMutator,
    MobStoreRestoreLifecycleMachineTransition, MobStoreRestoreLifecyclePhase,
    MobStoreRestoreOutcome, MobStoreRestoreRefusal, MobStoreRetryTrigger, MobStoreStop,
    MobStoreUnloadOutcome,
};

type Authority = MobStoreRestoreLifecycleMachineAuthority;
type Input = MobStoreRestoreLifecycleInput;
type Effect = MobStoreRestoreLifecycleEffect;
type Phase = MobStoreRestoreLifecyclePhase;
type Transition = MobStoreRestoreLifecycleMachineTransition;
type Trigger = MobStoreRetryTrigger;
type LoadTrigger = MobStoreLoadTrigger;
type Outcome = MobStoreRestoreOutcome;
type Origin = MobStoreOrigin;
type Refusal = MobStoreRestoreRefusal;
type Unload = MobStoreUnloadOutcome;
type Create = MobStoreCreateOutcome;
type Stop = MobStoreStop;

fn apply(authority: &mut Authority, input: Input) -> Transition {
    MobStoreRestoreLifecycleMachineMutator::apply(authority, input)
        .expect("every input resolves to an explicit typed transition")
}

fn only_effect(transition: &Transition) -> Effect {
    let effects = transition.effects().to_vec();
    assert_eq!(
        effects.len(),
        1,
        "every transition emits exactly one verdict, got {effects:?}"
    );
    effects[0].clone()
}

/// A host's handle over a record whose mob, if it manages one, still runs.
fn register() -> Input {
    Input::Register {
        holder_ended: false,
    }
}

/// A host's handle that says the mob the record manages has ended.
fn register_over_ended() -> Input {
    Input::Register { holder_ended: true }
}

fn request_load(trigger: LoadTrigger) -> Input {
    Input::RequestLoad { trigger }
}

fn declare(vacant: bool) -> Input {
    Input::Declare { vacant }
}

fn resolve_load(outcome: Outcome, class: MobQuarantineClass) -> Input {
    Input::ResolveLoad { outcome, class }
}

fn request_retry(trigger: Trigger) -> Input {
    Input::RequestRetry { trigger }
}

fn resolve_retry(outcome: Outcome, class: MobQuarantineClass) -> Input {
    Input::ResolveRetry { outcome, class }
}

fn release_held(outcome: Outcome) -> Input {
    Input::ReleaseHeld { outcome }
}

/// The actor's answer to an admitted unload, with the mob's stop not read.
fn resolve_unload(outcome: Unload) -> Input {
    resolve_unload_with(outcome, Stop::Unread)
}

fn resolve_unload_with(outcome: Unload, stop: Stop) -> Input {
    Input::ResolveUnload { outcome, stop }
}

fn read_stop(stopped: bool) -> Input {
    Input::ReadStop { stopped }
}

fn request_create(launch: bool) -> Input {
    Input::RequestCreate { launch }
}

fn resolve_create(outcome: Create, in_memory: bool) -> Input {
    Input::ResolveCreate { outcome, in_memory }
}

/// The record the set files for a create, with that create in flight.
fn create_in_flight() -> Authority {
    let mut authority = Authority::new();
    let admitted = apply(&mut authority, request_create(false));
    assert!(matches!(only_effect(&admitted), Effect::CreateAdmitted));
    assert_eq!(authority.state().lifecycle_phase, Phase::Creating);
    authority
}

/// The record of a mob the set created, managed: with a store file, or in
/// memory.
fn created_by_the_set(in_memory: bool) -> Authority {
    let mut authority = create_in_flight();
    apply(&mut authority, resolve_create(Create::Created, in_memory));
    assert_eq!(authority.state().lifecycle_phase, Phase::Managed);
    authority
}

/// The declared record of a store that holds no mob, with the launch's
/// create of its mob in flight.
fn launch_create_in_flight() -> Authority {
    let mut authority = Authority::new();
    apply(&mut authority, declare(true));
    let admitted = apply(&mut authority, request_create(true));
    assert!(matches!(only_effect(&admitted), Effect::CreateAdmitted));
    assert_eq!(authority.state().lifecycle_phase, Phase::Creating);
    authority
}

/// Build an authority parked in `phase`; a quarantine it passes through is
/// one that could not serve.
fn authority_in(phase: Phase) -> Authority {
    authority_in_class(phase, MobQuarantineClass::StoreUnavailable)
}

/// Build the authority of a persisted store parked in `phase`, quarantined
/// under `class` where the phase holds a quarantine; a re-attempt it needs
/// is an explicit one.
fn authority_in_class(phase: Phase, class: MobQuarantineClass) -> Authority {
    // A create's record is filed for the create alone: no census listed it.
    if phase == Phase::Creating {
        return create_in_flight();
    }
    let mut authority = Authority::new();
    apply(&mut authority, Input::Census);
    match phase {
        Phase::Dormant | Phase::Creating => {}
        Phase::Loading => {
            apply(&mut authority, request_load(LoadTrigger::Lookup));
        }
        Phase::Managed => {
            apply(&mut authority, request_load(LoadTrigger::Lookup));
            apply(&mut authority, resolve_load(Outcome::Managed, class));
        }
        Phase::Unloading => {
            apply(&mut authority, request_load(LoadTrigger::Lookup));
            apply(&mut authority, resolve_load(Outcome::Managed, class));
            apply(&mut authority, Input::RequestUnload);
        }
        Phase::Held => {
            apply(&mut authority, request_load(LoadTrigger::Lookup));
            apply(&mut authority, resolve_load(Outcome::Held, class));
        }
        Phase::Quarantined => {
            apply(&mut authority, request_load(LoadTrigger::Lookup));
            apply(&mut authority, resolve_load(Outcome::Failed, class));
        }
        Phase::Retrying => {
            apply(&mut authority, request_load(LoadTrigger::Lookup));
            apply(&mut authority, resolve_load(Outcome::Failed, class));
            apply(&mut authority, request_retry(Trigger::Explicit));
        }
        Phase::Gone => {
            apply(&mut authority, request_load(LoadTrigger::Lookup));
            apply(&mut authority, resolve_load(Outcome::Managed, class));
            apply(&mut authority, Input::Destroy);
        }
    }
    assert_eq!(authority.state().lifecycle_phase, phase);
    authority
}

const ALL_PHASES: [Phase; 9] = [
    Phase::Dormant,
    Phase::Loading,
    Phase::Managed,
    Phase::Unloading,
    Phase::Held,
    Phase::Quarantined,
    Phase::Retrying,
    Phase::Gone,
    Phase::Creating,
];

const ALL_CLASSES: [MobQuarantineClass; 3] = [
    MobQuarantineClass::StoreUnavailable,
    MobQuarantineClass::StoreUnreadable,
    MobQuarantineClass::ResumeRefused,
];

const ALL_TRIGGERS: [Trigger; 2] = [Trigger::Explicit, Trigger::OnDemand];

fn assert_refused(transition: &Transition, reason: Refusal) {
    assert!(
        matches!(
            only_effect(transition),
            Effect::RequestRefused { reason: refused } if refused == reason
        ),
        "expected the typed refusal {reason:?}, got {:?}",
        transition.effects()
    );
}

/// `input` is refused with `reason` in every phase of `phases`, and changes
/// nothing there.
fn assert_refused_unchanged(phases: &[Phase], input: fn() -> Input, reason: Refusal) {
    for &phase in phases {
        let mut authority = authority_in(phase);
        let before = authority.state().clone();
        let refused = apply(&mut authority, input());
        assert_refused(&refused, reason);
        assert_eq!(
            authority.state(),
            &before,
            "a refusal in {phase:?} changes nothing"
        );
    }
}

#[test]
fn a_census_lists_a_dormant_store_and_finds_a_quarantined_one_afresh() {
    let mut authority = Authority::new();
    assert_eq!(authority.state().lifecycle_phase, Phase::Dormant);
    assert_eq!(authority.state().attempts, 0);
    assert_eq!(authority.state().origin, Origin::Persisted);
    let listed = apply(&mut authority, Input::Census);
    assert!(matches!(only_effect(&listed), Effect::StoreListed));
    assert_eq!(authority.state().lifecycle_phase, Phase::Dormant);
    let again = apply(&mut authority, Input::Census);
    assert!(
        matches!(only_effect(&again), Effect::StoreListed),
        "a later pass lists a dormant store again"
    );

    // A later pass finds a quarantined store afresh: dormant, no class, no
    // attempt counted.
    let mut quarantined = authority_in_class(Phase::Retrying, MobQuarantineClass::StoreUnreadable);
    apply(
        &mut quarantined,
        resolve_retry(Outcome::Failed, MobQuarantineClass::ResumeRefused),
    );
    assert_eq!(quarantined.state().attempts, 1);
    let found = apply(&mut quarantined, Input::Census);
    assert!(matches!(only_effect(&found), Effect::StoreListed));
    assert_eq!(quarantined.state().lifecycle_phase, Phase::Dormant);
    assert_eq!(
        quarantined.state().class,
        MobQuarantineClass::StoreUnavailable
    );
    assert_eq!(quarantined.state().attempts, 0);

    assert_refused_unchanged(
        &[
            Phase::Loading,
            Phase::Managed,
            Phase::Unloading,
            Phase::Held,
            Phase::Retrying,
        ],
        || Input::Census,
        Refusal::AlreadyHosted,
    );
    assert_refused_unchanged(&[Phase::Gone], || Input::Census, Refusal::Gone);
}

/// The records of the mobs the set itself runs: a store it loaded, and a
/// mob it created, with a store file and in memory.
fn the_sets_own_mobs() -> Vec<Authority> {
    vec![
        authority_in(Phase::Managed),
        created_by_the_set(false),
        created_by_the_set(true),
    ]
}

#[test]
fn a_mob_built_outside_a_load_joins_the_set_as_managed() {
    // A host's handle on a record that stands for nothing yet, and on one
    // a census listed: the record is managed, served, and the host's.
    for mut authority in [Authority::new(), authority_in(Phase::Dormant)] {
        let registered = apply(&mut authority, register());
        assert!(matches!(
            only_effect(&registered),
            Effect::StoreRegistered {
                origin: Origin::HostHandle
            }
        ));
        assert_eq!(authority.state().lifecycle_phase, Phase::Managed);
        assert_eq!(authority.state().origin, Origin::HostHandle);
        assert!(authority.state().served);
    }

    // A host's handle replaces the handle of a mob a host inserted: the
    // host inserts its own mob again, whether or not the first has ended.
    for again in [register as fn() -> Input, register_over_ended] {
        let mut inserted = Authority::new();
        apply(&mut inserted, register());
        let before = inserted.state().clone();
        let replaced = apply(&mut inserted, again());
        assert!(matches!(
            only_effect(&replaced),
            Effect::StoreRegistered {
                origin: Origin::HostHandle
            }
        ));
        assert_eq!(
            inserted.state(),
            &before,
            "the record stays the host's and managed"
        );
    }

    // A mob the set itself runs keeps its id against a host's handle while
    // it runs. Its actor runs for the set, so a handle registered over it
    // would leave that actor with nobody holding it.
    for mut managed in the_sets_own_mobs() {
        let before = managed.state().clone();
        let refused = apply(&mut managed, register());
        assert_refused(&refused, Refusal::AlreadyHosted);
        assert_eq!(managed.state(), &before);
    }
    // Once the mob the set ran has ended (its actor is over) nothing is
    // left to hold: the host's handle takes the record, whether the set
    // loaded that mob or created it. The record is the host's from then
    // on, and stays managed and served.
    for mut record in the_sets_own_mobs() {
        let taken = apply(&mut record, register_over_ended());
        assert!(matches!(
            only_effect(&taken),
            Effect::StoreRegistered {
                origin: Origin::HostHandle
            }
        ));
        assert_eq!(record.state().lifecycle_phase, Phase::Managed);
        assert_eq!(record.state().origin, Origin::HostHandle);
        assert!(record.state().served);
    }

    // In every other phase the word that the holder has ended changes
    // nothing: the record answers as it does without it. A store that is
    // loading, held, quarantined or re-attempted, and a create in flight,
    // keep the id; nothing registers over an admitted unload; a record
    // that is gone admits nothing.
    for input in [register as fn() -> Input, register_over_ended] {
        assert_refused_unchanged(
            &[
                Phase::Loading,
                Phase::Held,
                Phase::Quarantined,
                Phase::Retrying,
                Phase::Creating,
            ],
            input,
            Refusal::AlreadyHosted,
        );
        assert_refused_unchanged(&[Phase::Unloading], input, Refusal::UnloadInFlight);
        assert_refused_unchanged(&[Phase::Gone], input, Refusal::Gone);
    }
}

#[test]
fn one_load_is_in_flight_at_a_time() {
    let mut authority = authority_in(Phase::Dormant);
    let admitted = apply(&mut authority, request_load(LoadTrigger::Lookup));
    assert!(matches!(only_effect(&admitted), Effect::LoadAdmitted));
    assert_eq!(authority.state().lifecycle_phase, Phase::Loading);

    // A second request is refused at once: its caller waits for the first.
    let before = authority.state().clone();
    let second = apply(&mut authority, request_load(LoadTrigger::Lookup));
    assert_refused(&second, Refusal::LoadInFlight);
    assert_eq!(authority.state(), &before);

    assert_refused_unchanged(
        &[Phase::Managed, Phase::Held, Phase::Quarantined],
        || request_load(LoadTrigger::Lookup),
        Refusal::NotDormant,
    );
    assert_refused_unchanged(
        &[Phase::Retrying],
        || request_load(LoadTrigger::Lookup),
        Refusal::RetryInFlight,
    );
    // A lookup that meets an admitted unload waits for its answer.
    assert_refused_unchanged(
        &[Phase::Unloading],
        || request_load(LoadTrigger::Lookup),
        Refusal::UnloadInFlight,
    );
    assert_refused_unchanged(
        &[Phase::Gone],
        || request_load(LoadTrigger::Lookup),
        Refusal::Gone,
    );
}

#[test]
fn a_load_resolves_into_the_phase_its_outcome_names() {
    for (outcome, phase) in [
        (Outcome::Managed, Phase::Managed),
        (Outcome::Held, Phase::Held),
        (Outcome::Settled, Phase::Gone),
    ] {
        let mut authority = authority_in(Phase::Loading);
        // The class of a load that did not fail is ignored.
        let loaded = apply(
            &mut authority,
            resolve_load(outcome, MobQuarantineClass::ResumeRefused),
        );
        assert!(matches!(
            only_effect(&loaded),
            Effect::StoreLoaded { outcome: resolved } if resolved == outcome
        ));
        assert_eq!(authority.state().lifecycle_phase, phase);
        assert_eq!(
            authority.state().class,
            MobQuarantineClass::StoreUnavailable
        );
        assert_eq!(authority.state().attempts, 0, "a load is not a re-attempt");
    }

    for class in ALL_CLASSES {
        let mut authority = authority_in(Phase::Loading);
        let failed = apply(&mut authority, resolve_load(Outcome::Failed, class));
        assert!(matches!(
            only_effect(&failed),
            Effect::StoreQuarantined { class: met } if met == class
        ));
        assert_eq!(authority.state().lifecycle_phase, Phase::Quarantined);
        assert_eq!(authority.state().class, class);
        assert_eq!(authority.state().attempts, 0);
    }

    assert_refused_unchanged(
        &[
            Phase::Dormant,
            Phase::Managed,
            Phase::Unloading,
            Phase::Held,
            Phase::Quarantined,
            Phase::Retrying,
        ],
        || resolve_load(Outcome::Managed, MobQuarantineClass::StoreUnavailable),
        Refusal::NotLoading,
    );
    assert_refused_unchanged(
        &[Phase::Gone],
        || resolve_load(Outcome::Managed, MobQuarantineClass::StoreUnavailable),
        Refusal::Gone,
    );
}

#[test]
fn one_re_attempt_is_in_flight_at_a_time() {
    let mut authority = authority_in(Phase::Quarantined);
    let admitted = apply(&mut authority, request_retry(Trigger::Explicit));
    assert!(matches!(
        only_effect(&admitted),
        Effect::RetryAdmitted { attempts: 0 }
    ));
    assert_eq!(authority.state().lifecycle_phase, Phase::Retrying);

    // A second request is refused at once: it never waits for the first.
    for trigger in ALL_TRIGGERS {
        let before = authority.state().clone();
        let second = apply(&mut authority, request_retry(trigger));
        assert_refused(&second, Refusal::RetryInFlight);
        assert_eq!(authority.state(), &before);
    }

    assert_refused_unchanged(
        &[
            Phase::Dormant,
            Phase::Loading,
            Phase::Managed,
            Phase::Unloading,
            Phase::Held,
        ],
        || request_retry(Trigger::Explicit),
        Refusal::NotQuarantined,
    );
    assert_refused_unchanged(
        &[Phase::Gone],
        || request_retry(Trigger::Explicit),
        Refusal::Gone,
    );
}

#[test]
fn an_on_demand_re_attempt_is_admitted_only_for_a_store_that_could_not_serve() {
    for class in ALL_CLASSES {
        let mut explicit = authority_in_class(Phase::Quarantined, class);
        let admitted = apply(&mut explicit, request_retry(Trigger::Explicit));
        assert!(
            matches!(only_effect(&admitted), Effect::RetryAdmitted { .. }),
            "an explicit re-attempt is admitted for {class:?}"
        );

        let mut on_demand = authority_in_class(Phase::Quarantined, class);
        let before = on_demand.state().clone();
        let answer = apply(&mut on_demand, request_retry(Trigger::OnDemand));
        if class == MobQuarantineClass::StoreUnavailable {
            assert!(matches!(only_effect(&answer), Effect::RetryAdmitted { .. }));
            assert_eq!(on_demand.state().lifecycle_phase, Phase::Retrying);
        } else {
            assert_refused(&answer, Refusal::NotRetryable);
            assert_eq!(
                on_demand.state(),
                &before,
                "a store that needs a repair is not re-attempted on demand"
            );
        }
    }
}

#[test]
fn a_re_attempt_releases_the_store_or_keeps_it_quarantined() {
    for (outcome, phase) in [
        (Outcome::Managed, Phase::Managed),
        (Outcome::Held, Phase::Held),
        (Outcome::Settled, Phase::Gone),
    ] {
        let mut authority =
            authority_in_class(Phase::Retrying, MobQuarantineClass::StoreUnreadable);
        let released = apply(
            &mut authority,
            resolve_retry(outcome, MobQuarantineClass::ResumeRefused),
        );
        assert!(matches!(
            only_effect(&released),
            Effect::StoreReleased { outcome: resolved, attempts: 1 } if resolved == outcome
        ));
        assert_eq!(authority.state().lifecycle_phase, phase);
        assert_eq!(
            authority.state().class,
            MobQuarantineClass::StoreUnavailable,
            "a released store holds no class"
        );
        assert_eq!(authority.state().attempts, 1);
    }

    // A failure keeps the store quarantined under the class it met, and
    // every ended attempt is counted.
    let mut authority = authority_in_class(Phase::Retrying, MobQuarantineClass::StoreUnavailable);
    let failed = apply(
        &mut authority,
        resolve_retry(Outcome::Failed, MobQuarantineClass::StoreUnreadable),
    );
    assert!(matches!(
        only_effect(&failed),
        Effect::StoreStillQuarantined {
            class: MobQuarantineClass::StoreUnreadable,
            attempts: 1
        }
    ));
    assert_eq!(authority.state().lifecycle_phase, Phase::Quarantined);
    assert_eq!(authority.state().class, MobQuarantineClass::StoreUnreadable);
    apply(&mut authority, request_retry(Trigger::Explicit));
    let again = apply(
        &mut authority,
        resolve_retry(Outcome::Failed, MobQuarantineClass::StoreUnreadable),
    );
    assert!(matches!(
        only_effect(&again),
        Effect::StoreStillQuarantined { attempts: 2, .. }
    ));

    assert_refused_unchanged(
        &[
            Phase::Dormant,
            Phase::Loading,
            Phase::Managed,
            Phase::Unloading,
            Phase::Held,
            Phase::Quarantined,
        ],
        || resolve_retry(Outcome::Managed, MobQuarantineClass::StoreUnavailable),
        Refusal::NotRetrying,
    );
    assert_refused_unchanged(
        &[Phase::Gone],
        || resolve_retry(Outcome::Managed, MobQuarantineClass::StoreUnavailable),
        Refusal::Gone,
    );
}

#[test]
fn an_unload_is_admitted_only_for_a_store_the_set_can_load_again() {
    // A persisted store: the unload is admitted before the actor is asked,
    // and from that step the mob's handle is not handed out.
    let mut authority = authority_in(Phase::Managed);
    assert!(authority.state().served);
    let admitted = apply(&mut authority, Input::RequestUnload);
    assert!(matches!(only_effect(&admitted), Effect::UnloadAdmitted));
    assert_eq!(authority.state().lifecycle_phase, Phase::Unloading);
    assert!(
        !authority.state().served,
        "a record whose unload was admitted is not handed out"
    );

    // So is a store a create of this host wrote.
    let mut created = created_by_the_set(false);
    let admitted = apply(&mut created, Input::RequestUnload);
    assert!(matches!(only_effect(&admitted), Effect::UnloadAdmitted));

    // A volatile store and a host's own handle are never asked: there would
    // be nothing to load, and the question alone would close the mob.
    let mut inserted = Authority::new();
    apply(&mut inserted, register());
    for mut authority in [created_by_the_set(true), inserted] {
        let before = authority.state().clone();
        let refused = apply(&mut authority, Input::RequestUnload);
        assert_refused(&refused, Refusal::NotReloadable);
        assert_eq!(authority.state(), &before);
        assert!(authority.state().served);
    }

    // One unload at a time.
    assert_refused_unchanged(
        &[Phase::Unloading],
        || Input::RequestUnload,
        Refusal::UnloadInFlight,
    );
    assert_refused_unchanged(
        &[
            Phase::Dormant,
            Phase::Loading,
            Phase::Held,
            Phase::Quarantined,
            Phase::Retrying,
        ],
        || Input::RequestUnload,
        Refusal::NotManaged,
    );
    assert_refused_unchanged(&[Phase::Gone], || Input::RequestUnload, Refusal::Gone);
}

#[test]
fn an_admitted_unload_resolves_as_the_actor_answered() {
    // The actor closed idle and was shut down: dormant again, and loadable
    // again.
    let mut authority = authority_in(Phase::Unloading);
    let unloaded = apply(&mut authority, resolve_unload(Unload::Closed));
    assert!(matches!(only_effect(&unloaded), Effect::StoreUnloaded));
    assert_eq!(authority.state().lifecycle_phase, Phase::Dormant);
    assert!(!authority.state().served);
    let reloaded = apply(&mut authority, request_load(LoadTrigger::Lookup));
    assert!(matches!(only_effect(&reloaded), Effect::LoadAdmitted));

    // The mob was busy: managed again, and handed out again.
    let mut authority = authority_in(Phase::Unloading);
    let declined = apply(&mut authority, resolve_unload(Unload::Busy));
    assert!(matches!(only_effect(&declined), Effect::UnloadDeclined));
    assert_eq!(authority.state().lifecycle_phase, Phase::Managed);
    assert!(authority.state().served);

    for outcome in [Unload::Closed, Unload::Busy] {
        for phase in [
            Phase::Dormant,
            Phase::Loading,
            Phase::Managed,
            Phase::Held,
            Phase::Quarantined,
            Phase::Retrying,
        ] {
            let mut authority = authority_in(phase);
            let before = authority.state().clone();
            let refused = apply(&mut authority, resolve_unload(outcome));
            assert_refused(&refused, Refusal::NotUnloading);
            assert_eq!(authority.state(), &before);
        }
    }
    assert_refused_unchanged(
        &[Phase::Gone],
        || resolve_unload(Unload::Closed),
        Refusal::Gone,
    );
}

#[test]
fn a_mob_is_handed_out_only_while_its_record_is_managed() {
    for phase in ALL_PHASES {
        for class in ALL_CLASSES {
            let authority = authority_in_class(phase, class);
            assert_eq!(
                authority.state().served,
                phase == Phase::Managed,
                "{phase:?} ({class:?})"
            );
        }
    }
    // Every way into Managed hands the mob out; a destroy stops it.
    let mut released = authority_in(Phase::Held);
    apply(&mut released, release_held(Outcome::Managed));
    assert!(released.state().served);
    let mut retried = authority_in(Phase::Retrying);
    apply(
        &mut retried,
        resolve_retry(Outcome::Managed, MobQuarantineClass::StoreUnavailable),
    );
    assert!(retried.state().served);
    let mut destroyed = authority_in(Phase::Managed);
    apply(&mut destroyed, Input::Destroy);
    assert!(!destroyed.state().served);
}

#[test]
fn a_held_mob_leaves_its_hold_in_one_step() {
    for (outcome, phase) in [
        (Outcome::Managed, Phase::Managed),
        (Outcome::Settled, Phase::Gone),
    ] {
        let mut authority = authority_in(Phase::Held);
        let released = apply(&mut authority, release_held(outcome));
        assert!(matches!(
            only_effect(&released),
            Effect::HoldReleased { outcome: resolved } if resolved == outcome
        ));
        assert_eq!(authority.state().lifecycle_phase, phase);
    }

    // A release that did not resolve the hold leaves the mob held.
    for outcome in [Outcome::Held, Outcome::Failed] {
        let mut authority = authority_in(Phase::Held);
        let before = authority.state().clone();
        let kept = apply(&mut authority, release_held(outcome));
        assert_refused(&kept, Refusal::HoldKept);
        assert_eq!(authority.state(), &before);
    }

    assert_refused_unchanged(
        &[
            Phase::Dormant,
            Phase::Loading,
            Phase::Managed,
            Phase::Unloading,
            Phase::Quarantined,
            Phase::Retrying,
        ],
        || release_held(Outcome::Managed),
        Refusal::NotHeld,
    );
    assert_refused_unchanged(
        &[Phase::Gone],
        || release_held(Outcome::Managed),
        Refusal::Gone,
    );
}

#[test]
fn a_destroyed_mob_is_gone_for_good() {
    let mut authority = authority_in(Phase::Managed);
    let gone = apply(&mut authority, Input::Destroy);
    assert!(matches!(only_effect(&gone), Effect::StoreGone));
    assert_eq!(authority.state().lifecycle_phase, Phase::Gone);

    assert_refused_unchanged(
        &[
            Phase::Dormant,
            Phase::Loading,
            Phase::Held,
            Phase::Quarantined,
            Phase::Retrying,
        ],
        || Input::Destroy,
        Refusal::NotManaged,
    );
    // A destroy that meets an admitted unload waits for its answer.
    assert_refused_unchanged(
        &[Phase::Unloading],
        || Input::Destroy,
        Refusal::UnloadInFlight,
    );
    assert_refused_unchanged(&[Phase::Gone], || Input::Destroy, Refusal::Gone);
}

#[test]
fn every_input_in_every_phase_is_total_and_only_mutates_where_intended() {
    let inputs: [(&str, fn() -> Input); 25] = [
        ("Census", || Input::Census),
        ("Declare", || declare(false)),
        ("EndLaunchWait", || Input::EndLaunchWait),
        ("Register", register),
        ("RegisterOverEnded", register_over_ended),
        ("RequestCreate", || request_create(false)),
        ("RequestCreateLaunch", || request_create(true)),
        ("ResolveCreateCreated", || {
            resolve_create(Create::Created, false)
        }),
        ("ResolveCreateFailed", || {
            resolve_create(Create::Failed, true)
        }),
        ("ReadStop", || read_stop(true)),
        // The record's stop is unread: a demand is told to read first.
        ("RequestLoadDemand", || request_load(LoadTrigger::Demand)),
        ("RequestLoad", || request_load(LoadTrigger::Lookup)),
        ("ResolveLoadManaged", || {
            resolve_load(Outcome::Managed, MobQuarantineClass::StoreUnavailable)
        }),
        ("ResolveLoadFailed", || {
            resolve_load(Outcome::Failed, MobQuarantineClass::StoreUnreadable)
        }),
        ("RequestRetryExplicit", || request_retry(Trigger::Explicit)),
        ("RequestRetryOnDemand", || request_retry(Trigger::OnDemand)),
        ("ResolveRetryFailed", || {
            resolve_retry(Outcome::Failed, MobQuarantineClass::StoreUnreadable)
        }),
        ("ResolveRetryManaged", || {
            resolve_retry(Outcome::Managed, MobQuarantineClass::StoreUnavailable)
        }),
        ("RequestUnload", || Input::RequestUnload),
        ("ResolveUnloadClosed", || resolve_unload(Unload::Closed)),
        ("ResolveUnloadBusy", || resolve_unload(Unload::Busy)),
        ("ReleaseHeldManaged", || release_held(Outcome::Managed)),
        ("ReleaseHeldFailed", || release_held(Outcome::Failed)),
        ("ReleaseHeldSettled", || release_held(Outcome::Settled)),
        ("Destroy", || Input::Destroy),
    ];
    for class in ALL_CLASSES {
        for phase in ALL_PHASES {
            for (name, input) in inputs {
                let mut authority = authority_in_class(phase, class);
                let before = authority.state().clone();
                let transition = apply(&mut authority, input());
                // The one intended transition of each admitted pair. Two of
                // them keep the state as it is and are told from a refusal
                // by their effect.
                let admitted = match phase {
                    Phase::Dormant => matches!(
                        name,
                        "Census"
                            | "Declare"
                            | "Register"
                            | "RegisterOverEnded"
                            | "RequestLoad"
                            // The record is one a census listed: it stands
                            // for a store, so a create under it is refused
                            // (and so is one that names the launch: the
                            // record is not a declared one).
                            | "ReadStop"
                    ),
                    Phase::Loading => matches!(name, "ResolveLoadManaged" | "ResolveLoadFailed"),
                    // The record is a store the set loaded: a host's handle
                    // does not register over it while its mob runs, and
                    // takes it once that mob has ended.
                    Phase::Managed => {
                        matches!(name, "RequestUnload" | "Destroy" | "RegisterOverEnded")
                    }
                    Phase::Unloading => {
                        matches!(name, "ResolveUnloadClosed" | "ResolveUnloadBusy")
                    }
                    Phase::Held => matches!(name, "ReleaseHeldManaged" | "ReleaseHeldSettled"),
                    Phase::Quarantined => match name {
                        "Census" | "RequestRetryExplicit" => true,
                        // On demand, only a store that could not serve now.
                        "RequestRetryOnDemand" => class == MobQuarantineClass::StoreUnavailable,
                        _ => false,
                    },
                    Phase::Retrying => matches!(name, "ResolveRetryFailed" | "ResolveRetryManaged"),
                    Phase::Gone => false,
                    // Only the create's own end moves the record.
                    Phase::Creating => {
                        matches!(name, "ResolveCreateCreated" | "ResolveCreateFailed")
                    }
                };
                let refused = matches!(only_effect(&transition), Effect::RequestRefused { .. });
                assert_eq!(
                    admitted, !refused,
                    "{name} in {phase:?} ({class:?}): admitted pairs answer their verdict, every other pair a typed refusal"
                );
                if refused {
                    assert_eq!(
                        authority.state(),
                        &before,
                        "{name} in {phase:?} ({class:?}) is a refusal that changes nothing"
                    );
                }
            }
        }
    }
}

/// A store the host declared, parked where its launch's first serve finds
/// it: dormant and awaiting the launch.
fn declared_store(vacant: bool) -> Authority {
    let mut authority = Authority::new();
    let declared = apply(&mut authority, declare(vacant));
    assert!(matches!(only_effect(&declared), Effect::LaunchDeclared));
    let state = authority.state();
    assert_eq!(state.lifecycle_phase, Phase::Dormant);
    assert_eq!(state.origin, Origin::Declared);
    assert!(state.awaits_launch);
    assert_eq!(state.vacant, vacant);
    authority
}

/// A declared store in `phase`, still awaiting its launch: the launch's own
/// load put it there.
fn declared_store_in(phase: Phase) -> Authority {
    let mut authority = declared_store(false);
    let unavailable = MobQuarantineClass::StoreUnavailable;
    match phase {
        Phase::Dormant => {}
        Phase::Loading => {
            apply(&mut authority, request_load(LoadTrigger::Launch));
        }
        Phase::Managed => {
            apply(&mut authority, request_load(LoadTrigger::Launch));
            apply(&mut authority, resolve_load(Outcome::Managed, unavailable));
        }
        Phase::Held => {
            apply(&mut authority, request_load(LoadTrigger::Launch));
            apply(&mut authority, resolve_load(Outcome::Held, unavailable));
        }
        Phase::Quarantined => {
            apply(&mut authority, request_load(LoadTrigger::Launch));
            apply(&mut authority, resolve_load(Outcome::Failed, unavailable));
        }
        Phase::Retrying => {
            apply(&mut authority, request_load(LoadTrigger::Launch));
            apply(&mut authority, resolve_load(Outcome::Failed, unavailable));
            apply(&mut authority, request_retry(Trigger::Explicit));
        }
        Phase::Unloading | Phase::Gone => {
            panic!("a declared store is never unloading and never gone")
        }
        Phase::Creating => {
            panic!("the launch creates only under a declared store that holds no mob")
        }
    }
    assert_eq!(authority.state().lifecycle_phase, phase);
    assert!(authority.state().awaits_launch);
    authority
}

/// `input` is refused with `reason` and changes nothing.
fn assert_refused_as_is(authority: &mut Authority, input: Input, reason: Refusal) {
    let before = authority.state().clone();
    let refused = apply(authority, input);
    assert_refused(&refused, reason);
    assert_eq!(authority.state(), &before, "a refusal changes nothing");
}

#[test]
fn a_declared_store_waits_for_its_launch_and_stays_its_launchs() {
    // A store is declared once, and only a record nothing else is done
    // with becomes the declared one.
    let mut store = declared_store(false);
    assert_refused_as_is(&mut store, declare(false), Refusal::AlreadyHosted);
    assert_refused_unchanged(
        &[
            Phase::Loading,
            Phase::Managed,
            Phase::Unloading,
            Phase::Held,
            Phase::Quarantined,
            Phase::Retrying,
        ],
        || declare(false),
        Refusal::AlreadyHosted,
    );
    assert_refused_unchanged(&[Phase::Gone], || declare(false), Refusal::Gone);

    // While it awaits its launch: a lookup's load waits, a pass never
    // loads it, and no create but the launch's joins under it.
    assert_refused_as_is(
        &mut store,
        request_load(LoadTrigger::Lookup),
        Refusal::AwaitsLaunch,
    );
    assert_refused_as_is(
        &mut store,
        request_load(LoadTrigger::Pass),
        Refusal::LaunchOwned,
    );
    for input in [register as fn() -> Input, register_over_ended] {
        assert_refused_as_is(&mut store, input(), Refusal::LaunchOwned);
    }
    // The launch's own load is admitted, and the wait goes on.
    let admitted = apply(&mut store, request_load(LoadTrigger::Launch));
    assert!(matches!(only_effect(&admitted), Effect::LoadAdmitted));
    assert_eq!(store.state().lifecycle_phase, Phase::Loading);
    assert!(store.state().awaits_launch);

    // The wait ends once, in whatever phase the serve left the record, and
    // changes nothing else.
    for phase in [
        Phase::Dormant,
        Phase::Loading,
        Phase::Managed,
        Phase::Held,
        Phase::Quarantined,
        Phase::Retrying,
    ] {
        let mut store = declared_store_in(phase);
        let mut expected = store.state().clone();
        expected.awaits_launch = false;
        let ended = apply(&mut store, Input::EndLaunchWait);
        assert!(
            matches!(only_effect(&ended), Effect::LaunchWaitEnded),
            "the wait ends in {phase:?}"
        );
        assert_eq!(store.state(), &expected, "only the wait ends in {phase:?}");
        assert_refused_as_is(&mut store, Input::EndLaunchWait, Refusal::NotAwaitingLaunch);
    }
    // A store that awaits no launch has no wait to end.
    assert_refused_unchanged(
        &[
            Phase::Dormant,
            Phase::Loading,
            Phase::Managed,
            Phase::Unloading,
            Phase::Held,
            Phase::Quarantined,
            Phase::Retrying,
        ],
        || Input::EndLaunchWait,
        Refusal::NotAwaitingLaunch,
    );
    assert_refused_unchanged(&[Phase::Gone], || Input::EndLaunchWait, Refusal::Gone);

    // A store the launch's serve left quarantined: nothing re-attempts it
    // on demand before the serve has ended; the launch's explicit
    // re-attempt is admitted. Once the wait ended, on demand is as for
    // every store.
    let mut quarantined = declared_store_in(Phase::Quarantined);
    assert_refused_as_is(
        &mut quarantined,
        request_retry(Trigger::OnDemand),
        Refusal::AwaitsLaunch,
    );
    let mut explicit = declared_store_in(Phase::Quarantined);
    let retried = apply(&mut explicit, request_retry(Trigger::Explicit));
    assert!(matches!(
        only_effect(&retried),
        Effect::RetryAdmitted { .. }
    ));
    apply(&mut quarantined, Input::EndLaunchWait);
    let on_demand = apply(&mut quarantined, request_retry(Trigger::OnDemand));
    assert!(matches!(
        only_effect(&on_demand),
        Effect::RetryAdmitted { .. }
    ));

    // Once the wait ended a lookup loads the store like any other; a pass
    // still never does.
    let mut waited = declared_store(false);
    apply(&mut waited, Input::EndLaunchWait);
    assert_refused_as_is(
        &mut waited,
        request_load(LoadTrigger::Pass),
        Refusal::LaunchOwned,
    );
    let loaded = apply(&mut waited, request_load(LoadTrigger::Lookup));
    assert!(matches!(only_effect(&loaded), Effect::LoadAdmitted));

    // The served mob is its launch's: never unloaded for idleness, never
    // destroyed through the set, never replaced by a host's handle.
    let mut served = declared_store_in(Phase::Managed);
    assert!(served.state().served);
    assert_refused_as_is(&mut served, Input::RequestUnload, Refusal::NotReloadable);
    assert_refused_as_is(&mut served, Input::Destroy, Refusal::LaunchOwned);
    for input in [register as fn() -> Input, register_over_ended] {
        assert_refused_as_is(&mut served, input(), Refusal::LaunchOwned);
    }

    // A load that finds no mob in a declared store keeps the record: it is
    // dormant and known to hold no mob, where a store under the root is
    // gone. A lookup then finds nothing to load; the launch's create joins
    // under the record and ends the vacancy.
    let unavailable = MobQuarantineClass::StoreUnavailable;
    let mut empty = declared_store_in(Phase::Loading);
    let settled = apply(&mut empty, resolve_load(Outcome::Settled, unavailable));
    assert!(matches!(
        only_effect(&settled),
        Effect::StoreLoaded {
            outcome: Outcome::Settled
        }
    ));
    assert_eq!(empty.state().lifecycle_phase, Phase::Dormant);
    assert!(empty.state().vacant);
    assert_refused_as_is(
        &mut empty,
        request_load(LoadTrigger::Lookup),
        Refusal::AwaitsLaunch,
    );
    apply(&mut empty, Input::EndLaunchWait);
    assert_refused_as_is(
        &mut empty,
        request_load(LoadTrigger::Lookup),
        Refusal::NotDormant,
    );
    let admitted = apply(&mut empty, request_create(true));
    assert!(matches!(only_effect(&admitted), Effect::CreateAdmitted));
    let created = apply(&mut empty, resolve_create(Create::Created, false));
    assert!(matches!(
        only_effect(&created),
        Effect::StoreRegistered {
            origin: Origin::Declared
        }
    ));
    assert_eq!(empty.state().lifecycle_phase, Phase::Managed);
    assert!(empty.state().served && !empty.state().vacant);

    // The same for a re-attempt that finds no mob, and for a hold that is
    // settled.
    let mut retried = declared_store_in(Phase::Retrying);
    let released = apply(&mut retried, resolve_retry(Outcome::Settled, unavailable));
    assert!(matches!(
        only_effect(&released),
        Effect::StoreReleased {
            outcome: Outcome::Settled,
            attempts: 1
        }
    ));
    assert_eq!(retried.state().lifecycle_phase, Phase::Dormant);
    assert!(retried.state().vacant);
    let mut held = declared_store_in(Phase::Held);
    let let_go = apply(&mut held, release_held(Outcome::Settled));
    assert!(matches!(
        only_effect(&let_go),
        Effect::HoldReleased {
            outcome: Outcome::Settled
        }
    ));
    assert_eq!(held.state().lifecycle_phase, Phase::Dormant);
    assert!(held.state().vacant);
    // A launch's load of a store known to hold no mob that then finds one
    // ends the vacancy.
    let found = apply(&mut held, request_load(LoadTrigger::Launch));
    assert!(matches!(only_effect(&found), Effect::LoadAdmitted));
    apply(&mut held, resolve_load(Outcome::Managed, unavailable));
    assert!(!held.state().vacant);

    // A declared mob that lives in memory has no store to load: it is
    // vacant from its declaration. Its lookup waits for the launch like any
    // declared store's, and finds nothing to load once the wait ended.
    let mut in_memory = declared_store(true);
    assert_refused_as_is(
        &mut in_memory,
        request_load(LoadTrigger::Lookup),
        Refusal::AwaitsLaunch,
    );
    apply(&mut in_memory, Input::EndLaunchWait);
    assert_refused_as_is(
        &mut in_memory,
        request_load(LoadTrigger::Lookup),
        Refusal::NotDormant,
    );

    // For a store under the root nothing of this applies: what asks for
    // its load decides nothing, and a load that finds no mob ends it.
    for trigger in [LoadTrigger::Lookup, LoadTrigger::Pass, LoadTrigger::Launch] {
        let mut persisted = authority_in(Phase::Dormant);
        let admitted = apply(&mut persisted, request_load(trigger));
        assert!(matches!(only_effect(&admitted), Effect::LoadAdmitted));
        apply(&mut persisted, resolve_load(Outcome::Settled, unavailable));
        assert_eq!(persisted.state().lifecycle_phase, Phase::Gone);
    }
}

#[test]
fn only_a_store_that_could_not_serve_is_retryable() {
    assert!(MobQuarantineClass::StoreUnavailable.is_retryable());
    assert!(!MobQuarantineClass::StoreUnreadable.is_retryable());
    assert!(!MobQuarantineClass::ResumeRefused.is_retryable());
    assert_eq!(
        MobQuarantineClass::StoreUnavailable.as_str(),
        "store_unavailable"
    );
    assert_eq!(
        MobQuarantineClass::StoreUnreadable.as_str(),
        "store_unreadable"
    );
    assert_eq!(MobQuarantineClass::ResumeRefused.as_str(), "resume_refused");
}

/// The record of the store a host's launch declares, as the schema names it:
/// the wait for the launch's first serve is a field of the record, begun
/// and ended by inputs of their own; a load names what asks for it; and the
/// refusals of a store that is its launch's are transitions of the record.
#[test]
fn the_schema_names_the_declared_stores_wait_for_its_launch() {
    let schema = meerkat_machine_schema::catalog::dsl::dsl_mob_store_restore_lifecycle_machine();
    let named = |names: Vec<&str>, wanted: &[&str]| -> Vec<String> {
        wanted
            .iter()
            .filter(|name| !names.contains(name))
            .map(|name| (*name).to_string())
            .collect()
    };
    let mut missing = Vec::new();
    missing.extend(named(
        schema
            .state
            .fields
            .iter()
            .map(|field| field.name.as_str())
            .collect(),
        &["awaits_launch"],
    ));
    missing.extend(named(
        schema
            .inputs
            .variants
            .iter()
            .map(|variant| variant.name.as_str())
            .collect(),
        &["Declare", "EndLaunchWait"],
    ));
    missing.extend(named(
        schema
            .effects
            .variants
            .iter()
            .map(|variant| variant.name.as_str())
            .collect(),
        &["LaunchDeclared", "LaunchWaitEnded"],
    ));
    missing.extend(named(
        schema
            .invariants
            .iter()
            .map(|invariant| invariant.name.as_str())
            .collect(),
        &[
            "only_a_declared_store_awaits_its_launch",
            "a_store_that_is_gone_awaits_no_launch",
            "a_store_whose_unload_is_admitted_awaits_no_launch",
        ],
    ));
    missing.extend(named(
        schema
            .transitions
            .iter()
            .map(|transition| transition.name.as_str())
            .collect(),
        &[
            "DeclareDormant",
            "EndLaunchWaitDormant",
            "EndLaunchWaitQuarantined",
            "EndLaunchWaitUnloading",
            "RequestLoadDormantAwaitsLaunch",
            "RequestLoadDormantLaunchOwned",
            "RequestRetryQuarantinedAwaitsLaunch",
            "RegisterDormantLaunchOwned",
            "RequestCreateDormantNotDeclared",
            "RegisterManagedLaunchOwned",
            "DestroyManagedLaunchOwned",
        ],
    ));
    let load_names_what_asks = schema
        .inputs
        .variants
        .iter()
        .find(|variant| variant.name.as_str() == "RequestLoad")
        .is_some_and(|variant| variant.field_named("trigger").is_ok());
    assert!(
        missing.is_empty() && load_names_what_asks && schema.version == 7,
        "the schema (version {}) does not name: {missing:?}; a load names what asks: \
         {load_names_what_asks}",
        schema.version
    );
}

/// A store becomes the declared one by `Declare` alone: a create that names
/// the launch on a record that was never declared is refused and changes
/// nothing, whether the record stands for a store or for nothing yet.
#[test]
fn a_create_does_not_make_a_store_the_declared_one() {
    for mut store in [authority_in(Phase::Dormant), Authority::new()] {
        let before = store.state().clone();
        let refused = apply(&mut store, request_create(true));
        assert_refused(&refused, Refusal::NotDeclared);
        assert_eq!(store.state(), &before, "and the record is as it was");
    }
}

/// A create takes its mob's id before anything of the mob is built: the
/// record the set files for the create admits it and is the create's until
/// the create is resolved. A created mob is managed and served from that
/// step, under the origin its create gives it; a create that ended without a
/// mob frees the id.
#[test]
fn a_create_takes_its_mobs_id_before_the_mob_is_built() {
    // The record filed for a create: with a store file, and in memory.
    for (in_memory, origin) in [(false, Origin::Persisted), (true, Origin::Volatile)] {
        let mut record = create_in_flight();
        assert!(
            !record.state().served,
            "nothing is served while it is built"
        );
        let created = apply(&mut record, resolve_create(Create::Created, in_memory));
        assert!(matches!(
            only_effect(&created),
            Effect::StoreRegistered { origin: recorded } if recorded == origin
        ));
        assert_eq!(record.state().lifecycle_phase, Phase::Managed);
        assert_eq!(record.state().origin, origin);
        assert!(record.state().served);
    }
    // A create that ended without a mob: the record is gone, whatever it
    // says of a store file, and admits nothing more.
    for in_memory in [false, true] {
        let mut record = create_in_flight();
        let released = apply(&mut record, resolve_create(Create::Failed, in_memory));
        assert!(matches!(only_effect(&released), Effect::CreateReleased));
        assert_eq!(record.state().lifecycle_phase, Phase::Gone);
        assert!(!record.state().served);
        assert_refused_as_is(&mut record, request_create(false), Refusal::Gone);
    }
    // One create at a time: a second request, the launch's included, is
    // refused while the first is in flight, and changes nothing.
    for launch in [false, true] {
        let mut record = create_in_flight();
        assert_refused_as_is(&mut record, request_create(launch), Refusal::CreateInFlight);
    }
    // A record that stands for a store or a mob the set knows refuses a
    // create: the id is taken. A record whose unload is admitted answers
    // the refusal its caller waits on.
    for launch in [false, true] {
        for phase in [
            Phase::Loading,
            Phase::Managed,
            Phase::Held,
            Phase::Quarantined,
            Phase::Retrying,
        ] {
            let mut record = authority_in(phase);
            assert_refused_as_is(&mut record, request_create(launch), Refusal::AlreadyHosted);
        }
        let mut unloading = authority_in(Phase::Unloading);
        assert_refused_as_is(
            &mut unloading,
            request_create(launch),
            Refusal::UnloadInFlight,
        );
        let mut gone = authority_in(Phase::Gone);
        assert_refused_as_is(&mut gone, request_create(launch), Refusal::Gone);
    }
    // A create that names the launch joins under the declared record only.
    let mut dormant = authority_in(Phase::Dormant);
    assert_refused_as_is(&mut dormant, request_create(true), Refusal::NotDeclared);

    // The declared record: only the launch's create is admitted, and only
    // while the store is known to hold no mob.
    let mut declared = declared_store(true);
    assert_refused_as_is(&mut declared, request_create(false), Refusal::LaunchOwned);
    let admitted = apply(&mut declared, request_create(true));
    assert!(matches!(only_effect(&admitted), Effect::CreateAdmitted));
    assert_eq!(declared.state().lifecycle_phase, Phase::Creating);
    assert!(
        declared.state().vacant,
        "it holds no mob until the create ends"
    );
    assert!(declared.state().awaits_launch, "the serve has not ended");
    // The launch's mob is managed under the declaration, whatever the
    // create says of a store file, and the store holds a mob from then on.
    for in_memory in [false, true] {
        let mut created = launch_create_in_flight();
        let registered = apply(&mut created, resolve_create(Create::Created, in_memory));
        assert!(matches!(
            only_effect(&registered),
            Effect::StoreRegistered {
                origin: Origin::Declared
            }
        ));
        assert_eq!(created.state().lifecycle_phase, Phase::Managed);
        assert_eq!(created.state().origin, Origin::Declared);
        assert!(created.state().served);
        assert!(!created.state().vacant);
    }
    // A failed create of the launch keeps the declared record: dormant,
    // known to hold no mob, for the next serve's create.
    let mut failed = launch_create_in_flight();
    let released = apply(&mut failed, resolve_create(Create::Failed, false));
    assert!(matches!(only_effect(&released), Effect::CreateReleased));
    assert_eq!(failed.state().lifecycle_phase, Phase::Dormant);
    assert_eq!(failed.state().origin, Origin::Declared);
    assert!(failed.state().vacant);
    let again = apply(&mut failed, request_create(true));
    assert!(matches!(only_effect(&again), Effect::CreateAdmitted));
    // A declared store that holds a mob is loaded by its launch, not
    // created.
    let mut holds_a_mob = declared_store(false);
    assert_refused_as_is(
        &mut holds_a_mob,
        request_create(true),
        Refusal::AlreadyHosted,
    );
    assert_refused_as_is(
        &mut holds_a_mob,
        request_create(false),
        Refusal::LaunchOwned,
    );

    // An outcome with no create in flight is refused in every other phase.
    for outcome in [Create::Created, Create::Failed] {
        for phase in ALL_PHASES {
            if phase == Phase::Creating {
                continue;
            }
            let mut record = authority_in(phase);
            assert_refused_as_is(
                &mut record,
                resolve_create(outcome, false),
                Refusal::NotCreating,
            );
        }
    }
}

/// A create is admitted only by a record that stands for nothing yet: the
/// one the set files for that create. A record a census listed stands for a
/// store that is known and not loaded, and so does the record of a mob that
/// was loaded and unloaded again: each refuses a create as every record of a
/// known store or mob does, and changes nothing.
#[test]
fn a_record_that_stands_for_a_store_refuses_a_create() {
    // Filed for the create alone: no census listed it, nothing joined it.
    let mut filed = Authority::new();
    assert!(matches!(
        only_effect(&apply(&mut filed, request_create(false))),
        Effect::CreateAdmitted
    ));
    assert_eq!(filed.state().lifecycle_phase, Phase::Creating);

    // Listed by a census: the store is known and not loaded.
    let mut listed = Authority::new();
    assert!(matches!(
        only_effect(&apply(&mut listed, Input::Census)),
        Effect::StoreListed
    ));
    assert_eq!(listed.state().lifecycle_phase, Phase::Dormant);
    assert_refused_as_is(&mut listed, request_create(false), Refusal::AlreadyHosted);

    // Loaded once and unloaded again: dormant, and still that store's.
    let mut unloaded = authority_in(Phase::Unloading);
    apply(&mut unloaded, resolve_unload(Unload::Closed));
    assert_eq!(unloaded.state().lifecycle_phase, Phase::Dormant);
    assert_refused_as_is(&mut unloaded, request_create(false), Refusal::AlreadyHosted);

    // Created by the set, unloaded: the record the create was filed under
    // stands for the created mob's store from then on.
    let mut created = create_in_flight();
    apply(&mut created, resolve_create(Create::Created, false));
    apply(&mut created, Input::RequestUnload);
    apply(&mut created, resolve_unload(Unload::Closed));
    assert_eq!(created.state().lifecycle_phase, Phase::Dormant);
    assert_refused_as_is(&mut created, request_create(false), Refusal::AlreadyHosted);
}

/// While a create is in flight nothing else moves its record: the id is
/// the create's. A host's handle does not register over it, no census lists
/// it, and no load, re-attempt, unload, release or destroy applies to it.
#[test]
fn nothing_else_moves_a_record_whose_create_is_in_flight() {
    let refusals: [(fn() -> Input, Refusal); 13] = [
        (|| Input::Census, Refusal::AlreadyHosted),
        (register, Refusal::AlreadyHosted),
        (register_over_ended, Refusal::AlreadyHosted),
        (|| declare(false), Refusal::AlreadyHosted),
        (|| Input::EndLaunchWait, Refusal::NotAwaitingLaunch),
        (|| request_load(LoadTrigger::Lookup), Refusal::NotDormant),
        (
            || resolve_load(Outcome::Managed, MobQuarantineClass::StoreUnavailable),
            Refusal::NotLoading,
        ),
        (|| request_retry(Trigger::Explicit), Refusal::NotQuarantined),
        (
            || resolve_retry(Outcome::Managed, MobQuarantineClass::StoreUnavailable),
            Refusal::NotRetrying,
        ),
        (|| Input::RequestUnload, Refusal::NotManaged),
        (|| resolve_unload(Unload::Closed), Refusal::NotUnloading),
        (|| release_held(Outcome::Managed), Refusal::NotHeld),
        (|| Input::Destroy, Refusal::NotManaged),
    ];
    for (input, reason) in refusals {
        let mut record = create_in_flight();
        assert_refused_as_is(&mut record, input(), reason);
    }
    // The launch's create runs while its record still waits for the
    // launch's serve: the wait ends once during the create, and the create
    // goes on.
    let mut declared = declared_store(true);
    apply(&mut declared, request_create(true));
    let mut expected = declared.state().clone();
    expected.awaits_launch = false;
    let ended = apply(&mut declared, Input::EndLaunchWait);
    assert!(matches!(only_effect(&ended), Effect::LaunchWaitEnded));
    assert_eq!(declared.state(), &expected, "only the wait ended");
    assert_refused_as_is(
        &mut declared,
        Input::EndLaunchWait,
        Refusal::NotAwaitingLaunch,
    );
    let created = apply(&mut declared, resolve_create(Create::Created, false));
    assert!(matches!(
        only_effect(&created),
        Effect::StoreRegistered {
            origin: Origin::Declared
        }
    ));
}

/// A dormant persisted store whose record knows `stop`.
fn dormant_with_stop(stop: Stop) -> Authority {
    let mut record = authority_in(Phase::Dormant);
    match stop {
        Stop::Unread => {}
        Stop::Stopped => {
            apply(&mut record, read_stop(true));
        }
        Stop::NotStopped => {
            apply(&mut record, read_stop(false));
        }
    }
    assert_eq!(record.state().stop, stop);
    record
}

/// The stop of a mob that is not loaded is a fact of its store's record.
/// It is written by the mob's own answer when its actor closed at an
/// unload, and by the set's read of the store while the record is dormant
/// and does not know it. A demand (what originates work for the mob) loads
/// a dormant mob that is known not to be stopped, and never a stopped one;
/// a lookup, a pass and the launch load it stopped or not. Every step that
/// leaves dormant and loading clears it: a loaded mob speaks for itself.
#[test]
fn a_mob_that_is_not_loaded_keeps_its_stop_and_no_demand_loads_a_stopped_one() {
    // A record starts without the answer, and a census does not give it.
    assert_eq!(authority_in(Phase::Dormant).state().stop, Stop::Unread);

    // The mob's own answer at the unload: stopped, not stopped, or not
    // read (a resumed unload has no moment to read it).
    for stop in [Stop::Stopped, Stop::NotStopped, Stop::Unread] {
        let mut record = authority_in(Phase::Unloading);
        let closed = apply(&mut record, resolve_unload_with(Unload::Closed, stop));
        assert!(matches!(only_effect(&closed), Effect::StoreUnloaded));
        assert_eq!(record.state().lifecycle_phase, Phase::Dormant);
        assert_eq!(record.state().stop, stop);
    }
    // A mob that stays managed keeps speaking for itself.
    let mut busy = authority_in(Phase::Unloading);
    apply(&mut busy, resolve_unload_with(Unload::Busy, Stop::Stopped));
    assert_eq!(busy.state().lifecycle_phase, Phase::Managed);
    assert_eq!(busy.state().stop, Stop::Unread);

    // The set's read of the store: taken once, while dormant and unread.
    for (stopped, stop) in [(true, Stop::Stopped), (false, Stop::NotStopped)] {
        let mut record = authority_in(Phase::Dormant);
        let read = apply(&mut record, read_stop(stopped));
        assert!(matches!(
            only_effect(&read),
            Effect::StopRead { stopped: recorded } if recorded == stopped
        ));
        assert_eq!(record.state().lifecycle_phase, Phase::Dormant);
        assert_eq!(record.state().stop, stop);
        // A later read never overwrites what the record knows, whatever
        // it says: the mob's own answer at an unload is newer than any
        // read that began before it.
        for late in [true, false] {
            assert_refused_as_is(&mut record, read_stop(late), Refusal::StopKnown);
        }
    }
    let mut answered = authority_in(Phase::Unloading);
    apply(
        &mut answered,
        resolve_unload_with(Unload::Closed, Stop::NotStopped),
    );
    assert_refused_as_is(&mut answered, read_stop(true), Refusal::StopKnown);
    // Only a dormant record takes a read.
    for phase in ALL_PHASES {
        if phase == Phase::Dormant {
            continue;
        }
        for stopped in [true, false] {
            let mut record = authority_in(phase);
            assert_refused_as_is(&mut record, read_stop(stopped), Refusal::NotDormant);
        }
    }

    // A demand: admitted for a mob known not to be stopped, refused for a
    // stopped one, and answered "read first" while the stop is unread.
    let mut running = dormant_with_stop(Stop::NotStopped);
    let admitted = apply(&mut running, request_load(LoadTrigger::Demand));
    assert!(matches!(only_effect(&admitted), Effect::LoadAdmitted));
    assert_eq!(running.state().lifecycle_phase, Phase::Loading);
    let mut stopped = dormant_with_stop(Stop::Stopped);
    assert_refused_as_is(
        &mut stopped,
        request_load(LoadTrigger::Demand),
        Refusal::Stopped,
    );
    let mut unread = dormant_with_stop(Stop::Unread);
    assert_refused_as_is(
        &mut unread,
        request_load(LoadTrigger::Demand),
        Refusal::StopUnread,
    );
    // A lookup (a read of the mob, an operator's verb) and a pass load
    // the mob whatever its stop.
    for stop in [Stop::Stopped, Stop::NotStopped, Stop::Unread] {
        for trigger in [LoadTrigger::Lookup, LoadTrigger::Pass] {
            let mut record = dormant_with_stop(stop);
            let admitted = apply(&mut record, request_load(trigger));
            assert!(
                matches!(only_effect(&admitted), Effect::LoadAdmitted),
                "{trigger:?} loads a mob whose stop is {stop:?}"
            );
            // The record keeps the answer while the load is in flight.
            assert_eq!(record.state().stop, stop);
        }
    }
    // A demand that meets a load in flight waits for it like any lookup.
    let mut loading = dormant_with_stop(Stop::Stopped);
    apply(&mut loading, request_load(LoadTrigger::Lookup));
    assert_refused_as_is(
        &mut loading,
        request_load(LoadTrigger::Demand),
        Refusal::LoadInFlight,
    );

    // Every way out of dormant and loading clears the answer.
    let unavailable = MobQuarantineClass::StoreUnavailable;
    for outcome in [
        Outcome::Managed,
        Outcome::Held,
        Outcome::Settled,
        Outcome::Failed,
    ] {
        let mut record = dormant_with_stop(Stop::Stopped);
        apply(&mut record, request_load(LoadTrigger::Lookup));
        apply(&mut record, resolve_load(outcome, unavailable));
        assert_eq!(
            record.state().stop,
            Stop::Unread,
            "a load that ended {outcome:?} leaves no answer"
        );
    }
    let mut taken = dormant_with_stop(Stop::Stopped);
    apply(&mut taken, register());
    assert_eq!(taken.state().lifecycle_phase, Phase::Managed);
    assert_eq!(taken.state().stop, Stop::Unread);

    // The declared store: a demand asks as a lookup does. It waits for the
    // launch, finds no mob in a store that holds none, and after the wait
    // follows the same rule as any store.
    let mut awaiting = declared_store(false);
    assert_refused_as_is(
        &mut awaiting,
        request_load(LoadTrigger::Demand),
        Refusal::AwaitsLaunch,
    );
    apply(&mut awaiting, Input::EndLaunchWait);
    assert_refused_as_is(
        &mut awaiting,
        request_load(LoadTrigger::Demand),
        Refusal::StopUnread,
    );
    apply(&mut awaiting, read_stop(false));
    let admitted = apply(&mut awaiting, request_load(LoadTrigger::Demand));
    assert!(matches!(only_effect(&admitted), Effect::LoadAdmitted));
    let mut vacant = declared_store(true);
    apply(&mut vacant, Input::EndLaunchWait);
    assert_refused_as_is(
        &mut vacant,
        request_load(LoadTrigger::Demand),
        Refusal::NotDormant,
    );
}
