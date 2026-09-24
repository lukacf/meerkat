//! Generated MeerkatMachine authority bridge for mob operator access.
//!
//! Host and mob surfaces use this module to mint or widen
//! `MobToolAuthorityContext`. The context itself is a projection; the
//! capability/admission/provenance facts are changed only by generated
//! MeerkatMachine inputs in this module.

use std::collections::{BTreeMap, BTreeSet};

use meerkat_core::ToolCategoryOverride;
use meerkat_core::service::{
    MobToolAuthorityContext, MobToolCallerProvenance, OpaquePrincipalToken,
};

use crate::meerkat_machine::dsl;

struct GeneratedAuthorityBridgeToken;

static GENERATED_AUTHORITY_BRIDGE_TOKEN: GeneratedAuthorityBridgeToken =
    GeneratedAuthorityBridgeToken;

fn generated_authority_bridge_token() -> &'static (dyn std::any::Any + Send + Sync) {
    &GENERATED_AUTHORITY_BRIDGE_TOKEN
}

#[doc(hidden)]
#[allow(improper_ctypes_definitions, unsafe_code)]
#[unsafe(export_name = concat!(
    "__meerkat_runtime_generated_authority_bridge_token_is_valid_v1_mob_operator_authority_",
    env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")
))]
pub extern "Rust" fn generated_authority_bridge_token_is_valid(
    token: &(dyn std::any::Any + Send + Sync),
) -> bool {
    token.is::<GeneratedAuthorityBridgeToken>()
}

#[allow(improper_ctypes_definitions, unsafe_code)]
unsafe extern "Rust" {
    #[link_name = concat!(
        "__meerkat_core_runtime_generated_mob_tool_authority_context_build_v1_",
        env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")
    )]
    fn core_generated_mob_tool_authority_context_build(
        token: &'static (dyn std::any::Any + Send + Sync),
        principal_token: OpaquePrincipalToken,
        can_create_mobs: bool,
        can_mutate_profiles: bool,
        can_run_adaptive_packs: bool,
        managed_mob_scope: BTreeSet<String>,
        spawn_profile_scope: BTreeMap<String, BTreeSet<String>>,
        caller_provenance: Option<MobToolCallerProvenance>,
        audit_invocation_id: Option<String>,
    ) -> Result<MobToolAuthorityContext, String>;
}

pub fn resolve_mob_operator_access(
    enable_mob: ToolCategoryOverride,
    persisted_authority_context: Option<MobToolAuthorityContext>,
) -> (ToolCategoryOverride, Option<MobToolAuthorityContext>) {
    if matches!(enable_mob, ToolCategoryOverride::Disable) {
        return (ToolCategoryOverride::Disable, None);
    }

    if let Some(authority_context) = persisted_authority_context {
        return match restore_mob_operator_authority(&authority_context) {
            Ok(authority_context) => (ToolCategoryOverride::Enable, Some(authority_context)),
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "generated mob operator authority rejected persisted context"
                );
                (ToolCategoryOverride::Disable, None)
            }
        };
    }

    match resolve_requested_mob_operator_authority(enable_mob) {
        Ok(Some(authority_context)) => (ToolCategoryOverride::Enable, Some(authority_context)),
        Ok(None) => (enable_mob, None),
        Err(error) => {
            tracing::warn!(
                error = %error,
                "generated mob operator authority rejected access request"
            );
            (ToolCategoryOverride::Disable, None)
        }
    }
}

pub fn create_only_mob_operator_authority() -> Result<MobToolAuthorityContext, String> {
    resolve_requested_mob_operator_authority(ToolCategoryOverride::Enable)?
        .ok_or_else(|| "generated mob operator authority omitted explicit enablement".to_string())
}

fn resolve_requested_mob_operator_authority(
    enable_mob: ToolCategoryOverride,
) -> Result<Option<MobToolAuthorityContext>, String> {
    let request_kind = match enable_mob {
        ToolCategoryOverride::Enable => dsl::MobOperatorAccessRequestKind::Enable,
        ToolCategoryOverride::Disable => dsl::MobOperatorAccessRequestKind::Disable,
        ToolCategoryOverride::Inherit => dsl::MobOperatorAccessRequestKind::Inherit,
    };
    let mut authority = fresh_authority();
    apply(
        &mut authority,
        dsl::MeerkatMachineInput::ResolveMobOperatorCreateAuthority {
            request_kind,
            principal_token: OpaquePrincipalToken::generated(),
            caller_provenance: None,
            audit_invocation_id: None,
        },
    )?;
    if authority.state().mob_operator_authority_present {
        context_from_authority_state(authority.state()).map(Some)
    } else {
        Ok(None)
    }
}

