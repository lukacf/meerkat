//! `ClassifyDeliveryStatus` partition pin.
//!
//! The runtime delivery status read must give exactly one verdict for every
//! delivery id in `Active`, by construction rather than by the kernel's
//! runtime ambiguity refusal. The six arms are guarded over five facts about
//! one id:
//!
//! - committed: `delivery_ids` contains it;
//! - refused: `refused_deliveries` holds a settled refusal for it;
//! - at or below the cursor: its sequence `<=` the applied cursor;
//! - acknowledged: `acknowledged_sequences` contains its sequence.
//! - recipient_group: no summary, or one of its four exact enum values.
//!
//! This evaluates each arm's guard skeleton over every assignment of those
//! facts and requires exactly one arm to hold. A mutant that drops one negation
//! must be refused by the same check, so the check itself cannot go vacuous.
//!
//! The six arms, the source-sequence conflict rejection and the repeated
//! refusal observation are also pinned read-only: each has an empty
//! `update {}`. A rejection that rewrote the
//! stored source sequence passes every bounded TLC check and witness (the
//! witnesses cannot name a map value), so this static pin is what refuses it.

#![allow(clippy::expect_used, clippy::panic)]

use meerkat_machine_schema::catalog::dsl::dsl_runtime_delivery_machine;
use meerkat_machine_schema::{Expr, Guard, TransitionSchema, TriggerMatch, Update};

const CLASSIFY: &str = "ClassifyDeliveryStatus";
const ARMS: [&str; 6] = [
    "ClassifyAcknowledgedAhead",
    "ClassifyApplied",
    "ClassifyMixed",
    "ClassifyNotCommitted",
    "ClassifyPending",
    "ClassifyRefused",
];

#[derive(Clone, Copy, Debug)]
struct Facts {
    committed: bool,
    refused: bool,
    at_or_below_cursor: bool,
    acknowledged: bool,
    recipient_group: Option<&'static str>,
}

fn group_value(expr: &Expr, facts: Facts) -> Option<&'static str> {
    match expr {
        Expr::None => None,
        Expr::Some(value) => group_value(value, facts),
        Expr::IfElse {
            condition,
            then_expr,
            else_expr,
        } => group_value(
            if evaluate(condition, facts) {
                then_expr
            } else {
                else_expr
            },
            facts,
        ),
        Expr::MapGet { map, .. } if format!("{map:?}").contains("recipient_group_outcomes") => {
            facts.recipient_group
        }
        Expr::NamedVariant { enum_name, variant }
            if enum_name.as_str() == "DeliveryRecipientGroupOutcome" =>
        {
            Some(match variant.as_str() {
                "AllApplied" => "AllApplied",
                "AllRefused" => "AllRefused",
                "AllAuthorizationUnavailable" => "AllAuthorizationUnavailable",
                "Mixed" => "Mixed",
                other => panic!("unknown recipient group: {other}"),
            })
        }
        _ => panic!("unexpected recipient-group expression: {expr:?}"),
    }
}

fn evaluate(expr: &Expr, facts: Facts) -> bool {
    match expr {
        Expr::Bool(value) => *value,
        Expr::And(terms) => terms.iter().all(|term| evaluate(term, facts)),
        Expr::Or(terms) => terms.iter().any(|term| evaluate(term, facts)),
        Expr::Not(inner) => !evaluate(inner, facts),
        Expr::Eq(left, right) => match (left.as_ref(), right.as_ref()) {
            (inner, Expr::Bool(expected)) | (Expr::Bool(expected), inner) => {
                evaluate(inner, facts) == *expected
            }
            _ if format!("{expr:?}").contains("recipient_group_outcomes") => {
                group_value(left, facts) == group_value(right, facts)
            }
            // `lifecycle_phase == Phase::Active`: the only phase.
            _ if format!("{expr:?}").contains("lifecycle_phase") => true,
            _ => panic!("unexpected equality in a classify guard: {expr:?}"),
        },
        Expr::MapContainsKey { map, .. }
            if format!("{map:?}").contains("recipient_group_outcomes") =>
        {
            facts.recipient_group.is_some()
        }
        Expr::MapContainsKey { map, .. } if format!("{map:?}").contains("refused_deliveries") => {
            facts.refused
        }
        // The #1811 definedness conjunct: `delivery_ids`, `delivery_sequences`
        // and `delivery_source_sequences` are inserted together and never removed.
        Expr::MapContainsKey { map, .. } if format!("{map:?}").contains("delivery_sequences") => {
            facts.committed
        }
        Expr::Contains { collection, .. } => {
            let collection = format!("{collection:?}");
            if collection.contains("acknowledged_sequences") {
                facts.acknowledged
            } else if collection.contains("delivery_ids") {
                facts.committed
            } else {
                panic!("unexpected membership in a classify guard: {expr:?}")
            }
        }
        Expr::Lte(_, right) if format!("{right:?}").contains("applied_cursor") => {
            facts.at_or_below_cursor
        }
        _ => panic!("unexpected term in a classify guard: {expr:?}"),
    }
}

