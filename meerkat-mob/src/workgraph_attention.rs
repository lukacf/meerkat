use crate::{AgentIdentity, MobId};

/// Lower a mob-owned `AgentIdentity` into the WorkGraph owner-key form.
///
/// Core WorkGraph intentionally does not name `AgentIdentity` or `MobId`.
/// Mob validates the identity at its feature boundary and persists only the
/// lowered owner key in WorkGraph attention bindings.
pub fn lower_agent_identity_attention_target(
    mob_id: &MobId,
    identity: &AgentIdentity,
) -> Result<meerkat::WorkAttentionTarget, meerkat::WorkGraphError> {
    Ok(meerkat::WorkAttentionTarget::LoweredOwner {
        owner_key: lower_agent_identity_owner_key(mob_id, identity)?,
    })
}

pub fn lower_agent_identity_owner_key(
    mob_id: &MobId,
    identity: &AgentIdentity,
) -> Result<meerkat::WorkOwnerKey, meerkat::WorkGraphError> {
    meerkat::WorkOwnerKey::agent(format!(
        "mob/{}/agent/{}",
        mob_id.as_str(),
        identity.as_str()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_identity_lowers_to_mob_scoped_workgraph_owner() {
        let mob_id = MobId::from("match3-mob");
        let identity = AgentIdentity::from("reviewer");
        let target =
            lower_agent_identity_attention_target(&mob_id, &identity).expect("lower identity");

        let meerkat::WorkAttentionTarget::LoweredOwner { owner_key } = target else {
            panic!("mob identities must lower to owner keys");
        };
        assert_eq!(owner_key.kind, meerkat::WorkOwnerKind::Agent);
        assert_eq!(owner_key.id, "mob/match3-mob/agent/reviewer");
        assert_eq!(owner_key.canonical(), "agent:mob/match3-mob/agent/reviewer");
    }

    #[test]
    fn same_agent_identity_in_different_mobs_lowers_to_distinct_owner_keys() {
        let identity = AgentIdentity::from("reviewer");
        let first =
            lower_agent_identity_owner_key(&MobId::from("mob-a"), &identity).expect("first owner");
        let second =
            lower_agent_identity_owner_key(&MobId::from("mob-b"), &identity).expect("second owner");

        assert_ne!(first, second);
        assert_eq!(first.canonical(), "agent:mob/mob-a/agent/reviewer");
        assert_eq!(second.canonical(), "agent:mob/mob-b/agent/reviewer");
    }
}
