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

/// The mob tools that start a member or a council: denied to a helper at
/// the configured depth limit (`[helpers] max_depth`), so helpers nest no
/// deeper. `fork_off` is denied to every helper anyway
/// ([`crate::session::UNGATED_SPAWN_TOOLS`]).
pub const NESTING_TOOLS: [&str; 4] = ["delegate", "mob_create", "mob_spawn_member", "council"];

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
    /// How deep helpers may nest (`[helpers] max_depth`).
    max_depth: u8,
}

impl std::fmt::Debug for HelperCustomizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HelperCustomizer").finish_non_exhaustive()
    }
}

impl HelperCustomizer {
    /// A customizer gating helpers with `engine`; a helper's shell
    /// commands get `shell_env` (see [`inherit_shell_env`]); helpers nest
    /// at most `max_depth` levels deep (see [`Depth`]).
    #[must_use]
    pub fn new(
        registry: Arc<HelperRegistry>,
        engine: Arc<ApprovalEngine>,
        tools: HelperTools,
        shell_env: Option<HashMap<String, String>>,
        spawning: mpsc::UnboundedSender<Spawning>,
        max_depth: u8,
    ) -> Self {
        Self {
            registry,
            engine,
            tools,
            shell_env,
            spawning,
            max_depth,
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

/// Where a helper sits against the depth limit. A helper's depth is the
/// length of its path: the session's own helpers are at depth 1, theirs at
/// 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Depth {
    /// It may start helpers of its own.
    MayNest,
    /// It may not: its [`NESTING_TOOLS`] are denied.
    Last,
    /// It should not exist: its spawner should not have had the tools.
    TooDeep,
}

impl Depth {
    /// The [`NESTING_TOOLS`] a helper at this depth is denied.
    fn nesting_denied(self) -> &'static [&'static str] {
        match self {
            Self::MayNest => &[],
            Self::Last | Self::TooDeep => &NESTING_TOOLS,
        }
    }

    fn of(depth: usize, max_depth: u8) -> Self {
        match depth.cmp(&usize::from(max_depth)) {
            std::cmp::Ordering::Less => Self::MayNest,
            std::cmp::Ordering::Equal => Self::Last,
            std::cmp::Ordering::Greater => Self::TooDeep,
        }
    }

    /// Where the helper `label` at `depth` sits, refusing a `fresh` spawn
    /// beyond the limit: a second line of defence, as its spawner was at
    /// the limit and should not have had the tools. A member restored from
    /// before the limit was lowered keeps running, with the tools denied.
    fn admit(label: &str, depth: usize, max_depth: u8, fresh: bool) -> Result<Self, MobError> {
        match Self::of(depth, max_depth) {
            Self::TooDeep if fresh => Err(refuse(format_args!(
                "`{label}` would be a helper {depth} levels deep, but helpers nest at most {max_depth} (tui.toml [helpers] max_depth, or --set helpers.max_depth=N); do the work without delegating"
            ))),
            Self::TooDeep => Ok(Self::Last),
            depth => Ok(depth),
        }
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
/// `also` adds names to deny (a helper at the depth limit gets
/// [`NESTING_TOOLS`]); with any, `ReadOnly` becomes a read-only constraint
/// beside the deny constraint, so the names are denied whatever read-only
/// admits.
fn deny_ungated_tools(spec: &mut SpawnMemberSpec, also: &[&str]) {
    use meerkat_core::ops::{ToolAccessConstraint, ToolAccessPolicy};
    let ToolAccessPolicy::DenyList(mut denied) = crate::session::tool_access_policy() else {
        // The TUI policy is a deny list; anything else is a programming
        // error that must not widen a helper's tools.
        spec.tool_access_policy = Some(crate::session::tool_access_policy());
        return;
    };
    for name in also {
        denied.insert((*name).to_string());
    }
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
        Some(ToolAccessPolicy::ReadOnly) if also.is_empty() => ToolAccessPolicy::ReadOnly,
        Some(ToolAccessPolicy::ReadOnly) => ToolAccessPolicy::Constraints(vec![
            ToolAccessConstraint::ReadOnly,
            ToolAccessConstraint::DenyNames(denied),
        ]),
        Some(ToolAccessPolicy::Constraints(mut constraints)) => {
            constraints.push(ToolAccessConstraint::DenyNames(denied));
            ToolAccessPolicy::Constraints(constraints)
        }
    });
}

/// Keep the mob tools out of a helper's tool list: the deny list refuses
/// their calls, but every schema left in the list costs tokens in every
/// call (the mob and council tools are about 25k).
///
/// The mob category goes off through the spec's category overrides. A
/// member that inherits its spawner's tool filter (a `delegate` helper)
/// without an override profile otherwise gets every category opened for
/// that filter to narrow, which the engine does only while the spawn
/// overrides nothing; so such a spawn names every other category open
/// itself, exactly as the engine would have.
fn drop_mob_tools(spec: &mut SpawnMemberSpec) {
    use meerkat_core::ToolCategoryOverride;
    let overrides = &mut spec.tool_category_overrides;
    if spec.inherited_tool_filter.is_some() && spec.override_profile.is_none() {
        for category in [
            &mut overrides.builtins,
            &mut overrides.shell,
            &mut overrides.comms,
            &mut overrides.memory,
            &mut overrides.workgraph,
            &mut overrides.schedule,
            &mut overrides.image_generation,
        ] {
            if *category == ToolCategoryOverride::Inherit {
                *category = ToolCategoryOverride::Enable;
            }
        }
    }
    overrides.mob = ToolCategoryOverride::Disable;
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
        fresh: bool,
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
        let depth = Depth::admit(&label, tag.path.len(), self.max_depth, fresh)?;
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
        deny_ungated_tools(spec, depth.nesting_denied());
        if depth != Depth::MayNest {
            drop_mob_tools(spec);
        }
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
        let fresh = !matches!(ctx.spawn_source, SpawnSource::Resume);
        self.admit(key, &owner, spec, None, fresh)
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
        self.admit(key, &owner, spec, Some(&durable.session_id), false)
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
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
            deny_ungated_tools(&mut spec, &[]);
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
        deny_ungated_tools(&mut spec, &[]);
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
        deny_ungated_tools(&mut spec, &[]);
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
        deny_ungated_tools(&mut spec, &[]);
        assert_eq!(spec.tool_access_policy, Some(ToolAccessPolicy::ReadOnly));

        let mut spec = SpawnMemberSpec::new("delegate", "worker");
        spec.tool_access_policy = Some(ToolAccessPolicy::Constraints(vec![
            ToolAccessConstraint::ReadOnly,
        ]));
        deny_ungated_tools(&mut spec, &[]);
        let Some(ToolAccessPolicy::Constraints(constraints)) = &spec.tool_access_policy else {
            panic!("{:?}", spec.tool_access_policy);
        };
        assert_eq!(constraints.len(), 2);
        assert!(matches!(
            &constraints[1],
            ToolAccessConstraint::DenyNames(names) if denies_fork_off(names)
        ));
    }

