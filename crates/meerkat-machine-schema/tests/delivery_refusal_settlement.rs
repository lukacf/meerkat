//! Refused settlement against the generated runtime-delivery kernel.
//!
//! Composition witnesses cannot say "this input must be refused" (a refused
//! input just stalls the script), so the arms that must have no transition are
//! asserted here on the real mutator:
//!
//! - a refusal only settles the row at the cursor, never an applied row, a
//!   row ahead of the cursor, or one acknowledged out of band;
//! - a refused row is never applied or acknowledged afterwards;
//! - repeating a settlement with the same reason observes it and changes
//!   nothing.

#![allow(clippy::expect_used)]

use meerkat_machine_schema::catalog::dsl::runtime_delivery::{
    DeliveryRefusalReason, RuntimeDeliveryEffect, RuntimeDeliveryInput,
    RuntimeDeliveryMachineAuthority, RuntimeDeliveryMachineMutator,
};

const REASON: DeliveryRefusalReason = DeliveryRefusalReason::NoAdmissibleWorkBinding;

/// Three committed rows `a`, `b`, `c` at sequences 1, 2, 3; nothing applied.
fn three_committed() -> RuntimeDeliveryMachineAuthority {
    let mut authority = RuntimeDeliveryMachineAuthority::new();
    for id in ["a", "b", "c"] {
        RuntimeDeliveryMachineMutator::apply(
            &mut authority,
            RuntimeDeliveryInput::CommitDelivery {
                delivery_id: id.to_string(),
                source_sequence: 1,
            },
        )
        .expect("commit");
    }
    authority
}

fn refuse(id: &str, sequence: u64) -> RuntimeDeliveryInput {
    RuntimeDeliveryInput::SettleRefusedDelivery {
        delivery_id: id.to_string(),
        delivery_sequence: sequence,
        reason: REASON,
    }
}

fn apply(id: &str, sequence: u64) -> RuntimeDeliveryInput {
    RuntimeDeliveryInput::MarkDeliveryApplied {
        delivery_id: id.to_string(),
        delivery_sequence: sequence,
    }
}

fn acknowledge(id: &str, sequence: u64) -> RuntimeDeliveryInput {
    RuntimeDeliveryInput::AcknowledgeDelivery {
        delivery_id: id.to_string(),
        delivery_sequence: sequence,
    }
}

/// The input has no transition and leaves the authority untouched.
fn refused(
    authority: &mut RuntimeDeliveryMachineAuthority,
    input: RuntimeDeliveryInput,
    what: &str,
) {
    let before = format!("{:?}", authority.state());
    assert!(
        RuntimeDeliveryMachineMutator::apply(authority, input).is_err(),
        "{what} must have no transition"
    );
    assert_eq!(
        format!("{:?}", authority.state()),
        before,
        "{what} changed state"
    );
}

#[test]
fn refusal_settles_only_the_row_at_the_cursor() {
    let mut authority = three_committed();

    // (b) Ahead of the cursor: no transition, the cursor never jumps.
    refused(
        &mut authority,
        refuse("b", 2),
        "a refusal ahead of the cursor",
    );

    // (e) At the cursor: settled, the cursor passes it, the refusal is recorded.
    let settled = RuntimeDeliveryMachineMutator::apply(&mut authority, refuse("a", 1))
        .expect("refuse the row at the cursor");
    assert!(settled.effects().iter().any(|effect| matches!(
        effect,
        RuntimeDeliveryEffect::DeliveryRefused { delivery_id, delivery_sequence: 1, reason }
            if delivery_id == "a" && *reason == REASON
    )));
    assert_eq!(authority.state().applied_cursor, 1);
    assert_eq!(authority.state().refused_deliveries.get("a"), Some(&REASON));

    // (f) The same settlement again observes it and changes nothing.
    let before = format!("{:?}", authority.state());
    let observed = RuntimeDeliveryMachineMutator::apply(&mut authority, refuse("a", 1))
        .expect("a repeated settlement is an observation");
    assert!(observed.effects().iter().any(|effect| matches!(
        effect,
        RuntimeDeliveryEffect::DeliveryRefused { delivery_id, .. } if delivery_id == "a"
    )));
    assert_eq!(format!("{:?}", authority.state()), before);

    // (c) A refused row is never applied or acknowledged.
    refused(&mut authority, apply("a", 1), "applying a refused row");
    refused(
        &mut authority,
        acknowledge("a", 1),
        "acknowledging a refused row",
    );

    // (a) An applied row has no refusal transition (the defaulted reason read
    // would otherwise observe a refusal that never happened).
    RuntimeDeliveryMachineMutator::apply(&mut authority, apply("b", 2)).expect("apply b");
    refused(&mut authority, refuse("b", 2), "refusing an applied row");
}

