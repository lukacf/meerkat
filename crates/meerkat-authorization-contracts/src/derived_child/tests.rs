#![allow(clippy::expect_used)]

use std::collections::BTreeSet;

use serde_json::{Value, json};

use super::DerivedChildRestrictions;
use crate::constraints::{
    ActionRef, DelegationDepth, DelegationFailure, DepthBound, ExactRestriction,
    ExecutionRestrictions, LifetimeBound, LifetimeRestriction, SetBound, UnresolvedConstraint,
};

fn action(name: &str) -> ActionRef {
    ActionRef {
        feature: "private-fixture-feature".into(),
        action: name.into(),
    }
}

fn operands() -> (ExecutionRestrictions, ExecutionRestrictions) {
    let parent = ExecutionRestrictions {
        actions: ExactRestriction::exact([action("read"), action("write")]),
        resource_domains: ExactRestriction::exact([]),
        processors: ExactRestriction::exact([]),
        audiences: ExactRestriction::exact([]),
        lifetime: LifetimeRestriction::window(100, 300),
        delegation_depth: DelegationDepth::remaining(3),
    };
    let requested = ExecutionRestrictions {
        actions: ExactRestriction::exact([action("read"), action("delete")]),
        lifetime: LifetimeRestriction::window(150, 400),
        delegation_depth: DelegationDepth::remaining(9),
        ..ExecutionRestrictions::unrestricted()
    };
    (parent, requested)
}

fn derived() -> DerivedChildRestrictions {
    let (parent, requested) = operands();
    DerivedChildRestrictions::new(parent, requested).expect("valid mathematical operands")
}

fn wire() -> Value {
    serde_json::to_value(derived()).expect("serializable exact data")
}

fn rejected(value: Value) {
    assert!(serde_json::from_value::<DerivedChildRestrictions>(value).is_err());
}

#[test]
fn retains_exact_operands_and_literal_expected_attenuation() {
    let (parent, requested) = operands();
    let result = DerivedChildRestrictions::new(parent.clone(), requested.clone()).expect("child");
    assert_eq!(result.parent(), &parent);
    assert_eq!(result.requested(), &requested);
    let expected = ExecutionRestrictions {
        actions: ExactRestriction::exact([action("read")]),
        resource_domains: ExactRestriction::exact([]),
        processors: ExactRestriction::exact([]),
        audiences: ExactRestriction::exact([]),
        lifetime: LifetimeRestriction::window(150, 300),
        delegation_depth: DelegationDepth::remaining(2),
    };
    assert_eq!(result.effective(), &expected);
    let mut independent_copy = result.effective().clone();
    independent_copy.actions = ExactRestriction::unrestricted();
    assert_eq!(result.effective(), &expected);
    assert_ne!(result.effective(), &independent_copy);
}

#[test]
fn checked_wire_roundtrip_retains_all_three_values() {
    let original = derived();
    let encoded = serde_json::to_vec(&original).expect("encode");
    let decoded: DerivedChildRestrictions = serde_json::from_slice(&encoded).expect("decode");
    assert_eq!(original, decoded);
    assert_eq!(serde_json::to_vec(&decoded).expect("re-encode"), encoded);
}

#[test]
fn decode_rejects_widening_in_every_restriction_dimension() {
    for field in [
        "actions",
        "resource_domains",
        "processors",
        "audiences",
        "lifetime",
        "delegation_depth",
    ] {
        let mut value = wire();
        value["effective"][field]["bound"] = json!({"bound": "unrestricted"});
        rejected(value);
    }
}

#[test]
fn decode_requires_exact_effective_result_even_if_substitution_is_narrower() {
    for (field, bound) in [
        ("actions", json!({"bound": "exact", "values": []})),
        (
            "lifetime",
            json!({"bound": "window", "window": {"not_before_ms": 200, "expires_at_ms": 250}}),
        ),
        (
            "delegation_depth",
            json!({"bound": "remaining", "edges": 1}),
        ),
    ] {
        let mut value = wire();
        value["effective"][field]["bound"] = bound;
        rejected(value);
    }
}

