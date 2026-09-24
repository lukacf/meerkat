#![allow(clippy::unwrap_used, clippy::expect_used)]

use meerkat_core::{
    ApplicationToolPolicyBinding, CompiledApplicationToolPolicy,
    CompiledApplicationToolPolicyError, MobMemberBinding, PolicyDigest, PolicyEvaluationProvenance,
    PolicyEvaluationSupervisorConfig, PolicyId, PolicyProviderGeneration, PolicyProviderId,
    PolicyRevision, ToolConsequenceFailure, ToolConsequenceNarrowingPolicy,
    ToolConsequencePolicyRegistry, ToolConsequencePolicySnapshot, ToolConsequenceRequest,
    ToolConsequenceVerdict,
};
use std::sync::{Arc, RwLock};

const VALID: &[u8] = include_bytes!("fixtures/compiled_application_tool_policy_valid_v1.json");
const INVALID_UNKNOWN_FIELD: &[u8] =
    include_bytes!("fixtures/compiled_application_tool_policy_unknown_field_v1.json");
const INVALID_ABSENT_DEFAULT_DENY: &[u8] =
    include_bytes!("fixtures/compiled_application_tool_policy_absent_default_deny_v1.json");

#[test]
fn canonical_compiled_policy_fixture_round_trips_exactly() {
    let policy = CompiledApplicationToolPolicy::parse_canonical_json(VALID).unwrap();
    assert_eq!(policy.revision, PolicyRevision(7));
    assert!(policy.default_deny);
    assert_eq!(policy.members[0].member_identity, "alpha");
    assert_eq!(policy.canonical_json().unwrap(), VALID);
}

#[test]
fn unknown_meaningful_fields_fail_before_installation() {
    let error = CompiledApplicationToolPolicy::parse_canonical_json(INVALID_UNKNOWN_FIELD)
        .expect_err("unknown fields must be rejected");
    assert!(matches!(
        error,
        CompiledApplicationToolPolicyError::InvalidJson(ref detail)
            if detail.contains("unknown field `future_mode`")
    ));
}

#[test]
fn absent_default_deny_fails_before_installation() {
    let error = CompiledApplicationToolPolicy::parse_canonical_json(INVALID_ABSENT_DEFAULT_DENY)
        .expect_err("default_deny must be explicit");
    assert!(matches!(
        error,
        CompiledApplicationToolPolicyError::InvalidJson(ref detail)
            if detail.contains("missing field `default_deny`")
    ));
}

#[test]
fn application_policy_binding_rejects_unknown_fields() {
    for (binding, unknown_field) in [
        (
            r#"{"kind":"provider","provider_id":"homecore","policy_id":"household-tools","risk_tier":"r9"}"#,
            "risk_tier",
        ),
        (
            r#"{"kind":"unmanaged","provider_id":"homecore"}"#,
            "provider_id",
        ),
        (
            r#"{"kind":"inherit","policy_id":"household-tools"}"#,
            "policy_id",
        ),
    ] {
        let error = serde_json::from_str::<ApplicationToolPolicyBinding>(binding)
            .expect_err("every application policy binding variant must reject unknown fields");
        assert!(
            error
                .to_string()
                .contains(&format!("unknown field `{unknown_field}`")),
            "unexpected error for {binding}: {error}"
        );
    }
}

#[test]
fn digest_and_canonical_bytes_are_both_mandatory() {
    let mut policy = CompiledApplicationToolPolicy::parse_canonical_json(VALID).unwrap();
    policy.members[0].grants.pop();
    assert!(matches!(
        policy.validate(),
        Err(CompiledApplicationToolPolicyError::DigestMismatch)
    ));

    let pretty = serde_json::to_vec_pretty(
        &CompiledApplicationToolPolicy::parse_canonical_json(VALID).unwrap(),
    )
    .unwrap();
    assert!(matches!(
        CompiledApplicationToolPolicy::parse_canonical_json(&pretty),
        Err(CompiledApplicationToolPolicyError::NonCanonicalJson)
    ));
}

#[derive(Clone)]
struct Snapshot(PolicyEvaluationProvenance);

impl ToolConsequencePolicySnapshot for Snapshot {
    fn provenance(&self) -> PolicyEvaluationProvenance {
        self.0.clone()
    }

    fn evaluate(&self, _request: &ToolConsequenceRequest) -> ToolConsequenceVerdict {
        ToolConsequenceVerdict::Allow
    }
}

struct MutableProvider {
    provider_id: PolicyProviderId,
    provenance: RwLock<PolicyEvaluationProvenance>,
    accepted_revision: u64,
}

impl ToolConsequenceNarrowingPolicy for MutableProvider {
    fn provider_id(&self) -> &PolicyProviderId {
        &self.provider_id
    }

    fn generation(&self) -> PolicyProviderGeneration {
        PolicyProviderGeneration(self.provenance.read().unwrap().revision.0)
    }

    fn snapshot(
        &self,
        policy_id: &PolicyId,
    ) -> Result<Arc<dyn ToolConsequencePolicySnapshot>, ToolConsequenceFailure> {
        let provenance = self.provenance.read().unwrap().clone();
        if provenance.revision.0 < self.accepted_revision {
            return Err(ToolConsequenceFailure::RevisionRollback {
                provider_id: self.provider_id.clone(),
                policy_id: policy_id.clone(),
                accepted_revision: self.accepted_revision,
                observed_revision: provenance.revision.0,
            });
        }
        Ok(Arc::new(Snapshot(provenance)))
    }
}

#[test]
fn provider_owned_snapshot_pointer_rejects_revision_rollback() {
    let provider_id = PolicyProviderId::new("homecore").unwrap();
    let policy_id = PolicyId::new("household-tools").unwrap();
    let provider = Arc::new(MutableProvider {
        provider_id: provider_id.clone(),
        provenance: RwLock::new(PolicyEvaluationProvenance {
            revision: PolicyRevision(2),
            digest: PolicyDigest::from_canonical_bytes(b"revision-2"),
        }),
        accepted_revision: 2,
    });
    let registry = Arc::new(
        ToolConsequencePolicyRegistry::new(
            vec![provider.clone()],
            PolicyEvaluationSupervisorConfig::default(),
            None,
        )
        .unwrap(),
    );
    let member = MobMemberBinding {
        mob_id: "homecore".to_string(),
        role: "coordinator".to_string(),
        member: "alpha".to_string(),
    };
    registry
        .bind(member.clone(), provider_id.clone(), policy_id.clone())
        .unwrap();

    *provider.provenance.write().unwrap() = PolicyEvaluationProvenance {
        revision: PolicyRevision(1),
        digest: PolicyDigest::from_canonical_bytes(b"revision-1"),
    };
    assert!(matches!(
        registry.bind(member, provider_id, policy_id),
        Err(ToolConsequenceFailure::RevisionRollback {
            accepted_revision: 2,
            observed_revision: 1,
            ..
        })
    ));
}
