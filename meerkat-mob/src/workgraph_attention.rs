use crate::AgentIdentity;

/// Lower a mob-owned `AgentIdentity` into the WorkGraph owner-key form.
///
/// Core WorkGraph intentionally does not name `AgentIdentity` or `MobId`.
/// Mob validates the identity at its feature boundary and persists only the
/// lowered owner key in WorkGraph attention bindings.
pub fn lower_agent_identity_attention_target(
    identity: &AgentIdentity,
) -> Result<meerkat::WorkAttentionTarget, meerkat::WorkGraphError> {
    Ok(meerkat::WorkAttentionTarget::LoweredOwner {
        owner_key: lower_agent_identity_owner_key(identity)?,
    })
}

pub fn lower_agent_identity_owner_key(
    identity: &AgentIdentity,
) -> Result<meerkat::WorkOwnerKey, meerkat::WorkGraphError> {
    meerkat::WorkOwnerKey::agent(identity.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_identity_lowers_to_workgraph_owner_without_mob_id() {
        let identity = AgentIdentity::from("reviewer");
        let target = lower_agent_identity_attention_target(&identity).expect("lower identity");

        let meerkat::WorkAttentionTarget::LoweredOwner { owner_key } = target else {
            panic!("mob identities must lower to owner keys");
        };
        assert_eq!(owner_key.kind, meerkat::WorkOwnerKind::Agent);
        assert_eq!(owner_key.id, "reviewer");
        assert_eq!(owner_key.canonical(), "agent:reviewer");
    }
}
