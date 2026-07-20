//! Controlling-host portable member-spec compiler (multi-host mobs phase 3,
//! design-spec-compiler-dispatch §3.2).
//!
//! Pure and actor-state-free: identical inputs compile to an identical
//! [`PortableMemberSpec`] and canonical digest. Consumed by `enqueue_spawn`,
//! `spawn_from_policy_inline`, and revival re-materialization. The member
//! host re-runs `build_agent_config` over the DECODED spec — every field row
//! below states its `build_agent_config` twin so "one compiler, one owner"
//! stays checkable.
//!
//! House rules: typed errors only (no silent fallbacks); residual `Inherit`
//! (prompt or policy) is a typed compile failure, never a resolve-to-
//! unrestricted mapping (DEC-P3F-3); secret-bearing value maps are
//! fail-closed rejects, never stripped (R5/A10).

use std::collections::BTreeMap;

use meerkat_contracts::wire::portable_member_spec_digest;
use meerkat_contracts::wire::supervisor_bridge::WireOpaqueJson;
use meerkat_contracts::wire::{
    PortableDefinitionExtract, PortableMemberSpec, PortableSkillSource, PortableSpawnOverlay,
    PortableSystemPrompt, WireMobToolAuthorityContext, WireNonPortableResourceKind,
    WireResolvedToolAccessPolicy, WireSpawnContinuityIntent,
};

use crate::definition::{MobDefinition, SkillSource};
use crate::error::MobError;
use crate::identity::{
    DesiredExecution, DesiredMemberMaterial, DesiredMemberOverlay, IdentityProfileMemberDeclaration,
};
use crate::ids::{AgentIdentity, MobId, ProfileName};
use crate::portable_profile::{project_portable_profile, wire_runtime_mode};
use crate::profile::Profile;

/// R3 case-3 base-prompt seam (ADJ-2): resolves the CONTROLLING host's
/// assembled base system prompt (config/AGENTS.md chain) for remote spawns
/// whose profile carries no skills and no per-spawn prompt override.
///
/// Implemented by the facade over the `AgentFactory` prompt chain and
/// injected at `MobBuilder::with_spawn_base_prompt_source`. ABSENT on a
/// case-3 remote spawn ⇒ typed compile failure — never a silent `Inherit`
/// (which would diverge remote members from local semantics invisibly).
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
pub trait SpawnBasePromptSource: Send + Sync {
    /// Resolve the controlling host's base system prompt text. An EMPTY
    /// resolved base is a truthful answer (compiles to `Disable`); a
    /// resolution fault must be a typed error.
    async fn resolve_base_system_prompt(&self) -> Result<String, MobError>;
}

/// A static, pre-resolved base prompt (explicit injection; also the test
/// seam). The facade implementation resolves the factory chain instead.
pub struct StaticSpawnBasePromptSource(pub String);

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl SpawnBasePromptSource for StaticSpawnBasePromptSource {
    async fn resolve_base_system_prompt(&self) -> Result<String, MobError> {
        Ok(self.0.clone())
    }
}

/// ADJ-2 facade-chain implementation: resolves the controlling host's base
/// system prompt through the ONE meerkat prompt-assembly precedence chain
/// (`system_prompt_file` → config inline → AGENTS.md → default) — exactly
/// what the factory's `Inherit` resolution produces for a local skill-less
/// member, minus the per-session appended sections (skill inventory, tool
/// instructions), which are session facts, not base-prompt facts.
///
/// Lives in meerkat-mob (which depends on the facade) because the facade
/// cannot name this crate's trait; composition surfaces (CLI, embedders)
/// construct it with the SAME config + context root their factory uses.
#[cfg(not(target_arch = "wasm32"))]
pub struct FactoryChainSpawnBasePromptSource {
    config: std::sync::Arc<meerkat::Config>,
    context_root: Option<std::path::PathBuf>,
}

