//! The member tool grant gate a member's next build resolves, as the grant
//! surfaces read it: the member's own gate (from the profile its build runs
//! on, or its role's current gate for a store-owned identity declared with
//! its role's named profile) joined with its role's current gate. Nothing
//! here is stored and nothing is woken: the reads are the roster entry, the
//! identity's sealed intent, the definition, the realm profile store and the
//! published machine state.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::sync::Arc;

use meerkat_core::tool_grant::GrantGatedTools;

use crate::definition::MobDefinition;
use crate::error::MobError;
use crate::identity::{DesiredProfileSource, IdentityIntent, IdentityStoredObservation};
use crate::ids::{AgentIdentity, ProfileName};
use crate::profile::{Profile, ToolConfig};

/// The gate a member's next build resolves, split into what that build
/// gates and the role entries it ignores as not mounted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemberGrantGate {
    /// What the build gates: the member's own gate and every role entry it
    /// installs. A role tool name and a role MCP server are gated from the
    /// moment they appear, so they are always here; a role family is here
    /// when the member mounts it. The unconfigured-servers mode is here when
    /// the member's own gate or its role's has it.
    pub gated: GrantGatedTools,
    /// The role families the member's switches and own gate leave off: no
    /// tool of theirs can appear within the build, so the build ignores
    /// them, never refusing for them.
    pub not_mounted: GrantGatedTools,
    /// The MCP servers the member's own profile configures that the gate
    /// does not name: while the unconfigured-servers mode is on, the only
    /// servers the gate does not gate.
    pub ungated_servers: BTreeSet<String>,
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
            ungated_servers: BTreeSet::new(),
        };
        if let Some(role) = role {
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
            gate.gated
                .mcp_servers
                .extend(entries.mcp_servers.iter().cloned());
            gate.gated.unconfigured_mcp_servers |= entries.unconfigured_mcp_servers;
        }
        gate.ungated_servers = own
            .mcp_servers
            .iter()
            .map(|server| server.name.clone())
            .filter(|server| !gate.gated.mcp_servers.contains(server))
            .collect();
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

/// The own gate of `identity`'s member in the mob `definition` when the
/// identity is a store-owned identity declared with its role's named profile
/// ([`DesiredProfileSource::Definition`]): such a member is built from its
/// role's definition, as a worker of the role is, so its own gate is its
/// role's current gate, less the families its declaration's category
/// overrides switch off. `None` for any other member (no identity intent, an
/// absent or unreadable one, one declared with an exact profile of its own,
/// one whose material was sealed before it recorded its profile source
/// ([`DesiredProfileSource::Unrecorded`]), or one of another role): its own
/// gate is the gate of the profile its build runs on, and its role's gate
/// joins as role entries.
///
/// # Errors
/// The identity store's or the realm profile store's failure to read.
pub(crate) async fn identity_role_gate(
    identity_store: &dyn crate::store::MobIdentityStore,
    definition: &MobDefinition,
    realm_profile_store: Option<&Arc<dyn crate::store::RealmProfileStore>>,
    identity: &AgentIdentity,
    role: &ProfileName,
) -> Result<Option<GrantGatedTools>, MobError> {
    let IdentityStoredObservation::Valid(record) = identity_store
        .observe_identity_intent(&definition.id, identity)
        .await
        .map_err(MobError::from)?
    else {
        return Ok(None);
    };
    let IdentityIntent::Present { member, .. } = &record.intent else {
        return Ok(None);
    };
    let material = &member.material;
    if material.profile_source != DesiredProfileSource::Definition || &material.profile_name != role
    {
        return Ok(None);
    }
    let Some(role_profile) =
        crate::build::current_role_profile(definition, role, realm_profile_store).await?
    else {
        return Ok(None);
    };
    let mut gate = role_profile.tools.grant_gated;
    super::spec_compiler::drop_disabled_families(
        &mut gate,
        material.overlay.tool_category_overrides,
    );
    Ok(Some(gate))
}

