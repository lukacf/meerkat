//! Rows for the SessionDocumentMachine activation-body retirement region.
//!
//! The machine holds, per session and per instruction activation key, how
//! many activation rows of the key the session's transcript holds (every
//! origin), how many of them, oldest first, carry a retired body, and
//! whether the key's newest row is the session's own. Before a compaction
//! the shell recovers those counts from the transcript and asks the machine
//! which bodies to retire: every row a later row of its key follows. The
//! machine never lowers a recovered count and never retires an own newest
//! row, the session's effective activation of its key.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::BTreeMap;

use meerkat_machine_schema::canonical_machine_coverage_manifests;
use meerkat_machine_schema::catalog::dsl::dsl_session_document_machine;
use meerkat_machine_schema::catalog::dsl::session_document::{
    ActivationHeadOrigin, SessionDocumentEffect, SessionDocumentInput,
    SessionDocumentMachineAuthority, SessionDocumentMachineMutator, SessionDocumentMachineState,
    SessionDocumentMachineTransition, SessionDocumentMachineTransitionError, SessionId,
};

type Authority = SessionDocumentMachineAuthority;
type Input = SessionDocumentInput;
type Effect = SessionDocumentEffect;
type State = SessionDocumentMachineState;
type Transition = SessionDocumentMachineTransition;
type TransitionError = SessionDocumentMachineTransitionError;

const ARMS: [&str; 2] = [
    "RecoverSessionActivationBodies",
    "AuthorizeActivationBodyRetirement",
];

const INVARIANTS: [&str; 2] = [
    "activation_body_maps_lockstep",
    "activation_effective_body_live",
];

const CALENDAR: &str = r#"["app.skills","calendar"]"#;
const BILLING: &str = r#"["app.skills","billing"]"#;
const FILES: &str = r#"["app.skills","files"]"#;

fn session() -> SessionId {
    SessionId::from("s")
}

fn apply(authority: &mut Authority, input: Input) -> Transition {
    SessionDocumentMachineMutator::apply(authority, input)
        .expect("the input must resolve to exactly one activation-body arm")
}

fn try_apply(authority: &mut Authority, input: Input) -> Result<Transition, TransitionError> {
    SessionDocumentMachineMutator::apply(authority, input)
}

/// One session's activation rows per key, as the shell recovers them from
/// its transcript.
#[derive(Debug, Clone, Default)]
struct Projection {
    rows: BTreeMap<String, u64>,
    retired: BTreeMap<String, u64>,
    head_origin: BTreeMap<String, ActivationHeadOrigin>,
}

impl Projection {
    fn empty() -> Self {
        Self::default()
    }

    fn key(mut self, key: &str, rows: u64, retired: u64, head: ActivationHeadOrigin) -> Self {
        self.rows.insert(key.to_string(), rows);
        self.retired.insert(key.to_string(), retired);
        self.head_origin.insert(key.to_string(), head);
        self
    }

    fn input(&self) -> Input {
        Input::RecoverSessionActivationBodies {
            session_id: session(),
            rows: self.rows.clone(),
            retired: self.retired.clone(),
            head_origin: self.head_origin.clone(),
        }
    }
}

fn recovered(projection: &Projection) -> Authority {
    let mut authority = Authority::new();
    let transition = apply(&mut authority, projection.input());
    assert_eq!(
        transition.effects(),
        [Effect::SessionInstructionStateRecovered].as_slice()
    );
    authority
}

fn retire() -> Input {
    Input::AuthorizeActivationBodyRetirement {
        session_id: session(),
    }
}

fn rows(authority: &Authority) -> BTreeMap<String, u64> {
    authority.state().session_activation_rows[&session()].clone()
}

fn retired(authority: &Authority) -> BTreeMap<String, u64> {
    authority.state().session_activation_retired[&session()].clone()
}

fn head_origin(authority: &Authority) -> BTreeMap<String, ActivationHeadOrigin> {
    authority.state().session_activation_head_origin[&session()].clone()
}

fn counts(entries: &[(&str, u64)]) -> BTreeMap<String, u64> {
    entries
        .iter()
        .map(|(key, count)| ((*key).to_string(), *count))
        .collect()
}

// ---------------------------------------------------------------------------
// Recovery and the decision
// ---------------------------------------------------------------------------

