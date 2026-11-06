//! The member tool grant gate a member's next build resolves, as the grant
//! surfaces read it: the member's own gate (from the profile its build runs
//! on) joined with its role's current gate. Nothing here is stored and
//! nothing is woken: the reads are the roster entry, the definition, the
//! realm profile store and the published machine state.

use meerkat_core::tool_grant::GrantGatedTools;

use crate::error::MobError;
use crate::ids::AgentIdentity;
use crate::profile::ToolConfig;

/// The gate a member's next build resolves, split into what that build
/// gates and the role entries it ignores as not mounted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemberGrantGate {
    /// What the build gates: the member's own gate and every role entry it
    /// installs. A role tool name is gated from the moment it appears, so it
    /// is always here; a role family is here when the member mounts it.
    pub gated: GrantGatedTools,
    /// The role families the member's switches and own gate leave off: no
    /// tool of theirs can appear within the build, so the build ignores
    /// them, never refusing for them.
    pub not_mounted: GrantGatedTools,
}

impl MemberGrantGate {
    /// Every entry the gate names, gated or not mounted: what a grant to the
    /// member may target.
    #[must_use]
    pub fn named(&self) -> GrantGatedTools {
        let mut named = self.gated.clone();
        named.extend(&self.not_mounted);
        named
    }

    /// The gate of a member whose own tools are `own` and whose role's
    /// current tools are `role` (`None` when the role no longer resolves).
    #[must_use]
    pub(crate) fn resolve(own: &ToolConfig, role: Option<&ToolConfig>) -> Self {
        let mut gate = Self {
            gated: own.grant_gated.clone(),
            not_mounted: GrantGatedTools::default(),
        };
        let Some(role) = role else {
            return gate;
        };
        let entries = role.grant_gated.without(&own.grant_gated);
        for family in &entries.families {
            if family_switch(own, family) == Some(false) {
                gate.not_mounted.families.insert(family.clone());
            } else {
                gate.gated.families.insert(family.clone());
            }
        }
        for name in entries.tools.iter() {
            gate.gated.tools.insert(name.clone());
        }
        gate
    }
}

/// The member tool switch that mounts a built-in tool `family`, when a mob
/// profile has one: `None` for a family no switch of a mob profile mounts,
/// whose mounting the build decides.
pub(crate) fn family_switch(tools: &ToolConfig, family: &str) -> Option<bool> {
    match family {
        "shell" => Some(tools.shell),
        "memory" => Some(tools.memory),
        "workgraph" => Some(tools.workgraph),
        "schedule" => Some(tools.schedule),
        "image_generation" => Some(tools.image_generation),
        "comms" => Some(tools.comms),
        "tasks" | "utility" => Some(tools.builtins),
        _ => None,
    }
}

impl super::MobHandle {
    /// The tool grant gate `identity`'s next build resolves: the union of
    /// its own gate (the profile its build runs on: its spawn-time profile,
    /// else its role's current definition) and its role's current gate,
    /// split into what the build gates and the role families it ignores as
    /// not mounted. A member placed on another host carries no gate (its
    /// placement refuses one), so it answers the empty gate. Nothing is
    /// woken and nothing is stored.
    ///
    /// # Errors
    /// [`MobError::MemberNotFound`] for an identity outside the roster, and
    /// the realm profile store's failure to read the role.
    pub async fn member_grant_gate(
        &self,
        identity: &AgentIdentity,
    ) -> Result<MemberGrantGate, MobError> {
        let (role, spawn_profile) = {
            let roster = self.roster.read().await;
            let entry = roster
                .get(identity)
                .ok_or_else(|| MobError::MemberNotFound(identity.clone()))?;
            (entry.role.clone(), entry.effective_profile_override.clone())
        };
        let placed = self
            .machine_state_watch_rx
            .borrow()
            .member_placement
            .contains_key(&crate::machines::mob_machine::AgentIdentity::from_domain(
                identity,
            ));
        if placed {
            return Ok(MemberGrantGate::default());
        }
        let role_profile = crate::build::current_role_profile(
            &self.definition,
            &role,
            self.realm_profile_store.as_ref(),
        )
        .await?;
        let own = match (&spawn_profile, &role_profile) {
            (Some(profile), _) | (None, Some(profile)) => &profile.tools,
            (None, None) => return Ok(MemberGrantGate::default()),
        };
        Ok(MemberGrantGate::resolve(
            own,
            role_profile.as_ref().map(|profile| &profile.tools),
        ))
    }
}