#[test]
fn an_acknowledged_row_is_never_refused() {
    let mut authority = three_committed();
    RuntimeDeliveryMachineMutator::apply(&mut authority, acknowledge("b", 2))
        .expect("acknowledge b ahead of a");

    // (d) Acknowledged ahead: its effect reached the runtime out of band, so it
    // is delivered, not refusable, before or after the cursor reaches it.
    refused(
        &mut authority,
        refuse("b", 2),
        "refusing an acknowledged-ahead row",
    );
    RuntimeDeliveryMachineMutator::apply(&mut authority, refuse("a", 1)).expect("refuse a");
    assert_eq!(
        authority.state().applied_cursor,
        1,
        "a refusal does not consume the acknowledged prefix by itself"
    );
    refused(
        &mut authority,
        refuse("b", 2),
        "refusing an acknowledged row at the cursor",
    );
    RuntimeDeliveryMachineMutator::apply(
        &mut authority,
        RuntimeDeliveryInput::AdvanceAcknowledgedPrefix {},
    )
    .expect("advance over b");
    assert_eq!(authority.state().applied_cursor, 2);
    assert!(!authority.state().refused_deliveries.contains_key("b"));
}

/// (g) A refusal is settled with one reason for good: a repeat naming
/// another reason has no transition (it never observes, and never rewrites
/// the stored reason), while a repeat with the settled reason still observes.
#[test]
fn a_repeated_refusal_with_another_reason_is_refused() {
    let mut authority = three_committed();
    RuntimeDeliveryMachineMutator::apply(&mut authority, refuse("a", 1))
        .expect("refuse the row at the cursor");
    refused(
        &mut authority,
        RuntimeDeliveryInput::SettleRefusedDelivery {
            delivery_id: "a".to_string(),
            delivery_sequence: 1,
            reason: DeliveryRefusalReason::AuthorityDenied,
        },
        "a repeated refusal with another reason",
    );
    assert_eq!(authority.state().refused_deliveries.get("a"), Some(&REASON));
    let before = format!("{:?}", authority.state());
    RuntimeDeliveryMachineMutator::apply(&mut authority, refuse("a", 1))
        .expect("the settled reason still observes");
    assert_eq!(format!("{:?}", authority.state()), before);
}

fn recipients() -> std::collections::BTreeMap<String, String> {
    [("a/first", "session-one"), ("a/second", "session-two")]
        .into_iter()
        .map(|(id, target)| (id.to_owned(), target.to_owned()))
        .collect()
}

fn bind(recipients: std::collections::BTreeMap<String, String>) -> RuntimeDeliveryInput {
    RuntimeDeliveryInput::BindDeliveryRecipients {
        delivery_id: "a".into(),
        delivery_sequence: 1,
        recipients,
    }
}

fn settle(
    recipient_id: &str,
    target: &str,
    outcome: meerkat_machine_schema::catalog::dsl::runtime_delivery::DeliveryRecipientOutcome,
) -> RuntimeDeliveryInput {
    RuntimeDeliveryInput::SettleDeliveryRecipient {
        delivery_id: "a".into(),
        delivery_sequence: 1,
        recipient_id: recipient_id.into(),
        target_binding: target.into(),
        outcome,
    }
}

fn finish() -> RuntimeDeliveryInput {
    RuntimeDeliveryInput::FinishDeliveryRecipients {
        delivery_id: "a".into(),
        delivery_sequence: 1,
    }
}

