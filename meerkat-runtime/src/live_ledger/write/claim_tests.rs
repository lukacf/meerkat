use super::restore_tests::scope_owner;
use super::*;
use crate::live_ledger::LiveEffectPolicyObservation;
use crate::live_ledger::authority::store::LiveRequestAuthorityError;
use meerkat_core::execution_scope::ScopedEffectTarget;
use meerkat_core::ops::{OperationId, ToolAccessPolicy};
use meerkat_core::{ToolExecutionPolicy, ToolMutationClass, ToolName, ToolNameSet};
use std::num::NonZeroU64;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

#[path = "claim_collision_tests.rs"]
mod collision;

pub(super) fn tool_target(name: &str, mutation: ToolMutationClass) -> ScopedEffectTarget {
    tool_target_for_call("call", name, mutation)
}

pub(super) fn tool_target_for_call(
    call_id: &str,
    name: &str,
    mutation: ToolMutationClass,
) -> ScopedEffectTarget {
    ScopedEffectTarget::ToolDispatch {
        call_id: call_id.into(),
        tool: ToolName::new(name),
        invocation_digest: [41; 32],
        mutation,
    }
}

fn policy_observation(
    policy: ToolExecutionPolicy,
    fence: Arc<dyn RuntimeStoreWriteFence>,
) -> TestResult<LiveEffectPolicyObservation> {
    Ok(LiveEffectPolicyObservation::new(
        policy,
        NonZeroU64::new(7).ok_or("policy revision")?,
        fence,
    )?)
}

pub(super) fn read_only_observation() -> TestResult<LiveEffectPolicyObservation> {
    policy_observation(
        ToolExecutionPolicy::resolve(ToolAccessPolicy::ReadOnly)?,
        current_fence(),
    )
}

struct ManagedPolicyState {
    generation: meerkat_core::PolicyProviderGeneration,
    provenance: meerkat_core::PolicyEvaluationProvenance,
}

struct ManagedPolicyProvider {
    id: meerkat_core::PolicyProviderId,
    policy_id: meerkat_core::PolicyId,
    current: Mutex<ManagedPolicyState>,
}

struct ManagedPolicySnapshot(meerkat_core::PolicyEvaluationProvenance);

impl meerkat_core::ToolConsequencePolicySnapshot for ManagedPolicySnapshot {
    fn provenance(&self) -> meerkat_core::PolicyEvaluationProvenance {
        self.0.clone()
    }

    fn evaluate(
        &self,
        _request: &meerkat_core::ToolConsequenceRequest,
    ) -> meerkat_core::ToolConsequenceVerdict {
        meerkat_core::ToolConsequenceVerdict::Allow
    }
}

impl meerkat_core::ToolConsequenceNarrowingPolicy for ManagedPolicyProvider {
    fn provider_id(&self) -> &meerkat_core::PolicyProviderId {
        &self.id
    }

    fn generation(&self) -> meerkat_core::PolicyProviderGeneration {
        self.current.lock().expect("test policy").generation
    }

    fn snapshot(
        &self,
        policy_id: &meerkat_core::PolicyId,
    ) -> Result<
        Arc<dyn meerkat_core::ToolConsequencePolicySnapshot>,
        meerkat_core::ToolConsequenceFailure,
    > {
        assert_eq!(policy_id, &self.policy_id);
        Ok(Arc::new(ManagedPolicySnapshot(
            self.current.lock().expect("test policy").provenance.clone(),
        )))
    }

    fn publish_if_current(
        &self,
        policy_id: &meerkat_core::PolicyId,
        generation: meerkat_core::PolicyProviderGeneration,
        provenance: &meerkat_core::PolicyEvaluationProvenance,
        publication: Box<dyn FnOnce() + '_>,
    ) -> Result<(), meerkat_core::PolicyPublicationError> {
        let current = self.current.lock().expect("test policy");
        if policy_id != &self.policy_id
            || generation != current.generation
            || provenance != &current.provenance
        {
            return Err(meerkat_core::PolicyPublicationError::NotCurrent);
        }
        publication();
        drop(current);
        Ok(())
    }
}