#[test]
fn decode_rejects_depth_reset_and_both_lifetime_extensions() {
    for (field, bound) in [
        (
            "delegation_depth",
            json!({"bound": "remaining", "edges": 3}),
        ),
        (
            "lifetime",
            json!({"bound": "window", "window": {"not_before_ms": 149, "expires_at_ms": 300}}),
        ),
        (
            "lifetime",
            json!({"bound": "window", "window": {"not_before_ms": 150, "expires_at_ms": 301}}),
        ),
    ] {
        let mut value = wire();
        value["effective"][field]["bound"] = bound;
        rejected(value);
    }
}

#[test]
fn constructor_and_decoder_refuse_exhausted_or_unresolved_parent_depth() {
    let (parent, requested) = operands();
    for depth in [
        DelegationDepth::remaining(0),
        DelegationDepth::unresolved(UnresolvedConstraint::Absent),
        DelegationDepth::unresolved(UnresolvedConstraint::Unknown),
        DelegationDepth::unresolved(UnresolvedConstraint::Unavailable),
    ] {
        let mut changed_parent = parent.clone();
        changed_parent.delegation_depth = depth.clone();
        let result = DerivedChildRestrictions::new(changed_parent, requested.clone());
        match depth.bound() {
            DepthBound::Remaining(0) => assert_eq!(result, Err(DelegationFailure::Exhausted)),
            _ => assert_eq!(
                result,
                Err(DelegationFailure::Unresolved(
                    depth.unresolved_facts().clone()
                ))
            ),
        }
        let mut value = wire();
        value["parent"]["delegation_depth"] = serde_json::to_value(depth).expect("depth");
        rejected(value);
    }
}

#[test]
fn requested_zero_or_unresolved_depth_remains_restrictive_data() {
    let (parent, mut requested) = operands();
    requested.delegation_depth = DelegationDepth::remaining(0);
    let zero =
        DerivedChildRestrictions::new(parent.clone(), requested.clone()).expect("zero child");
    assert_eq!(
        zero.effective().delegation_depth.bound(),
        DepthBound::Remaining(0)
    );
    requested.delegation_depth = DelegationDepth::unresolved(UnresolvedConstraint::Unknown);
    let unresolved = DerivedChildRestrictions::new(parent, requested).expect("unresolved child");
    assert_eq!(
        unresolved.effective().delegation_depth.bound(),
        DepthBound::Remaining(2)
    );
    assert_eq!(
        unresolved.effective().delegation_depth.unresolved_facts(),
        &BTreeSet::from([UnresolvedConstraint::Unknown])
    );
    let mut value = serde_json::to_value(&unresolved).expect("wire");
    assert_eq!(
        serde_json::from_value::<DerivedChildRestrictions>(value.clone())
            .expect("exact unresolved"),
        unresolved
    );
    value["effective"]["delegation_depth"]["unresolved"] = json!([]);
    rejected(value);
}

#[test]
fn unresolved_facts_cannot_be_erased_in_any_dimension() {
    for field in [
        "actions",
        "resource_domains",
        "processors",
        "audiences",
        "lifetime",
        "delegation_depth",
    ] {
        let mut value = wire();
        // Requested depth uncertainty is a restrictive result; parent depth
        // uncertainty instead prevents deriving any child at all.
        value["requested"][field]["unresolved"] = json!(["unknown"]);
        value["effective"][field]["unresolved"] = json!(["unknown"]);
        let accepted: DerivedChildRestrictions =
            serde_json::from_value(value.clone()).expect("retained uncertainty");
        assert_eq!(serde_json::to_value(accepted).expect("wire"), value);
        value["effective"][field]["unresolved"] = json!([]);
        rejected(value);
    }
}

#[test]
fn empty_and_disjoint_bounds_do_not_become_unrestricted() {
    let (parent, mut requested) = operands();
    requested.actions = ExactRestriction::exact([action("delete")]);
    requested.lifetime = LifetimeRestriction::window(400, 500);
    let result =
        DerivedChildRestrictions::new(parent, requested).expect("empty mathematical result");
    assert_eq!(
        result.effective().actions.bound(),
        &SetBound::Exact(BTreeSet::new())
    );
    assert_eq!(result.effective().lifetime.bound(), LifetimeBound::Empty);
    let encoded = serde_json::to_vec(&result).expect("wire");
    assert_eq!(
        serde_json::from_slice::<DerivedChildRestrictions>(&encoded).expect("empty roundtrip"),
        result
    );
}