#[test]
fn recipient_binding_and_settlement_cannot_drop_retarget_or_reclassify_a_sibling() {
    use meerkat_machine_schema::catalog::dsl::runtime_delivery::{
        DeliveryRecipientGroupOutcome as G, DeliveryRecipientOutcome as O,
    };
    let mut authority = three_committed();
    RuntimeDeliveryMachineMutator::apply(&mut authority, bind(recipients()))
        .expect("bind committed recipients");
    let before = format!("{:?}", authority.state());
    RuntimeDeliveryMachineMutator::apply(&mut authority, bind(recipients()))
        .expect("same binding replay");
    assert_eq!(format!("{:?}", authority.state()), before);
    let mut retargeted = recipients();
    retargeted.insert("a/second".into(), "other-session".into());
    refused(&mut authority, bind(retargeted), "recipient retarget");
    let mut missing = recipients();
    missing.remove("a/second");
    refused(&mut authority, bind(missing), "recipient omission");
    refused(
        &mut authority,
        settle("a/first", "other-session", O::Applied),
        "wrong recipient target",
    );
    refused(
        &mut authority,
        finish(),
        "finish before any recipient settles",
    );
    RuntimeDeliveryMachineMutator::apply(
        &mut authority,
        settle("a/first", "session-one", O::Applied),
    )
    .expect("first effect settled");
    refused(
        &mut authority,
        finish(),
        "finish with one pending recipient",
    );
    refused(
        &mut authority,
        apply("a", 1),
        "whole-row apply over pending sibling",
    );
    refused(
        &mut authority,
        refuse("a", 1),
        "whole-row refusal over successful sibling",
    );
    refused(
        &mut authority,
        acknowledge("a", 1),
        "whole-row acknowledgement over pending sibling",
    );
    refused(
        &mut authority,
        settle("a/first", "session-one", O::Refused),
        "reclassify applied recipient",
    );
    RuntimeDeliveryMachineMutator::apply(
        &mut authority,
        settle("a/second", "session-two", O::Refused),
    )
    .expect("local refusal settles sibling");
    RuntimeDeliveryMachineMutator::apply(&mut authority, finish())
        .expect("all recipients locally settled");
    assert_eq!(authority.state().applied_cursor, 1);
    assert_eq!(
        authority.state().recipient_group_outcomes.get("a"),
        Some(&G::Mixed)
    );
    assert!(
        authority.state().refused_deliveries.is_empty(),
        "mixed success is not a whole-row refusal"
    );
    let after = format!("{:?}", authority.state());
    RuntimeDeliveryMachineMutator::apply(
        &mut authority,
        settle("a/first", "session-one", O::Applied),
    )
    .expect("same successful outcome is observed");
    RuntimeDeliveryMachineMutator::apply(&mut authority, finish())
        .expect("same parent settlement is observed");
    assert_eq!(format!("{:?}", authority.state()), after);
    RuntimeDeliveryMachineMutator::apply(&mut authority, apply("b", 2))
        .expect("healthy following delivery proceeds");
}

#[test]
fn recipient_authorization_unavailable_is_distinct_and_parent_summary_is_truthful() {
    use meerkat_machine_schema::catalog::dsl::runtime_delivery::{
        DeliveryRecipientGroupOutcome as G, DeliveryRecipientOutcome as O,
    };
    for (left, right, expected) in [
        (O::Applied, O::Applied, G::AllApplied),
        (O::Refused, O::Refused, G::AllRefused),
        (
            O::OperationAuthorizationUnavailable,
            O::OperationAuthorizationUnavailable,
            G::AllAuthorizationUnavailable,
        ),
        (O::Refused, O::OperationAuthorizationUnavailable, G::Mixed),
    ] {
        let mut authority = three_committed();
        RuntimeDeliveryMachineMutator::apply(&mut authority, bind(recipients()))
            .expect("bind recipients");
        RuntimeDeliveryMachineMutator::apply(
            &mut authority,
            settle("a/first", "session-one", left),
        )
        .expect("first settlement");
        RuntimeDeliveryMachineMutator::apply(
            &mut authority,
            settle("a/second", "session-two", right),
        )
        .expect("second settlement");
        RuntimeDeliveryMachineMutator::apply(&mut authority, finish()).expect("finish group");
        assert_eq!(
            authority.state().recipient_group_outcomes.get("a"),
            Some(&expected)
        );
        let classified = RuntimeDeliveryMachineMutator::apply(
            &mut authority,
            RuntimeDeliveryInput::ClassifyDeliveryStatus {
                delivery_id: "a".into(),
            },
        )
        .expect("classify parent");
        let reason = match expected {
            G::AllApplied | G::Mixed => None,
            G::AllRefused => Some(DeliveryRefusalReason::AuthorityDenied),
            G::AllAuthorizationUnavailable => {
                Some(DeliveryRefusalReason::OperationAuthorizationUnavailable)
            }
        };
        assert_eq!(
            authority.state().refused_deliveries.get("a").copied(),
            reason
        );
        assert!(
            classified
                .effects()
                .iter()
                .any(|effect| match (expected, effect) {
                    (
                        G::AllApplied,
                        RuntimeDeliveryEffect::DeliveryStatusApplied {
                            delivery_id,
                            delivery_sequence,
                        },
                    ) => delivery_id == "a" && *delivery_sequence == 1,
                    (
                        G::AllRefused | G::AllAuthorizationUnavailable,
                        RuntimeDeliveryEffect::DeliveryStatusRefused {
                            delivery_id,
                            delivery_sequence,
                            reason: observed,
                        },
                    ) => delivery_id == "a" && *delivery_sequence == 1 && Some(*observed) == reason,
                    (
                        G::Mixed,
                        RuntimeDeliveryEffect::DeliveryStatusMixed {
                            delivery_id,
                            delivery_sequence,
                            bindings,
                            outcomes,
                        },
                    ) => {
                        delivery_id == "a"
                            && *delivery_sequence == 1
                            && bindings == &recipients()
                            && outcomes.get("a/first") == Some(&left)
                            && outcomes.get("a/second") == Some(&right)
                    }
                    _ => false,
                })
        );
        // Every old whole-row path stays fenced, even after a unanimous
        // group has populated the existing refused-delivery map.
        refused(
            &mut authority,
            apply("a", 1),
            "whole-row apply after enrollment",
        );
        refused(
            &mut authority,
            acknowledge("a", 1),
            "whole-row acknowledgement after enrollment",
        );
        refused(
            &mut authority,
            RuntimeDeliveryInput::SettleRefusedDelivery {
                delivery_id: "a".into(),
                delivery_sequence: 1,
                reason: reason.unwrap_or(DeliveryRefusalReason::AuthorityDenied),
            },
            "whole-row refusal observation after enrollment",
        );
        let valid = authority.state().clone();
        RuntimeDeliveryMachineAuthority::recover_from_state(valid.clone())
            .expect("truthful group recovers");
        let mut false_row = valid;
        if reason.is_some() {
            false_row.refused_deliveries.remove("a");
        } else {
            false_row
                .refused_deliveries
                .insert("a".into(), DeliveryRefusalReason::AuthorityDenied);
        }
        assert!(
            RuntimeDeliveryMachineAuthority::recover_from_state(false_row).is_err(),
            "row refusal must agree with the exact recipient summary"
        );
    }
}

