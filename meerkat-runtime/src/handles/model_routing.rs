//! Runtime impl of [`meerkat_core::handles::ModelRoutingHandle`].

use std::sync::Arc;

use meerkat_core::handles::{
    DslTransitionError, ModelRoutingHandle, StickyModelFallbackMachineCommit,
    StickyModelFallbackVisibilityPlan,
};
use meerkat_core::lifecycle::run_primitive::ModelId;
use meerkat_core::model_profile::ModelProfile;
use meerkat_core::{
    ModelProfileWitness, SessionLlmIdentity, StickyModelFallbackActivationProof, ToolFilter,
};

use super::HandleDslAuthority;
use crate::meerkat_machine::dsl as mm_dsl;

fn capability_surface_from_profile(profile: &ModelProfile) -> mm_dsl::SessionLlmCapabilitySurface {
    mm_dsl::SessionLlmCapabilitySurface {
        supports_temperature: profile.supports_temperature,
        supports_thinking: profile.supports_thinking,
        supports_reasoning: profile.supports_reasoning,
        inline_video: profile.inline_video,
        vision: profile.vision,
        image_input: profile.image_input,
        image_tool_results: profile.image_tool_results,
        supports_web_search: profile.supports_web_search,
        image_generation: profile.image_generation,
        realtime: profile.realtime,
        call_timeout_secs: profile.call_timeout_secs,
    }
}

/// Runtime-backed [`ModelRoutingHandle`] impl.
#[derive(Debug)]
pub struct RuntimeModelRoutingHandle {
    dsl: Arc<HandleDslAuthority>,
    visibility_owner: Option<Arc<crate::meerkat_machine::MachineToolVisibilityOwner>>,
}

struct RuntimeStickyModelFallbackMachineCommit {
    dsl: Arc<HandleDslAuthority>,
    visibility_owner: Arc<crate::meerkat_machine::MachineToolVisibilityOwner>,
    expected_authority: mm_dsl::MeerkatMachineAuthoritySnapshot,
    input: mm_dsl::MeerkatMachineInput,
}

impl StickyModelFallbackMachineCommit for RuntimeStickyModelFallbackMachineCommit {
    fn commit(self: Box<Self>) -> Result<(), DslTransitionError> {
        self.visibility_owner
            .commit_previewed_sticky_model_fallback(&self.dsl, &self.expected_authority, self.input)
    }
}

impl RuntimeModelRoutingHandle {
    /// Construct a handle backed by the session's shared DSL authority.
    pub fn new(dsl: Arc<HandleDslAuthority>) -> Self {
        Self {
            dsl,
            visibility_owner: None,
        }
    }

    /// Construct the session-owned handle with the canonical visibility
    /// projection that shares this DSL authority.
    pub(crate) fn new_with_visibility_owner(
        dsl: Arc<HandleDslAuthority>,
        visibility_owner: Arc<crate::meerkat_machine::MachineToolVisibilityOwner>,
    ) -> Self {
        Self {
            dsl,
            visibility_owner: Some(visibility_owner),
        }
    }

    /// Construct a handle backed by an ephemeral DSL authority.
    pub fn ephemeral() -> Self {
        Self::new(Arc::new(HandleDslAuthority::ephemeral()))
    }