fn holds(guards: &[Guard], facts: Facts) -> bool {
    guards.iter().all(|guard| evaluate(&guard.expr, facts))
}

/// Every arm that holds for each assignment of the five facts.
fn verdicts(arms: &[(String, Vec<Guard>)]) -> Vec<(Facts, Vec<String>)> {
    let mut table = Vec::new();
    for committed in [false, true] {
        for refused in [false, true] {
            for at_or_below_cursor in [false, true] {
                for acknowledged in [false, true] {
                    for recipient_group in [
                        None,
                        Some("AllApplied"),
                        Some("AllRefused"),
                        Some("AllAuthorizationUnavailable"),
                        Some("Mixed"),
                    ] {
                        let facts = Facts {
                            committed,
                            refused,
                            at_or_below_cursor,
                            acknowledged,
                            recipient_group,
                        };
                        let holding = arms
                            .iter()
                            .filter(|(_, guards)| holds(guards, facts))
                            .map(|(name, _)| name.clone())
                            .collect();
                        table.push((facts, holding));
                    }
                }
            }
        }
    }
    table
}

fn classify_arms() -> Vec<(String, Vec<Guard>)> {
    let schema = dsl_runtime_delivery_machine();
    let mut arms: Vec<(String, Vec<Guard>)> = schema
        .transitions
        .iter()
        .filter(|transition| match &transition.on {
            TriggerMatch::Input { variant, .. } => variant.as_ref() == CLASSIFY,
            TriggerMatch::Signal { .. } => false,
        })
        .map(|transition| {
            (
                transition.name.as_ref().to_string(),
                transition.guards.clone(),
            )
        })
        .collect();
    arms.sort_by(|left, right| left.0.cmp(&right.0));
    arms
}

#[test]
fn classify_delivery_status_arms_partition_every_delivery_id() {
    let arms = classify_arms();
    let names: Vec<&str> = arms.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names, ARMS,
        "ClassifyDeliveryStatus has exactly six verdict arms"
    );

    let table = verdicts(&arms);
    assert_eq!(
        table.len(),
        80,
        "all Boolean assignments and all five group values, without invariant filtering"
    );
    for (facts, holding) in table {
        assert_eq!(
            holding.len(),
            1,
            "exactly one verdict must hold for {facts:?}, got {holding:?}"
        );
    }
}

/// Drop the first `== false` negation found in `expr`, if any.
fn drop_one_negation(expr: &mut Expr) -> bool {
    match expr {
        Expr::Eq(left, right) if matches!(right.as_ref(), Expr::Bool(false)) => {
            *expr = (**left).clone();
            true
        }
        Expr::And(terms) | Expr::Or(terms) => terms.iter_mut().any(drop_one_negation),
        _ => false,
    }
}

#[test]
fn a_classify_guard_missing_one_negation_is_refused() {
    let arms = classify_arms();
    let mut refused = 0;
    for target in 0..arms.len() {
        let mut mutant = arms.clone();
        let Some(guard) = mutant[target]
            .1
            .iter_mut()
            .find(|guard| matches!(guard.expr, Expr::And(_) | Expr::Eq(..)))
        else {
            continue;
        };
        if !drop_one_negation(&mut guard.expr) {
            continue;
        }
        let partition_broken = verdicts(&mutant)
            .iter()
            .any(|(_, holding)| holding.len() != 1);
        assert!(
            partition_broken,
            "dropping a negation in {} must break the partition",
            mutant[target].0
        );
        refused += 1;
    }
    assert!(
        refused >= 4,
        "the negated arms each yield a refused mutant, got {refused}"
    );
}

