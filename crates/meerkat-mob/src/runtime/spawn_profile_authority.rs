use crate::error::MobError;
use crate::ids::{AgentIdentity, ProfileName};
use crate::machines::mob_machine as mob_dsl;
use crate::profile::Profile;
use serde::Serialize;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AuthorizedSpawnProfileMaterial {
    pub(super) agent_identity: mob_dsl::AgentIdentity,
    pub(super) profile_name: String,
    pub(super) model: String,
    pub(super) profile_material_digest: String,
    pub(super) tool_config_digest: String,
    pub(super) skills_digest: String,
    pub(super) provider_params_digest: Option<String>,
    pub(super) output_schema_digest: Option<String>,
    pub(super) external_addressable: bool,
    pub(super) resolved_spec_digest: Option<String>,
}

pub(super) fn authorize_spawn_profile_material(
    authority: &mut mob_dsl::MobMachineAuthority,
    agent_identity: &AgentIdentity,
    profile_name: &ProfileName,
    profile: &Profile,
    resolved_spec_digest: Option<String>,
    context: &str,
) -> Result<AuthorizedSpawnProfileMaterial, MobError> {
    let (input, expected) =
        authorize_spawn_profile_input(agent_identity, profile_name, profile, resolved_spec_digest)?;
    let input_debug = format!("{input:?}");
    let transition = mob_dsl::MobMachineMutator::apply(authority, input).map_err(|error| {
        MobError::Internal(format!(
            "{context}: MobMachine rejected spawn profile material {input_debug}: {error}"
        ))
    })?;
    require_authorized_effect(&transition, &expected, context)?;
    Ok(expected)
}

pub(super) fn authorize_spawn_profile_input(
    agent_identity: &AgentIdentity,
    profile_name: &ProfileName,
    profile: &Profile,
    resolved_spec_digest: Option<String>,
) -> Result<(mob_dsl::MobMachineInput, AuthorizedSpawnProfileMaterial), MobError> {
    let profile_material_digest = digest_serializable(&SpawnProfileMaterialDigest {
        profile_name: profile_name.as_str(),
        profile,
    })?;
    let tool_config_digest = digest_serializable(&profile.tools)?;
    let skills_digest = digest_serializable(&profile.skills)?;
    let provider_params_digest = optional_value_digest(profile.provider_params.as_ref())?;
    let output_schema_digest = optional_value_digest(
        profile
            .output_schema
            .as_ref()
            .map(meerkat_core::MeerkatSchema::as_value),
    )?;
    let dsl_identity =
        mob_dsl::AgentIdentity::from_domain(&AgentIdentity::from(agent_identity.as_str()));
    // Local-arm spawn authorization passes `None`; placed spawns pass the
    // compiled portable-spec digest (the machine records it single-shot and
    // `CommitSpawnMembershipRemote` guards the ack echo against it).
    let expected = AuthorizedSpawnProfileMaterial {
        agent_identity: dsl_identity.clone(),
        profile_name: profile_name.as_str().to_string(),
        model: profile.model.clone(),
        profile_material_digest: profile_material_digest.clone(),
        tool_config_digest: tool_config_digest.clone(),
        skills_digest: skills_digest.clone(),
        provider_params_digest: provider_params_digest.clone(),
        output_schema_digest: output_schema_digest.clone(),
        external_addressable: profile.external_addressable,
        resolved_spec_digest: resolved_spec_digest.clone(),
    };
    Ok((
        mob_dsl::MobMachineInput::AuthorizeSpawnProfile {
            agent_identity: dsl_identity,
            profile_name: expected.profile_name.clone(),
            model: expected.model.clone(),
            profile_material_digest,
            tool_config_digest,
            skills_digest,
            provider_params_digest,
            output_schema_digest,
            external_addressable: profile.external_addressable,
            resolved_spec_digest,
        },
        expected,
    ))
}

pub(super) fn require_authorized_effect(
    transition: &mob_dsl::MobMachineTransition,
    expected: &AuthorizedSpawnProfileMaterial,
    context: &str,
) -> Result<(), MobError> {
    let authorized = transition.effects().iter().any(|effect| {
        matches!(
            effect,
            mob_dsl::MobMachineEffect::SpawnProfileAuthorized {
                agent_identity,
                profile_name,
                model,
                profile_material_digest,
                tool_config_digest,
                skills_digest,
                provider_params_digest,
                output_schema_digest,
                external_addressable,
                resolved_spec_digest,
            } if agent_identity == &expected.agent_identity
                && profile_name == &expected.profile_name
                && model == &expected.model
                && profile_material_digest == &expected.profile_material_digest
                && tool_config_digest == &expected.tool_config_digest
                && skills_digest == &expected.skills_digest
                && provider_params_digest == &expected.provider_params_digest
                && output_schema_digest == &expected.output_schema_digest
                && *external_addressable == expected.external_addressable
                && resolved_spec_digest == &expected.resolved_spec_digest
        )
    });
    if authorized {
        return Ok(());
    }
    Err(MobError::Internal(format!(
        "{context}: MobMachine accepted spawn profile material without matching typed authorization"
    )))
}

fn optional_value_digest<T: Serialize>(value: Option<&T>) -> Result<Option<String>, MobError> {
    value.map(digest_serializable).transpose()
}

fn digest_serializable(value: &impl Serialize) -> Result<String, MobError> {
    let encoded = serde_json::to_vec(value).map_err(|error| {
        MobError::Internal(format!(
            "failed to encode spawn profile material for authority digest: {error}"
        ))
    })?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}

#[derive(Serialize)]
struct SpawnProfileMaterialDigest<'a> {
    profile_name: &'a str,
    profile: &'a Profile,
}