/// Every row but each key's newest is retired; the newest row of every key
/// keeps its body, an own head and a copied one alike, and the recovered
/// row counts and head origins stay as they were.
#[test]
fn retirement_retires_every_superseded_row_and_keeps_each_head() {
    let projection = Projection::empty()
        .key(CALENDAR, 3, 0, ActivationHeadOrigin::Own)
        .key(BILLING, 1, 0, ActivationHeadOrigin::Own)
        .key(FILES, 2, 1, ActivationHeadOrigin::Copied);
    let mut authority = recovered(&projection);
    let transition = apply(&mut authority, retire());
    assert_eq!(
        transition.effects(),
        [Effect::ActivationBodyRetirementAuthorized].as_slice()
    );
    assert_eq!(
        retired(&authority),
        counts(&[(CALENDAR, 2), (BILLING, 0), (FILES, 1)]),
        "a key with three rows retires two, a key with one retires none"
    );
    assert_eq!(rows(&authority), projection.rows);
    assert_eq!(head_origin(&authority), projection.head_origin);

    let after_first = authority.state().clone();
    apply(&mut authority, retire());
    assert_eq!(
        authority.state(),
        &after_first,
        "a second decision retires nothing more"
    );
}

/// A torn projection is taken by no arm and changes nothing: a retired own
/// newest row, more retired rows than rows, a key without rows, and maps
/// that name different keys.
#[test]
fn a_projection_with_a_retired_own_head_is_refused() {
    let mut torn_key_sets = Projection::empty().key(CALENDAR, 2, 1, ActivationHeadOrigin::Own);
    torn_key_sets.retired.insert(BILLING.to_string(), 0);
    let mut missing_head = Projection::empty().key(CALENDAR, 2, 1, ActivationHeadOrigin::Own);
    missing_head.head_origin.clear();
    for (case, projection) in [
        (
            "a retired own newest row",
            Projection::empty().key(CALENDAR, 2, 2, ActivationHeadOrigin::Own),
        ),
        (
            "more retired rows than rows",
            Projection::empty().key(CALENDAR, 2, 3, ActivationHeadOrigin::Copied),
        ),
        (
            "a key without rows",
            Projection::empty().key(CALENDAR, 0, 0, ActivationHeadOrigin::Copied),
        ),
        ("retired counts for a key without rows", torn_key_sets),
        ("a key without a head origin", missing_head),
    ] {
        let mut authority = Authority::new();
        let before = authority.state().clone();
        let result = try_apply(&mut authority, projection.input());
        assert!(result.is_err(), "{case}: no arm may take it: {result:?}");
        assert_eq!(authority.state(), &before, "{case}: nothing changes");
    }

    // The copied newest row of a fork may be retired.
    let copied = recovered(&Projection::empty().key(CALENDAR, 2, 2, ActivationHeadOrigin::Copied));
    assert_eq!(retired(&copied), counts(&[(CALENDAR, 2)]));
}

/// A copied newest row a fork copied already retired stays retired: the
/// decision never lowers a recovered count.
#[test]
fn a_copied_retired_head_stays_retired() {
    let mut authority = recovered(
        &Projection::empty()
            .key(CALENDAR, 2, 2, ActivationHeadOrigin::Copied)
            .key(BILLING, 3, 1, ActivationHeadOrigin::Copied),
    );
    apply(&mut authority, retire());
    assert_eq!(
        retired(&authority),
        counts(&[(CALENDAR, 2), (BILLING, 2)]),
        "the copied retired head stays retired; the other key retires its superseded rows"
    );
}

/// A session the shell has not recovered takes no decision, and a decision
/// for one session leaves another alone.
#[test]
fn retirement_requires_a_recovered_session() {
    let mut authority = Authority::new();
    let before = authority.state().clone();
    let result = try_apply(&mut authority, retire());
    assert!(result.is_err(), "no arm takes it: {result:?}");
    assert_eq!(authority.state(), &before, "nothing is recorded");

    let mut authority =
        recovered(&Projection::empty().key(CALENDAR, 2, 0, ActivationHeadOrigin::Own));
    let other = SessionDocumentMachineMutator::apply(
        &mut authority,
        Input::AuthorizeActivationBodyRetirement {
            session_id: SessionId::from("other"),
        },
    );
    assert!(
        other.is_err(),
        "another session is not recovered: {other:?}"
    );
    assert_eq!(retired(&authority), counts(&[(CALENDAR, 0)]));
}

