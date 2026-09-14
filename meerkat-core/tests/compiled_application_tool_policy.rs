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

struct PublicationState {
    generation: PolicyProviderGeneration,
    provenance: PolicyEvaluationProvenance,
}

struct PublicationProvider {
    provider_id: PolicyProviderId,
    policy_id: PolicyId,
    state: RwLock<PublicationState>,
}

impl ToolConsequenceNarrowingPolicy for PublicationProvider {
    fn provider_id(&self) -> &PolicyProviderId {
        &self.provider_id
    }

    fn generation(&self) -> PolicyProviderGeneration {
        self.state.read().unwrap().generation
    }

    fn snapshot(
        &self,
        policy_id: &PolicyId,
    ) -> Result<Arc<dyn ToolConsequencePolicySnapshot>, ToolConsequenceFailure> {
        assert_eq!(policy_id, &self.policy_id);
        Ok(Arc::new(Snapshot(
            self.state.read().unwrap().provenance.clone(),
        )))
    }

    fn publish_if_current(
        &self,
        policy_id: &PolicyId,
        generation: PolicyProviderGeneration,
        provenance: &PolicyEvaluationProvenance,
        publication: Box<dyn FnOnce() + '_>,
    ) -> Result<(), meerkat_core::PolicyPublicationError> {
        let current = self.state.read().unwrap();
        if policy_id != &self.policy_id
            || current.generation != generation
            || &current.provenance != provenance
        {
            return Err(meerkat_core::PolicyPublicationError::NotCurrent);
        }
        publication();
        drop(current);
        Ok(())
    }
}

fn publication_provider() -> Arc<PublicationProvider> {
    Arc::new(PublicationProvider {
        provider_id: PolicyProviderId::new("publication-owner").unwrap(),
        policy_id: PolicyId::new("read-policy").unwrap(),
        state: RwLock::new(PublicationState {
            generation: PolicyProviderGeneration(3),
            provenance: PolicyEvaluationProvenance {
                revision: PolicyRevision(7),
                digest: PolicyDigest::from_canonical_bytes(b"read-policy-v7"),
            },
        }),
    })
}