    fn denied_names(spec: &SpawnMemberSpec) -> meerkat_core::ToolNameSet {
        use meerkat_core::ops::ToolAccessPolicy;
        let Some(ToolAccessPolicy::DenyList(names)) = &spec.tool_access_policy else {
            panic!("{:?}", spec.tool_access_policy);
        };
        names.clone()
    }

    #[test]
    fn a_helper_at_the_depth_limit_is_denied_the_nesting_tools() {
        // The session's own helpers are at depth 1; with the default limit
        // they may not start helpers.
        let depth = Depth::admit("scout", 1, 1, true).unwrap();
        assert_eq!(depth, Depth::Last);
        let mut spec = SpawnMemberSpec::new("delegate", "scout");
        deny_ungated_tools(&mut spec, &NESTING_TOOLS);
        let names = denied_names(&spec);
        for name in ["delegate", "mob_create", "mob_spawn_member", "council"] {
            assert!(names.contains(name), "{name}: {names:?}");
        }
        assert!(denies_fork_off(&names), "{names:?}");
        // A read-only helper keeps read-only and loses them too.
        let mut spec = SpawnMemberSpec::new("delegate", "scout");
        spec.tool_access_policy = Some(meerkat_core::ops::ToolAccessPolicy::ReadOnly);
        deny_ungated_tools(&mut spec, &NESTING_TOOLS);
        let Some(meerkat_core::ops::ToolAccessPolicy::Constraints(constraints)) =
            &spec.tool_access_policy
        else {
            panic!("{:?}", spec.tool_access_policy);
        };
        assert!(matches!(
            constraints.as_slice(),
            [
                meerkat_core::ops::ToolAccessConstraint::ReadOnly,
                meerkat_core::ops::ToolAccessConstraint::DenyNames(names),
            ] if names.contains("delegate") && names.contains("council")
        ));
    }

    #[test]
    fn a_helper_at_the_depth_limit_has_the_mob_category_off() {
        use meerkat_core::ToolCategoryOverride::{Disable, Inherit};
        // Without an inherited filter only the mob category changes.
        let mut spec = SpawnMemberSpec::new("delegate", "scout");
        drop_mob_tools(&mut spec);
        let overrides = &spec.tool_category_overrides;
        assert_eq!(overrides.mob, Disable);
        assert_eq!((overrides.shell, overrides.builtins), (Inherit, Inherit));
    }

    #[test]
    fn a_helper_below_the_depth_limit_keeps_the_nesting_tools() {
        let depth = Depth::admit("scout", 1, 2, true).unwrap();
        assert_eq!(depth, Depth::MayNest);
        let mut spec = SpawnMemberSpec::new("delegate", "scout");
        deny_ungated_tools(&mut spec, &[]);
        let names = denied_names(&spec);
        for name in NESTING_TOOLS {
            assert!(!names.contains(name), "{name}: {names:?}");
        }
        assert!(denies_fork_off(&names));
        assert_eq!(Depth::admit("inner", 2, 2, true).unwrap(), Depth::Last);
    }

    #[test]
    fn a_spawn_beyond_the_depth_limit_is_refused() {
        let err = Depth::admit("inner", 2, 1, true).unwrap_err().to_string();
        assert!(err.contains("helper refused"), "{err}");
        assert!(err.contains("`inner`") && err.contains("helpers.max_depth"), "{err}");
        // At 0 even the session's own helpers are refused.
        assert!(Depth::admit("scout", 1, 0, true).is_err());
        // A restored member from before the limit was lowered keeps running
        // with the tools denied.
        assert_eq!(Depth::admit("inner", 2, 1, false).unwrap(), Depth::Last);
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