#[test]
fn missing_unknown_duplicate_and_malformed_fields_refuse() {
    for field in ["parent", "requested", "effective"] {
        let mut missing = wire();
        missing.as_object_mut().expect("object").remove(field);
        rejected(missing);
        let value = wire();
        let duplicate = format!(
            "{{\"{field}\":{},{}",
            value[field],
            serde_json::to_string(&value)
                .expect("wire")
                .trim_start_matches('{')
        );
        assert!(serde_json::from_str::<DerivedChildRestrictions>(&duplicate).is_err());
    }
    let mut unknown = wire();
    unknown["permit"] = json!(true);
    rejected(unknown);
    let mut malformed = wire();
    malformed["parent"]["lifetime"]["bound"]["window"]["expires_at_ms"] = json!("300");
    rejected(malformed);
    let mut nested_unknown = wire();
    nested_unknown["effective"]["actions"]["bypass"] = json!(true);
    rejected(nested_unknown);
}

#[test]
fn nested_reserved_unit_payload_nulls_are_not_silently_ignored() {
    let (parent, _) = operands();
    let original = DerivedChildRestrictions::new(parent, ExecutionRestrictions::unrestricted())
        .expect("valid unrestricted request");
    let baseline = serde_json::to_value(&original).expect("wire");
    assert_eq!(
        serde_json::from_value::<DerivedChildRestrictions>(baseline.clone())
            .expect("valid control"),
        original
    );
    for (field, payload) in [
        ("actions", "values"),
        ("resource_domains", "values"),
        ("processors", "values"),
        ("audiences", "values"),
        ("lifetime", "window"),
        ("delegation_depth", "edges"),
    ] {
        let mut value = baseline.clone();
        value["requested"][field]["bound"][payload] = Value::Null;
        assert!(
            value["requested"][field]["bound"]
                .as_object()
                .expect("bound object")
                .contains_key(payload)
        );
        rejected(value.clone());
        value["requested"][field]["bound"]
            .as_object_mut()
            .expect("bound object")
            .remove(payload);
        assert_eq!(
            serde_json::from_value::<DerivedChildRestrictions>(value).expect("payload removed"),
            original
        );
    }
}

#[test]
fn changed_operands_cannot_retain_a_result_from_another_derivation() {
    for operand in ["parent", "requested"] {
        let mut value = wire();
        value[operand]["actions"]["bound"] = json!({"bound": "exact", "values": []});
        rejected(value);
    }
    // Another mathematically valid parent is allowed as data. Actual retained
    // parent identity/currentness must be checked by the generated grant owner.
    let (mut parent, requested) = operands();
    parent.lifetime = LifetimeRestriction::window(125, 350);
    let other = DerivedChildRestrictions::new(parent, requested).expect("different valid relation");
    assert_ne!(other.parent(), derived().parent());
    assert_eq!(
        serde_json::from_value::<DerivedChildRestrictions>(
            serde_json::to_value(&other).expect("wire")
        )
        .expect("valid data"),
        other
    );
}

#[test]
fn diagnostics_do_not_expose_private_restriction_values() {
    assert_eq!(
        format!("{:?}", derived()),
        "DerivedChildRestrictions { .. }"
    );
    let mut value = wire();
    value["effective"]["actions"]["bound"] = json!({"bound": "unrestricted"});
    let error = serde_json::from_value::<DerivedChildRestrictions>(value).expect_err("mismatch");
    assert_eq!(
        error.to_string(),
        "child restrictions differ from the exact attenuation result"
    );
    assert!(!error.to_string().contains("private-fixture-feature"));
}

#[test]
fn readonly_view_is_detached_from_checked_value_and_wire_has_no_wrapper() {
    let derived = derived();
    let before = wire();
    let mut view = std::ops::Deref::deref(&derived).clone();
    view.effective = ExecutionRestrictions::unrestricted();
    assert_ne!(view.effective, *derived.effective());
    assert_eq!(serde_json::to_value(&derived).expect("encode"), before);
    assert!(before.get("view").is_none());
}