#[cfg(not(target_arch = "wasm32"))]
impl FactoryChainSpawnBasePromptSource {
    pub fn new(
        config: std::sync::Arc<meerkat::Config>,
        context_root: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            config,
            context_root,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[async_trait::async_trait]
impl SpawnBasePromptSource for FactoryChainSpawnBasePromptSource {
    async fn resolve_base_system_prompt(&self) -> Result<String, MobError> {
        meerkat::assemble_system_prompt(
            &self.config,
            &meerkat::SystemPromptOverride::Inherit,
            self.context_root.as_deref(),
            &[],
            "",
        )
        .await
        .map_err(|error| {
            MobError::WiringError(format!(
                "controlling-host base system prompt resolution failed: {error}"
            ))
        })
    }
}

/// Inputs to [`compile_portable_member_spec`] — the same resolved material
/// `build_agent_config` would receive, plus the overlay inputs straight from
/// the spawn-spec destructure.
pub(crate) struct CompileMemberSpecParams<'a> {
    pub mob_id: &'a MobId,
    pub profile_name: &'a ProfileName,
    pub agent_identity: &'a AgentIdentity,
    /// AFTER override/realm/inherited-open resolution.
    pub profile: &'a Profile,
    pub definition: &'a MobDefinition,
    pub context: Option<&'a serde_json::Value>,
    pub labels: Option<&'a BTreeMap<String, String>>,
    pub additional_instructions: Option<&'a Vec<String>>,
    pub system_prompt_override: Option<&'a super::handle::SpawnSystemPromptOverride>,
    pub tool_access_policy: Option<&'a meerkat_core::ops::ToolAccessPolicy>,
    pub auth_binding: Option<&'a meerkat_core::AuthBindingRef>,
    pub budget_limits: Option<&'a meerkat_core::BudgetLimits>,
    /// The SELECTED runtime mode (spec-or-profile resolution).
    pub runtime_mode: crate::MobRuntimeMode,
    pub continuity_intent: &'a super::handle::SpawnContinuityIntent,
    /// R3 case-3 seam (ADJ-2). `None` ⇒ case-3 remote spawns fail typed.
    pub base_prompt: Option<&'a dyn SpawnBasePromptSource>,
    /// Non-portable resolve-to-disabled record. v1: always empty — the
    /// inherited-filter case is hard-denied at the ladder (ADJ-6), so the
    /// projection field stays a typed always-empty record.
    pub non_portable_disabled: Vec<WireNonPortableResourceKind>,
}

/// Inputs for compiling the sole durable, authority-free member material.
///
/// `resolved_profile` is present only when the declaration names a canonical
/// definition/realm profile. A declaration carrying `profile_override`
/// already supplies the exact portable profile and therefore must not also
/// carry a second mutable profile copy.
pub(crate) struct CompileDesiredMemberMaterialParams<'a> {
    pub agent_identity: &'a AgentIdentity,
    pub declaration: &'a IdentityProfileMemberDeclaration,
    pub resolved_profile: Option<&'a Profile>,
    pub definition: &'a MobDefinition,
    pub base_prompt: Option<&'a dyn SpawnBasePromptSource>,
    pub non_portable_disabled: Vec<WireNonPortableResourceKind>,
}

/// Canonical wire ordering for an unordered tool-name set: the digest-covered
/// spec must hash identically for identical policies.
fn sorted_tool_names(names: &meerkat_core::types::ToolNameSet) -> Vec<String> {
    let mut sorted: Vec<String> = names.iter().map(|name| name.as_str().to_string()).collect();
    sorted.sort();
    sorted
}