#[test]
fn applied_refused_applied_group_is_mixed_and_retry_only_observes_success() {
    use meerkat_machine_schema::catalog::dsl::runtime_delivery::{
        DeliveryRecipientGroupOutcome as G, DeliveryRecipientOutcome as O,
    };
    let mut authority = three_committed();
    let bindings: std::collections::BTreeMap<String, String> = [
        ("A".into(), "session-A".into()),
        ("B".into(), "session-B".into()),
        ("C".into(), "session-C".into()),
    ]
    .into();
    RuntimeDeliveryMachineMutator::apply(&mut authority, bind(bindings))
        .expect("complete three-recipient manifest");
    for (recipient, target, outcome) in [
        ("A", "session-A", O::Applied),
        ("B", "session-B", O::Refused),
        ("C", "session-C", O::Applied),
    ] {
        RuntimeDeliveryMachineMutator::apply(&mut authority, settle(recipient, target, outcome))
            .expect("settle exact recipient");
        if recipient != "C" {
            refused(&mut authority, finish(), "partial group cannot advance");
            assert_eq!(authority.state().applied_cursor, 0);
        }
    }
    RuntimeDeliveryMachineMutator::apply(&mut authority, finish()).expect("finish mixed group");
    assert_eq!(
        authority.state().recipient_group_outcomes.get("a"),
        Some(&G::Mixed)
    );
    assert!(!authority.state().refused_deliveries.contains_key("a"));
    let classified = RuntimeDeliveryMachineMutator::apply(
        &mut authority,
        RuntimeDeliveryInput::ClassifyDeliveryStatus {
            delivery_id: "a".into(),
        },
    )
    .expect("read mixed");
    let expected_outcomes: std::collections::BTreeMap<String, O> = [
        ("A".into(), O::Applied),
        ("B".into(), O::Refused),
        ("C".into(), O::Applied),
    ]
    .into();
    assert!(
        matches!(classified.effects(), [RuntimeDeliveryEffect::DeliveryStatusMixed { delivery_id, delivery_sequence: 1, bindings, outcomes }]
        if delivery_id == "a" && bindings.len() == 3 && outcomes == &expected_outcomes)
    );
    RuntimeDeliveryMachineMutator::apply(&mut authority, apply("b", 2)).expect("next row proceeds");
    assert_eq!(authority.state().applied_cursor, 2);
    let before = format!("{:?}", authority.state());
    let observed =
        RuntimeDeliveryMachineMutator::apply(&mut authority, settle("A", "session-A", O::Applied))
            .expect("retry observes A");
    assert!(
        matches!(observed.effects(), [RuntimeDeliveryEffect::DeliveryRecipientSettled { recipient_id, outcome: O::Applied, .. }] if recipient_id == "A")
    );
    RuntimeDeliveryMachineMutator::apply(&mut authority, finish()).expect("repeat group finish");
    assert_eq!(format!("{:?}", authority.state()), before);
    refused(
        &mut authority,
        settle("B", "session-B", O::Applied),
        "healing cannot revive B",
    );
}