const CONFLICT_REJECTION: &str = "RejectSourceSequenceConflict";
const REFUSAL_OBSERVATION: &str = "ObserveAlreadyRefusedDelivery";

fn is_read_only_arm(name: &str) -> bool {
    ARMS.contains(&name) || name == CONFLICT_REJECTION || name == REFUSAL_OBSERVATION
}

/// The read-only arms among `transitions` that update any state.
fn mutating_read_only_arms(transitions: &[TransitionSchema]) -> Vec<String> {
    transitions
        .iter()
        .filter(|transition| is_read_only_arm(transition.name.as_ref()))
        .filter(|transition| !transition.updates.is_empty())
        .map(|transition| transition.name.as_ref().to_string())
        .collect()
}

#[test]
fn classify_arms_and_the_conflict_rejection_leave_state_unchanged() {
    let schema = dsl_runtime_delivery_machine();
    let pinned = schema
        .transitions
        .iter()
        .filter(|transition| is_read_only_arm(transition.name.as_ref()))
        .count();
    assert_eq!(pinned, ARMS.len() + 2, "every read-only arm is present");
    assert_eq!(
        mutating_read_only_arms(&schema.transitions),
        Vec::<String>::new(),
        "classification, the conflict rejection and the refusal observation must have an empty update"
    );
}

/// tlc-gate mutant m4: the conflict rejection records the rejected source
/// sequence over the committed one.
#[test]
fn a_conflict_rejection_that_rewrites_the_stored_source_is_refused() {
    let mut transitions = dsl_runtime_delivery_machine().transitions;
    let source_write = transitions
        .iter()
        .flat_map(|transition| transition.updates.iter())
        .find(|update| {
            matches!(update, Update::MapInsert { field, .. }
                if field.as_ref() == "delivery_source_sequences")
        })
        .cloned()
        .expect("a commit records the source sequence");
    let rejection = transitions
        .iter_mut()
        .find(|transition| transition.name.as_ref() == CONFLICT_REJECTION)
        .expect("the conflict rejection");
    rejection.updates.push(source_write);
    assert_eq!(
        mutating_read_only_arms(&transitions),
        vec![CONFLICT_REJECTION.to_string()],
        "a rejection that writes the source sequence must be refused"
    );
}

/// A repeated refusal that re-records its reason is refused by the same pin.
#[test]
fn a_refusal_observation_that_writes_state_is_refused() {
    let mut transitions = dsl_runtime_delivery_machine().transitions;
    let refusal_write = transitions
        .iter()
        .flat_map(|transition| transition.updates.iter())
        .find(|update| {
            matches!(update, Update::MapInsert { field, .. }
                if field.as_ref() == "refused_deliveries")
        })
        .cloned()
        .expect("the settlement records the refusal");
    transitions
        .iter_mut()
        .find(|transition| transition.name.as_ref() == REFUSAL_OBSERVATION)
        .expect("the refusal observation")
        .updates
        .push(refusal_write);
    assert_eq!(
        mutating_read_only_arms(&transitions),
        vec![REFUSAL_OBSERVATION.to_string()]
    );
}

/// The repeated-refusal observation must guard on membership: a missing
/// key's lookup falls back to the enum default, so a guard that only
/// compares the stored reason would also hold for a row that was never
/// refused (an applied one), and refusing it would falsely observe a
/// refusal (tlc-gate r2; generator class #1811).
#[test]
fn the_refusal_observation_requires_membership_in_refused_deliveries() {
    fn mentions_membership(expr: &Expr) -> bool {
        match expr {
            Expr::MapContainsKey { map, .. } => format!("{map:?}").contains("refused_deliveries"),
            Expr::And(terms) => terms.iter().any(mentions_membership),
            _ => false,
        }
    }
    let schema = dsl_runtime_delivery_machine();
    let observation = schema
        .transitions
        .iter()
        .find(|transition| transition.name.as_ref() == REFUSAL_OBSERVATION)
        .expect("the refusal observation");
    assert!(
        observation
            .guards
            .iter()
            .any(|guard| mentions_membership(&guard.expr)),
        "the observation guard must require refused_deliveries to contain the id"
    );
}
