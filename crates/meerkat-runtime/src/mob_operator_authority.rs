//! Generated MeerkatMachine authority bridge for mob operator access.
//!
//! Host and mob surfaces use this module to mint or widen
//! `MobToolAuthorityContext`. The context itself is a projection; the
//! capability/admission/provenance facts are changed only by generated
//! MeerkatMachine inputs in this module.

use std::collections::{BTreeMap, BTreeSet};

use meerkat_core::ToolCategoryOverride;
use meerkat_core::service::{
    LiveMobIncarnation, MobIncarnationId, MobToolAuthorityContext, MobToolCallerProvenance,
    OpaquePrincipalToken,
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
        "__meerkat_core_runtime_generated_mob_tool_authority_context_build_v2_",
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
        mob_incarnations: BTreeMap<String, MobIncarnationId>,
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
///
/// The facts name mobs by id only: they are the durable record of the one
/// mob that holds them, and the re-mint binds every scope entry to that
/// mob's live incarnation.
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
/// `holder_mob_id` and `holder_incarnation` name the mob whose store holds
/// the facts, as it runs now (the witness its handle hands out): every scope
/// entry is bound to that incarnation, and facts that scope any other mob
/// are refused, since a record never grants authority over a mob that does
/// not hold it. `caller_provenance`
/// carries the member identity; `audit_invocation_id` carries the upcall
/// `request_id` so operator-action provenance names the member caller and
/// request.
pub fn mint_mob_operator_authority_from_facts(
    facts: &MobOperatorAuthorityFacts,
    holder_mob_id: &str,
    holder_incarnation: LiveMobIncarnation,
    caller_provenance: MobToolCallerProvenance,
    audit_invocation_id: impl Into<String>,
) -> Result<MobToolAuthorityContext, String> {
    if let Some(foreign) = facts
        .managed_mob_scope
        .iter()
        .chain(facts.spawn_profile_scope.keys())
        .find(|mob_id| mob_id.as_str() != holder_mob_id)
    {
        return Err(format!(
            "mob operator authority facts held by mob '{holder_mob_id}' scope another mob '{foreign}'"
        ));
    }
    let mut context = create_only_mob_operator_authority()?;
    context = set_create_authority(&context, facts.can_create_mobs)?;
    context = set_profile_mutation(&context, facts.can_mutate_profiles)?;
    for mob_id in &facts.managed_mob_scope {
        context = grant_manage_mob(&context, mob_id.clone(), holder_incarnation)?;
    }
    for (mob_id, profiles) in &facts.spawn_profile_scope {
        context = grant_spawn_profiles_in_mob(
            &context,
            mob_id.clone(),
            holder_incarnation,
            profiles.iter().cloned(),
        )?;
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

/// Grant manage scope over the mob `mob_id` as it runs at `mob_incarnation`,
/// the witness its handle hands out (`meerkat_mob::MobHandle::live_incarnation`).
/// The grant never reaches a later mob under the same id. A grant for a new
/// incarnation of an id replaces the old binding and drops the spawn-profile
/// scope held for it.
///
/// A bare [`MobIncarnationId`], such as one decoded from input, is not a
/// witness and does not compile here:
///
/// ```compile_fail
/// use meerkat_core::service::{LiveMobIncarnation, MobIncarnationId};
/// use meerkat_runtime::mob_operator_authority::{
///     create_only_mob_operator_authority, grant_manage_mob,
/// };
///
/// # fn main() -> Result<(), String> {
/// let authority = create_only_mob_operator_authority()?;
/// let incarnation = MobIncarnationId::mint();
/// let granted = grant_manage_mob(&authority, "mob-1", incarnation)?;
/// assert!(granted.can_manage_mob("mob-1", incarnation));
/// # Ok(())
/// # }
/// ```
///
/// The same snippet passing the witness compiles and grants (only the mob
/// runtime calls `LiveMobIncarnation::witnessed`; see its docs):
///
/// ```
/// use meerkat_core::service::{LiveMobIncarnation, MobIncarnationId};
/// use meerkat_runtime::mob_operator_authority::{
///     create_only_mob_operator_authority, grant_manage_mob,
/// };
///
/// # fn main() -> Result<(), String> {
/// let authority = create_only_mob_operator_authority()?;
/// let incarnation = MobIncarnationId::mint();
/// let granted = grant_manage_mob(&authority, "mob-1", LiveMobIncarnation::witnessed(incarnation))?;
/// assert!(granted.can_manage_mob("mob-1", incarnation));
/// # Ok(())
/// # }
/// ```
pub fn grant_manage_mob(
    authority_context: &MobToolAuthorityContext,
    mob_id: impl Into<String>,
    mob_incarnation: LiveMobIncarnation,
) -> Result<MobToolAuthorityContext, String> {
    let mut authority = authority_from_context(authority_context)?;
    apply(
        &mut authority,
        dsl::MeerkatMachineInput::GrantMobOperatorManageMob {
            mob_id: mob_id.into(),
            mob_incarnation: mob_incarnation.incarnation(),
        },
    )?;
    context_from_authority_state(authority.state())
}

/// Grant one spawn profile in the mob `mob_id` as it runs at
/// `mob_incarnation`; see [`grant_spawn_profiles_in_mob`].
pub fn grant_spawn_profile_in_mob(
    authority_context: &MobToolAuthorityContext,
    mob_id: impl Into<String>,
    mob_incarnation: LiveMobIncarnation,
    profile: impl Into<String>,
) -> Result<MobToolAuthorityContext, String> {
    grant_spawn_profiles_in_mob(authority_context, mob_id, mob_incarnation, [profile.into()])
}

/// Grant spawn-profile scope in the mob `mob_id` as it runs at
/// `mob_incarnation`, the witness its handle hands out. A grant for a new
/// incarnation of an id replaces the old binding and drops the manage scope
/// held for it.
pub fn grant_spawn_profiles_in_mob<I, S>(
    authority_context: &MobToolAuthorityContext,
    mob_id: impl Into<String>,
    mob_incarnation: LiveMobIncarnation,
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
            mob_incarnation: mob_incarnation.incarnation(),
            profiles: profiles.into_iter().map(Into::into).collect(),
        },
    )?;
    context_from_authority_state(authority.state())
}

/// Narrow the spawn-profile scope `authority_context` holds in the mob
/// `mob_id` to the profiles in `profiles` it already holds there.
///
/// This needs no [`LiveMobIncarnation`] witness, because it can only take
/// rights away. The scope stays bound to the incarnation the context already
/// binds for `mob_id`, so it never reaches another mob under the same id. A
/// profile the context does not already hold there is ignored, never added.
/// Every other fact is kept as it was: create and profile-mutation authority,
/// manage scope (which carries its own rights over the mob), the scope held
/// in other mobs, caller provenance and the audit invocation id. When none of
/// `profiles` is held, the context keeps the binding with no spawn profiles,
/// so it can spawn nothing there.
///
/// A host session hook can use this on the context a member's session is
/// built with, for example the default scope over every profile of its own
/// mob that a `tools.mob` member gets when it has no persisted authority,
/// including a member the mob restores before the host holds its handle.
/// Every mob operator surface of that session admits against the narrowed
/// context: a `MobToolsFactory` surface, and the mob runtime's own mob
/// operator tools for the member (`spawn_member`, `spawn_many_members` and
/// the rest). This covers a member whose session this process builds: a
/// host-materialized member's operator tools are upcalls that the
/// controlling host admits against its recorded authority facts, which a
/// session hook does not edit.
///
/// The narrowing applies to the one session build the hook edits, and so
/// does a disable of the member's mob tools made there. Neither is restored
/// from persisted state: the build with which the mob restores a member
/// reaches the hook with its profile's mob tools setting and the default
/// authority again. A host applies the narrowing (or the disable) on every
/// build, restores included, or a restored member returns to the default
/// authority. When a restored member's settled authority holds operator facts
/// that the authority recorded with its session did not, the build logs one
/// warning naming them. That record is unsealed and advisory, so the warning
/// is a hint, not enforcement; it also shows a profile the mob gained since
/// the member's previous build.
///
/// It refuses a mob the context holds no spawn-profile scope for, and a
/// context that generated authority did not mint.
///
/// ```
/// # use meerkat_core::service::{LiveMobIncarnation, MobIncarnationId};
/// # use meerkat_runtime::mob_operator_authority::{
/// #     create_only_mob_operator_authority, grant_spawn_profiles_in_mob,
/// # };
/// use meerkat_runtime::mob_operator_authority::restrict_spawn_profiles_in_mob;
///
/// # fn main() -> Result<(), String> {
/// # let incarnation = MobIncarnationId::mint();
/// # let granted = grant_spawn_profiles_in_mob(
/// #     &create_only_mob_operator_authority()?,
/// #     "mob-1",
/// #     LiveMobIncarnation::witnessed(incarnation),
/// #     ["lead", "worker"],
/// # )?;
/// // `granted` holds spawn scope over "lead" and "worker" in mob-1.
/// // Keep only "worker"; "auditor" is not held, so it is not added.
/// let narrowed = restrict_spawn_profiles_in_mob(&granted, "mob-1", ["worker", "auditor"])?;
/// assert!(narrowed.spawn_profile_scope_contains("mob-1", incarnation, "worker"));
/// assert!(!narrowed.spawn_profile_scope_contains("mob-1", incarnation, "lead"));
/// assert!(!narrowed.spawn_profile_scope_contains("mob-1", incarnation, "auditor"));
/// assert!(restrict_spawn_profiles_in_mob(&granted, "mob-2", ["worker"]).is_err());
/// # Ok(())
/// # }
/// ```
pub fn restrict_spawn_profiles_in_mob<I, S>(
    authority_context: &MobToolAuthorityContext,
    mob_id: &str,
    profiles: I,
) -> Result<MobToolAuthorityContext, String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let Some(held) = authority_context.spawn_profile_scope().get(mob_id) else {
        return Err(format!(
            "mob operator authority holds no spawn-profile scope in mob '{mob_id}' to restrict"
        ));
    };
    let Some(mob_incarnation) = authority_context.mob_incarnations().get(mob_id).copied() else {
        return Err(format!(
            "mob operator authority binds no incarnation for mob '{mob_id}'"
        ));
    };
    let kept: BTreeSet<String> = profiles
        .into_iter()
        .map(Into::into)
        .filter(|profile| held.contains(profile))
        .collect();
    let mut authority = authority_from_context(authority_context)?;
    // The context's own binding: the machine sets the narrowed scope in
    // place, and never takes its rebind path that drops manage scope.
    apply(
        &mut authority,
        dsl::MeerkatMachineInput::SetMobOperatorSpawnProfilesInMob {
            mob_id: mob_id.to_string(),
            mob_incarnation,
            profiles: kept,
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
            mob_incarnations: authority_context.mob_incarnations().clone(),
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
            state.mob_operator_mob_incarnations.clone(),
            state.mob_operator_caller_provenance.clone(),
            state.mob_operator_audit_invocation_id.clone(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The witness for an incarnation no mob runtime runs: these tests
    /// exercise the grant authority alone.
    #[allow(clippy::disallowed_methods)] // a test helper listed in host_launch_sites.rs
    fn live(incarnation: MobIncarnationId) -> LiveMobIncarnation {
        LiveMobIncarnation::witnessed(incarnation)
    }

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
        let incarnation = MobIncarnationId::mint();
        let authority = grant_manage_mob(&authority, "mob-1", live(incarnation))
            .expect("generated authority should accept manage scope grant");
        let authority = grant_spawn_profile_in_mob(
            &authority,
            "mob-2",
            live(MobIncarnationId::mint()),
            "investigator",
        )
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
        assert!(!projected.can_manage_mob("mob-1", incarnation));

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
        let incarnation = MobIncarnationId::mint();
        let minted = mint_mob_operator_authority_from_facts(
            &facts,
            "mob-1",
            live(incarnation),
            MobToolCallerProvenance::default().with_mob_id("mob-1"),
            "req-42",
        )
        .expect("facts must re-mint through the generated ladder");

        assert!(minted.is_generated_authority_context());
        assert!(!minted.can_create_mobs());
        assert!(minted.can_mutate_profiles());
        assert!(minted.can_manage_mob("mob-1", incarnation));
        assert!(!minted.can_manage_mob("mob-1", MobIncarnationId::mint()));
        assert!(!minted.can_manage_mob("mob-2", incarnation));
        assert!(minted.spawn_profile_scope_contains("mob-1", incarnation, "writer"));
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
            "mob-1",
            live(MobIncarnationId::mint()),
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

        let error = grant_manage_mob(&projected, "mob-1", live(MobIncarnationId::mint()))
            .expect_err("unsealed projection must not drive live authority mutation");

        assert!(
            error.contains("was not minted by generated authority"),
            "unexpected error: {error}"
        );
    }

    fn create_only() -> MobToolAuthorityContext {
        create_only_mob_operator_authority()
            .expect("generated authority should allow explicit create request")
    }

    /// A grant reaches only the incarnation it names: a later mob under the
    /// same id is another incarnation and is not in scope.
    #[test]
    fn grant_binds_scope_to_the_named_incarnation() {
        let first = MobIncarnationId::mint();
        let later = MobIncarnationId::mint();
        let authority = grant_manage_mob(&create_only(), "mob-1", live(first))
            .expect("generated authority should accept manage scope grant");
        let authority = grant_spawn_profile_in_mob(&authority, "mob-2", live(first), "worker")
            .expect("generated authority should accept spawn-profile scope grant");

        assert!(authority.can_manage_mob("mob-1", first));
        assert!(!authority.can_manage_mob("mob-1", later));
        assert!(authority.spawn_profile_scope_contains("mob-2", first, "worker"));
        assert!(!authority.spawn_profile_scope_present("mob-2", later));
        assert_eq!(authority.mob_incarnations().get("mob-1"), Some(&first));

        // The same grant again is idempotent.
        let again = grant_manage_mob(&authority, "mob-1", live(first))
            .expect("a repeated grant for the same incarnation is accepted");
        assert_eq!(again.managed_mob_scope(), authority.managed_mob_scope());
        assert_eq!(again.mob_incarnations(), authority.mob_incarnations());
    }

    /// Manage scope for mob X at I1, then a spawn grant for X at I2: the
    /// spawn grant replaces the binding and the stale manage scope is gone,
    /// for both incarnations.
    #[test]
    fn a_spawn_grant_for_a_new_incarnation_drops_stale_manage_scope() {
        let stale = MobIncarnationId::mint();
        let current = MobIncarnationId::mint();
        let authority = grant_manage_mob(&create_only(), "mob-x", live(stale))
            .expect("generated authority should accept manage scope grant");
        assert!(authority.can_manage_mob("mob-x", stale));

        let authority = grant_spawn_profile_in_mob(&authority, "mob-x", live(current), "worker")
            .expect("a spawn grant for a new incarnation rebinds the mob id");

        assert!(!authority.can_manage_mob("mob-x", current));
        assert!(!authority.can_manage_mob("mob-x", stale));
        assert!(!authority.managed_mob_scope().contains("mob-x"));
        assert!(authority.spawn_profile_scope_contains("mob-x", current, "worker"));
        assert!(!authority.spawn_profile_scope_present("mob-x", stale));
        assert_eq!(authority.mob_incarnations().get("mob-x"), Some(&current));
    }

    /// Spawn scope for mob X at I1, then a manage grant for X at I2: the
    /// stale spawn-profile scope does not carry over to the new mob.
    #[test]
    fn a_manage_grant_for_a_new_incarnation_drops_stale_spawn_scope() {
        let stale = MobIncarnationId::mint();
        let current = MobIncarnationId::mint();
        let authority = grant_spawn_profile_in_mob(&create_only(), "mob-x", live(stale), "worker")
            .expect("generated authority should accept spawn-profile scope grant");

        let authority = grant_manage_mob(&authority, "mob-x", live(current))
            .expect("a manage grant for a new incarnation rebinds the mob id");

        assert!(authority.can_manage_mob("mob-x", current));
        assert!(!authority.spawn_profile_scope_present("mob-x", current));
        assert!(!authority.spawn_profile_scope_contains("mob-x", current, "worker"));
        assert!(authority.spawn_profile_scope().get("mob-x").is_none());
    }

    /// Re-minting provenance or the audit id keeps every binding.
    #[test]
    fn provenance_and_audit_re_mints_keep_incarnation_bindings() {
        let incarnation = MobIncarnationId::mint();
        let authority = grant_manage_mob(&create_only(), "mob-1", live(incarnation))
            .expect("generated authority should accept manage scope grant");
        let authority = with_caller_provenance(
            &authority,
            MobToolCallerProvenance::default().with_mob_id("mob-1"),
        )
        .expect("generated authority should accept caller provenance");
        let authority = with_audit_invocation_id(&authority, "audit-1")
            .expect("generated authority should accept audit invocation id");
        let restored =
            restore_mob_operator_authority(&authority).expect("a sealed context restores");

        assert!(restored.can_manage_mob("mob-1", incarnation));
        assert_eq!(restored.mob_incarnations(), authority.mob_incarnations());
    }

    /// The generated restore refuses a scope that names a mob without an
    /// incarnation binding.
    #[test]
    fn generated_restore_refuses_an_unbound_scope() {
        let mut authority = fresh_authority();
        let error = apply(
            &mut authority,
            dsl::MeerkatMachineInput::RestoreMobOperatorAuthority {
                principal_token: OpaquePrincipalToken::generated(),
                can_create_mobs: true,
                can_mutate_profiles: true,
                managed_mob_scope: BTreeSet::from(["mob-1".to_string()]),
                spawn_profile_scope: BTreeMap::new(),
                mob_incarnations: BTreeMap::new(),
                caller_provenance: None,
                audit_invocation_id: None,
            },
        )
        .expect_err("a restore whose scope names an unbound mob must be refused");
        assert!(error.contains("rejected"), "{error}");
        assert!(!authority.state().mob_operator_authority_present);
    }

    /// Facts are the record of the mob that holds them: facts that scope any
    /// other mob are refused instead of re-minted.
    #[test]
    fn facts_naming_another_mob_are_refused() {
        for facts in [
            MobOperatorAuthorityFacts {
                managed_mob_scope: BTreeSet::from(["other-mob".to_string()]),
                ..MobOperatorAuthorityFacts::default()
            },
            MobOperatorAuthorityFacts {
                spawn_profile_scope: BTreeMap::from([(
                    "other-mob".to_string(),
                    BTreeSet::from(["worker".to_string()]),
                )]),
                ..MobOperatorAuthorityFacts::default()
            },
        ] {
            let error = mint_mob_operator_authority_from_facts(
                &facts,
                "holder-mob",
                live(MobIncarnationId::mint()),
                MobToolCallerProvenance::default(),
                "req-1",
            )
            .expect_err("facts scoping another mob must be refused");
            assert!(error.contains("other-mob"), "{error}");
        }
    }

    /// A context with spawn scope over three profiles of mob-1, and manage
    /// plus spawn scope over mob-2, each bound to its own incarnation, with
    /// caller provenance and an audit invocation id.
    fn spawn_and_manage_context() -> (MobToolAuthorityContext, MobIncarnationId, MobIncarnationId) {
        let first = MobIncarnationId::mint();
        let second = MobIncarnationId::mint();
        let context = create_only_mob_operator_authority().expect("create-only authority");
        let context = grant_spawn_profiles_in_mob(
            &context,
            "mob-1",
            live(first),
            ["lead", "worker", "auditor"],
        )
        .expect("spawn scope in mob-1");
        let context = grant_manage_mob(&context, "mob-2", live(second)).expect("manage mob-2");
        let context = grant_spawn_profile_in_mob(&context, "mob-2", live(second), "scout")
            .expect("spawn scope in mob-2");
        let context = with_caller_provenance(
            &context,
            MobToolCallerProvenance::default().with_mob_id("mob-1"),
        )
        .expect("caller provenance");
        let context = with_audit_invocation_id(&context, "audit-7").expect("audit invocation id");
        (context, first, second)
    }

    /// Restricting keeps the incarnation binding and every other fact, and
    /// drops the profiles left out.
    #[test]
    fn restricting_spawn_profiles_keeps_the_binding_and_drops_the_rest() {
        let (context, first, second) = spawn_and_manage_context();
        let narrowed = restrict_spawn_profiles_in_mob(&context, "mob-1", ["worker", "auditor"])
            .expect("narrow mob-1");

        assert!(narrowed.is_generated_authority_context());
        assert_eq!(
            narrowed.spawn_profile_scope()["mob-1"],
            BTreeSet::from(["worker".to_string(), "auditor".to_string()])
        );
        assert!(narrowed.spawn_profile_scope_contains("mob-1", first, "worker"));
        assert!(!narrowed.spawn_profile_scope_contains("mob-1", first, "lead"));
        assert!(!narrowed.spawn_profile_scope_present("mob-1", MobIncarnationId::mint()));
        assert_eq!(narrowed.mob_incarnations(), context.mob_incarnations());
        assert_eq!(
            narrowed.spawn_profile_scope()["mob-2"],
            context.spawn_profile_scope()["mob-2"]
        );
        assert!(narrowed.can_manage_mob("mob-2", second));
        assert_eq!(narrowed.managed_mob_scope(), context.managed_mob_scope());
        assert_eq!(narrowed.can_create_mobs(), context.can_create_mobs());
        assert_eq!(
            narrowed.can_mutate_profiles(),
            context.can_mutate_profiles()
        );
        assert_eq!(narrowed.caller_provenance(), context.caller_provenance());
        assert_eq!(narrowed.audit_invocation_id(), Some("audit-7"));
    }

    /// A profile the context does not hold in the mob is never added.
    #[test]
    fn restricting_spawn_profiles_never_adds_a_profile() {
        let (context, first, _) = spawn_and_manage_context();
        let narrowed = restrict_spawn_profiles_in_mob(&context, "mob-1", ["worker", "admin"])
            .expect("narrow mob-1");
        assert_eq!(
            narrowed.spawn_profile_scope()["mob-1"],
            BTreeSet::from(["worker".to_string()])
        );
        assert!(!narrowed.spawn_profile_scope_contains("mob-1", first, "admin"));
        // Narrowing what is already narrow changes nothing.
        let again = restrict_spawn_profiles_in_mob(&narrowed, "mob-1", ["lead", "worker"])
            .expect("narrow mob-1 again");
        assert_eq!(again.spawn_profile_scope(), narrowed.spawn_profile_scope());
    }

    /// A mob the context holds no spawn-profile scope for is refused, and the
    /// context is left as it was.
    #[test]
    fn restricting_spawn_profiles_in_an_unscoped_mob_is_refused() {
        let (context, _, _) = spawn_and_manage_context();
        let error = restrict_spawn_profiles_in_mob(&context, "mob-9", ["worker"])
            .expect_err("an unbound mob is refused");
        assert!(error.contains("mob-9"), "{error}");
        let manage_only = grant_manage_mob(
            &create_only_mob_operator_authority().expect("create-only authority"),
            "mob-3",
            live(MobIncarnationId::mint()),
        )
        .expect("manage mob-3");
        let error = restrict_spawn_profiles_in_mob(&manage_only, "mob-3", ["worker"])
            .expect_err("a mob with manage scope only holds no spawn scope to restrict");
        assert!(error.contains("mob-3"), "{error}");
    }

    /// A persisted projection of a context that held spawn scope in the mob
    /// is not generated authority, and is refused.
    #[test]
    fn restricting_spawn_profiles_in_a_projection_context_is_refused() {
        let (context, _, _) = spawn_and_manage_context();
        let projected = persisted_projection(context);
        assert!(!projected.is_generated_authority_context());
        assert!(
            restrict_spawn_profiles_in_mob(&projected, "mob-1", ["worker"]).is_err(),
            "a projection is refused"
        );
    }

    /// Keeping none of the held profiles leaves the mob bound with no spawn
    /// rights, and manage scope over a mob keeps its own rights.
    #[test]
    fn restricting_to_no_held_profile_leaves_no_spawn_rights() {
        let (context, first, second) = spawn_and_manage_context();
        let narrowed = restrict_spawn_profiles_in_mob(&context, "mob-1", Vec::<String>::new())
            .expect("narrow mob-1 to nothing");
        assert!(!narrowed.spawn_profile_scope_present("mob-1", first));
        assert_eq!(narrowed.mob_incarnations()["mob-1"], first);

        let narrowed = restrict_spawn_profiles_in_mob(&narrowed, "mob-2", ["other"])
            .expect("narrow mob-2 to nothing");
        assert!(!narrowed.spawn_profile_scope_present("mob-2", second));
        assert!(narrowed.can_manage_mob("mob-2", second));
    }
}