    fn stage_authorized_sticky_model_fallback(
        &self,
        previous_identity: &SessionLlmIdentity,
        target_identity: &SessionLlmIdentity,
        target_profile: &ModelProfileWitness,
        visibility_plan: &StickyModelFallbackVisibilityPlan,
        retry_attempt: u32,
    ) -> Result<Box<dyn StickyModelFallbackMachineCommit>, DslTransitionError> {
        if !target_profile.matches_identity(target_identity) {
            return Err(DslTransitionError::guard_rejected(
                "ModelRoutingHandle::stage_sticky_model_fallback",
                "fallback profile witness must belong to the exact target identity",
            ));
        }
        let expected_filter = meerkat_core::capability_base_filter_for_image_tool_results(
            target_profile.profile().image_tool_results,
        );
        if visibility_plan.next_state.capability_base_filter != expected_filter {
            return Err(DslTransitionError::guard_rejected(
                "ModelRoutingHandle::stage_sticky_model_fallback",
                "fallback visibility plan does not match the core-authorized target profile",
            ));
        }
        let target_profile_provider = target_profile.provider();
        let target_profile_model = target_profile.model();
        let target_profile = target_profile.profile();
        if target_profile_provider != target_profile.provider
            || target_profile_model != target_identity.model
        {
            return Err(DslTransitionError::guard_rejected(
                "ModelRoutingHandle::stage_sticky_model_fallback",
                "fallback capability profile and exact provider/model provenance must be paired",
            ));
        }
        let input = mm_dsl::MeerkatMachineInput::CommitStickyModelFallback {
            previous_identity: mm_dsl::SessionLlmIdentity::from_domain(previous_identity),
            previous_visibility_state: mm_dsl::SessionToolVisibilityState::from_domain(
                &visibility_plan.previous_state,
            ),
            target_identity: mm_dsl::SessionLlmIdentity::from_domain(target_identity),
            target_model: target_identity.model.clone(),
            target_profile_provider: Some(target_profile_provider.into()),
            target_profile_model: Some(target_profile_model.to_string()),
            target_capability_surface: Some(capability_surface_from_profile(target_profile)),
            target_capability_surface_status: mm_dsl::SessionLlmCapabilitySurfaceStatus::Resolved,
            target_capability_base_filter: mm_dsl::ToolFilter::from_domain(
                &visibility_plan.next_state.capability_base_filter,
            ),
            target_realtime_capable: target_profile.realtime,
            view_image_tool_available: visibility_plan.view_image_tool_available,
            previous_view_image_visible: visibility_plan.previous_view_image_visible,
            next_view_image_visible: visibility_plan.next_view_image_visible,
            previous_active_visibility_revision: visibility_plan.previous_state.active_revision,
            previous_staged_visibility_revision: visibility_plan.previous_state.staged_revision,
            next_visibility_state: mm_dsl::SessionToolVisibilityState::from_domain(
                &visibility_plan.next_state,
            ),
            next_active_visibility_revision: visibility_plan.next_state.active_revision,
            tool_visibility_delta: mm_dsl::SessionToolVisibilityDelta {
                previous_capability_base_filter: mm_dsl::ToolFilter::from_domain(
                    &visibility_plan.previous_state.capability_base_filter,
                ),
                current_capability_base_filter: mm_dsl::ToolFilter::from_domain(
                    &visibility_plan.next_state.capability_base_filter,
                ),
                committed_visible_set_changed: visibility_plan.committed_visible_set_changed,
                revision_bumped: visibility_plan.revision_bumped,
            },
            retry_attempt: u64::from(retry_attempt),
        };
        let visibility_owner = self.visibility_owner.as_ref().ok_or_else(|| {
            DslTransitionError::no_matching(
                "ModelRoutingHandle::stage_sticky_model_fallback",
                "session-owned model routing handle is missing its canonical visibility owner",
            )
        })?;
        let expected_authority = self
            .dsl
            .preview_input(&input, "ModelRoutingHandle::stage_sticky_model_fallback")?;
        Ok(Box::new(RuntimeStickyModelFallbackMachineCommit {
            dsl: Arc::clone(&self.dsl),
            visibility_owner: Arc::clone(visibility_owner),
            expected_authority,
            input,
        }))
    }

    #[cfg(test)]
    pub(crate) fn commit_sticky_model_fallback_for_test(
        &self,
        previous_identity: &SessionLlmIdentity,
        target_identity: &SessionLlmIdentity,
        target_profile: &ModelProfileWitness,
        visibility_plan: &StickyModelFallbackVisibilityPlan,
        retry_attempt: u32,
    ) -> Result<(), DslTransitionError> {
        self.stage_authorized_sticky_model_fallback(
            previous_identity,
            target_identity,
            target_profile,
            visibility_plan,
            retry_attempt,
        )?
        .commit()
    }
}

