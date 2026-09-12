use std::collections::BTreeSet;

use meerkat_core::execution_scope::{
    DescendantExecutionScopeRecord, RunEffectScopeRecord, RunExecutionAuthority,
    RunExecutionAuthorityRecord, ScopedEffectStartPermit, ScopedRunAuthority,
    ScopedRunPolicyRecord,
};
use meerkat_core::ops::ToolAccessPolicy;
use meerkat_core::{ToolExecutionPolicy, ToolMutationClass};
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn record() -> Value {
    json!({
        "format":"v1",
        "request_id":"00000000-0000-0000-0000-000000000001",
        "grant": {
            "id":"00000000-0000-0000-0000-000000000002",
            "issuer_realm":"owner", "generation":7
        },
        "executor": {
            "session_id":"00000000-0000-0000-0000-000000000003",
            "realm":"executor",
            "runtime_epoch":"00000000-0000-0000-0000-000000000004",
            "binding_generation":8
        },
        "input_id":"00000000-0000-0000-0000-000000000005",
        "run_id":"00000000-0000-0000-0000-000000000006",
        "admission_commit":{"revision":9, "digest":vec![3;32]},
        "parent_scope":null,
        "policy":{
            "tool_access":{"type":"read_only"},
            "allowed_mutations":["read_only"]
        },
        "remaining":{"model_computations":0,"tool_dispatches":0,"descendant_admissions":0}
    })
}

#[test]
fn exact_scope_content_round_trips_without_private_final_turn_proof() -> TestResult {
    let value = record();
    let scope: RunEffectScopeRecord = serde_json::from_value(value.clone())?;
    assert_eq!(serde_json::to_value(scope)?, value);
    for forbidden in [
        "provider_turn_id",
        "final_transcript",
        "bridge_proof",
        "native_tools",
    ] {
        let mut value = value.clone();
        value[forbidden] = json!("must-not-become-authority");
        assert!(serde_json::from_value::<RunEffectScopeRecord>(value).is_err());
    }
    Ok(())
}

#[test]
fn scope_record_requires_commit_grant_executor_and_exact_actual_input_run() -> TestResult {
    for field in [
        "grant",
        "executor",
        "input_id",
        "run_id",
        "admission_commit",
        "policy",
        "remaining",
    ] {
        let mut value = record();
        value.as_object_mut().ok_or("record")?.remove(field);
        assert!(
            serde_json::from_value::<RunEffectScopeRecord>(value).is_err(),
            "{field} must remain mandatory"
        );
    }
    for pointer in [
        "/grant/generation",
        "/executor/binding_generation",
        "/admission_commit/revision",
    ] {
        let mut value = record();
        *value.pointer_mut(pointer).ok_or("required scope field")? = json!(0);
        assert!(serde_json::from_value::<RunEffectScopeRecord>(value).is_err());
    }
    let value: RunEffectScopeRecord = serde_json::from_value(record())?;
    assert_eq!(
        value.remaining.tool_dispatches, 0,
        "exhaustion is not default permission"
    );
    Ok(())
}

#[test]
fn scope_policy_rejects_unresolved_inherit_and_nested_permission_typos() -> TestResult {
    assert!(ScopedRunPolicyRecord::new(ToolAccessPolicy::Inherit, BTreeSet::new()).is_err());
    for policy in [
        json!({"type":"inherit"}),
        json!({"type":"constraints","value":[]}),
        json!({"type":"read_only","permission":"all"}),
        json!({"type":"constraints","value":[{"type":"read_only","permission":"all"}]}),
    ] {
        let mut value = record();
        value["policy"]["tool_access"] = policy;
        assert!(serde_json::from_value::<RunEffectScopeRecord>(value).is_err());
    }
    let ordinary: ToolAccessPolicy = serde_json::from_value(json!({"type":"inherit"}))?;
    assert_eq!(
        ordinary,
        ToolAccessPolicy::Inherit,
        "ordinary policy decoder is unchanged"
    );
    Ok(())
}

#[test]
fn descendant_policy_intersects_instead_of_replacing_parent_ceiling() -> TestResult {
    let parent = ScopedRunPolicyRecord::new(
        serde_json::from_value(json!({"type":"allow_list","value":["read","write"]}))?,
        BTreeSet::from([ToolMutationClass::ReadOnly]),
    )?;
    let child = ScopedRunPolicyRecord::new(
        serde_json::from_value(json!({"type":"allow_list","value":["write","escape"]}))?,
        BTreeSet::from([ToolMutationClass::ReadOnly, ToolMutationClass::Mutating]),
    )?;
    let joined = parent.intersect(&child)?;
    let gate = ToolExecutionPolicy::resolve(joined.tool_access().clone())?;
    assert!(gate.permits("write"));
    assert!(!gate.permits("read"));
    assert!(!gate.permits("escape"));
    assert_eq!(
        joined.allowed_mutations(),
        &BTreeSet::from([ToolMutationClass::ReadOnly])
    );
    let source: RunEffectScopeRecord = serde_json::from_value(record())?;
    let descendant = DescendantExecutionScopeRecord {
        parent_scope: meerkat_core::execution_scope::RunEffectScopeId::from_uuid(uuid::Uuid::nil()),
        grant: source.grant,
        target: meerkat_core::SessionId::new(),
        intersected_policy: joined,
    };
    assert_eq!(
        serde_json::from_value::<DescendantExecutionScopeRecord<meerkat_core::SessionId>>(
            serde_json::to_value(&descendant)?
        )?,
        descendant
    );
    Ok(())
}

#[test]
fn persisted_authority_choice_cannot_silently_drop_scope_into_session_policy() -> TestResult {
    for value in [
        json!({"kind":"session_policy","grant":"must-not-disappear"}),
        json!({"kind":"scoped"}),
        json!({"kind":"scoped","scope_id":uuid::Uuid::nil()}),
    ] {
        assert!(serde_json::from_value::<RunExecutionAuthorityRecord>(value).is_err());
    }
    let value = json!({"kind":"scoped","scope_id":uuid::Uuid::nil(),"record":record()});
    let decoded: RunExecutionAuthorityRecord = serde_json::from_value(value.clone())?;
    assert_eq!(serde_json::to_value(decoded)?, value);
    let ordinary = RunExecutionAuthority::SessionPolicy;
    assert!(matches!(ordinary, RunExecutionAuthority::SessionPolicy));
    let _: Option<ScopedRunAuthority> = None;
    let _: Option<ScopedEffectStartPermit> = None;
    Ok(())
}
