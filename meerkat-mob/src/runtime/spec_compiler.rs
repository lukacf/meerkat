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

/// Canonical wire ordering for an unordered tool-name set: the digest-covered
/// spec must hash identically for identical policies.
fn sorted_tool_names(names: &meerkat_core::types::ToolNameSet) -> Vec<String> {
    let mut sorted: Vec<String> = names.iter().map(|name| name.as_str().to_string()).collect();
    sorted.sort();
    sorted
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

    let portable_profile = project_portable_profile(
        profile,
        runtime_mode,
        &definition.models,
        agent_identity.as_str(),
        profile_name.as_str(),
        non_portable_disabled,
    )
    .map_err(MobError::WiringError)?;

    // Skills: Path → Inline read fail-closed (A1); missing name → typed.
    let mut skills = BTreeMap::new();
    for skill_name in &profile.skills {
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
                            "failed to read skill file '{path}' for skill '{skill_name}' while compiling portable member spec: {error}"
                        ))
                    })?;
                    PortableSkillSource::Inline { content }
                }
                #[cfg(target_arch = "wasm32")]
                {
                    return Err(MobError::WiringError(format!(
                        "file-based skill path '{path}' cannot be compiled into a portable member spec on wasm32"
                    )));
                }
            }
        };
        skills.insert(skill_name.clone(), inline);
    }

    // R3 never-Inherit prompt tree (build.rs:209-217 twin, with the case-3
    // seam replacing the local factory `Inherit` resolution).
    let assembled = assemble_inline_skill_prompt(profile, &skills);
    let system_prompt = match system_prompt_override {
        Some(super::handle::SpawnSystemPromptOverride::Replace(prompt)) => {
            PortableSystemPrompt::Set {
                text: prompt.clone(),
            }
        }
        None if !assembled.is_empty() => PortableSystemPrompt::Set { text: assembled },
        None => {
            let Some(base_prompt) = base_prompt else {
                return Err(MobError::WiringError(format!(
                    "remote spawn of '{agent_identity}' needs the controlling host's base system prompt (profile '{profile_name}' has no skills and no prompt override) but no SpawnBasePromptSource is wired; inject one via MobBuilder::with_spawn_base_prompt_source (ADJ-2: never a silent Inherit)"
                )));
            };
            let base = base_prompt.resolve_base_system_prompt().await?;
            if base.is_empty() {
                PortableSystemPrompt::Disable
            } else {
                PortableSystemPrompt::Set { text: base }
            }
        }
    };

    // O3/DEC-P3F-3: resolved policy only. The build.rs Inherit→None fold is
    // NEVER replicated here — residual Inherit is a typed compile failure.
    let tool_access_policy = match tool_access_policy {
        None => None,
        // The core set is unordered; the digest-covered wire carrier is a
        // SORTED name list so identical policies always hash identically.
        Some(meerkat_core::ops::ToolAccessPolicy::AllowList(names)) => Some(
            WireResolvedToolAccessPolicy::AllowList(sorted_tool_names(names)),
        ),
        Some(meerkat_core::ops::ToolAccessPolicy::DenyList(names)) => Some(
            WireResolvedToolAccessPolicy::DenyList(sorted_tool_names(names)),
        ),
        Some(meerkat_core::ops::ToolAccessPolicy::Inherit) => {
            return Err(MobError::WiringError(format!(
                "tool access policy for remote spawn of '{agent_identity}' is still Inherit at spec-mint; agent-facing surfaces must resolve Inherit to the parent's effective policy before the spec reaches the actor (O3)"
            )));
        }
    };

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

    let spec = PortableMemberSpec {
        mob_id: mob_id.as_str().to_string(),
        profile_name: profile_name.as_str().to_string(),
        agent_identity: agent_identity.as_str().to_string(),
        profile: portable_profile,
        definition_extract: PortableDefinitionExtract {
            models: definition.models.clone(),
            image_generation_provider: definition.image_generation_provider,
            skills,
            profile_names: definition
                .profiles
                .keys()
                .map(|profile| profile.as_str().to_string())
                .collect(),
        },
        overlay: PortableSpawnOverlay {
            context: context.map(WireOpaqueJson::from_value),
            labels: labels.cloned(),
            additional_instructions: additional_instructions.cloned(),
            system_prompt,
            tool_access_policy,
            mob_tool_authority_context,
            auth_binding: auth_binding.cloned().map(Into::into),
            budget_limits: budget_limits.cloned(),
            runtime_mode: wire_runtime_mode(runtime_mode),
            continuity_intent: wire_continuity_intent(continuity_intent),
        },
        // v1: names-only future vocabulary (no producer yet).
        required_env_keys: Vec::new(),
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
    profile: &Profile,
    skills: &BTreeMap<String, PortableSkillSource>,
) -> String {
    let mut sections = Vec::new();
    for skill_name in &profile.skills {
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