/// Compile the exact authority-free desired material sealed by identity
/// intent. This function never receives a mob id, continuity target, operator
/// authority, callback dispatcher, or secret value.
pub(crate) async fn compile_desired_member_material(
    params: CompileDesiredMemberMaterialParams<'_>,
) -> Result<DesiredMemberMaterial, MobError> {
    let CompileDesiredMemberMaterialParams {
        agent_identity,
        declaration,
        resolved_profile,
        definition,
        base_prompt,
        non_portable_disabled,
    } = params;

    declaration.validate().map_err(|error| {
        MobError::WiringError(format!(
            "identity declaration for '{agent_identity}' is invalid: {error}"
        ))
    })?;

    let declared_runtime_mode =
        if matches!(declaration.execution, DesiredExecution::External { .. }) {
            Some(meerkat_contracts::wire::WireMobRuntimeMode::TurnDriven)
        } else {
            declaration.runtime_mode
        };
    let mut portable_profile = match (&declaration.profile_override, resolved_profile) {
        (Some(portable), None) => {
            let mut portable = portable.clone();
            if let Some(model) = &declaration.model_override {
                portable.model.clone_from(model);
            }
            if let Some(external_addressable) = declaration.external_addressable_override {
                portable.external_addressable = external_addressable;
            }
            portable
        }
        (None, Some(profile)) => {
            let mut profile = profile.clone();
            if let Some(model) = &declaration.model_override {
                profile.model.clone_from(model);
            }
            if let Some(external_addressable) = declaration.external_addressable_override {
                profile.external_addressable = external_addressable;
            }
            let runtime_mode = declared_runtime_mode
                .map(domain_runtime_mode)
                .unwrap_or(profile.runtime_mode);
            project_portable_profile(
                &profile,
                runtime_mode,
                &definition.models,
                agent_identity.as_str(),
                declaration.profile_name.as_str(),
                non_portable_disabled,
            )
            .map_err(MobError::WiringError)?
        }
        (Some(_), Some(_)) => {
            return Err(MobError::Internal(format!(
                "identity declaration compiler received both portable and resolved profiles for '{agent_identity}'"
            )));
        }
        (None, None) => {
            return Err(MobError::Internal(format!(
                "identity declaration compiler received no resolved profile for '{agent_identity}'"
            )));
        }
    };

    let runtime_mode = declared_runtime_mode.unwrap_or(portable_profile.runtime_mode);
    portable_profile.runtime_mode = runtime_mode;
    let skills = compile_inline_skills(
        definition,
        &portable_profile.skills,
        &declaration.profile_name,
    )
    .await?;
    let system_prompt = resolve_portable_system_prompt(
        agent_identity,
        &declaration.profile_name,
        &portable_profile.skills,
        &skills,
        declaration.system_prompt_override.as_ref(),
        base_prompt,
    )
    .await?;

    let mut profile_names = definition
        .profiles
        .keys()
        .map(|profile| profile.as_str().to_string())
        .collect::<Vec<_>>();
    if profile_names
        .binary_search_by(|name| name.as_str().cmp(declaration.profile_name.as_str()))
        .is_err()
    {
        profile_names.push(declaration.profile_name.as_str().to_string());
        profile_names.sort();
    }
    let mut required_local_callback_tools = declaration.required_local_callback_tools.clone();
    required_local_callback_tools.sort_by(|left, right| left.name.cmp(&right.name));
    let material = DesiredMemberMaterial {
        profile_name: declaration.profile_name.clone(),
        profile: portable_profile,
        definition_extract: PortableDefinitionExtract {
            models: definition.models.clone(),
            image_generation_provider: definition.image_generation_provider,
            skills,
            profile_names,
        },
        overlay: DesiredMemberOverlay {
            context: declaration.context.clone(),
            labels: declaration.labels.clone(),
            additional_instructions: declaration.additional_instructions.clone(),
            system_prompt,
            tool_access_policy: declaration.tool_access_policy.clone(),
            auth_binding: declaration.auth_binding.clone(),
            budget_limits: declaration.budget_limits.clone(),
            runtime_mode,
        },
        required_env_keys: declaration.required_env_keys.clone(),
        required_local_callback_tools,
        execution: declaration.execution.clone(),
    };
    material.validate().map_err(|error| {
        MobError::WiringError(format!(
            "compiled identity material for '{agent_identity}' is invalid: {error}"
        ))
    })?;
    Ok(material)
}

/// Lower one already sealed authority-free desired member into the existing
/// local resume mechanic. This path never recompiles caller material and
/// never carries the one-shot initial delivery; the identity reconciler owns
/// that effect separately by stable `InputId`.
pub(crate) fn spawn_spec_from_desired_member(
    identity: &AgentIdentity,
    session: &crate::identity::DesiredSessionTarget,
    member: &crate::identity::DesiredMemberSpec,
) -> Result<super::handle::SpawnMemberSpec, MobError> {
    use meerkat_contracts::wire::{PortableSystemPrompt, WireResolvedToolAccessPolicy};

    member.validate().map_err(|error| {
        MobError::WiringError(format!(
            "sealed desired member for '{identity}' is invalid: {error}"
        ))
    })?;
    let material = &member.material;
    let profile = crate::portable_profile::rehydrate_portable_profile(&material.profile).map_err(
        |error| {
            MobError::WiringError(format!(
                "sealed desired profile for '{identity}' cannot be materialized: {error}"
            ))
        },
    )?;
    let mut spec =
        super::handle::SpawnMemberSpec::new(material.profile_name.clone(), identity.clone());
    spec.initial_message = None;
    spec.launch_mode = crate::launch::MemberLaunchMode::Resume {
        bridge_session_id: session.session_id.clone(),
    };
    spec.runtime_mode = Some(domain_runtime_mode(material.overlay.runtime_mode));
    spec.context = material
        .overlay
        .context
        .as_ref()
        .map(|value| value.to_value())
        .transpose()
        .map_err(|error| {
            MobError::WiringError(format!(
                "sealed desired context for '{identity}' is malformed: {error}"
            ))
        })?;
    spec.labels = material.overlay.labels.clone();
    spec.additional_instructions = material.overlay.additional_instructions.clone();
    spec.tool_access_policy =
        material
            .overlay
            .tool_access_policy
            .as_ref()
            .map(|policy| match policy {
                WireResolvedToolAccessPolicy::AllowList(names) => {
                    meerkat_core::ops::ToolAccessPolicy::AllowList(names.iter().cloned().collect())
                }
                WireResolvedToolAccessPolicy::DenyList(names) => {
                    meerkat_core::ops::ToolAccessPolicy::DenyList(names.iter().cloned().collect())
                }
            });
    spec.auth_binding = material.overlay.auth_binding.clone().map(Into::into);
    spec.budget_limits = material.overlay.budget_limits.clone();
    spec.system_prompt_override = Some(match &material.overlay.system_prompt {
        PortableSystemPrompt::Set { text } => {
            super::handle::SpawnSystemPromptOverride::Replace(text.clone())
        }
        PortableSystemPrompt::Disable => super::handle::SpawnSystemPromptOverride::Disable,
    });
    spec.override_profile = Some(profile);
    spec.continuity_intent = super::handle::SpawnContinuityIntent::DurableIdentity {
        continuity_key: identity.as_str().to_string(),
    };
    match &material.execution {
        DesiredExecution::ControllingSession => {
            spec.binding = Some(crate::RuntimeBinding::Session);
        }
        DesiredExecution::PlacedSession { .. } => {
            return Err(MobError::WiringError(format!(
                "identity '{identity}' placed-session recovery is not available in the local convergence slice"
            )));
        }
        DesiredExecution::AnyBoundHostSession => {
            return Err(MobError::WiringError(format!(
                "identity '{identity}' requires a fresh bound-host observation before materialization"
            )));
        }
        DesiredExecution::External { .. } => {
            return Err(MobError::WiringError(format!(
                "identity '{identity}' requires fresh external ceremony/trust before materialization"
            )));
        }
    }
    Ok(spec)
}