#[test]
fn recipient_enrollment_cannot_claim_acknowledged_or_finished_legacy_deliveries() {
    let mut authority = three_committed();
    RuntimeDeliveryMachineMutator::apply(&mut authority, acknowledge("a", 1))
        .expect("legacy applied effect");
    refused(
        &mut authority,
        bind(recipients()),
        "retroactive enrollment of applied row",
    );
    RuntimeDeliveryMachineMutator::apply(&mut authority, acknowledge("c", 3))
        .expect("out-of-band effect ahead");
    refused(
        &mut authority,
        RuntimeDeliveryInput::BindDeliveryRecipients {
            delivery_id: "c".into(),
            delivery_sequence: 3,
            recipients: recipients(),
        },
        "enrollment of acknowledged-ahead row",
    );
}

fn same_subscription_settled_independently() -> RuntimeDeliveryMachineAuthority {
    use meerkat_machine_schema::catalog::dsl::runtime_delivery::{
        DeliveryRecipientGroupOutcome as G, DeliveryRecipientOutcome as O,
    };
    let mut authority = three_committed();
    for (id, sequence, target, outcome, summary) in [
        ("a", 1, "session-one", O::Applied, G::AllApplied),
        ("b", 2, "session-two", O::Refused, G::AllRefused),
    ] {
        RuntimeDeliveryMachineMutator::apply(
            &mut authority,
            RuntimeDeliveryInput::BindDeliveryRecipients {
                delivery_id: id.into(),
                delivery_sequence: sequence,
                recipients: [("shared-subscription".into(), target.into())].into(),
            },
        )
        .expect("the same subscription can independently receive another delivery");
        refused(
            &mut authority,
            RuntimeDeliveryInput::FinishDeliveryRecipients {
                delivery_id: id.into(),
                delivery_sequence: sequence,
            },
            "a previous delivery's settlement cannot complete this recipient",
        );
        RuntimeDeliveryMachineMutator::apply(
            &mut authority,
            RuntimeDeliveryInput::SettleDeliveryRecipient {
                delivery_id: id.into(),
                delivery_sequence: sequence,
                recipient_id: "shared-subscription".into(),
                target_binding: target.into(),
                outcome,
            },
        )
        .expect("each exact delivery owns its outcome");
        RuntimeDeliveryMachineMutator::apply(
            &mut authority,
            RuntimeDeliveryInput::FinishDeliveryRecipients {
                delivery_id: id.into(),
                delivery_sequence: sequence,
            },
        )
        .expect("each complete group advances once");
        assert_eq!(
            authority.state().recipient_group_outcomes.get(id),
            Some(&summary)
        );
    }
    authority
}

#[test]
fn same_subscription_in_different_deliveries_has_independent_settlement() {
    use meerkat_machine_schema::catalog::dsl::runtime_delivery::DeliveryRecipientOutcome as O;
    let authority = same_subscription_settled_independently();
    assert_eq!(authority.state().applied_cursor, 2);
    assert_eq!(
        authority.state().recipient_outcomes["a"].get("shared-subscription"),
        Some(&O::Applied)
    );
    assert_eq!(
        authority.state().recipient_outcomes["b"].get("shared-subscription"),
        Some(&O::Refused)
    );
}

#[test]
fn recovered_recipient_outcome_cannot_move_to_another_delivery() {
    let state = same_subscription_settled_independently().state().clone();
    RuntimeDeliveryMachineAuthority::recover_from_state(state.clone())
        .expect("real settled state recovers");
    let mut moved = state.clone();
    let outcome = moved
        .recipient_outcomes
        .remove("a")
        .expect("first delivery outcome");
    moved.recipient_outcomes.insert("c".into(), outcome);
    assert!(
        RuntimeDeliveryMachineAuthority::recover_from_state(moved).is_err(),
        "a committed but unenrolled delivery cannot inherit another group's outcomes"
    );
    let mut overwritten = state;
    let first = overwritten.recipient_outcomes["a"].clone();
    overwritten.recipient_outcomes.insert("b".into(), first);
    assert!(
        RuntimeDeliveryMachineAuthority::recover_from_state(overwritten).is_err(),
        "a shared subscription id cannot replace the other delivery's retained refusal"
    );
}
