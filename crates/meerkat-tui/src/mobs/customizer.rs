//! The spawn customizer every helper passes: it gives the helper the
//! approval gate of the session it works for, that session's model binding,
//! the TUI's file tools and the helper instructions, and tells the
//! [`super::follow`] task to observe it.

use std::sync::Arc;

use meerkat_core::AgentToolDispatcher;
use meerkat_mob::{MobError, SpawnCustomizationContext, SpawnMemberCustomizer, SpawnMemberSpec};
use tokio::sync::mpsc;

use super::registry::{HelperKey, HelperRegistry, HelperTag, Owner};
use crate::approval::{ApprovalEngine, ApprovalGate};
use crate::bootstrap::HELPER_PERSONA;

/// Builds the external tools of one helper (the TUI's file tools).
pub type HelperTools = Arc<dyn Fn() -> Arc<dyn AgentToolDispatcher> + Send + Sync>;

/// A helper about to be spawned, for the follower task.
#[derive(Debug, Clone)]
pub struct Spawning {
    /// Mob id and member identity.
    pub key: HelperKey,
    /// What the helper is.
    pub tag: HelperTag,
}

/// The TUI's [`SpawnMemberCustomizer`].
pub struct HelperCustomizer {
    registry: Arc<HelperRegistry>,
    engine: Arc<ApprovalEngine>,
    tools: HelperTools,
    spawning: mpsc::UnboundedSender<Spawning>,
}

impl std::fmt::Debug for HelperCustomizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HelperCustomizer").finish_non_exhaustive()
    }
}

impl HelperCustomizer {
    /// A customizer gating helpers with `engine`.
    #[must_use]
    pub fn new(
        registry: Arc<HelperRegistry>,
        engine: Arc<ApprovalEngine>,
        tools: HelperTools,
        spawning: mpsc::UnboundedSender<Spawning>,
    ) -> Self {
        Self {
            registry,
            engine,
            tools,
            spawning,
        }
    }
}

/// The model `spec` asks for, when it names one (an explicit override or an
/// inline profile); otherwise the member runs on its mob's profile, which
/// for a `delegate` helper is its delegator's model.
fn requested_model(spec: &SpawnMemberSpec) -> Option<String> {
    spec.model_override.clone().or_else(|| {
        spec.override_profile
            .as_ref()
            .map(|profile| profile.model.clone())
            .filter(|model| !model.is_empty())
    })
}

/// Apply the owner's gate, binding, tools and instructions to `spec`.
fn shape(
    spec: &mut SpawnMemberSpec,
    owner: &Owner,
    label: &str,
    model: &str,
    engine: &Arc<ApprovalEngine>,
    tools: &HelperTools,
) {
    // A helper on its owner's model reaches it through the same endpoint;
    // a helper that asks for another model resolves that model's default
    // credentials (its owner's binding may serve another provider).
    if spec.auth_binding.is_none() && model == owner.model {
        spec.auth_binding.clone_from(&owner.auth_binding);
    }
    let gate = ApprovalGate::for_helper(Arc::clone(engine), owner.session.clone(), label)
        .after(spec.tool_dispatch_admission.take());
    spec.tool_dispatch_admission = Some(Arc::new(gate));
    if spec.external_tools.is_none() {
        spec.external_tools = Some(tools());
    }
    spec.additional_instructions
        .get_or_insert_with(Vec::new)
        .push(HELPER_PERSONA.to_string());
}

impl SpawnMemberCustomizer for HelperCustomizer {
    fn customize_spawn(
        &self,
        ctx: &SpawnCustomizationContext,
        spec: &mut SpawnMemberSpec,
    ) -> Result<(), MobError> {
        // Fail closed: a helper nobody's approval gate can judge never runs.
        let owner = self.registry.owner_of(ctx.mob_id.as_str()).ok_or_else(|| {
            MobError::Internal("rkat-tui: no session to run this helper for".to_string())
        })?;
        let label = spec.identity.to_string();
        let model = requested_model(spec).unwrap_or_else(|| owner.model.clone());
        shape(spec, &owner, &label, &model, &self.engine, &self.tools);
        let mut path = owner.path;
        path.push(label.clone());
        let spawning = Spawning {
            key: (ctx.mob_id.to_string(), label),
            tag: HelperTag {
                owner: owner.session,
                path,
                model,
            },
        };
        self.registry
            .spawning(spawning.key.clone(), spawning.tag.clone());
        let _ = self.spawning.send(spawning);
        Ok(())
    }
}
