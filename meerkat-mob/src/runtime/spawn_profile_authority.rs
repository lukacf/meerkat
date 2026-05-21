use crate::error::MobError;
use crate::ids::{AgentIdentity, MeerkatId, ProfileName};
use crate::machines::mob_machine as mob_dsl;
use crate::profile::Profile;
use sha2::{Digest, Sha256};

pub(super) fn authorize_spawn_profile_material(
    state: &mob_dsl::MobMachineState,
    agent_identity: &MeerkatId,
    profile_name: &ProfileName,
    profile: &Profile,
    context: &str,
) -> Result<(), MobError> {
    let provider_params_digest = provider_params_digest(profile.provider_params.as_ref())?;
    let dsl_identity =
        mob_dsl::AgentIdentity::from_domain(&AgentIdentity::from(agent_identity.as_str()));
    let input = mob_dsl::MobMachineInput::AuthorizeSpawnProfile {
        agent_identity: dsl_identity.clone(),
        profile_name: profile_name.as_str().to_string(),
        model: profile.model.clone(),
        provider_params_digest: provider_params_digest.clone(),
        external_addressable: profile.external_addressable,
    };
    let input_debug = format!("{input:?}");
    let mut authority =
        mob_dsl::MobMachineAuthority::recover_from_state(state.clone()).map_err(|error| {
            MobError::Internal(format!(
                "{context}: could not recover MobMachine state: {error}"
            ))
        })?;
    let transition = mob_dsl::MobMachineMutator::apply(&mut authority, input).map_err(|error| {
        MobError::Internal(format!(
            "{context}: MobMachine rejected spawn profile material {input_debug}: {error}"
        ))
    })?;
    let authorized = transition.effects().iter().any(|effect| {
        matches!(
            effect,
            mob_dsl::MobMachineEffect::SpawnProfileAuthorized {
                agent_identity,
                profile_name: effect_profile_name,
                model,
                provider_params_digest: effect_digest,
                external_addressable,
            } if *agent_identity == dsl_identity
                && effect_profile_name == profile_name.as_str()
                && model == &profile.model
                && effect_digest == &provider_params_digest
                && *external_addressable == profile.external_addressable
        )
    });
    if authorized {
        return Ok(());
    }
    Err(MobError::Internal(format!(
        "{context}: MobMachine accepted spawn profile material without matching typed authorization"
    )))
}

fn provider_params_digest(
    provider_params: Option<&serde_json::Value>,
) -> Result<Option<String>, MobError> {
    let Some(provider_params) = provider_params else {
        return Ok(None);
    };
    let encoded = serde_json::to_vec(provider_params).map_err(|error| {
        MobError::Internal(format!(
            "failed to encode spawn profile provider params for authority digest: {error}"
        ))
    })?;
    Ok(Some(format!("{:x}", Sha256::digest(encoded))))
}