fn domain_runtime_mode(mode: meerkat_contracts::wire::WireMobRuntimeMode) -> crate::MobRuntimeMode {
    match mode {
        meerkat_contracts::wire::WireMobRuntimeMode::AutonomousHost => {
            crate::MobRuntimeMode::AutonomousHost
        }
        meerkat_contracts::wire::WireMobRuntimeMode::TurnDriven => {
            crate::MobRuntimeMode::TurnDriven
        }
    }
}

async fn compile_inline_skills(
    definition: &MobDefinition,
    skill_names: &[String],
    profile_name: &ProfileName,
) -> Result<BTreeMap<String, PortableSkillSource>, MobError> {
    let mut skills = BTreeMap::new();
    for skill_name in skill_names {
        let source = definition.skills.get(skill_name).ok_or_else(|| {
            MobError::WiringError(format!(
                "profile '{profile_name}' references skill '{skill_name}' that is not defined in the mob definition"
            ))
        })?;
        let inline = match source {
            SkillSource::Inline { content } => PortableSkillSource::Inline {
                content: content.clone(),
            },
            SkillSource::Path { path } => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let content = tokio::fs::read_to_string(path).await.map_err(|error| {
                        MobError::WiringError(format!(
                            "failed to read skill file '{path}' for skill '{skill_name}' while compiling portable member material: {error}"
                        ))
                    })?;
                    PortableSkillSource::Inline { content }
                }
                #[cfg(target_arch = "wasm32")]
                {
                    return Err(MobError::WiringError(format!(
                        "file-based skill path '{path}' cannot be compiled into portable member material on wasm32"
                    )));
                }
            }
        };
        skills.insert(skill_name.clone(), inline);
    }
    Ok(skills)
}

async fn resolve_portable_system_prompt(
    agent_identity: &AgentIdentity,
    profile_name: &ProfileName,
    skill_names: &[String],
    skills: &BTreeMap<String, PortableSkillSource>,
    override_value: Option<&PortableSystemPrompt>,
    base_prompt: Option<&dyn SpawnBasePromptSource>,
) -> Result<PortableSystemPrompt, MobError> {
    if let Some(override_value) = override_value {
        return Ok(override_value.clone());
    }
    let assembled = assemble_inline_skill_prompt(skill_names, skills);
    if !assembled.is_empty() {
        return Ok(PortableSystemPrompt::Set { text: assembled });
    }
    let Some(base_prompt) = base_prompt else {
        return Err(MobError::WiringError(format!(
            "member material for '{agent_identity}' needs the controlling host's base system prompt (profile '{profile_name}' has no skills and no prompt override) but no SpawnBasePromptSource is wired; inject one via MobBuilder::with_spawn_base_prompt_source (never a silent Inherit)"
        )));
    };
    let base = base_prompt.resolve_base_system_prompt().await?;
    if base.is_empty() {
        Ok(PortableSystemPrompt::Disable)
    } else {
        Ok(PortableSystemPrompt::Set { text: base })
    }
}