impl ModelRoutingHandle for RuntimeModelRoutingHandle {
    fn set_baseline(
        &self,
        baseline_model: ModelId,
        realtime_capable: bool,
    ) -> Result<(), DslTransitionError> {
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::SetModelRoutingBaseline {
                baseline_model: baseline_model.to_string(),
                realtime_capable,
            },
            "ModelRoutingHandle::set_baseline",
        )
    }

    fn hydrate_llm_capability_surface(
        &self,
        identity: &SessionLlmIdentity,
        profile: Option<&ModelProfile>,
        capability_base_filter: &ToolFilter,
    ) -> Result<(), DslTransitionError> {
        let (current_capability_surface, current_capability_surface_status) = match profile {
            Some(profile) => (
                Some(capability_surface_from_profile(profile)),
                mm_dsl::SessionLlmCapabilitySurfaceStatus::Resolved,
            ),
            None => (None, mm_dsl::SessionLlmCapabilitySurfaceStatus::Unresolved),
        };
        // intra-machine: no route; dispatcher not applicable (handle targets the meerkat DSL directly, not a CompositionDispatcher seam)
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::HydrateSessionLlmState {
                current_identity: mm_dsl::SessionLlmIdentity::from_domain(identity),
                current_capability_surface,
                current_capability_surface_status,
                current_capability_base_filter: mm_dsl::ToolFilter::from_domain(
                    capability_base_filter,
                ),
            },
            "ModelRoutingHandle::hydrate_llm_capability_surface",
        )
    }

    fn stage_sticky_model_fallback(
        &self,
        activation: StickyModelFallbackActivationProof,
        visibility_plan: &StickyModelFallbackVisibilityPlan,
    ) -> Result<Box<dyn StickyModelFallbackMachineCommit>, DslTransitionError> {
        if activation.target_capability_base_filter()
            != &visibility_plan.next_state.capability_base_filter
        {
            return Err(DslTransitionError::guard_rejected(
                "ModelRoutingHandle::stage_sticky_model_fallback",
                "fallback visibility plan does not match the sealed activation capability",
            ));
        }
        self.stage_authorized_sticky_model_fallback(
            activation.previous_identity(),
            activation.target_identity(),
            activation.target_profile(),
            visibility_plan,
            activation.retry_attempt(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::ToolVisibilityOwner as _;

    fn identity(model: &str, provider: meerkat_core::Provider) -> SessionLlmIdentity {
        SessionLlmIdentity {
            model: model.to_string(),
            provider,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        }
    }

    fn empty_model_catalog() -> meerkat_core::ModelCatalog {
        meerkat_core::ModelCatalog {
            entries: &[],
            capabilities: &[],
            provider_defaults: &[],
            image_generation_models: &[],
            providers: &[],
            default_models: &[],
            image_generation_defaults: &[],
            global_default_model: "",
            provider_priority: &[],
        }
    }

    fn custom_profile_witness(
        model: &str,
        provider: meerkat_core::Provider,
        vision: bool,
    ) -> ModelProfileWitness {
        let mut config = meerkat_core::Config::default();
        config.models.custom.insert(
            model.to_string(),
            meerkat_core::config::CustomModelConfig {
                provider,
                display_name: Some(format!("Test {model}")),
                context_window: Some(128_000),
                max_output_tokens: Some(4096),
                vision: Some(vision),
                web_search: Some(false),
                call_timeout_secs: None,
            },
        );
        meerkat_core::ModelRegistry::from_config(&config, empty_model_catalog())
            .expect("custom-model registry")
            .profile_witness_for_provider(provider, model)
            .expect("custom-model profile witness")
    }

    #[test]
    fn sticky_fallback_requires_matching_machine_recovery_and_replaces_routing_truth() {
        let mut authority = mm_dsl::MeerkatMachineAuthority::new();
        authority
            .apply_signal(mm_dsl::MeerkatMachineSignal::Initialize)
            .expect("initialize authority");
        mm_dsl::MeerkatMachineMutator::apply(
            &mut authority,
            mm_dsl::MeerkatMachineInput::RegisterSession {
                session_id: mm_dsl::SessionId("session-fallback".to_string()),
            },
        )
        .expect("register session");

        let shared = Arc::new(std::sync::Mutex::new(authority));
        let visibility_owner = Arc::new(crate::meerkat_machine::MachineToolVisibilityOwner::new());
        visibility_owner.bind_dsl_authority(Arc::clone(&shared));
        let handle = RuntimeModelRoutingHandle::new_with_visibility_owner(
            Arc::new(HandleDslAuthority::from_shared(Arc::clone(&shared))),
            Arc::clone(&visibility_owner),
        );
        let previous = identity("claude-primary", meerkat_core::Provider::Anthropic);
        let target = identity("gpt-backup", meerkat_core::Provider::OpenAI);
        let target_profile =
            custom_profile_witness("gpt-backup", meerkat_core::Provider::OpenAI, false);
        let mismatched_profile = custom_profile_witness(
            "gpt-different-backup",
            meerkat_core::Provider::OpenAI,
            false,
        );
        let target_filter = ToolFilter::Deny(
            [meerkat_core::VIEW_IMAGE_TOOL_NAME.to_string()]
                .into_iter()
                .collect(),
        );
        let mut next_visibility_state = meerkat_core::SessionToolVisibilityState {
            capability_base_filter: target_filter.clone(),
            ..Default::default()
        };
        next_visibility_state.active_revision = 1;
        next_visibility_state.staged_revision = 1;
        let visibility_plan = StickyModelFallbackVisibilityPlan {
            previous_state: Default::default(),
            next_state: next_visibility_state.clone(),
            view_image_tool_available: true,
            previous_view_image_visible: true,
            next_view_image_visible: false,
            committed_visible_set_changed: true,
            revision_bumped: true,
        };
        handle
            .hydrate_llm_capability_surface(&previous, None, &ToolFilter::All)
            .expect("hydrate initial identity");

        {
            let mut authority = shared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let run_id = mm_dsl::RunId("run-fallback".to_string());
            for input in [
                mm_dsl::MeerkatMachineInput::Prepare {
                    session_id: mm_dsl::SessionId("session-fallback".to_string()),
                    run_id: run_id.clone(),
                },
                mm_dsl::MeerkatMachineInput::StartConversationRun {
                    run_id: run_id.clone(),
                    primitive_kind: mm_dsl::TurnPrimitiveKind::ConversationTurn,
                    admitted_content_shape: mm_dsl::ContentShape::Conversation,
                    vision_enabled: false,
                    image_tool_results_enabled: false,
                    max_extraction_retries: 0,
                },
                mm_dsl::MeerkatMachineInput::PrimitiveApplied {
                    run_id: run_id.clone(),
                },
                mm_dsl::MeerkatMachineInput::RecoverableFailure {
                    run_id,
                    failure_kind: mm_dsl::LlmRetryFailureKind::RetryableProviderError,
                    retry_attempt: 1,
                    max_retries: 1,
                    selected_delay_ms: 0,
                    error: "provider overloaded".to_string(),
                },
            ] {
                mm_dsl::MeerkatMachineMutator::apply(&mut *authority, input)
                    .expect("drive fallback recovery precondition");
            }
        }

        let mismatched_profile_provenance = handle
            .commit_sticky_model_fallback_for_test(
                &previous,
                &target,
                &mismatched_profile,
                &visibility_plan,
                1,
            )
            .expect_err("profile provenance for a different model must fail closed");
        assert_eq!(
            mismatched_profile_provenance.kind,
            meerkat_core::handles::DslRejectionKind::GuardRejected
        );
        {
            let authority = shared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(
                authority.state().current_session_llm_identity,
                Some(mm_dsl::SessionLlmIdentity::from_domain(&previous)),
                "rejected profile provenance must not mutate routing identity"
            );
        }

        let wrong_attempt = handle
            .commit_sticky_model_fallback_for_test(
                &previous,
                &target,
                &target_profile,
                &visibility_plan,
                2,
            )
            .expect_err("wrong retry attempt must fail closed");
        assert_eq!(
            wrong_attempt.kind,
            meerkat_core::handles::DslRejectionKind::GuardRejected
        );
        {
            let authority = shared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(
                authority.state().current_session_llm_identity,
                Some(mm_dsl::SessionLlmIdentity::from_domain(&previous)),
                "a rejected fallback attempt must not mutate routing identity"
            );
        }

        handle
            .commit_sticky_model_fallback_for_test(
                &previous,
                &target,
                &target_profile,
                &visibility_plan,
                1,
            )
            .expect("matching machine recovery should authorize fallback");

        let authority = shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = authority.state();
        assert_eq!(
            state.current_session_llm_identity,
            Some(mm_dsl::SessionLlmIdentity::from_domain(&target))
        );
        assert_eq!(
            state.model_routing_baseline_model.as_deref(),
            Some("gpt-backup")
        );
        assert_eq!(state.model_routing_baseline_realtime, Some(false));
        assert_eq!(state.model_routing_topology_epoch, 1);
        assert_eq!(state.active_visibility_revision, 1);
        assert_eq!(
            state.current_session_capability_base_filter,
            mm_dsl::ToolFilter::from_domain(&target_filter)
        );
        drop(authority);
        assert_eq!(
            visibility_owner.visibility_state().unwrap(),
            next_visibility_state,
            "the machine-owned visibility projection must mirror the same accepted transition"
        );
        let after_boundary = visibility_owner
            .boundary_applied()
            .expect("normal boundary after fallback must remain monotonic");
        assert_eq!(after_boundary.active_revision, 1);
        assert_eq!(after_boundary.staged_revision, 1);
    }

    #[test]
    fn standalone_authorities_share_turn_routing_and_visibility_for_fallback() {
        use meerkat_core::TurnStateHandle as _;

        let session_id = meerkat_core::SessionId::new();
        let previous = identity("claude-primary", meerkat_core::Provider::Anthropic);
        let target = identity("gpt-backup", meerkat_core::Provider::OpenAI);
        let authorities = crate::standalone_session_runtime_authorities(
            &session_id,
            &previous,
            None,
            &ToolFilter::All,
        )
        .expect("standalone authority bundle");

        let run_id = meerkat_core::lifecycle::RunId::new();
        authorities
            .turn_state()
            .start_conversation_run(
                run_id.clone(),
                meerkat_core::turn_execution_authority::TurnPrimitiveKind::ConversationTurn,
                meerkat_core::turn_execution_authority::ContentShape::Conversation,
                false,
                false,
                0,
            )
            .expect("standalone shared authority should admit the turn");
        authorities
            .turn_state()
            .primitive_applied(run_id.clone())
            .expect("primitive applied");
        authorities
            .turn_state()
            .recoverable_failure(
                run_id,
                meerkat_core::LlmRetrySchedule {
                    failure: meerkat_core::LlmRetryFailure {
                        provider: "anthropic".to_string(),
                        kind: meerkat_core::LlmRetryFailureKind::RetryableProviderError,
                        retry_after_ms: None,
                        duration_ms: None,
                        message: "overloaded".to_string(),
                    },
                    plan: meerkat_core::LlmRetryPlan {
                        attempt: 1,
                        max_retries: 1,
                        computed_delay_ms: 0,
                        selected_delay_ms: 0,
                        retry_after_hint_ms: None,
                        rate_limit_floor_applied: false,
                        budget_capped: false,
                    },
                },
            )
            .expect("recoverable failure");

        let target_filter = ToolFilter::Deny(
            [meerkat_core::VIEW_IMAGE_TOOL_NAME.to_string()]
                .into_iter()
                .collect(),
        );
        let mut next_state = meerkat_core::SessionToolVisibilityState {
            capability_base_filter: target_filter,
            ..Default::default()
        };
        next_state.active_revision = 1;
        next_state.staged_revision = 1;
        let plan = StickyModelFallbackVisibilityPlan {
            previous_state: Default::default(),
            next_state: next_state.clone(),
            view_image_tool_available: true,
            previous_view_image_visible: true,
            next_view_image_visible: false,
            committed_visible_set_changed: true,
            revision_bumped: true,
        };
        let target_profile =
            custom_profile_witness("gpt-backup", meerkat_core::Provider::OpenAI, false);
        authorities
            .commit_sticky_model_fallback_for_test(&previous, &target, &target_profile, &plan, 1)
            .expect("standalone sticky fallback must use the shared turn authority");

        assert_eq!(
            authorities
                .tool_visibility_owner()
                .visibility_state()
                .unwrap(),
            next_state,
            "standalone fallback must mirror the accepted visibility state"
        );
    }

    #[test]
    fn sticky_fallback_rebases_pending_staged_visibility_before_next_boundary() {
        use meerkat_core::{ToolVisibilityOwner as _, TurnStateHandle as _};

        let session_id = meerkat_core::SessionId::new();
        let previous = identity("claude-primary", meerkat_core::Provider::Anthropic);
        let target = identity("gpt-backup", meerkat_core::Provider::OpenAI);
        let authorities = crate::standalone_session_runtime_authorities(
            &session_id,
            &previous,
            None,
            &ToolFilter::All,
        )
        .expect("standalone authority bundle");
        let witness = meerkat_core::ToolVisibilityWitness {
            last_seen_provenance: Some(meerkat_core::ToolProvenance {
                kind: meerkat_core::ToolSourceKind::Builtin,
                source_id: meerkat_core::ToolSourceId::new("builtin:view_image"),
            }),
        };
        let view_image = meerkat_core::ToolName::from(meerkat_core::VIEW_IMAGE_TOOL_NAME);
        let witness_catalog = [(view_image.clone(), witness.clone())]
            .into_iter()
            .collect();
        authorities
            .tool_visibility_owner()
            .replace_filter_tool_authority_catalog(witness_catalog)
            .expect("install filter witness catalog");
        let pending_filter = ToolFilter::Deny([view_image.clone()].into_iter().collect());
        let pending_revision = authorities
            .tool_visibility_owner()
            .stage_persistent_filter(
                pending_filter.clone(),
                [(view_image, witness)].into_iter().collect(),
            )
            .expect("stage pending visibility filter");
        assert_eq!(pending_revision, meerkat_core::ToolScopeRevision(1));
        let previous_visibility = authorities
            .tool_visibility_owner()
            .visibility_state()
            .expect("pending visibility state");
        assert_eq!(previous_visibility.active_revision, 0);
        assert_eq!(previous_visibility.staged_revision, 1);

        let run_id = meerkat_core::lifecycle::RunId::new();
        authorities
            .turn_state()
            .start_conversation_run(
                run_id.clone(),
                meerkat_core::turn_execution_authority::TurnPrimitiveKind::ConversationTurn,
                meerkat_core::turn_execution_authority::ContentShape::Conversation,
                false,
                false,
                0,
            )
            .expect("admit turn");
        authorities
            .turn_state()
            .primitive_applied(run_id.clone())
            .expect("primitive applied");
        authorities
            .turn_state()
            .recoverable_failure(
                run_id,
                meerkat_core::LlmRetrySchedule {
                    failure: meerkat_core::LlmRetryFailure {
                        provider: "anthropic".to_string(),
                        kind: meerkat_core::LlmRetryFailureKind::RetryableProviderError,
                        retry_after_ms: None,
                        duration_ms: None,
                        message: "overloaded".to_string(),
                    },
                    plan: meerkat_core::LlmRetryPlan {
                        attempt: 1,
                        max_retries: 1,
                        computed_delay_ms: 0,
                        selected_delay_ms: 0,
                        retry_after_hint_ms: None,
                        rate_limit_floor_applied: false,
                        budget_capped: false,
                    },
                },
            )
            .expect("recoverable failure");

        let target_filter = ToolFilter::Deny(
            [meerkat_core::VIEW_IMAGE_TOOL_NAME.to_string()]
                .into_iter()
                .collect(),
        );
        let mut next_visibility = previous_visibility.clone();
        next_visibility.capability_base_filter = target_filter;
        next_visibility.active_revision = 2;
        next_visibility.staged_revision = 3;
        let plan = StickyModelFallbackVisibilityPlan {
            previous_state: previous_visibility,
            next_state: next_visibility.clone(),
            view_image_tool_available: true,
            previous_view_image_visible: true,
            next_view_image_visible: false,
            committed_visible_set_changed: true,
            revision_bumped: true,
        };
        let target_profile =
            custom_profile_witness("gpt-backup", meerkat_core::Provider::OpenAI, false);
        authorities
            .commit_sticky_model_fallback_for_test(&previous, &target, &target_profile, &plan, 1)
            .expect("fallback must rebase the pending staged revision");
        assert_eq!(
            authorities
                .tool_visibility_owner()
                .visibility_state()
                .unwrap(),
            next_visibility
        );

        let after_boundary = authorities
            .tool_visibility_owner()
            .boundary_applied()
            .expect("pending staged state must promote monotonically after fallback");
        assert_eq!(after_boundary.active_revision, 3);
        assert_eq!(after_boundary.staged_revision, 3);
        assert_eq!(after_boundary.active_filter, pending_filter);
    }

    #[derive(Clone)]
    struct PostFallbackReconfigureHost {
        identity: Arc<std::sync::Mutex<SessionLlmIdentity>>,
        visibility: Arc<std::sync::Mutex<meerkat_core::SessionToolVisibilityState>>,
        current_surface: crate::SessionLlmCapabilitySurface,
        target_identity: SessionLlmIdentity,
        target_surface: crate::SessionLlmCapabilitySurface,
    }

    #[async_trait::async_trait]
    impl crate::SessionLlmReconfigureHost for PostFallbackReconfigureHost {
        async fn hydrate_session_llm_state(
            &self,
            _session_id: &meerkat_core::SessionId,
        ) -> Result<crate::HydratedSessionLlmState, crate::RuntimeDriverError> {
            Ok(crate::HydratedSessionLlmState {
                current_identity: self
                    .identity
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone(),
                current_visibility_state: self
                    .visibility
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone(),
                current_capability_surface: Some(self.current_surface.clone()),
                capability_surface_status: crate::SessionLlmCapabilitySurfaceStatus::Resolved,
                base_tool_names: [meerkat_core::ToolName::from(
                    meerkat_core::VIEW_IMAGE_TOOL_NAME,
                )]
                .into_iter()
                .collect(),
            })
        }

        async fn resolve_target_session_llm_identity(
            &self,
            _request: &crate::SessionLlmReconfigureRequest,
            _current_identity: &SessionLlmIdentity,
        ) -> Result<crate::ResolvedSessionLlmReconfigure, crate::RuntimeDriverError> {
            Ok(crate::ResolvedSessionLlmReconfigure {
                target_identity: self.target_identity.clone(),
                target_capability_surface: self.target_surface.clone(),
            })
        }

        async fn apply_live_session_llm_identity(
            &self,
            _session_id: &meerkat_core::SessionId,
            identity: &SessionLlmIdentity,
        ) -> Result<(), crate::RuntimeDriverError> {
            *self
                .identity
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = identity.clone();
            Ok(())
        }

        async fn apply_live_session_tool_visibility_state(
            &self,
            _session_id: &meerkat_core::SessionId,
            visibility: Option<meerkat_core::SessionToolVisibilityState>,
        ) -> Result<(), crate::RuntimeDriverError> {
            *self
                .visibility
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                visibility.unwrap_or_default();
            Ok(())
        }

        async fn persist_live_session(
            &self,
            _session_id: &meerkat_core::SessionId,
        ) -> Result<(), crate::RuntimeDriverError> {
            Ok(())
        }

        async fn discard_live_session(
            &self,
            _session_id: &meerkat_core::SessionId,
        ) -> Result<(), crate::RuntimeDriverError> {
            Ok(())
        }
    }

    fn runtime_capability_surface(image_tool_results: bool) -> crate::SessionLlmCapabilitySurface {
        crate::SessionLlmCapabilitySurface {
            supports_temperature: true,
            supports_thinking: false,
            supports_reasoning: false,
            inline_video: false,
            vision: image_tool_results,
            image_input: image_tool_results,
            image_tool_results,
            supports_web_search: false,
            image_generation: false,
            realtime: false,
            call_timeout_secs: None,
        }
    }

    #[tokio::test]
    async fn prepared_bindings_fallback_projects_status_visibility_and_allows_reconfigure() {
        use crate::SessionServiceRuntimeExt as _;

        let machine = Arc::new(crate::MeerkatMachine::ephemeral());
        let session_id = meerkat_core::SessionId::new();
        let bindings = machine
            .prepare_bindings(session_id.clone())
            .await
            .expect("production bindings");
        let model_routing = machine
            .model_routing_handle_for_test(&session_id)
            .await
            .expect("concrete production routing handle");
        let previous = identity("claude-primary", meerkat_core::Provider::Anthropic);
        let target = identity("gpt-backup", meerkat_core::Provider::OpenAI);
        let target_profile =
            custom_profile_witness("gpt-backup", meerkat_core::Provider::OpenAI, false);
        bindings
            .model_routing()
            .set_baseline(
                meerkat_core::lifecycle::run_primitive::ModelId::new(previous.model.clone()),
                false,
            )
            .expect("baseline");
        bindings
            .model_routing()
            .hydrate_llm_capability_surface(&previous, None, &ToolFilter::All)
            .expect("initial identity hydration");

        let run_id = meerkat_core::lifecycle::RunId::new();
        bindings
            .session_admission()
            .prepare(&run_id)
            .expect("prepare run");
        bindings
            .turn_state()
            .start_conversation_run(
                run_id.clone(),
                meerkat_core::turn_execution_authority::TurnPrimitiveKind::ConversationTurn,
                meerkat_core::turn_execution_authority::ContentShape::Conversation,
                false,
                false,
                0,
            )
            .expect("start turn");
        bindings
            .turn_state()
            .primitive_applied(run_id.clone())
            .expect("primitive applied");
        bindings
            .turn_state()
            .recoverable_failure(
                run_id,
                meerkat_core::LlmRetrySchedule {
                    failure: meerkat_core::LlmRetryFailure {
                        provider: "anthropic".to_string(),
                        kind: meerkat_core::LlmRetryFailureKind::RetryableProviderError,
                        retry_after_ms: None,
                        duration_ms: None,
                        message: "overloaded".to_string(),
                    },
                    plan: meerkat_core::LlmRetryPlan {
                        attempt: 1,
                        max_retries: 1,
                        computed_delay_ms: 0,
                        selected_delay_ms: 0,
                        retry_after_hint_ms: None,
                        rate_limit_floor_applied: false,
                        budget_capped: false,
                    },
                },
            )
            .expect("recoverable failure");

        let target_filter = ToolFilter::Deny(
            [meerkat_core::VIEW_IMAGE_TOOL_NAME.to_string()]
                .into_iter()
                .collect(),
        );
        let mut fallback_visibility = meerkat_core::SessionToolVisibilityState {
            capability_base_filter: target_filter,
            ..Default::default()
        };
        fallback_visibility.active_revision = 1;
        fallback_visibility.staged_revision = 1;
        model_routing
            .commit_sticky_model_fallback_for_test(
                &previous,
                &target,
                &target_profile,
                &StickyModelFallbackVisibilityPlan {
                    previous_state: Default::default(),
                    next_state: fallback_visibility.clone(),
                    view_image_tool_available: true,
                    previous_view_image_visible: true,
                    next_view_image_visible: false,
                    committed_visible_set_changed: true,
                    revision_bumped: true,
                },
                1,
            )
            .expect("fallback commit through production bindings");

        let status = machine
            .session_model_routing_status(&session_id)
            .await
            .expect("routing status");
        assert_eq!(status.baseline_model.as_str(), "gpt-backup");
        assert_eq!(status.effective_model.as_str(), "gpt-backup");
        assert_eq!(
            status.session_provider,
            Some(meerkat_core::Provider::OpenAI)
        );
        assert_eq!(
            bindings.tool_visibility_owner().visibility_state().unwrap(),
            fallback_visibility,
        );

        let next_identity = identity("gemini-next", meerkat_core::Provider::Gemini);
        let host_identity = Arc::new(std::sync::Mutex::new(target.clone()));
        let host_visibility = Arc::new(std::sync::Mutex::new(fallback_visibility));
        machine.set_session_llm_reconfigure_host(Arc::new(PostFallbackReconfigureHost {
            identity: Arc::clone(&host_identity),
            visibility: Arc::clone(&host_visibility),
            current_surface: runtime_capability_surface(false),
            target_identity: next_identity.clone(),
            target_surface: runtime_capability_surface(true),
        }));
        let report = machine
            .reconfigure_session_llm_identity(
                &session_id,
                crate::SessionLlmReconfigureRequest {
                    model: Some(next_identity.model.clone()),
                    provider: Some("gemini".to_string()),
                    self_hosted_server_id: None,
                    provider_params: None,
                    auth_binding: None,
                },
            )
            .await
            .expect("fallback state must remain valid input to subsequent reconfigure");
        assert_eq!(report.previous_identity, target);
        assert_eq!(report.new_identity, next_identity);
        assert!(report.tool_visibility_delta.committed_visible_set_changed);
        let reconfigured_visibility = bindings.tool_visibility_owner().visibility_state().unwrap();
        assert_eq!(
            reconfigured_visibility.capability_base_filter,
            ToolFilter::All
        );
        assert_eq!(reconfigured_visibility.active_revision, 2);
    }
}
