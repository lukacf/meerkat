//! Non-shipping semantic binding seam for the direct GPT Live Gate0 harness.
//!
//! This module owns no provider admission and cannot construct a qualified
//! target. It only lets the dedicated harness reuse the shared generated
//! WebRTC answer-and-bind transition after a distinct candidate transport has
//! observed provider readiness.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat_live::{
    LiveWebrtcBindingRequest, ProviderWebrtcBinding, ProviderWebrtcBoundReadyReceipt,
};

use crate::surface::{
    LiveWebrtcBoundReadyBindFailure, LiveWebrtcBoundReadyBinder, LiveWebrtcBoundReadyCustody,
};

/// Candidate transport custody required by the generated semantic binder.
/// Shipping transport/admission types do not implement or consume this trait.
#[async_trait]
pub trait Gate0CandidateTransportCustody: Send + Sync {
    async fn active_binding(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Option<ProviderWebrtcBinding>;

    async fn retire_after_semantic_rollback(
        &self,
        channel_id: &meerkat_live::LiveChannelId,
        session_id: &meerkat_core::SessionId,
    );

    async fn prepare_bound_channel_activation(
        &self,
        provider_binding: &ProviderWebrtcBinding,
        answer_observation_sequence: u64,
        runtime_binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
        activator: Arc<dyn crate::experimental_gpt_live::ExperimentalLiveBoundChannelActivator>,
        control: Arc<dyn crate::experimental_gpt_live::ExperimentalGptLiveControlPlane>,
        live_adapter_host: Arc<meerkat_live::LiveAdapterHost>,
        public_observation_publisher: Arc<
            dyn crate::experimental_gpt_live::ExperimentalLivePublicObservationPublisher,
        >,
    ) -> Result<(), String>;

    async fn commit_bound_channel_activation(
        &self,
        binding: &meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
    ) -> bool;

    async fn cancel_bound_channel_activation(
        &self,
        binding: &meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
    );
}

/// Candidate-only bound-ready binder. It consumes the same generated machine
/// answer receipt as other transports but does not activate shipping GPT Live
/// control, advertise a capability, or mint a qualification witness.
pub struct Gate0CandidateBoundReadyBinder {
    transport: Arc<dyn Gate0CandidateTransportCustody>,
    control: Arc<dyn crate::experimental_gpt_live::ExperimentalGptLiveControlPlane>,
    activator: Arc<dyn crate::experimental_gpt_live::ExperimentalLiveBoundChannelActivator>,
    live_adapter_host: Arc<meerkat_live::LiveAdapterHost>,
    public_observation_publisher:
        Arc<dyn crate::experimental_gpt_live::ExperimentalLivePublicObservationPublisher>,
}

impl Gate0CandidateBoundReadyBinder {
    #[must_use]
    pub fn new(
        transport: Arc<dyn Gate0CandidateTransportCustody>,
        control: Arc<dyn crate::experimental_gpt_live::ExperimentalGptLiveControlPlane>,
        activator: Arc<dyn crate::experimental_gpt_live::ExperimentalLiveBoundChannelActivator>,
        live_adapter_host: Arc<meerkat_live::LiveAdapterHost>,
        public_observation_publisher: Arc<
            dyn crate::experimental_gpt_live::ExperimentalLivePublicObservationPublisher,
        >,
    ) -> Self {
        Self {
            transport,
            control,
            activator,
            live_adapter_host,
            public_observation_publisher,
        }
    }
}

struct Gate0CandidateBoundReadyCustody {
    runtime: Arc<meerkat_runtime::MeerkatMachine>,
    live_adapter_host: Arc<meerkat_live::LiveAdapterHost>,
    transport: Arc<dyn Gate0CandidateTransportCustody>,
    authority: meerkat_runtime::meerkat_machine::LiveWebrtcAnswerExecutionBindingAuthority,
    activator: Arc<dyn crate::experimental_gpt_live::ExperimentalLiveBoundChannelActivator>,
}

#[async_trait]
impl LiveWebrtcBoundReadyBinder for Gate0CandidateBoundReadyBinder {
    async fn bind_answer_ready(
        &self,
        runtime: Arc<meerkat_runtime::MeerkatMachine>,
        binding: &LiveWebrtcBindingRequest,
        receipt: ProviderWebrtcBoundReadyReceipt,
        answer_observation_sequence: u64,
    ) -> Result<Box<dyn LiveWebrtcBoundReadyCustody>, LiveWebrtcBoundReadyBindFailure> {
        let runtime_binding = binding.runtime_binding.ok_or_else(|| {
            LiveWebrtcBoundReadyBindFailure::before_binding(
                "Gate0 candidate answer omitted its runtime incarnation",
            )
        })?;
        let provider_binding = ProviderWebrtcBinding::new(
            binding.channel_id.clone(),
            binding.session_id.clone(),
            meerkat_live::LiveRuntimeBindingGeneration::new(runtime_binding.generation),
            meerkat_live::LiveRuntimeBindingFence::new(runtime_binding.fence),
        );
        if self
            .transport
            .active_binding(&binding.session_id)
            .await
            .as_ref()
            != Some(&provider_binding)
        {
            return Err(LiveWebrtcBoundReadyBindFailure::before_binding(
                "Gate0 candidate bound-ready answer does not match active transport custody",
            ));
        }
        let authority = runtime
            .accept_live_webrtc_answer_and_bind_execution(
                &provider_binding,
                &receipt,
                answer_observation_sequence,
            )
            .await
            .map_err(|error| LiveWebrtcBoundReadyBindFailure::before_binding(error.to_string()))?;
        let custody = Box::new(Gate0CandidateBoundReadyCustody {
            runtime,
            live_adapter_host: Arc::clone(&self.live_adapter_host),
            transport: Arc::clone(&self.transport),
            authority,
            activator: Arc::clone(&self.activator),
        });
        if !custody.authority.answer().answered
            || !matches!(
                custody.authority.answer().status,
                meerkat_runtime::meerkat_machine::dsl::LiveWebrtcAnswerPublicStatus::Answered
            )
        {
            return Err(LiveWebrtcBoundReadyBindFailure::after_binding(
                "Gate0 candidate answer-and-bind returned a non-answered state",
                custody,
            ));
        }
        if let Err(error) = self
            .transport
            .prepare_bound_channel_activation(
                &provider_binding,
                answer_observation_sequence,
                custody.authority.binding().clone(),
                Arc::clone(&self.activator),
                Arc::clone(&self.control),
                Arc::clone(&self.live_adapter_host),
                Arc::clone(&self.public_observation_publisher),
            )
            .await
        {
            return Err(LiveWebrtcBoundReadyBindFailure::after_binding(
                error, custody,
            ));
        }
        Ok(custody)
    }
}

#[async_trait]
impl LiveWebrtcBoundReadyCustody for Gate0CandidateBoundReadyCustody {
    async fn commit(self: Box<Self>) -> Result<(), String> {
        let binding = self.authority.binding().clone();
        let activated = self
            .transport
            .commit_bound_channel_activation(&binding)
            .await;
        if !activated {
            return self.rollback().await.and_then(|()| {
                Err("Gate0 candidate activation did not start all bound tasks".to_string())
            });
        }
        let _ = self.authority.commit();
        Ok(())
    }