fn resolved_tool_access_policy(
    policy: Option<&meerkat_core::ops::ToolAccessPolicy>,
    agent_identity: &AgentIdentity,
) -> Result<Option<WireResolvedToolAccessPolicy>, MobError> {
    match policy {
        None => Ok(None),
        Some(meerkat_core::ops::ToolAccessPolicy::AllowList(names)) => Ok(Some(
            WireResolvedToolAccessPolicy::AllowList(sorted_tool_names(names)),
        )),
        Some(meerkat_core::ops::ToolAccessPolicy::DenyList(names)) => Ok(Some(
            WireResolvedToolAccessPolicy::DenyList(sorted_tool_names(names)),
        )),
        Some(meerkat_core::ops::ToolAccessPolicy::Inherit) => Err(MobError::WiringError(format!(
            "tool access policy for member '{agent_identity}' is still Inherit at material compile; callers must resolve it before the actor seals desired material"
        ))),
    }
}

/// Output of the compiler: the digest-covered spec, its canonical digest
/// (computed EXACTLY ONCE, here — every later appearance is the machine's
/// copy or the wire echo, never a shell recompute), and the minted operator
/// authority when `profile.tools.mob`.
#[derive(Debug)]
pub(crate) struct CompiledMemberSpec {
    pub spec: PortableMemberSpec,
    pub digest: String,
    /// Minted iff `profile.tools.mob`; durably recorded before dispatch
    /// (ADJ-15 — write-site lives with the spawn ladder open, same
    /// transition witness as the spec's authority context).
    pub operator_authority: Option<meerkat_core::service::MobToolAuthorityContext>,
}

pub(crate) async fn compile_portable_member_spec(
    params: CompileMemberSpecParams<'_>,
) -> Result<CompiledMemberSpec, MobError> {
    let CompileMemberSpecParams {
        mob_id,
        profile_name,
        agent_identity,
        profile,
        definition,
        context,
        labels,
        additional_instructions,
        system_prompt_override,
        tool_access_policy,
        auth_binding,
        budget_limits,
        runtime_mode,
        continuity_intent,
        base_prompt,
        non_portable_disabled,
    } = params;

    let system_prompt_override =
        system_prompt_override.map(|override_value| match override_value {
            super::handle::SpawnSystemPromptOverride::Replace(prompt) => {
                PortableSystemPrompt::Set {
                    text: prompt.clone(),
                }
            }
            super::handle::SpawnSystemPromptOverride::Disable => PortableSystemPrompt::Disable,
        });
    let tool_access_policy = resolved_tool_access_policy(tool_access_policy, agent_identity)?;
    let declaration = IdentityProfileMemberDeclaration {
        profile_name: profile_name.clone(),
        profile_override: None,
        model_override: None,
        external_addressable_override: None,
        context: context.map(WireOpaqueJson::from_value),
        labels: labels.cloned(),
        additional_instructions: additional_instructions.cloned(),
        system_prompt_override,
        tool_access_policy,
        auth_binding: auth_binding.cloned().map(Into::into),
        budget_limits: budget_limits.cloned(),
        runtime_mode: Some(wire_runtime_mode(runtime_mode)),
        required_env_keys: Vec::new(),
        required_local_callback_tools: Vec::new(),
        execution: DesiredExecution::ControllingSession,
    };
    let material = compile_desired_member_material(CompileDesiredMemberMaterialParams {
        agent_identity,
        declaration: &declaration,
        resolved_profile: Some(profile),
        definition,
        base_prompt,
        non_portable_disabled,
    })
    .await?;

    // Operator authority: mint via the build.rs:249-273 path, then PROJECT
    // to the wire (visibility composition only; dispatch authority is
    // re-resolved controlling-side from the durable record, R6).
    let operator_authority = if profile.tools.mob {
        let (_override_mob, authority) =
            crate::build::resolve_profile_mob_operator_access(profile, None);
        match authority {
            Some(authority_context) => {
                let profile_names = definition
                    .profiles
                    .keys()
                    .map(|profile| profile.as_str().to_string())
                    .collect::<Vec<_>>();
                let authority_context =
                    meerkat_runtime::mob_operator_authority::grant_spawn_profiles_in_mob(
                        &authority_context,
                        mob_id.as_str(),
                        profile_names,
                    )
                    .map_err(|error| {
                        MobError::Internal(format!(
                            "generated mob operator authority rejected default spawn profiles for remote spawn of '{agent_identity}': {error}"
                        ))
                    })?;
                // A placed member's ONLY channel to its mob is the upcall
                // lane; the phase exit gate (plan §17 T-19: upcall
                // `list_members` / `retire_member` complete) requires the
                // re-minted authority to carry manage scope of the CURRENT
                // mob, so the durable record's facts include it.
                Some(
                    meerkat_runtime::mob_operator_authority::grant_manage_mob(
                        &authority_context,
                        mob_id.as_str(),
                    )
                    .map_err(|error| {
                        MobError::Internal(format!(
                            "generated mob operator authority rejected current-mob manage scope for remote spawn of '{agent_identity}': {error}"
                        ))
                    })?,
                )
            }
            None => None,
        }
    } else {
        None
    };
    let mob_tool_authority_context = operator_authority
        .as_ref()
        .map(project_mob_tool_authority_context)
        .transpose()?;
    let DesiredMemberMaterial {
        profile: portable_profile,
        definition_extract,
        overlay,
        required_env_keys,
        ..
    } = material;
    let DesiredMemberOverlay {
        context,
        labels,
        additional_instructions,
        system_prompt,
        tool_access_policy,
        auth_binding,
        budget_limits,
        runtime_mode,
    } = overlay;

    let spec = PortableMemberSpec {
        mob_id: mob_id.as_str().to_string(),
        profile_name: profile_name.as_str().to_string(),
        agent_identity: agent_identity.as_str().to_string(),
        profile: portable_profile,
        definition_extract,
        overlay: PortableSpawnOverlay {
            context,
            labels,
            additional_instructions,
            system_prompt,
            tool_access_policy,
            mob_tool_authority_context,
            auth_binding,
            budget_limits,
            runtime_mode,
            continuity_intent: wire_continuity_intent(continuity_intent),
        },
        required_env_keys,
    };

    let digest = portable_member_spec_digest(&spec).map_err(|error| {
        MobError::Internal(format!(
            "portable member spec for '{agent_identity}' failed canonical digest computation: {error}"
        ))
    })?;

    Ok(CompiledMemberSpec {
        spec,
        digest,
        operator_authority,
    })
}