/// Atomic capability FACTS for a mob member's operator authority (ADJ-15).
///
/// This is the read-side twin of the durable
/// `MobMemberOperatorAuthorityRecord`: the record stores facts, never a
/// context — a persisted `MobToolAuthorityContext` can NEVER be rehydrated
/// into dispatch authority (serde drops the seal and
/// [`restore_mob_operator_authority`] rejects unsealed contexts). Every read
/// re-mints a sealed context from these facts through the generated ladder
/// via [`mint_mob_operator_authority_from_facts`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MobOperatorAuthorityFacts {
    pub can_create_mobs: bool,
    pub can_mutate_profiles: bool,
    pub managed_mob_scope: BTreeSet<String>,
    pub spawn_profile_scope: BTreeMap<String, BTreeSet<String>>,
}

/// Re-mint a sealed `MobToolAuthorityContext` from capability facts through
/// the generated ladder (ADJ-15): `create_only_mob_operator_authority` →
/// `set_create_authority` → `set_profile_mutation` → `grant_manage_mob` per
/// managed scope → `grant_spawn_profiles_in_mob` per entry →
/// `with_caller_provenance` → `with_audit_invocation_id`. Every step is a
/// generated MeerkatMachine input, so the output satisfies
/// `is_generated_authority_context()` — no stored-context rehydration exists
/// on this path.
///
/// `caller_provenance` carries the member identity; `audit_invocation_id`
/// carries the upcall `request_id` so operator-action provenance names the
/// member caller and request.
pub fn mint_mob_operator_authority_from_facts(
    facts: &MobOperatorAuthorityFacts,
    caller_provenance: MobToolCallerProvenance,
    audit_invocation_id: impl Into<String>,
) -> Result<MobToolAuthorityContext, String> {
    let mut context = create_only_mob_operator_authority()?;
    context = set_create_authority(&context, facts.can_create_mobs)?;
    context = set_profile_mutation(&context, facts.can_mutate_profiles)?;
    for mob_id in &facts.managed_mob_scope {
        context = grant_manage_mob(&context, mob_id.clone())?;
    }
    for (mob_id, profiles) in &facts.spawn_profile_scope {
        context = grant_spawn_profiles_in_mob(&context, mob_id.clone(), profiles.iter().cloned())?;
    }
    context = with_caller_provenance(&context, caller_provenance)?;
    with_audit_invocation_id(&context, audit_invocation_id)
}

pub fn restore_mob_operator_authority(
    authority_context: &MobToolAuthorityContext,
) -> Result<MobToolAuthorityContext, String> {
    let mut authority = fresh_authority();
    apply_restore(&mut authority, authority_context, None, None)?;
    context_from_authority_state(authority.state())
}

pub fn set_create_authority(
    authority_context: &MobToolAuthorityContext,
    allowed: bool,
) -> Result<MobToolAuthorityContext, String> {
    let mut authority = authority_from_context(authority_context)?;
    apply(
        &mut authority,
        dsl::MeerkatMachineInput::SetMobOperatorCreateAuthority { allowed },
    )?;
    context_from_authority_state(authority.state())
}

pub fn set_profile_mutation(
    authority_context: &MobToolAuthorityContext,
    allowed: bool,
) -> Result<MobToolAuthorityContext, String> {
    let mut authority = authority_from_context(authority_context)?;
    apply(
        &mut authority,
        dsl::MeerkatMachineInput::SetMobOperatorProfileMutation { allowed },
    )?;
    context_from_authority_state(authority.state())
}

pub fn grant_manage_mob(
    authority_context: &MobToolAuthorityContext,
    mob_id: impl Into<String>,
) -> Result<MobToolAuthorityContext, String> {
    let mut authority = authority_from_context(authority_context)?;
    apply(
        &mut authority,
        dsl::MeerkatMachineInput::GrantMobOperatorManageMob {
            mob_id: mob_id.into(),
        },
    )?;
    context_from_authority_state(authority.state())
}