    async fn rollback(self: Box<Self>) -> Result<(), String> {
        let binding = self.authority.binding().clone();
        self.transport
            .cancel_bound_channel_activation(&binding)
            .await;
        let deactivation = self.activator.deactivate_bound_channel(&binding).await;
        let observation = self
            .live_adapter_host
            .reserve_channel_close_observation(binding.channel_id())
            .await
            .map_err(|error| format!("host close observation failed: {error}"))?;
        self.live_adapter_host
            .prepare_channel_physical_close(&observation)
            .await
            .map_err(|error| format!("live adapter close failed: {error}"))?;
        let authority = self
            .runtime
            .rollback_live_webrtc_answer_execution_binding(
                self.authority.into_rollback(),
                &observation,
            )
            .await
            .map_err(|error| format!("generated answer binding rollback failed: {error}"))?;
        let commit = authority.channel_close_commit_authority().ok_or_else(|| {
            "generated answer binding rollback omitted close commit authority".to_string()
        })?;
        self.live_adapter_host
            .commit_channel_close_observation(&observation, commit)
            .await
            .map_err(|error| format!("host close commit failed: {error}"))?;
        self.transport
            .retire_after_semantic_rollback(binding.channel_id(), binding.session_id())
            .await;
        deactivation?;
        Ok(())
    }
}