/// Prompt assembly over the ALREADY-INLINED skill map — byte-identical to
/// `build::assemble_system_prompt` output for the same profile (the member
/// host re-derives it from `definition_extract.skills`).
fn assemble_inline_skill_prompt(
    skill_names: &[String],
    skills: &BTreeMap<String, PortableSkillSource>,
) -> String {
    let mut sections = Vec::new();
    for skill_name in skill_names {
        if let Some(PortableSkillSource::Inline { content }) = skills.get(skill_name) {
            sections.push(content.as_str());
        }
    }
    sections.join("\n\n")
}

/// Project the sealed domain authority context onto the wire mirror through
/// its documented serde lane. The seal never serializes; the projection
/// cannot mint authority (portable_spec.rs doc contract).
pub(crate) fn project_mob_tool_authority_context(
    authority: &meerkat_core::service::MobToolAuthorityContext,
) -> Result<WireMobToolAuthorityContext, MobError> {
    let value = serde_json::to_value(authority).map_err(|error| {
        MobError::Internal(format!(
            "mob tool authority context failed wire projection encode: {error}"
        ))
    })?;
    serde_json::from_value(value).map_err(|error| {
        MobError::Internal(format!(
            "mob tool authority context failed wire projection decode: {error}"
        ))
    })
}

