//! Runtime impl of [`meerkat_core::handles::ModelRoutingHandle`].

use std::sync::Arc;

use meerkat_core::handles::{DslTransitionError, ModelRoutingHandle};
use meerkat_core::lifecycle::run_primitive::ModelId;
use meerkat_core::model_profile::ModelProfile;
use meerkat_core::{SessionLlmIdentity, ToolFilter};

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
}

impl RuntimeModelRoutingHandle {
    /// Construct a handle backed by the session's shared DSL authority.
    pub fn new(dsl: Arc<HandleDslAuthority>) -> Self {
        Self { dsl }
    }

    /// Construct a handle backed by an ephemeral DSL authority.
    pub fn ephemeral() -> Self {
        Self::new(Arc::new(HandleDslAuthority::ephemeral()))
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
}
