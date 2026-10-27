//! Optional in-process composition for an existing authenticated MobKit host.
//!
//! The host owns its roster, profile, model, native auth bindings, viewer access
//! and runtime. These components implement native traits; delegate to them from
//! existing host implementations, or register them in otherwise unused builder
//! slots. They never replace a host registration themselves.
//!
//! Recipe research and captured PairedHost preparation remain separate choices on
//! MobKit 0.8.47. This module does not bypass the native composite plan-owner
//! limitation or transport executable tools through a remote SDK customizer.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::{
    AgentToolDispatcher, AuthBindingRef, MobMemberBinding, ToolCategoryOverride, ToolFilter,
    ops::ToolAccessPolicy,
    service::{CreateSessionRequest, SessionError},
};
use meerkat_mobkit::{
    SessionHook,
    identity_first::{
        AgentBuildContext, AgentBuildDraft, AgentCustomizer, AgentIdentity, CustomizerError,
        DurableAgentSpec, LocalExternalToolOverlay,
    },
};

use crate::{
    PAIRED_HOST_PREPARATION_INSTRUCTIONS, PairedHostPreparationTools, RECIPE_TOOLS, RecipeTools,
};

const RECIPE_INSTRUCTIONS: &str = "Investigate the person's complete request using bundled recipe evidence. Preserve exact identity, digest, snapshot and cursor. Metadata is not installation, account authorization or runtime readiness. Frontend roles still require a compatible persistent host.";

/// Attach one setup tool surface to one host-selected durable identity.
///
/// Both native context and roster identity must match. Existing local tools are
/// refused rather than replaced or combined. All other draft fields and other
/// identities remain unchanged. The host must still supply its native profile,
/// viewer admission and containing runtime/realm; this is not an access check.
pub struct ConsoleSetupCustomizer {
    selected: AgentIdentity,
    tools: Arc<dyn AgentToolDispatcher>,
    instructions: &'static str,
}

impl ConsoleSetupCustomizer {
    /// Offer canonical service, frontend and hybrid recipe metadata, without a
    /// PairedHost client, prepared service resources or new native session.
    pub fn recipes(selected: AgentIdentity) -> Self {
        Self {
            selected,
            tools: Arc::new(RecipeTools::new()),
            instructions: RECIPE_INSTRUCTIONS,
        }
    }

    /// Offer the existing six preparation tools for an already captured client.
    ///
    /// The host must authorize this exact member and its viewers to use the
    /// captured account, including when reconstructing the member. Component
    /// provenance is not account authorization. Submission, secret entry and
    /// activation are not added; the preparation dispatcher's ceiling remains.
    pub fn paired_host_preparation(selected: AgentIdentity, tools: &PairedHostPreparationTools) -> Self {
        Self {
            selected,
            tools: tools.dispatcher(),
            instructions: PAIRED_HOST_PREPARATION_INSTRUCTIONS,
        }
    }
}

#[async_trait]
impl AgentCustomizer for ConsoleSetupCustomizer {
    async fn customize_build(
        &self,
        context: &AgentBuildContext,
        spec: &DurableAgentSpec,
        draft: &mut AgentBuildDraft,
    ) -> Result<(), CustomizerError> {
        if context.identity == self.selected && spec.identity == self.selected {
            if draft.local_external_tools.is_some() {
                return Err(CustomizerError::BuildFailed(
                    "setup member already has local tools".into(),
                ));
            }
            draft.local_external_tools = LocalExternalToolOverlay::new(self.tools.clone());
            draft.additional_instructions.push(self.instructions.into());
        }
        Ok(())
    }
}

/// Enable the qualified native recipe/research surface for one exact member.
///
/// The host explicitly selects a non-native-search member model and authorizes
/// both `member_auth` and the independently resolved native search helper route.
/// This hook does not change the model or configure/resolve the helper account.
/// It preserves the profile's declared execution restrictions and conjoins
/// native launch execution policy. Builtin and search construction are explicitly
/// enabled together with the exact native visibility and execution ceilings.
/// A conflicting auth binding or any existing initial filter is refused.
///
/// Use the real [`SessionHook`] service path, not MobKit's synthetic
/// `SessionHookCustomizerAdapter`, which discards these build mutations.
/// Delegate this call from an existing host hook to preserve its other work.
/// The exact tool ceiling excludes paired PairedHost preparation. Cancellation,
/// native auth, search execution and runtime teardown retain their native owners.
pub struct ConsoleResearchHook {
    selected: MobMemberBinding,
    member_auth: AuthBindingRef,
}

impl ConsoleResearchHook {
    pub fn new(selected: MobMemberBinding, member_auth: AuthBindingRef) -> Self {
        Self {
            selected,
            member_auth,
        }
    }
}

#[async_trait]
impl SessionHook for ConsoleResearchHook {
    async fn before_create(&self, request: &mut CreateSessionRequest) -> Result<(), SessionError> {
        let Some(build) = request.build.as_mut() else {
            return Ok(());
        };
        if build.mob_member_binding.as_ref() != Some(&self.selected) {
            return Ok(());
        }
        if build
            .auth_binding
            .as_ref()
            .is_some_and(|binding| binding != &self.member_auth)
        {
            return Err(SessionError::Unsupported(
                "conflicting host-selected member auth binding".into(),
            ));
        }
        if build.initial_tool_filter.is_some() {
            return Err(SessionError::Unsupported(
                "research requires an unassigned initial tool filter".into(),
            ));
        }
        let access = ToolAccessPolicy::AllowList(
            RECIPE_TOOLS.iter().copied().chain(["web_search"]).collect(),
        );
        let access = match build.tool_access_policy.clone() {
            Some(prior) => prior.conjoin(access).map_err(|_| {
                SessionError::Unsupported("research tool policy cannot be conjoined".into())
            })?,
            None => access,
        };
        build.auth_binding = Some(self.member_auth.clone());
        build.tool_access_policy = Some(access);
        build.initial_tool_filter = Some(ToolFilter::Allow(
            RECIPE_TOOLS
                .iter()
                .copied()
                .chain([
                    "web_search",
                    "send_message",
                    "reply_to_peer",
                    "send_request",
                    "send_response",
                ])
                .collect(),
        ));
        // Released native construction otherwise skips registering search.
        // Both native allowlists restrict the final surface; the host's profile
        // still supplies read-only policy and the denied comms declarations.
        build.override_builtins = ToolCategoryOverride::Enable;
        build.override_web_search = ToolCategoryOverride::Enable;
        Ok(())
    }
}