pub fn grant_spawn_profile_in_mob(
    authority_context: &MobToolAuthorityContext,
    mob_id: impl Into<String>,
    profile: impl Into<String>,
) -> Result<MobToolAuthorityContext, String> {
    grant_spawn_profiles_in_mob(authority_context, mob_id, [profile.into()])
}

pub fn grant_spawn_profiles_in_mob<I, S>(
    authority_context: &MobToolAuthorityContext,
    mob_id: impl Into<String>,
    profiles: I,
) -> Result<MobToolAuthorityContext, String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut authority = authority_from_context(authority_context)?;
    apply(
        &mut authority,
        dsl::MeerkatMachineInput::SetMobOperatorSpawnProfilesInMob {
            mob_id: mob_id.into(),
            profiles: profiles.into_iter().map(Into::into).collect(),
        },
    )?;
    context_from_authority_state(authority.state())
}

pub fn with_caller_provenance(
    authority_context: &MobToolAuthorityContext,
    caller_provenance: MobToolCallerProvenance,
) -> Result<MobToolAuthorityContext, String> {
    let mut authority = fresh_authority();
    apply_restore(
        &mut authority,
        authority_context,
        Some(Some(caller_provenance)),
        None,
    )?;
    context_from_authority_state(authority.state())
}

pub fn with_audit_invocation_id(
    authority_context: &MobToolAuthorityContext,
    audit_invocation_id: impl Into<String>,
) -> Result<MobToolAuthorityContext, String> {
    let mut authority = fresh_authority();
    apply_restore(
        &mut authority,
        authority_context,
        None,
        Some(Some(audit_invocation_id.into())),
    )?;
    context_from_authority_state(authority.state())
}

fn fresh_authority() -> dsl::MeerkatMachineAuthority {
    dsl::MeerkatMachineAuthority::new()
}

fn authority_from_context(
    authority_context: &MobToolAuthorityContext,
) -> Result<dsl::MeerkatMachineAuthority, String> {
    let mut authority = fresh_authority();
    apply_restore(&mut authority, authority_context, None, None)?;
    Ok(authority)
}

#[allow(clippy::option_option)]
fn apply_restore(
    authority: &mut dsl::MeerkatMachineAuthority,
    authority_context: &MobToolAuthorityContext,
    caller_provenance: Option<Option<MobToolCallerProvenance>>,
    audit_invocation_id: Option<Option<String>>,
) -> Result<(), String> {
    if !authority_context.is_generated_authority_context() {
        return Err("mob operator authority context was not minted by generated authority".into());
    }
    apply(
        authority,
        dsl::MeerkatMachineInput::RestoreMobOperatorAuthority {
            principal_token: authority_context.principal_token().clone(),
            can_create_mobs: authority_context.can_create_mobs(),
            can_mutate_profiles: authority_context.can_mutate_profiles(),
            managed_mob_scope: authority_context.managed_mob_scope().clone(),
            spawn_profile_scope: authority_context.spawn_profile_scope().clone(),
            caller_provenance: caller_provenance
                .unwrap_or_else(|| authority_context.caller_provenance().cloned()),
            audit_invocation_id: audit_invocation_id
                .unwrap_or_else(|| authority_context.audit_invocation_id().map(str::to_owned)),
        },
    )
}

fn apply(
    authority: &mut dsl::MeerkatMachineAuthority,
    input: dsl::MeerkatMachineInput,
) -> Result<(), String> {
    dsl::MeerkatMachineMutator::apply(authority, input)
        .map(|_| ())
        .map_err(|error| format!("MeerkatMachine rejected mob operator authority input: {error}"))
}