async fn allowed_evaluation(
    provider: Arc<dyn ToolConsequenceNarrowingPolicy>,
    policy_id: PolicyId,
) -> meerkat_core::AllowedToolConsequenceEvaluation {
    let provider_id = provider.provider_id().clone();
    let registry = Arc::new(
        ToolConsequencePolicyRegistry::new(
            vec![provider],
            PolicyEvaluationSupervisorConfig::default(),
            None,
        )
        .unwrap(),
    );
    let bound = registry
        .bind(
            MobMemberBinding {
                mob_id: "mob".to_string(),
                role: "reader".to_string(),
                member: "member".to_string(),
            },
            provider_id,
            policy_id,
        )
        .unwrap();
    let args = serde_json::value::RawValue::from_string(r#"{"path":"a.txt"}"#.into()).unwrap();
    bound
        .evaluate_with_witness(
            meerkat_core::ToolCallView {
                id: "read-call",
                name: "read_file",
                args: &args,
            },
            Some(meerkat_core::RunId::new()),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn allowed_witness_retains_actual_request_and_holds_owner_lock_through_publication() {
    let provider = publication_provider();
    let evaluation = allowed_evaluation(provider.clone(), provider.policy_id.clone()).await;
    assert_eq!(evaluation.request().provider_id, provider.provider_id);
    assert_eq!(evaluation.request().policy_id, provider.policy_id);
    assert_eq!(evaluation.request().tool_name.as_str(), "read_file");
    assert_eq!(evaluation.request().tool_call_id, "read-call");
    assert_eq!(evaluation.request().arguments_json, r#"{"path":"a.txt"}"#);
    assert!(evaluation.request().run_id.is_some());
    assert_eq!(evaluation.generation(), PolicyProviderGeneration(3));
    assert_eq!(evaluation.provenance().revision, PolicyRevision(7));
    let result = evaluation
        .publish_if_current(|| {
            assert!(provider.state.try_write().is_err());
            "actual-publication-result"
        })
        .unwrap();
    assert_eq!(result, "actual-publication-result");
    assert!(provider.state.try_write().is_ok());
}

#[tokio::test]
async fn evaluated_policy_changes_each_refuse_publication_after_await() {
    for change in 0..3 {
        let provider = publication_provider();
        let evaluation = allowed_evaluation(provider.clone(), provider.policy_id.clone()).await;
        {
            let mut current = provider.state.write().unwrap();
            match change {
                0 => current.generation = PolicyProviderGeneration(4),
                1 => current.provenance.revision = PolicyRevision(8),
                2 => {
                    current.provenance.digest =
                        PolicyDigest::from_canonical_bytes(b"same-revision-different-bytes");
                }
                _ => unreachable!(),
            }
        }
        let mut invoked = false;
        assert_eq!(
            evaluation.publish_if_current(|| invoked = true),
            Err(meerkat_core::PolicyPublicationError::NotCurrent)
        );
        assert!(!invoked);
    }
}

#[tokio::test]
async fn ordinary_policy_provider_without_publication_support_refuses_without_callback() {
    let provider = Arc::new(MutableProvider {
        provider_id: PolicyProviderId::new("ordinary-only").unwrap(),
        provenance: RwLock::new(PolicyEvaluationProvenance {
            revision: PolicyRevision(7),
            digest: PolicyDigest::from_canonical_bytes(b"ordinary-only-v7"),
        }),
        accepted_revision: 7,
    });
    let evaluation = allowed_evaluation(provider, PolicyId::new("ordinary-policy").unwrap()).await;
    let mut invoked = false;
    assert_eq!(
        evaluation.publish_if_current(|| invoked = true),
        Err(meerkat_core::PolicyPublicationError::Unsupported)
    );
    assert!(!invoked);
}

struct InvalidPublicationProvider {
    inner: Arc<PublicationProvider>,
    invoke_then_refuse: bool,
}

impl ToolConsequenceNarrowingPolicy for InvalidPublicationProvider {
    fn provider_id(&self) -> &PolicyProviderId {
        self.inner.provider_id()
    }

    fn generation(&self) -> PolicyProviderGeneration {
        self.inner.generation()
    }

    fn snapshot(
        &self,
        policy_id: &PolicyId,
    ) -> Result<Arc<dyn ToolConsequencePolicySnapshot>, ToolConsequenceFailure> {
        self.inner.snapshot(policy_id)
    }

    fn publish_if_current(
        &self,
        _policy_id: &PolicyId,
        _generation: PolicyProviderGeneration,
        _provenance: &PolicyEvaluationProvenance,
        publication: Box<dyn FnOnce() + '_>,
    ) -> Result<(), meerkat_core::PolicyPublicationError> {
        if self.invoke_then_refuse {
            publication();
            Err(meerkat_core::PolicyPublicationError::NotCurrent)
        } else {
            Ok(())
        }
    }
}

#[tokio::test]
async fn publication_provider_cannot_report_success_without_call_or_refusal_after_call() {
    for invoke_then_refuse in [false, true] {
        let inner = publication_provider();
        let policy_id = inner.policy_id.clone();
        let evaluation = allowed_evaluation(
            Arc::new(InvalidPublicationProvider {
                inner,
                invoke_then_refuse,
            }),
            policy_id,
        )
        .await;
        let mut invoked = false;
        let error = evaluation
            .publish_if_current(|| invoked = true)
            .expect_err("provider contract violations cannot be accepted");
        assert!(matches!(
            error,
            meerkat_core::PolicyPublicationError::ContractViolation { .. }
        ));
        assert_eq!(invoked, invoke_then_refuse);
    }
}
