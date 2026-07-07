use crate::{AgentIdentity, MobId};

/// Lower a mob-owned `AgentIdentity` into the WorkGraph owner-key form.
///
/// Core WorkGraph intentionally does not name `AgentIdentity` or `MobId`.
/// Mob validates the identity at its feature boundary and persists only the
/// lowered owner key in WorkGraph attention bindings.
pub fn lower_agent_identity_attention_target(
    mob_id: &MobId,
    identity: &AgentIdentity,
) -> Result<meerkat::GoalAttentionTarget, meerkat::WorkGraphError> {
    Ok(meerkat::GoalAttentionTarget::Owner {
        owner_key: lower_agent_identity_owner_key(mob_id, identity)?,
    })
}

pub fn lower_agent_identity_owner_key(
    mob_id: &MobId,
    identity: &AgentIdentity,
) -> Result<meerkat::WorkOwnerKey, meerkat::WorkGraphError> {
    reject_owner_key_segment("mob_id", mob_id.as_str())?;
    reject_owner_key_segment("agent_identity", identity.as_str())?;
    meerkat::WorkOwnerKey::agent(format!(
        "mob/{}/agent/{}",
        mob_id.as_str(),
        identity.as_str()
    ))
}

fn reject_owner_key_segment(label: &str, value: &str) -> Result<(), meerkat::WorkGraphError> {
    if value.is_empty() || value.contains('/') {
        return Err(meerkat::WorkGraphError::InvalidInput(format!(
            "mob attention owner key {label} must be non-empty and must not contain '/'"
        )));
    }
    Ok(())
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

        let meerkat::GoalAttentionTarget::Owner { owner_key } = target else {
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

    #[test]
    fn owner_key_lowering_rejects_ambiguous_separator_segments() {
        let err =
            lower_agent_identity_owner_key(&MobId::from("mob/a"), &AgentIdentity::from("reviewer"))
                .expect_err("mob ids with separators must fail closed");
        assert!(matches!(err, meerkat::WorkGraphError::InvalidInput(_)));

        let err = lower_agent_identity_owner_key(
            &MobId::from("mob-a"),
            &AgentIdentity::from("reviewer/a"),
        )
        .expect_err("agent identities with separators must fail closed");
        assert!(matches!(err, meerkat::WorkGraphError::InvalidInput(_)));
    }
}