/// `profile` as `identity`'s build runs on it: with its role's current gate
/// as its own gate when [`identity_role_gate`] answers one, else as it is.
///
/// # Errors
/// As [`identity_role_gate`].
pub(crate) async fn member_build_profile<'a>(
    identity_store: &dyn crate::store::MobIdentityStore,
    definition: &MobDefinition,
    realm_profile_store: Option<&Arc<dyn crate::store::RealmProfileStore>>,
    identity: &AgentIdentity,
    role: &ProfileName,
    profile: &'a Profile,
) -> Result<Cow<'a, Profile>, MobError> {
    Ok(
        match identity_role_gate(
            identity_store,
            definition,
            realm_profile_store,
            identity,
            role,
        )
        .await?
        {
            Some(gate) => {
                let mut built = profile.clone();
                built.tools.grant_gated = gate;
                Cow::Owned(built)
            }
            None => Cow::Borrowed(profile),
        },
    )
}

impl super::MobHandle {
    /// The tool grant gate `identity`'s next build resolves: the union of
    /// its own gate (the profile its build runs on: its spawn-time profile,
    /// else its role's current definition; for a store-owned identity
    /// declared with its role's named profile, its role's current gate) and
    /// its role's current gate, split into what the build gates and the role
    /// families it ignores as not mounted. A member placed on another host
    /// carries no gate (its placement refuses one), so it answers the empty
    /// gate. Nothing is woken and nothing is stored.
    ///
    /// # Errors
    /// [`MobError::MemberNotFound`] for an identity outside the roster, and
    /// the identity store's or the realm profile store's failure to read.
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
            (Some(profile), _) | (None, Some(profile)) => profile,
            (None, None) => return Ok(MemberGrantGate::default()),
        };
        let own = member_build_profile(
            self.identity.as_ref(),
            &self.definition,
            self.realm_profile_store.as_ref(),
            identity,
            &role,
            own,
        )
        .await?;
        Ok(MemberGrantGate::resolve(
            &own.tools,
            role_profile.as_ref().map(|profile| &profile.tools),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stdio(name: &str) -> meerkat_core::McpServerConfig {
        meerkat_core::McpServerConfig::stdio(
            name,
            format!("{name}-server"),
            Vec::new(),
            std::collections::HashMap::new(),
        )
    }

    /// A member's gate joins its own gate with its role's current gate: a
    /// role server and the role's unconfigured-servers mode are gated from
    /// the moment they appear; the servers the member's own profile
    /// configures and its gate does not name are its ungated servers, which
    /// the mode leaves open; a role family its switches leave off is not
    /// mounted.
    #[test]
    fn a_member_gate_joins_role_servers_and_the_mode_and_lists_its_ungated_servers() {
        let own = ToolConfig {
            shell: false,
            mcp_servers: vec![stdio("calendar"), stdio("tickets")],
            grant_gated: GrantGatedTools::default().with_mcp_server("tickets"),
            ..ToolConfig::default()
        };
        let role = ToolConfig {
            grant_gated: GrantGatedTools::default()
                .with_family("shell")
                .with_mcp_server("mail")
                .with_unconfigured_mcp_servers(),
            ..ToolConfig::default()
        };
        let gate = MemberGrantGate::resolve(&own, Some(&role));
        assert_eq!(
            gate.gated,
            GrantGatedTools::default()
                .with_mcp_server("mail")
                .with_mcp_server("tickets")
                .with_unconfigured_mcp_servers()
        );
        assert_eq!(
            gate.not_mounted,
            GrantGatedTools::default().with_family("shell")
        );
        assert_eq!(
            gate.ungated_servers,
            BTreeSet::from(["calendar".to_string()])
        );
        let named = gate.named();
        assert!(named.gates_mcp_server("drive", &gate.ungated_servers));
        assert!(named.gates_mcp_server("tickets", &gate.ungated_servers));
        assert!(!named.gates_mcp_server("calendar", &gate.ungated_servers));

        let alone = MemberGrantGate::resolve(&own, None);
        assert_eq!(
            alone.gated,
            GrantGatedTools::default().with_mcp_server("tickets")
        );
        assert!(
            !alone
                .named()
                .gates_mcp_server("drive", &alone.ungated_servers),
            "without the mode an unnamed server is not gated"
        );
    }
}