fn wire_continuity_intent(
    intent: &super::handle::SpawnContinuityIntent,
) -> WireSpawnContinuityIntent {
    match intent {
        super::handle::SpawnContinuityIntent::Ephemeral => WireSpawnContinuityIntent::Ephemeral,
        super::handle::SpawnContinuityIntent::DurableIdentity { continuity_key } => {
            WireSpawnContinuityIntent::DurableIdentity {
                continuity_key: continuity_key.clone(),
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::profile::ToolConfig;

    fn worker_profile() -> Profile {
        Profile {
            model: "claude-haiku-4-5-20251001".to_string(),
            provider: None,
            self_hosted_server_id: None,
            image_generation_provider: None,
            auto_compact_threshold: None,
            resume_overrides: Vec::new(),
            skills: vec![],
            tools: ToolConfig {
                comms: true,
                ..ToolConfig::default()
            },
            peer_description: "compiler unit worker".to_string(),
            external_addressable: true,
            backend: None,
            runtime_mode: crate::MobRuntimeMode::TurnDriven,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: None,
        }
    }

    fn definition() -> MobDefinition {
        MobDefinition::explicit(MobId::from("compiler-unit"))
    }

    fn params<'a>(
        mob_id: &'a MobId,
        profile_name: &'a ProfileName,
        identity: &'a AgentIdentity,
        profile: &'a Profile,
        definition: &'a MobDefinition,
        base: Option<&'a dyn SpawnBasePromptSource>,
        continuity: &'a super::super::handle::SpawnContinuityIntent,
    ) -> CompileMemberSpecParams<'a> {
        CompileMemberSpecParams {
            mob_id,
            profile_name,
            agent_identity: identity,
            profile,
            definition,
            context: None,
            labels: None,
            additional_instructions: None,
            system_prompt_override: None,
            tool_access_policy: None,
            auth_binding: None,
            budget_limits: None,
            runtime_mode: crate::MobRuntimeMode::TurnDriven,
            continuity_intent: continuity,
            base_prompt: base,
            non_portable_disabled: Vec::new(),
        }
    }

    /// U1: compile determinism + digest recompute agreement.
    #[tokio::test]
    async fn compile_is_deterministic_and_digest_recomputes() {
        let mob_id = MobId::from("compiler-unit");
        let profile_name = ProfileName::from("worker");
        let identity = AgentIdentity::from("b2");
        let profile = worker_profile();
        let definition = definition();
        let base = StaticSpawnBasePromptSource("base prompt".to_string());
        let continuity = super::super::handle::SpawnContinuityIntent::Ephemeral;
        let first = compile_portable_member_spec(params(
            &mob_id,
            &profile_name,
            &identity,
            &profile,
            &definition,
            Some(&base),
            &continuity,
        ))
        .await
        .expect("compile");
        let second = compile_portable_member_spec(params(
            &mob_id,
            &profile_name,
            &identity,
            &profile,
            &definition,
            Some(&base),
            &continuity,
        ))
        .await
        .expect("compile twice");
        assert_eq!(first.spec, second.spec);
        assert_eq!(first.digest, second.digest);
        assert_eq!(
            first.digest,
            portable_member_spec_digest(&first.spec).expect("recompute"),
        );
    }

    #[tokio::test]
    async fn desired_material_compiler_seals_overrides_and_canonical_callback_tools() {
        let identity = AgentIdentity::from("worker-a");
        let profile = worker_profile();
        let definition = definition();
        let declaration = IdentityProfileMemberDeclaration {
            profile_name: ProfileName::from("worker"),
            profile_override: None,
            model_override: Some("claude-opus-4-8".to_string()),
            external_addressable_override: Some(false),
            context: None,
            labels: None,
            additional_instructions: None,
            system_prompt_override: Some(PortableSystemPrompt::Disable),
            tool_access_policy: None,
            auth_binding: None,
            budget_limits: None,
            runtime_mode: Some(meerkat_contracts::wire::WireMobRuntimeMode::TurnDriven),
            required_env_keys: Vec::new(),
            required_local_callback_tools: vec![
                crate::identity::DesiredLocalCallbackTool::new(
                    "zeta",
                    "Tool zeta",
                    serde_json::json!({}),
                )
                .unwrap(),
                crate::identity::DesiredLocalCallbackTool::new(
                    "alpha",
                    "Tool alpha",
                    serde_json::json!({}),
                )
                .unwrap(),
            ],
            execution: DesiredExecution::ControllingSession,
        };
        let material = compile_desired_member_material(CompileDesiredMemberMaterialParams {
            agent_identity: &identity,
            declaration: &declaration,
            resolved_profile: Some(&profile),
            definition: &definition,
            base_prompt: None,
            non_portable_disabled: Vec::new(),
        })
        .await
        .expect("compile desired material");

        assert_eq!(material.profile.model, "claude-opus-4-8");
        assert!(!material.profile.external_addressable);
        assert_eq!(
            material
                .required_local_callback_tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "zeta"]
        );
        material.validate().expect("sealed material is canonical");

        let session_id = meerkat_core::SessionId::new();
        let session = crate::identity::DesiredSessionTarget {
            session_id: session_id.clone(),
            lineage_id: meerkat_core::SessionLineageId::for_session(&session_id),
            lineage_generation: meerkat_core::SessionGeneration::INITIAL,
            authority_policy: crate::identity::DesiredSessionAuthorityPolicy::CreateIfAbsent,
        };
        let intent_for = |material| crate::identity::IdentityIntent::Present {
            identity: identity.clone(),
            session: session.clone(),
            member: Box::new(crate::identity::DesiredMemberSpec {
                material,
                initial_delivery: None,
            }),
            owned_wiring: Default::default(),
        };
        let original_digest = intent_for(material.clone()).digest().unwrap();
        let mut changed = material;
        changed.required_local_callback_tools[0]
            .description
            .push_str(" changed");
        changed.validate().unwrap();
        assert_ne!(original_digest, intent_for(changed).digest().unwrap());
    }

    /// U3(iii): base source absent on a case-3 spawn is a typed failure.
    #[tokio::test]
    async fn case_three_without_base_source_fails_typed() {
        let mob_id = MobId::from("compiler-unit");
        let profile_name = ProfileName::from("worker");
        let identity = AgentIdentity::from("b2");
        let profile = worker_profile();
        let definition = definition();
        let continuity = super::super::handle::SpawnContinuityIntent::Ephemeral;
        let error = compile_portable_member_spec(params(
            &mob_id,
            &profile_name,
            &identity,
            &profile,
            &definition,
            None,
            &continuity,
        ))
        .await
        .expect_err("case-3 without a base source must fail typed");
        assert!(matches!(error, MobError::WiringError(_)), "got {error:?}");
    }

    /// U3(iv): empty resolved base compiles Disable; non-empty compiles Set.
    #[tokio::test]
    async fn base_prompt_resolution_maps_to_set_or_disable() {
        let mob_id = MobId::from("compiler-unit");
        let profile_name = ProfileName::from("worker");
        let identity = AgentIdentity::from("b2");
        let profile = worker_profile();
        let definition = definition();
        let continuity = super::super::handle::SpawnContinuityIntent::Ephemeral;
        let empty = StaticSpawnBasePromptSource(String::new());
        let compiled = compile_portable_member_spec(params(
            &mob_id,
            &profile_name,
            &identity,
            &profile,
            &definition,
            Some(&empty),
            &continuity,
        ))
        .await
        .expect("compile");
        assert!(matches!(
            compiled.spec.overlay.system_prompt,
            PortableSystemPrompt::Disable
        ));
        let non_empty = StaticSpawnBasePromptSource("You are a helpful mob member.".to_string());
        let compiled = compile_portable_member_spec(params(
            &mob_id,
            &profile_name,
            &identity,
            &profile,
            &definition,
            Some(&non_empty),
            &continuity,
        ))
        .await
        .expect("compile");
        assert!(matches!(
            compiled.spec.overlay.system_prompt,
            PortableSystemPrompt::Set { .. }
        ));
    }

    /// U4: residual Inherit at compile is a typed error (never the
    /// build.rs Inherit→None fold).
    #[tokio::test]
    async fn residual_inherit_policy_fails_typed() {
        let mob_id = MobId::from("compiler-unit");
        let profile_name = ProfileName::from("worker");
        let identity = AgentIdentity::from("b2");
        let profile = worker_profile();
        let definition = definition();
        let base = StaticSpawnBasePromptSource("base".to_string());
        let continuity = super::super::handle::SpawnContinuityIntent::Ephemeral;
        let mut p = params(
            &mob_id,
            &profile_name,
            &identity,
            &profile,
            &definition,
            Some(&base),
            &continuity,
        );
        let inherit = meerkat_core::ops::ToolAccessPolicy::Inherit;
        p.tool_access_policy = Some(&inherit);
        let error = compile_portable_member_spec(p)
            .await
            .expect_err("Inherit at spec-mint must fail typed");
        assert!(matches!(error, MobError::WiringError(_)), "got {error:?}");
    }

    /// U5: provider pinning — profile.provider wins; unknown model with no
    /// provider fails typed at compile.
    #[tokio::test]
    async fn unresolvable_provider_fails_at_compile() {
        let mob_id = MobId::from("compiler-unit");
        let profile_name = ProfileName::from("worker");
        let identity = AgentIdentity::from("b2");
        let mut profile = worker_profile();
        profile.model = "totally-unknown-model-xyz".to_string();
        let definition = definition();
        let base = StaticSpawnBasePromptSource("base".to_string());
        let continuity = super::super::handle::SpawnContinuityIntent::Ephemeral;
        let error = compile_portable_member_spec(params(
            &mob_id,
            &profile_name,
            &identity,
            &profile,
            &definition,
            Some(&base),
            &continuity,
        ))
        .await
        .expect_err("unknown model with no provider must fail at compile");
        assert!(matches!(error, MobError::WiringError(_)), "got {error:?}");
    }

    /// U6 (budget single carrier, ADJ-1 / T-U2): the budget lands in
    /// `overlay.budget_limits` and nowhere else in the serialized spec.
    #[tokio::test]
    async fn budget_lands_in_overlay_only() {
        let mob_id = MobId::from("compiler-unit");
        let profile_name = ProfileName::from("worker");
        let identity = AgentIdentity::from("b2");
        let profile = worker_profile();
        let definition = definition();
        let base = StaticSpawnBasePromptSource("base".to_string());
        let continuity = super::super::handle::SpawnContinuityIntent::Ephemeral;
        let limits = meerkat_core::BudgetLimits {
            max_tokens: None,
            max_duration: None,
            max_tool_calls: Some(3),
        };
        let mut p = params(
            &mob_id,
            &profile_name,
            &identity,
            &profile,
            &definition,
            Some(&base),
            &continuity,
        );
        p.budget_limits = Some(&limits);
        let compiled = compile_portable_member_spec(p).await.expect("compile");
        assert_eq!(compiled.spec.overlay.budget_limits.as_ref(), Some(&limits));
        let rendered = serde_json::to_value(&compiled.spec)
            .expect("spec serializes")
            .to_string();
        assert_eq!(rendered.matches("budget").count(), 1);
    }
}