fn context_from_authority_state(
    state: &dsl::MeerkatMachineState,
) -> Result<MobToolAuthorityContext, String> {
    if !state.mob_operator_authority_present {
        return Err("generated mob operator authority is absent".into());
    }
    let principal_token = state
        .mob_operator_principal_token
        .clone()
        .ok_or_else(|| "generated mob operator authority omitted principal token".to_string())?;
    #[allow(unsafe_code)]
    unsafe {
        core_generated_mob_tool_authority_context_build(
            generated_authority_bridge_token(),
            principal_token,
            state.mob_operator_can_create_mobs,
            state.mob_operator_can_mutate_profiles,
            false,
            state.mob_operator_managed_mob_scope.clone(),
            state.mob_operator_spawn_profile_scope.clone(),
            state.mob_operator_caller_provenance.clone(),
            state.mob_operator_audit_invocation_id.clone(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn persisted_projection(authority_context: MobToolAuthorityContext) -> MobToolAuthorityContext {
        serde_json::from_value(
            serde_json::to_value(authority_context)
                .expect("generated mob authority context must serialize"),
        )
        .expect("persisted mob authority projection must deserialize")
    }

    #[test]
    fn persisted_mob_authority_projection_does_not_restore_behavior_authority() {
        let authority = create_only_mob_operator_authority()
            .expect("generated authority should allow explicit create request");
        let authority = grant_manage_mob(&authority, "mob-1")
            .expect("generated authority should accept manage scope grant");
        let authority = grant_spawn_profile_in_mob(&authority, "mob-2", "investigator")
            .expect("generated authority should accept spawn-profile scope grant");
        let authority = with_caller_provenance(
            &authority,
            MobToolCallerProvenance::default().with_mob_id("mob-1"),
        )
        .expect("generated authority should accept caller provenance");
        let authority = with_audit_invocation_id(&authority, "audit-1")
            .expect("generated authority should accept audit invocation id");

        let projected = persisted_projection(authority);
        assert!(!projected.is_generated_authority_context());
        assert!(!projected.can_create_mobs());
        assert!(!projected.can_manage_mob("mob-1"));

        let (override_mob, restored) =
            resolve_mob_operator_access(ToolCategoryOverride::Enable, Some(projected));

        assert_eq!(override_mob, ToolCategoryOverride::Disable);
        assert!(restored.is_none());
    }

    /// T-10 (upcall lane): facts → sealed context through the generated
    /// ladder. The minted context is generated-sealed, carries exactly the
    /// facts, and stamps the member caller provenance + upcall request id.
    #[test]
    fn mint_from_facts_produces_sealed_context_with_exact_capabilities() {
        let facts = MobOperatorAuthorityFacts {
            can_create_mobs: false,
            can_mutate_profiles: true,
            managed_mob_scope: BTreeSet::from(["mob-1".to_string()]),
            spawn_profile_scope: BTreeMap::from([(
                "mob-1".to_string(),
                BTreeSet::from(["investigator".to_string(), "writer".to_string()]),
            )]),
        };
        let minted = mint_mob_operator_authority_from_facts(
            &facts,
            MobToolCallerProvenance::default().with_mob_id("mob-1"),
            "req-42",
        )
        .expect("facts must re-mint through the generated ladder");

        assert!(minted.is_generated_authority_context());
        assert!(!minted.can_create_mobs());
        assert!(minted.can_mutate_profiles());
        assert!(minted.can_manage_mob("mob-1"));
        assert!(!minted.can_manage_mob("mob-2"));
        assert_eq!(minted.managed_mob_scope(), &facts.managed_mob_scope);
        assert_eq!(minted.spawn_profile_scope(), &facts.spawn_profile_scope);
        assert_eq!(minted.audit_invocation_id(), Some("req-42"));
        assert!(minted.caller_provenance().is_some());
    }

    /// The one-fact-one-owner pin behind ADJ-15: the persisted PROJECTION of
    /// a facts-minted context still cannot restore behavior authority — only
    /// the facts-path re-mint can.
    #[test]
    fn minted_facts_context_projection_still_does_not_restore_authority() {
        let facts = MobOperatorAuthorityFacts {
            can_create_mobs: true,
            ..MobOperatorAuthorityFacts::default()
        };
        let minted = mint_mob_operator_authority_from_facts(
            &facts,
            MobToolCallerProvenance::default(),
            "req-1",
        )
        .expect("facts must re-mint through the generated ladder");
        let projected = persisted_projection(minted);
        assert!(!projected.is_generated_authority_context());
        let error = restore_mob_operator_authority(&projected)
            .expect_err("an unsealed projection must never restore authority");
        assert!(error.contains("was not minted by generated authority"));
    }

    #[test]
    fn persisted_mob_authority_projection_cannot_drive_live_mutation() {
        let authority = create_only_mob_operator_authority()
            .expect("generated authority should allow explicit create request");
        let projected = persisted_projection(authority);

        let error = grant_manage_mob(&projected, "mob-1")
            .expect_err("unsealed projection must not drive live authority mutation");

        assert!(
            error.contains("was not minted by generated authority"),
            "unexpected error: {error}"
        );
    }
}
