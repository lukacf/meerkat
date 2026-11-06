//! The spawn customizer every helper passes: it gives the helper the
//! approval gate of the session it works for, that session's model binding,
//! the TUI's file tools and the helper instructions, and tells the
//! [`super::follow`] task to observe it.

use std::collections::HashMap;
use std::sync::Arc;

use meerkat_core::{AgentToolDispatcher, SessionId};
use meerkat_mob::{
    MobError, ResumedMemberView, SpawnCustomizationContext, SpawnMemberCustomizer,
    SpawnMemberSpec, SpawnSource,
};
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
    /// The environment of the helper's shell commands (the confined
    /// shell's curated set), `None` when the shell runs unconfined.
    shell_env: Option<HashMap<String, String>>,
    spawning: mpsc::UnboundedSender<Spawning>,
}

impl std::fmt::Debug for HelperCustomizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HelperCustomizer").finish_non_exhaustive()
    }
}

impl HelperCustomizer {
    /// A customizer gating helpers with `engine`; a helper's shell
    /// commands get `shell_env` (see [`inherit_shell_env`]).
    #[must_use]
    pub fn new(
        registry: Arc<HelperRegistry>,
        engine: Arc<ApprovalEngine>,
        tools: HelperTools,
        shell_env: Option<HashMap<String, String>>,
        spawning: mpsc::UnboundedSender<Spawning>,
    ) -> Self {
        Self {
            registry,
            engine,
            tools,
            shell_env,
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

/// Give `spec`'s shell commands `shell_env` (the confined shell's curated
/// environment) unless the spawn already names one. A helper's shell runs
/// under the same confinement as its owner's, so it needs the same
/// environment.
fn inherit_shell_env(spec: &mut SpawnMemberSpec, shell_env: Option<&HashMap<String, String>>) {
    if spec.shell_env.is_none() {
        spec.shell_env = shell_env.cloned();
    }
}

/// Apply the owner's gate, binding, tools, instructions and the shell's
/// environment to `spec`.
fn shape(
    spec: &mut SpawnMemberSpec,
    owner: &Owner,
    label: &str,
    model: &str,
    engine: &Arc<ApprovalEngine>,
    tools: &HelperTools,
    shell_env: Option<&HashMap<String, String>>,
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
    inherit_shell_env(spec, shell_env);
    deny_ungated_tools(spec);
    spec.additional_instructions
        .get_or_insert_with(Vec::new)
        .push(HELPER_PERSONA.to_string());
}

/// A helper is a seated member, so the engine would let it `fork_off`; its
/// fork child would seat without this customizer and so without the gate.
/// Every helper's tool access policy denies the TUI's denied tools
/// ([`crate::session::tool_access_policy`]), merged into whatever policy its
/// spec names: none or `Inherit` becomes the TUI's deny list, a deny list
/// gains the names, an allow list loses them, constraints gain a deny
/// constraint, and `ReadOnly` (which already admits no mutating tool) stays.
fn deny_ungated_tools(spec: &mut SpawnMemberSpec) {
    use meerkat_core::ops::{ToolAccessConstraint, ToolAccessPolicy};
    let ToolAccessPolicy::DenyList(denied) = crate::session::tool_access_policy() else {
        // The TUI policy is a deny list; anything else is a programming
        // error that must not widen a helper's tools.
        spec.tool_access_policy = Some(crate::session::tool_access_policy());
        return;
    };
    spec.tool_access_policy = Some(match spec.tool_access_policy.take() {
        None | Some(ToolAccessPolicy::Inherit) => ToolAccessPolicy::DenyList(denied),
        Some(ToolAccessPolicy::DenyList(mut names)) => {
            for name in denied.iter() {
                names.insert(name.clone());
            }
            ToolAccessPolicy::DenyList(names)
        }
        Some(ToolAccessPolicy::AllowList(mut names)) => {
            names.retain(|name| !denied.contains(name.as_str()));
            ToolAccessPolicy::AllowList(names)
        }
        Some(ToolAccessPolicy::ReadOnly) => ToolAccessPolicy::ReadOnly,
        Some(ToolAccessPolicy::Constraints(mut constraints)) => {
            constraints.push(ToolAccessConstraint::DenyNames(denied));
            ToolAccessPolicy::Constraints(constraints)
        }
    });
}

/// A refused helper, as the mob reports it.
fn refuse(err: impl std::fmt::Display) -> MobError {
    MobError::Internal(format!("rkat-tui: helper refused: {err}"))
}

impl HelperCustomizer {
    /// Shape `spec` for `owner`, bind it as `key` and have it followed.
    fn admit(
        &self,
        key: HelperKey,
        owner: &Owner,
        spec: &mut SpawnMemberSpec,
        session: Option<&SessionId>,
    ) -> Result<(), MobError> {
        let label = spec.identity.to_string();
        let model = requested_model(spec).unwrap_or_else(|| owner.model.clone());
        let mut path = owner.path.clone();
        path.push(label.clone());
        let tag = HelperTag {
            owner: owner.session.clone(),
            path,
            model: model.clone(),
        };
        // Bound durably before the member exists: a member whose owner a
        // restart would lose never runs.
        self.registry
            .spawning(key.clone(), tag.clone())
            .map_err(refuse)?;
        if let Some(session) = session {
            self.registry.bind_session(&key, &tag, session);
        }
        shape(
            spec,
            owner,
            &label,
            &model,
            &self.engine,
            &self.tools,
            self.shell_env.as_ref(),
        );
        let _ = self.spawning.send(Spawning { key, tag });
        Ok(())
    }

    /// The owner a rebuilt member `key` was bound to.
    fn persisted_owner(&self, key: &HelperKey) -> Result<Owner, MobError> {
        let tag = self.registry.owner_for_resume(key).map_err(refuse)?;
        Ok(Owner {
            auth_binding: self.registry.auth_binding_of(&tag.owner),
            path: tag.parent_path().to_vec(),
            session: tag.owner,
            model: tag.model,
        })
    }
}

impl SpawnMemberCustomizer for HelperCustomizer {
    fn customize_spawn(
        &self,
        ctx: &SpawnCustomizationContext,
        spec: &mut SpawnMemberSpec,
    ) -> Result<(), MobError> {
        let key = HelperKey::new(ctx.mob_id.clone(), spec.identity.clone());
        // A fork-derived member's rebuild asks with `Resume`: it keeps the
        // owner it was bound to, when it was.
        let persisted = if matches!(ctx.spawn_source, SpawnSource::Resume) {
            self.persisted_owner(&key).ok()
        } else {
            None
        };
        // Fail closed: a helper whose owner does not resolve to a session
        // open here is refused, never attributed to another session.
        let owner = match persisted {
            Some(owner) => owner,
            None => self
                .registry
                .owner_for_spawn(&ctx.mob_id, ctx.spawner_identity.as_ref())
                .map_err(refuse)?,
        };
        self.admit(key, &owner, spec, None)
    }

    fn customize_resume(
        &self,
        ctx: &SpawnCustomizationContext,
        durable: &ResumedMemberView,
        spec: &mut SpawnMemberSpec,
    ) -> Result<(), MobError> {
        // A restored member is gated for the owner it was bound to, whether
        // or not that session is open; a member nobody bound is refused.
        let key = HelperKey::new(ctx.mob_id.clone(), spec.identity.clone());
        let owner = self.persisted_owner(&key)?;
        self.admit(key, &owner, spec, Some(&durable.session_id))
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;

    fn denies_fork_off(names: &meerkat_core::ToolNameSet) -> bool {
        names.contains("fork_off")
    }

    #[test]
    fn a_helper_without_a_policy_or_inheriting_gets_the_tui_deny_list() {
        use meerkat_core::ops::ToolAccessPolicy;
        for policy in [None, Some(ToolAccessPolicy::Inherit)] {
            let mut spec = SpawnMemberSpec::new("delegate", "worker");
            spec.tool_access_policy = policy;
            deny_ungated_tools(&mut spec);
            assert_eq!(spec.tool_access_policy, Some(crate::session::tool_access_policy()));
        }
    }

    #[test]
    fn a_helpers_deny_list_gains_the_tui_names() {
        use meerkat_core::ops::ToolAccessPolicy;
        let mut spec = SpawnMemberSpec::new("delegate", "worker");
        spec.tool_access_policy = Some(ToolAccessPolicy::DenyList(
            ["web_fetch"].into_iter().collect(),
        ));
        deny_ungated_tools(&mut spec);
        let Some(ToolAccessPolicy::DenyList(names)) = &spec.tool_access_policy else {
            panic!("{:?}", spec.tool_access_policy);
        };
        assert!(names.contains("web_fetch") && denies_fork_off(names), "{names:?}");
    }

    #[test]
    fn a_helpers_allow_list_loses_the_tui_names() {
        use meerkat_core::ops::ToolAccessPolicy;
        let mut spec = SpawnMemberSpec::new("delegate", "worker");
        spec.tool_access_policy = Some(ToolAccessPolicy::AllowList(
            ["read_file", "fork_off"].into_iter().collect(),
        ));
        deny_ungated_tools(&mut spec);
        let Some(ToolAccessPolicy::AllowList(names)) = &spec.tool_access_policy else {
            panic!("{:?}", spec.tool_access_policy);
        };
        assert!(names.contains("read_file") && !names.contains("fork_off"), "{names:?}");
    }

    #[test]
    fn a_read_only_helper_stays_read_only_and_constraints_gain_a_deny() {
        use meerkat_core::ops::{ToolAccessConstraint, ToolAccessPolicy};
        let mut spec = SpawnMemberSpec::new("delegate", "worker");
        spec.tool_access_policy = Some(ToolAccessPolicy::ReadOnly);
        deny_ungated_tools(&mut spec);
        assert_eq!(spec.tool_access_policy, Some(ToolAccessPolicy::ReadOnly));

        let mut spec = SpawnMemberSpec::new("delegate", "worker");
        spec.tool_access_policy = Some(ToolAccessPolicy::Constraints(vec![
            ToolAccessConstraint::ReadOnly,
        ]));
        deny_ungated_tools(&mut spec);
        let Some(ToolAccessPolicy::Constraints(constraints)) = &spec.tool_access_policy else {
            panic!("{:?}", spec.tool_access_policy);
        };
        assert_eq!(constraints.len(), 2);
        assert!(matches!(
            &constraints[1],
            ToolAccessConstraint::DenyNames(names) if denies_fork_off(names)
        ));
    }

    #[test]
    fn a_helper_gets_the_confined_shell_environment_unless_it_names_one() {
        let curated = HashMap::from([("PATH".to_string(), "/home/u/.cargo/bin".to_string())]);
        let mut spec = SpawnMemberSpec::new("delegate", "worker");
        inherit_shell_env(&mut spec, Some(&curated));
        assert_eq!(spec.shell_env.as_ref(), Some(&curated));

        let own = HashMap::from([("PATH".to_string(), "/opt/bin".to_string())]);
        let mut named = SpawnMemberSpec::new("delegate", "worker");
        named.shell_env = Some(own.clone());
        inherit_shell_env(&mut named, Some(&curated));
        assert_eq!(named.shell_env, Some(own));

        let mut unconfined = SpawnMemberSpec::new("delegate", "worker");
        inherit_shell_env(&mut unconfined, None);
        assert_eq!(unconfined.shell_env, None);
    }
}