fn managed_policy_provider() -> TestResult<Arc<ManagedPolicyProvider>> {
    Ok(Arc::new(ManagedPolicyProvider {
        id: meerkat_core::PolicyProviderId::new("live-test-policy")?,
        policy_id: meerkat_core::PolicyId::new("reader")?,
        current: Mutex::new(ManagedPolicyState {
            generation: meerkat_core::PolicyProviderGeneration(19),
            provenance: meerkat_core::PolicyEvaluationProvenance {
                revision: meerkat_core::PolicyRevision(73),
                digest: meerkat_core::PolicyDigest::from_canonical_bytes(b"actual-policy-73"),
            },
        }),
    }))
}

fn managed_binding(
    provider: Arc<ManagedPolicyProvider>,
) -> TestResult<meerkat_core::BoundToolConsequencePolicy> {
    let registry = Arc::new(meerkat_core::ToolConsequencePolicyRegistry::new(
        vec![provider.clone()],
        meerkat_core::PolicyEvaluationSupervisorConfig::default(),
        None,
    )?);
    Ok(registry.bind(
        meerkat_core::MobMemberBinding {
            mob_id: "test-mob".into(),
            role: "reader".into(),
            member: "test-reader".into(),
        },
        provider.id.clone(),
        provider.policy_id.clone(),
    )?)
}

async fn managed_evaluation(
    provider: Arc<ManagedPolicyProvider>,
    call: meerkat_core::ToolCallView<'_>,
    run_id: meerkat_core::RunId,
) -> TestResult<meerkat_core::AllowedToolConsequenceEvaluation> {
    Ok(managed_binding(provider)?
        .evaluate_with_witness(call, Some(run_id))
        .await?)
}

#[derive(Default)]
struct EvaluatedReadTool {
    calls: AtomicU64,
}

#[async_trait::async_trait]
impl meerkat_core::AgentToolDispatcher for EvaluatedReadTool {
    fn tools(&self) -> Arc<[Arc<meerkat_core::ToolDef>]> {
        vec![Arc::new(meerkat_core::ToolDef::new(
            "allowed_tool",
            "An owned read-only test tool",
            serde_json::json!({"type":"object"}),
        ))]
        .into()
    }

    fn tool_mutation_class(&self, _name: &str) -> ToolMutationClass {
        ToolMutationClass::ReadOnly
    }

    async fn dispatch(
        &self,
        _call: meerkat_core::ToolCallView<'_>,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, meerkat_core::ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(meerkat_core::ToolError::execution_failed(
            "unexpected invocation",
        ))
    }
}

