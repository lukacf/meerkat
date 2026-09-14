use std::collections::BTreeSet;

use meerkat_core::execution_scope::{
    DescendantExecutionScopeRecord, RunEffectScopeRecord, RunExecutionAuthority,
    RunExecutionAuthorityRecord, ScopedEffectClaimRecord, ScopedEffectStartPermit,
    ScopedRunAuthority, ScopedRunPolicyRecord,
};
use meerkat_core::ops::ToolAccessPolicy;
use meerkat_core::{ToolExecutionPolicy, ToolMutationClass};
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn immutable_policy_identity_is_canonical_content_not_a_synthetic_revision() -> TestResult {
    let expected = meerkat_core::PolicyDigest::parse(
        "sha256:5ce04c5d353afd9c107fa5be9c25852bde21133a6021399ae5af1031a380537e",
    )?;
    for constraints in [
        json!([
            {"type":"allow_names","value":["write","read"]},
            {"type":"read_only"}
        ]),
        json!([
            {"type":"read_only"},
            {"type":"allow_names","value":["read","extra","write"]},
            {"type":"read_only"},
            {"type":"allow_names","value":["write","read"]}
        ]),
    ] {
        let policy = ToolExecutionPolicy::resolve(ToolAccessPolicy::Constraints(
            serde_json::from_value(constraints)?,
        ))?;
        assert_eq!(policy.content_digest()?, expected);
        assert_eq!(policy.clone().content_digest()?, expected);
    }
    let unrestricted = ToolExecutionPolicy::unrestricted();
    let empty_deny =
        ToolExecutionPolicy::resolve(ToolAccessPolicy::DenyList(meerkat_core::ToolNameSet::new()))?;
    assert_eq!(unrestricted.content_digest()?, empty_deny.content_digest()?);
    assert_ne!(unrestricted.content_digest()?, expected);
    assert_ne!(
        ToolExecutionPolicy::resolve(ToolAccessPolicy::ReadOnly)?.content_digest()?,
        expected
    );
    Ok(())
}

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
    for pointer in ["/grant/generation", "/admission_commit/revision"] {
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
fn callback_continuation_content_binds_prior_scope_and_never_deserializes_permission() -> TestResult
{
    use meerkat_core::execution_scope::RunEffectScopeId;

    let scope_id = RunEffectScopeId::from_uuid(uuid::Uuid::from_u128(20));
    let mut image = record();
    image["callback_continuation"] = json!({
        "target": {
            "session_id": image["executor"]["session_id"],
            "run_id": uuid::Uuid::from_u128(21),
            "execution_scope": uuid::Uuid::from_u128(22),
            "execution_boundary": uuid::Uuid::from_u128(23),
            "batch_digest": vec![255; 32],
        },
        "results_digest": vec![254; 32],
    });
    let record: RunEffectScopeRecord = serde_json::from_value(image.clone())?;
    record.validate_callback_continuation(scope_id)?;
    assert_eq!(serde_json::to_value(&record)?, image);
    assert!(
        serde_json::from_value::<RunExecutionAuthority>(json!({
            "kind": "scoped",
            "scope_id": scope_id,
            "record": image,
        }))
        .is_err()
    );
    for (field, replacement) in [
        ("session_id", json!(uuid::Uuid::from_u128(24))),
        ("run_id", json!(record.run_id)),
        ("execution_scope", json!(scope_id)),
        ("execution_scope", Value::Null),
    ] {
        let mut wrong = image.clone();
        wrong["callback_continuation"]["target"][field] = replacement;
        let wrong: RunEffectScopeRecord = serde_json::from_value(wrong)?;
        assert!(wrong.validate_callback_continuation(scope_id).is_err());
    }
    for field in ["target", "results_digest"] {
        let mut wrong = image.clone();
        wrong["callback_continuation"]
            .as_object_mut()
            .ok_or("continuation")?
            .remove(field);
        assert!(serde_json::from_value::<RunEffectScopeRecord>(wrong).is_err());
    }
    image["callback_continuation"]["results_digest"] = json!([1, 2, 3]);
    assert!(serde_json::from_value::<RunEffectScopeRecord>(image).is_err());
    Ok(())
}

#[test]
fn scope_preserves_session_owned_zero_binding_generation_without_zero_grant_authority() -> TestResult
{
    let mut image = record();
    image["executor"]["binding_generation"] = json!(0);
    let scope: RunEffectScopeRecord = serde_json::from_value(image.clone())?;
    assert_eq!(scope.executor.binding_generation, 0);
    assert_eq!(serde_json::to_value(scope)?, image);
    for invalid in [json!(-1), json!(0.5), json!(null), json!("0")] {
        let mut changed = image.clone();
        changed["executor"]["binding_generation"] = invalid;
        assert!(serde_json::from_value::<RunEffectScopeRecord>(changed).is_err());
    }
    image["grant"]["generation"] = json!(0);
    assert!(serde_json::from_value::<RunEffectScopeRecord>(image).is_err());
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
        parent_request_id: source.request_id,
        parent_input_id: source.input_id,
        parent_run_id: source.run_id,
        parent_executor: source.executor,
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
    let _: Option<ScopedEffectStartPermit<meerkat_core::ToolName>> = None;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolTarget {
    name: meerkat_core::ToolName,
    arguments_digest: [u8; 32],
    mutation: ToolMutationClass,
}

fn claim() -> Value {
    let scope = record();
    json!({
        "claim_id":uuid::Uuid::from_u128(10),
        "scope_id":uuid::Uuid::from_u128(11),
        "request_id":scope["request_id"],
        "effect_id":uuid::Uuid::from_u128(12),
        "target":{"name":"read","arguments_digest":vec![4;32],"mutation":"read_only"},
        "executor":scope["executor"],
        "input_id":scope["input_id"],
        "run_id":scope["run_id"],
        "grant":scope["grant"],
        "candidate_policy_revision":{
            "kind":"trusted_host",
            "ordinary_policy":"sha256:5ce04c5d353afd9c107fa5be9c25852bde21133a6021399ae5af1031a380537e",
            "revision":5
        },
        "commit":{"revision":10,"digest":vec![5;32]}
    })
}

#[test]
fn effect_claim_binds_exact_target_arguments_run_executor_and_grant() -> TestResult {
    use meerkat_core::execution_scope::{RunEffectScopeId, ScopeRecordError};
    let scope: RunEffectScopeRecord = serde_json::from_value(record())?;
    let expected_scope = RunEffectScopeId::from_uuid(uuid::Uuid::from_u128(11));
    let image = claim();
    let original: ScopedEffectClaimRecord<ToolTarget> = serde_json::from_value(image.clone())?;
    assert_eq!(serde_json::to_value(&original)?, image);
    original.validate_scope_binding(expected_scope, &scope, &original.target)?;
    for pointer in [
        "/scope_id",
        "/request_id",
        "/input_id",
        "/run_id",
        "/executor/session_id",
        "/executor/runtime_epoch",
        "/grant/id",
    ] {
        let mut changed = image.clone();
        *changed.pointer_mut(pointer).ok_or("claim field")? = json!(uuid::Uuid::from_u128(99));
        let claim: ScopedEffectClaimRecord<ToolTarget> = serde_json::from_value(changed)?;
        assert_eq!(
            claim.validate_scope_binding(expected_scope, &scope, &original.target),
            Err(ScopeRecordError::ClaimScopeMismatch),
            "{pointer}",
        );
    }
    for pointer in ["/grant/generation", "/executor/binding_generation"] {
        let mut changed = image.clone();
        *changed.pointer_mut(pointer).ok_or("claim generation")? = json!(99);
        let claim: ScopedEffectClaimRecord<ToolTarget> = serde_json::from_value(changed)?;
        assert!(
            claim
                .validate_scope_binding(expected_scope, &scope, &original.target)
                .is_err()
        );
    }
    for (field, value) in [
        ("name", json!("write")),
        ("arguments_digest", json!(vec![9; 32])),
        ("mutation", json!("mutating")),
    ] {
        let mut changed = image.clone();
        changed["target"][field] = value;
        let claim: ScopedEffectClaimRecord<ToolTarget> = serde_json::from_value(changed)?;
        assert_eq!(
            claim.validate_scope_binding(expected_scope, &scope, &original.target),
            Err(ScopeRecordError::ClaimTargetMismatch)
        );
    }
    for revision in [1, 9] {
        let mut changed = image.clone();
        changed["commit"]["revision"] = json!(revision);
        let claim: ScopedEffectClaimRecord<ToolTarget> = serde_json::from_value(changed)?;
        assert_eq!(
            claim.validate_scope_binding(expected_scope, &scope, &original.target),
            Err(ScopeRecordError::ClaimBeforeAdmission)
        );
    }
    for field in [
        "target",
        "request_id",
        "executor",
        "grant",
        "candidate_policy_revision",
        "commit",
    ] {
        let mut missing = image.clone();
        missing.as_object_mut().ok_or("claim")?.remove(field);
        assert!(serde_json::from_value::<ScopedEffectClaimRecord<ToolTarget>>(missing).is_err());
    }
    Ok(())
}

#[test]
fn descendant_cannot_rebind_parent_request_run_or_revocation_reference() -> TestResult {
    use meerkat_core::execution_scope::RunEffectScopeId;
    let parent: RunEffectScopeRecord = serde_json::from_value(record())?;
    let parent_scope = RunEffectScopeId::from_uuid(uuid::Uuid::from_u128(11));
    let child = DescendantExecutionScopeRecord {
        parent_scope,
        parent_request_id: parent.request_id.clone(),
        parent_input_id: parent.input_id.clone(),
        parent_run_id: parent.run_id.clone(),
        parent_executor: parent.executor.clone(),
        grant: parent.grant.clone(),
        target: meerkat_core::SessionId::new(),
        intersected_policy: parent.policy.clone(),
    };
    child.validate_parent(parent_scope, &parent)?;
    let image = serde_json::to_value(&child)?;
    for pointer in [
        "/parent_scope",
        "/parent_request_id",
        "/parent_input_id",
        "/parent_run_id",
        "/parent_executor/session_id",
        "/parent_executor/runtime_epoch",
        "/grant/id",
    ] {
        let mut changed = image.clone();
        *changed.pointer_mut(pointer).ok_or("child field")? = json!(uuid::Uuid::from_u128(99));
        let child: DescendantExecutionScopeRecord<meerkat_core::SessionId> =
            serde_json::from_value(changed)?;
        assert!(
            child.validate_parent(parent_scope, &parent).is_err(),
            "{pointer}"
        );
    }
    Ok(())
}