// ---------------------------------------------------------------------------
// Invariants
// ---------------------------------------------------------------------------

fn consistent_state() -> State {
    let mut authority = recovered(
        &Projection::empty()
            .key(CALENDAR, 3, 0, ActivationHeadOrigin::Own)
            .key(FILES, 2, 2, ActivationHeadOrigin::Copied),
    );
    apply(&mut authority, retire());
    authority.state().clone()
}

fn rejected_invariant(state: State) -> &'static str {
    match Authority::recover_from_state(state) {
        Err(TransitionError::RecoveredStateInvariantRejected { invariant, .. }) => invariant,
        Err(other) => panic!("unexpected refusal {other}"),
        Ok(_) => panic!("a torn state must not recover"),
    }
}

#[test]
fn every_reached_activation_body_state_recovers() {
    let mut authority = Authority::new();
    for input in [
        Projection::empty()
            .key(CALENDAR, 1, 0, ActivationHeadOrigin::Own)
            .input(),
        retire(),
        Projection::empty()
            .key(CALENDAR, 2, 0, ActivationHeadOrigin::Own)
            .key(BILLING, 1, 1, ActivationHeadOrigin::Copied)
            .input(),
        retire(),
        Projection::empty()
            .key(CALENDAR, 3, 1, ActivationHeadOrigin::Own)
            .key(BILLING, 2, 1, ActivationHeadOrigin::Own)
            .input(),
        retire(),
    ] {
        apply(&mut authority, input);
        let state = authority.state().clone();
        let recovered = Authority::recover_from_state(state.clone())
            .unwrap_or_else(|error| panic!("a reached state recovers: {error}"));
        assert_eq!(recovered.state(), &state);
    }
}

#[test]
fn recovered_state_rejects_activation_body_maps_out_of_lockstep() {
    let mut state = consistent_state();
    state.session_activation_head_origin.clear();
    assert_eq!(rejected_invariant(state), "activation_body_maps_lockstep");

    let mut state = consistent_state();
    state
        .session_activation_retired
        .get_mut(&session())
        .unwrap()
        .remove(FILES);
    assert_eq!(rejected_invariant(state), "activation_body_maps_lockstep");
}

#[test]
fn recovered_state_rejects_a_retired_own_head() {
    let mut state = consistent_state();
    state
        .session_activation_retired
        .get_mut(&session())
        .unwrap()
        .insert(CALENDAR.to_string(), 3);
    assert_eq!(rejected_invariant(state), "activation_effective_body_live");
}

// ---------------------------------------------------------------------------
// Schema census
// ---------------------------------------------------------------------------

#[test]
fn the_activation_body_region_declares_every_arm_and_invariant() {
    let schema = dsl_session_document_machine();
    for arm in ARMS {
        assert!(
            schema
                .transitions
                .iter()
                .any(|transition| transition.name.as_str() == arm),
            "SessionDocumentMachine must declare {arm}"
        );
    }
    let declared_arms = schema
        .transitions
        .iter()
        .filter(|transition| transition.name.as_str().contains("ActivationBod"))
        .count();
    assert_eq!(
        declared_arms,
        ARMS.len(),
        "every activation-body arm has a row"
    );
    for invariant in INVARIANTS {
        assert!(
            schema
                .invariants
                .iter()
                .any(|declared| declared.name == invariant),
            "SessionDocumentMachine must declare invariant {invariant}"
        );
    }
    let manifest = canonical_machine_coverage_manifests()
        .into_iter()
        .find(|manifest| manifest.machine.as_str() == "SessionDocumentMachine")
        .expect("a SessionDocumentMachine coverage manifest");
    for arm in ARMS {
        assert!(
            manifest
                .transition_coverage
                .iter()
                .any(|entry| entry.name == arm && !entry.scenario_ids.is_empty()),
            "a coverage scenario claims {arm}"
        );
    }
    for invariant in INVARIANTS {
        assert!(
            manifest
                .invariant_coverage
                .iter()
                .any(|entry| entry.name == invariant && !entry.scenario_ids.is_empty()),
            "a coverage scenario claims {invariant}"
        );
    }
}