#[tokio::test]
async fn dispatcher_witness_claims_exact_native_tool_without_repairing_policy_or_target()
-> TestResult {
    for backend in backends() {
        for managed in [false, true] {
            let owned = OwnedFixture::new(backend).await?;
            let scope = owned.staged_scope().await?;
            let inner = Arc::new(EvaluatedReadTool::default());
            let ordinary = ToolExecutionPolicy::resolve(ToolAccessPolicy::ReadOnly)?;
            let mut dispatcher =
                meerkat_core::ExecutionPolicyGatedDispatcher::new(inner.clone(), ordinary.clone());
            let provider = managed_policy_provider()?;
            if managed {
                dispatcher = dispatcher.with_consequence_policy(managed_binding(provider.clone())?);
            }
            let args = serde_json::value::RawValue::from_string(r#"{"path":"actual"}"#.into())?;
            let evaluation = dispatcher
                .evaluate_for_scoped_run(
                    meerkat_core::ToolCallView {
                        id: "evaluated",
                        name: "allowed_tool",
                        args: &args,
                    },
                    &scope,
                )
                .await?;
            let target = evaluation.target()?;
            let revision = evaluation.revision()?;
            match &revision {
                meerkat_core::execution_scope::ScopedEffectPolicyRevision::Immutable {
                    ordinary_policy,
                } => {
                    assert!(!managed);
                    assert_eq!(ordinary_policy, &ordinary.content_digest()?);
                }
                meerkat_core::execution_scope::ScopedEffectPolicyRevision::Managed {
                    provider_id,
                    policy_id,
                    ..
                } => {
                    assert!(managed);
                    assert_eq!(provider_id, &provider.id);
                    assert_eq!(policy_id, &provider.policy_id);
                }
                meerkat_core::execution_scope::ScopedEffectPolicyRevision::TrustedHost {
                    ..
                } => {
                    return Err("dispatcher fabricated a trusted-host revision".into());
                }
            }
            let permit = owned
                .machine
                .claim_live_tool_effect(scope.clone(), OperationId::new(), evaluation)
                .await?;
            permit
                .claim()
                .validate_scope_binding(scope.scope_id(), scope.record(), &target)?;
            assert_eq!(permit.claim().candidate_policy_revision, revision);
            assert_eq!(inner.calls.load(Ordering::SeqCst), 0);
            owned
                .machine
                .settle_live_effect_not_started(
                    permit.into_not_started(),
                    crate::live_ledger::completion::LiveCompletionText::new("not invoked")?,
                )
                .await?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn dispatcher_witness_native_claim_refuses_changed_managed_policy() -> TestResult {
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let scope = owned.staged_scope().await?;
        let provider = managed_policy_provider()?;
        let inner = Arc::new(EvaluatedReadTool::default());
        let dispatcher = meerkat_core::ExecutionPolicyGatedDispatcher::new(
            inner.clone(),
            ToolExecutionPolicy::unrestricted(),
        )
        .with_consequence_policy(managed_binding(provider.clone())?);
        let args = serde_json::value::RawValue::from_string("{}".into())?;
        let evaluation = dispatcher
            .evaluate_for_scoped_run(
                meerkat_core::ToolCallView {
                    id: "stale",
                    name: "allowed_tool",
                    args: &args,
                },
                &scope,
            )
            .await?;
        let before = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?;
        provider
            .current
            .lock()
            .map_err(|_| "test policy")?
            .generation = meerkat_core::PolicyProviderGeneration(20);
        assert!(
            owned
                .machine
                .claim_live_tool_effect(scope, OperationId::new(), evaluation)
                .await
                .is_err()
        );
        assert_eq!(
            owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?,
            before
        );
        assert_eq!(inner.calls.load(Ordering::SeqCst), 0);
    }
    Ok(())
}

#[tokio::test]
async fn native_claim_publishes_under_actual_evaluated_policy_and_refuses_postawait_changes()
-> TestResult {
    for backend in backends() {
        for change in 0..4 {
            let owned = OwnedFixture::new(backend).await?;
            let scope = owned.staged_scope().await?;
            let provider = managed_policy_provider()?;
            let args = serde_json::value::RawValue::from_string(r#"{"path":"file"}"#.into())?;
            let call = meerkat_core::ToolCallView {
                id: "actual-read",
                name: "allowed_tool",
                args: &args,
            };
            let target = tool_target_for_call(call.id, call.name, ToolMutationClass::ReadOnly);
            let evaluation =
                managed_evaluation(provider.clone(), call, scope.record().run_id.clone()).await?;
            let observation = LiveEffectPolicyObservation::from_evaluated(
                ToolExecutionPolicy::resolve(ToolAccessPolicy::ReadOnly)?,
                evaluation,
                call,
                target.clone(),
            )?;
            let owner = scope_owner(&owned);
            let prepared = owner
                .prepare_effect_claim(scope, OperationId::new(), target, observation)
                .await?;
            let before = owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?;
            {
                let mut current = provider.current.lock().map_err(|_| "test policy")?;
                match change {
                    0 => {}
                    1 => current.generation = meerkat_core::PolicyProviderGeneration(20),
                    2 => current.provenance.revision = meerkat_core::PolicyRevision(74),
                    3 => {
                        current.provenance.digest =
                            meerkat_core::PolicyDigest::from_canonical_bytes(b"changed-bytes");
                    }
                    _ => unreachable!(),
                }
            }
            let outcome = owner.commit_effect_claim(prepared).await;
            if change == 0 {
                let permit = outcome?;
                assert_eq!(
                    permit.claim().candidate_policy_revision,
                    meerkat_core::execution_scope::ScopedEffectPolicyRevision::Managed {
                        ordinary_policy: ToolExecutionPolicy::resolve(ToolAccessPolicy::ReadOnly)?
                            .content_digest()?,
                        provider_id: provider.id.clone(),
                        policy_id: provider.policy_id.clone(),
                        generation: meerkat_core::PolicyProviderGeneration(19),
                        revision: NonZeroU64::new(73).ok_or("revision")?,
                        digest: meerkat_core::PolicyDigest::from_canonical_bytes(
                            b"actual-policy-73"
                        ),
                    }
                );
                owned
                    .machine
                    .settle_live_effect_not_started(
                        permit.into_not_started(),
                        crate::live_ledger::completion::LiveCompletionText::new("not invoked")?,
                    )
                    .await?;
            } else {
                assert!(outcome.is_err());
                assert_eq!(
                    owned
                        .fixture
                        .ops()?
                        .load_live_head(owned.fixture.session.id())
                        .await?,
                    before
                );
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn evaluated_live_policy_cannot_be_rebound_to_other_call_run_or_target() -> TestResult {
    let owned = OwnedFixture::new(Backend::Memory).await?;
    let scope = owned.staged_scope().await?;
    let args = serde_json::value::RawValue::from_string(r#"{"path":"file"}"#.into())?;
    let other_args = serde_json::value::RawValue::from_string(r#"{"path":"other"}"#.into())?;
    let call = meerkat_core::ToolCallView {
        id: "actual-read",
        name: "allowed_tool",
        args: &args,
    };
    let target = tool_target_for_call(call.id, call.name, ToolMutationClass::ReadOnly);
    for change in 0..8 {
        let evaluation = managed_evaluation(
            managed_policy_provider()?,
            call,
            if change == 4 {
                meerkat_core::RunId::new()
            } else {
                scope.record().run_id.clone()
            },
        )
        .await?;
        let candidate_call = match change {
            0 => meerkat_core::ToolCallView {
                id: "other-call",
                ..call
            },
            1 => meerkat_core::ToolCallView {
                name: "other-tool",
                ..call
            },
            2 => meerkat_core::ToolCallView {
                args: &other_args,
                ..call
            },
            _ => call,
        };
        let observation = LiveEffectPolicyObservation::from_evaluated(
            ToolExecutionPolicy::resolve(ToolAccessPolicy::ReadOnly)?,
            evaluation,
            candidate_call,
            if change == 3 {
                tool_target_for_call(call.id, "other-tool", ToolMutationClass::ReadOnly)
            } else if change == 7 {
                tool_target_for_call("other-call", call.name, ToolMutationClass::ReadOnly)
            } else {
                target.clone()
            },
        );
        if change < 4 || change == 7 {
            assert!(observation.is_err());
        } else {
            let changed_target = match change {
                5 => ScopedEffectTarget::ToolDispatch {
                    call_id: call.id.into(),
                    tool: ToolName::new(call.name),
                    invocation_digest: [42; 32],
                    mutation: ToolMutationClass::ReadOnly,
                },
                6 => tool_target_for_call(call.id, call.name, ToolMutationClass::Mutating),
                _ => target.clone(),
            };
            assert!(
                scope_owner(&owned)
                    .prepare_effect_claim(
                        scope.clone(),
                        OperationId::new(),
                        changed_target,
                        observation?,
                    )
                    .await
                    .is_err()
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn native_effect_claim_commits_one_permit_and_reserves_credits_without_rewriting_inputs()
-> TestResult {
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let scope = owned.staged_scope().await?;
        let runtime_id = LogicalRuntimeId::for_session(owned.fixture.session.id());
        let inputs = owned
            .fixture
            .store
            .load_input_states_with_versions(&runtime_id)
            .await?;
        let lifecycle = owned
            .fixture
            .store
            .observe_machine_lifecycle(&runtime_id)
            .await?;
        let source = owned
            .fixture
            .ops()?
            .lookup_live_source(&owned.source)
            .await?
            .ok_or("source")?;
        let before = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        let effect_id = OperationId::new();
        let target = tool_target("allowed_tool", ToolMutationClass::ReadOnly);
        let permit = owned
            .machine
            .claim_live_effect(
                scope.clone(),
                effect_id.clone(),
                target.clone(),
                read_only_observation()?,
            )
            .await?;
        let after = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        let claim = permit.claim();
        claim.validate_scope_binding(scope.scope_id(), scope.record(), &target)?;
        assert_eq!(claim.effect_id, effect_id);
        assert_eq!(
            claim.candidate_policy_revision,
            meerkat_core::execution_scope::ScopedEffectPolicyRevision::TrustedHost {
                ordinary_policy: ToolExecutionPolicy::resolve(ToolAccessPolicy::ReadOnly)?
                    .content_digest()?,
                revision: NonZeroU64::new(7).ok_or("revision")?,
            }
        );
        assert_eq!(claim.commit.revision.get(), after.reference.revision);
        assert_eq!(after.reference.revision, before.reference.revision + 1);
        assert_eq!(after.reference.event_count, before.reference.event_count);
        assert!(after.payload.reserved.records > before.payload.reserved.records);
        assert!(after.payload.reserved.encoded_bytes > before.payload.reserved.encoded_bytes);
        let state = crate::generated::live_request_state::decode(&after.payload.request_snapshot)?;
        let key = claim.claim_id.as_uuid().to_string();
        assert_eq!(state.claim_ids.len(), 1);
        assert_eq!(state.claim_effects.get(&key), Some(&effect_id.to_string()));
        assert_eq!(
            state.claim_targets.get(&key),
            Some(&serde_json::to_string(&target)?)
        );
        assert_eq!(
            state.claim_policy_revisions.get(&key),
            Some(&serde_json::to_string(&claim.candidate_policy_revision)?)
        );
        assert_eq!(
            state.claim_phases.get(&key),
            Some(&dsl::LiveEffectPhase::Claimed)
        );
        assert_eq!(state.claim_credit_spent_records.get(&key), Some(&0));
        assert_eq!(
            state
                .remaining_effects
                .get(&scope.record().request_id.to_string()),
            Some(&1)
        );
        let content = permit.into_claim();
        assert_eq!(
            serde_json::from_slice::<
                meerkat_core::execution_scope::ScopedEffectClaimRecord<ScopedEffectTarget>,
            >(&serde_json::to_vec(&content)?)?,
            content
        );
        assert!(
            owned
                .machine
                .claim_live_effect(scope.clone(), effect_id, target, read_only_observation()?,)
                .await
                .is_err()
        );
        assert_eq!(
            owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?,
            Some(after)
        );
        let after_inputs = owned
            .fixture
            .store
            .load_input_states_with_versions(&runtime_id)
            .await?;
        assert_eq!(after_inputs.exact_set_token(), inputs.exact_set_token());
        assert_eq!(
            after_inputs.input_set_revision(),
            inputs.input_set_revision()
        );
        assert_eq!(
            owned
                .fixture
                .store
                .observe_machine_lifecycle(&runtime_id)
                .await?,
            lifecycle
        );
        let after_source = owned
            .fixture
            .ops()?
            .lookup_live_source(&owned.source)
            .await?
            .ok_or("source")?;
        assert_eq!(after_source.bytes(), source.bytes());
        assert_eq!(after_source.digest(), source.digest());

        let _second = owned
            .machine
            .claim_live_effect(
                scope.clone(),
                OperationId::new(),
                ScopedEffectTarget::ModelComputation {
                    request_id: OperationId::new(),
                    attempt: 0,
                    invocation_digest: [42; 32],
                },
                read_only_observation()?,
            )
            .await?;
        let full = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?;
        assert!(
            owned
                .machine
                .claim_live_effect(
                    scope,
                    OperationId::new(),
                    ScopedEffectTarget::ModelComputation {
                        request_id: OperationId::new(),
                        attempt: 0,
                        invocation_digest: [43; 32]
                    },
                    read_only_observation()?,
                )
                .await
                .is_err()
        );
        assert_eq!(
            owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?,
            full
        );
    }
    Ok(())
}

#[tokio::test]
async fn native_effect_claim_intersects_ordinary_policy_scope_names_and_mutations() -> TestResult {
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let scope = owned.staged_scope().await?;
        let before = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?;
        for (target, ordinary) in [
            (
                tool_target("allowed_tool", ToolMutationClass::ReadOnly),
                ToolExecutionPolicy::resolve(ToolAccessPolicy::AllowList(ToolNameSet::new()))?,
            ),
            (
                tool_target("different_tool", ToolMutationClass::ReadOnly),
                ToolExecutionPolicy::unrestricted(),
            ),
            (
                tool_target("allowed_tool", ToolMutationClass::Mutating),
                ToolExecutionPolicy::unrestricted(),
            ),
            (
                tool_target("allowed_tool", ToolMutationClass::Unknown),
                ToolExecutionPolicy::unrestricted(),
            ),
            (
                ScopedEffectTarget::DescendantAdmission {
                    invocation_digest: [44; 32],
                },
                ToolExecutionPolicy::unrestricted(),
            ),
        ] {
            assert!(
                owned
                    .machine
                    .claim_live_effect(
                        scope.clone(),
                        OperationId::new(),
                        target,
                        policy_observation(ordinary, current_fence())?,
                    )
                    .await
                    .is_err()
            );
            assert_eq!(
                owned
                    .fixture
                    .ops()?
                    .load_live_head(owned.fixture.session.id())
                    .await?,
                before
            );
        }
    }
    Ok(())
}

struct PolicyRevisionFence {
    current: Mutex<u64>,
    expected: u64,
}

impl RuntimeStoreWriteFence for PolicyRevisionFence {
    fn execute_if_current(
        &self,
        operation: Box<dyn FnOnce() -> Result<(), RuntimeStoreError> + '_>,
    ) -> Result<RuntimeStoreWriteFenceOutcome, RuntimeStoreError> {
        let revision = self
            .current
            .lock()
            .map_err(|_| RuntimeStoreError::WriteFailed("poisoned test policy".into()))?;
        if *revision != self.expected {
            return Ok(RuntimeStoreWriteFenceOutcome::Conflict {
                reason: "test policy revision changed".into(),
            });
        }
        operation()?;
        drop(revision);
        Ok(RuntimeStoreWriteFenceOutcome::Applied)
    }
}

#[tokio::test]
async fn native_effect_claim_rechecks_policy_expiry_and_run_currentness_after_prepare() -> TestResult
{
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let scope = owned.staged_scope().await?;
        let policy = Arc::new(PolicyRevisionFence {
            current: Mutex::new(7),
            expected: 7,
        });
        let clock = Arc::new(AtomicU64::new(0));
        let observed_clock = Arc::clone(&clock);
        let owner = scope_owner(&owned)
            .with_clock(Arc::new(move || Ok(observed_clock.load(Ordering::SeqCst))));
        let pending = owner
            .prepare_effect_claim(
                scope.clone(),
                OperationId::new(),
                tool_target("allowed_tool", ToolMutationClass::ReadOnly),
                policy_observation(ToolExecutionPolicy::unrestricted(), policy.clone())?,
            )
            .await?;
        let before = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?;
        *policy.current.lock().map_err(|_| "policy lock")? = 8;
        assert!(matches!(
            owner.commit_effect_claim(pending).await,
            Err(LiveRequestAuthorityError::Store(
                RuntimeStoreError::WriteFenceConflict { .. }
            ))
        ));
        assert_eq!(
            owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?,
            before
        );
        let pending = owner
            .prepare_effect_claim(
                scope.clone(),
                OperationId::new(),
                tool_target("allowed_tool", ToolMutationClass::ReadOnly),
                read_only_observation()?,
            )
            .await?;
        clock.store(u64::MAX, Ordering::SeqCst);
        assert!(matches!(owner.commit_effect_claim(pending).await,
            Err(LiveRequestAuthorityError::Store(RuntimeStoreError::LiveRequestPublicationRejected { reason }))
                if reason.contains("changed before publication")));
        assert_eq!(
            owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?,
            before
        );
        clock.store(0, Ordering::SeqCst);
        let pending = owner
            .prepare_effect_claim(
                scope,
                OperationId::new(),
                tool_target("allowed_tool", ToolMutationClass::ReadOnly),
                read_only_observation()?,
            )
            .await?;
        owned
            .machine
            .stop_runtime_executor(owned.fixture.session.id(), "effect claim stop race")
            .await?;
        let stopped = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?;
        assert!(matches!(
            owner.commit_effect_claim(pending).await,
            Err(LiveRequestAuthorityError::Store(
                RuntimeStoreError::InputRowVersionConflict { .. }
                    | RuntimeStoreError::MachineLifecycleVersionConflict { .. }
            ))
        ));
        assert_eq!(
            owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?,
            stopped
        );
    }
    Ok(())
}

#[tokio::test]
async fn native_effect_claim_conflict_never_remints_a_permit() -> TestResult {
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let scope = owned.staged_scope().await?;
        let owner = scope_owner(&owned);
        let effect_id = OperationId::new();
        let target = tool_target("allowed_tool", ToolMutationClass::ReadOnly);
        let first = owner
            .prepare_effect_claim(
                scope.clone(),
                effect_id.clone(),
                target.clone(),
                read_only_observation()?,
            )
            .await?;
        let second = owner
            .prepare_effect_claim(
                scope.clone(),
                effect_id.clone(),
                target.clone(),
                read_only_observation()?,
            )
            .await?;
        let _permit = owner.commit_effect_claim(first).await?;
        let committed = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?;
        assert!(matches!(owner.commit_effect_claim(second).await,
            Err(LiveRequestAuthorityError::NotNewlyCommitted(outcome))
                if matches!(*outcome, LiveLedgerCommitOutcome::Conflict { .. })));
        assert!(matches!(
            owner
                .prepare_effect_claim(scope, effect_id, target, read_only_observation()?)
                .await,
            Err(LiveRequestAuthorityError::Transition(_))
        ));
        assert_eq!(
            owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?,
            committed
        );
    }
    Ok(())
}

#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn native_effect_claim_late_sqlite_failure_rolls_back_claim_and_credit_reservation()
-> TestResult {
    for backend in [Backend::WholeBlob, Backend::HeadCanonical] {
        let owned = OwnedFixture::new(backend).await?;
        let scope = owned.staged_scope().await?;
        let owner = scope_owner(&owned);
        let effect_id = OperationId::new();
        let target = tool_target("allowed_tool", ToolMutationClass::ReadOnly);
        let before = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?;
        let connection = rusqlite::Connection::open(&owned.fixture.path)?;
        connection.execute_batch(
            "CREATE TRIGGER reject_effect_claim BEFORE UPDATE ON runtime_live_heads
             BEGIN SELECT RAISE(ABORT, 'synthetic effect claim failure'); END;",
        )?;
        assert!(matches!(
            owner
                .claim_effect(
                    scope.clone(),
                    effect_id.clone(),
                    target.clone(),
                    read_only_observation()?
                )
                .await,
            Err(LiveRequestAuthorityError::Store(
                RuntimeStoreError::WriteFailed(_)
            ))
        ));
        assert_eq!(
            owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?,
            before
        );
        connection.execute_batch("DROP TRIGGER reject_effect_claim;")?;
        let permit = owner
            .claim_effect(scope, effect_id.clone(), target, read_only_observation()?)
            .await?;
        assert_eq!(permit.claim().effect_id, effect_id);
    }
    Ok(())
}
