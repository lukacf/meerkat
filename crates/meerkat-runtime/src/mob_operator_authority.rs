//! Generated MeerkatMachine authority bridge for mob operator access.
//!
//! Host and mob surfaces use this module to mint or widen
//! `MobToolAuthorityContext`. The context itself is a projection; the
//! capability/admission/provenance facts are changed only by generated
//! MeerkatMachine inputs in this module. The identity creation check of an
//! agent creator's commit ([`admit_identity_creation_from_template`]) is a
//! generated verdict over the same facts.

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
        "__meerkat_core_runtime_generated_mob_tool_authority_context_build_v3_",
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
        identity_creation_scope: BTreeMap<String, BTreeSet<String>>,
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
    /// The identity creation scope: per mob id, the templates the member
    /// may create identities from (see [`grant_identity_creation_in_mob`]).
    pub identity_creation_scope: BTreeMap<String, BTreeSet<String>>,
}

/// Re-mint a sealed `MobToolAuthorityContext` from capability facts through
/// the generated ladder (ADJ-15): `create_only_mob_operator_authority` →
/// `set_create_authority` → `set_profile_mutation` → `grant_manage_mob` per
/// managed scope → `grant_spawn_profiles_in_mob` per entry →
/// `grant_identity_creation_in_mob` per entry → `with_caller_provenance` →
/// `with_audit_invocation_id`. Every step is a generated MeerkatMachine
/// input, so the output satisfies `is_generated_authority_context()` — no
/// stored-context rehydration exists on this path.
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
        .chain(facts.identity_creation_scope.keys())
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
    for (mob_id, templates) in &facts.identity_creation_scope {
        context = grant_identity_creation_in_mob(
            &context,
            mob_id.clone(),
            holder_incarnation,
            templates.iter().cloned(),
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

/// Grant identity creation scope in the mob `mob_id` as it runs at
/// `mob_incarnation`, the witness its handle hands out: the templates an
/// agent creator holding this authority may create identities from in that
/// mob. A host mints it at the creator's build from its creation policy.
///
/// A grant at the bound incarnation replaces the templates held there, so a
/// host whose policy changed mints the scope again with the new templates;
/// an empty set keeps the binding and allows no template. A grant for a new
/// incarnation of an id replaces the old binding and drops the manage and
/// spawn-profile scope held for it, as a manage or spawn-profile grant for a
/// new incarnation drops the creation scope held for the old one.
///
/// The scope is part of the generated seal: a persisted projection of the
/// context never carries it, so a restored member holds it only when its
/// build mints it again. Manage, spawn-profile, create and profile-mutation
/// authority never admit a creation; the creation commit asks
/// [`admit_identity_creation_from_template`].
pub fn grant_identity_creation_in_mob<I, S>(
    authority_context: &MobToolAuthorityContext,
    mob_id: impl Into<String>,
    mob_incarnation: LiveMobIncarnation,
    templates: I,
) -> Result<MobToolAuthorityContext, String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut authority = authority_from_context(authority_context)?;
    apply(
        &mut authority,
        dsl::MeerkatMachineInput::SetMobOperatorIdentityCreationInMob {
            mob_id: mob_id.into(),
            mob_incarnation: mob_incarnation.incarnation(),
            templates: templates.into_iter().map(Into::into).collect(),
        },
    )?;
    context_from_authority_state(authority.state())
}

/// A creation-only mob operator authority: the identity creation scope for
/// `templates` in the mob `mob_id` as it runs at `mob_incarnation`, and
/// nothing else. It holds no manage or spawn-profile scope, and its create
/// and profile-mutation authority are off. A host mints it at the build of
/// an identity creator whose profile disables mob tools, with the witness
/// the build carries (`SessionBuildOptions::mob_incarnation`), and binds it
/// to the creator's session without mounting the mob tools (the build's
/// `mob_operator_authority_only`, which a build with mob tools refuses: a
/// creator with mob tools gets the scope granted into its full authority by
/// [`grant_identity_creation_in_mob`]). Every step is a generated MeerkatMachine
/// input: `create_only_mob_operator_authority` -> `set_create_authority`
/// (off) -> `set_profile_mutation` (off) ->
/// [`grant_identity_creation_in_mob`].
pub fn identity_creation_only_mob_operator_authority<I, S>(
    mob_id: impl Into<String>,
    mob_incarnation: LiveMobIncarnation,
    templates: I,
) -> Result<MobToolAuthorityContext, String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let context = create_only_mob_operator_authority()?;
    let context = set_create_authority(&context, false)?;
    let context = set_profile_mutation(&context, false)?;
    grant_identity_creation_in_mob(&context, mob_id, mob_incarnation, templates)
}

/// The generated refusal of an agent creator's identity creation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum IdentityCreationScopeRefusal {
    /// The creator's authority holds no identity creation scope for the
    /// template in the mob at the incarnation the creation acts on: no
    /// authority, a persisted projection, scope held for another incarnation
    /// of the mob, and manage or spawn-profile scope alone all end here.
    #[error(
        "the creator's mob operator authority does not allow identity creation from this \
         template in this mob"
    )]
    TemplateNotAllowed,
}

/// The identity creation check of an agent creator's commit: the generated
/// MeerkatMachine verdict on whether `creator_authority`, the sealed
/// authority of the creator's session, may create identities from
/// `template` in the mob `mob_id` as it runs at `mob_incarnation`.
///
/// Pass the creator session's live authority: the sealed context its build
/// settled and bound to the session's tools, whose identity creation scope
/// that build minted. Never pass a context decoded from a record (a session's
/// stored build state, a wire projection): a decoded context is unsealed and
/// carries no scope, so it is refused like `None` (the session holds no
/// generated authority). A fresh machine restores the sealed authority
/// through the generated ladder and answers
/// `ResolveMobOperatorIdentityCreation` from its own state; this function
/// mirrors the verdict. `mob_incarnation` is the incarnation of the live mob
/// the commit acts on (`meerkat_mob::MobHandle::incarnation`), never a value
/// decoded from input: scope held for another incarnation of the mob admits
/// nothing.
///
/// Only the identity creation scope admits a creation, so an agent holding
/// every other mob operator right, or a creation tool mounted by mistake,
/// is still refused. A commit with several identities asks once per
/// identity's template, before it writes anything.
pub fn admit_identity_creation_from_template(
    creator_authority: Option<&MobToolAuthorityContext>,
    mob_id: &str,
    mob_incarnation: MobIncarnationId,
    template: &str,
) -> Result<(), IdentityCreationScopeRefusal> {
    let mut authority = fresh_authority();
    if let Some(sealed) =
        creator_authority.filter(|context| context.is_generated_authority_context())
        && let Err(error) = apply_restore(&mut authority, sealed, None, None)
    {
        tracing::warn!(
            error = %error,
            "generated mob operator authority rejected a creator's sealed context; \
             identity creation refused"
        );
        return Err(IdentityCreationScopeRefusal::TemplateNotAllowed);
    }
    let verdict = match dsl::MeerkatMachineMutator::apply(
        &mut authority,
        dsl::MeerkatMachineInput::ResolveMobOperatorIdentityCreation {
            mob_id: mob_id.to_string(),
            mob_incarnation,
            template: template.to_string(),
        },
    ) {
        Ok(transition) => transition.effects().iter().find_map(|effect| match effect {
            dsl::MeerkatMachineEffect::MobOperatorIdentityCreationResolved { verdict } => {
                Some(*verdict)
            }
            _ => None,
        }),
        Err(error) => {
            tracing::warn!(
                error = %error,
                "generated mob operator authority rejected the identity creation check; \
                 identity creation refused"
            );
            None
        }
    };
    match verdict {
        Some(dsl::MobOperatorIdentityCreationVerdict::Admitted) => Ok(()),
        Some(dsl::MobOperatorIdentityCreationVerdict::TemplateNotAllowed) | None => {
            Err(IdentityCreationScopeRefusal::TemplateNotAllowed)
        }
    }
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
            identity_creation_scope: authority_context.identity_creation_scope().clone(),
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
            state.mob_operator_identity_creation_scope.clone(),
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
            identity_creation_scope: BTreeMap::from([(
                "mob-1".to_string(),
                BTreeSet::from(["template-a".to_string()]),
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
        assert_eq!(
            minted.identity_creation_scope(),
            &facts.identity_creation_scope
        );
        assert_eq!(
            admit_identity_creation_from_template(
                Some(&minted),
                "mob-1",
                incarnation,
                "template-a"
            ),
            Ok(())
        );
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
                identity_creation_scope: BTreeMap::new(),
                mob_incarnations: BTreeMap::new(),
                caller_provenance: None,
                audit_invocation_id: None,
            },
        )
        .expect_err("a restore whose scope names an unbound mob must be refused");
        assert!(error.contains("rejected"), "{error}");
        assert!(!authority.state().mob_operator_authority_present);

        let error = apply(
            &mut authority,
            dsl::MeerkatMachineInput::RestoreMobOperatorAuthority {
                principal_token: OpaquePrincipalToken::generated(),
                can_create_mobs: false,
                can_mutate_profiles: false,
                managed_mob_scope: BTreeSet::new(),
                spawn_profile_scope: BTreeMap::new(),
                identity_creation_scope: BTreeMap::from([(
                    "mob-1".to_string(),
                    BTreeSet::from(["template-a".to_string()]),
                )]),
                mob_incarnations: BTreeMap::new(),
                caller_provenance: None,
                audit_invocation_id: None,
            },
        )
        .expect_err("a restore whose identity creation scope names an unbound mob is refused");
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
            MobOperatorAuthorityFacts {
                identity_creation_scope: BTreeMap::from([(
                    "other-mob".to_string(),
                    BTreeSet::from(["template-a".to_string()]),
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

    const CREATION_REFUSED: Result<(), IdentityCreationScopeRefusal> =
        Err(IdentityCreationScopeRefusal::TemplateNotAllowed);

    /// Every mob operator right in `mob_id` except identity creation, bound
    /// to `incarnation`: create and profile-mutation authority, manage scope
    /// and spawn scope over two profiles. A member holding the mob tools has
    /// this, and may have a creation tool mounted by mistake.
    fn every_other_operator_right(
        mob_id: &str,
        incarnation: MobIncarnationId,
    ) -> MobToolAuthorityContext {
        let authority = grant_manage_mob(&create_only(), mob_id, live(incarnation))
            .expect("manage scope at the incarnation");
        grant_spawn_profiles_in_mob(&authority, mob_id, live(incarnation), ["lead", "worker"])
            .expect("spawn scope at the incarnation")
    }

    /// Row (RED): an agent creator's identity creation is refused unless its
    /// sealed authority holds the identity creation scope for the template,
    /// in the mob, at the incarnation the creation acts on. Every other mob
    /// operator right admits nothing, and neither does no authority or a
    /// persisted projection. A creation grant admits its templates only.
    #[test]
    fn an_agent_without_the_scope_is_refused_even_with_the_tool() {
        let incarnation = MobIncarnationId::mint();
        let tool_holder = every_other_operator_right("mob-1", incarnation);
        assert!(tool_holder.can_create_mobs() && tool_holder.can_mutate_profiles());
        assert!(tool_holder.can_manage_mob("mob-1", incarnation));
        assert!(tool_holder.spawn_profile_scope_contains("mob-1", incarnation, "worker"));
        assert!(!tool_holder.identity_creation_scope_present("mob-1", incarnation));
        assert_eq!(
            admit_identity_creation_from_template(
                Some(&tool_holder),
                "mob-1",
                incarnation,
                "template-a"
            ),
            CREATION_REFUSED,
            "manage, spawn, create and mutate rights never admit an identity creation"
        );
        assert_eq!(
            admit_identity_creation_from_template(None, "mob-1", incarnation, "template-a"),
            CREATION_REFUSED,
            "a session with no generated authority creates nothing"
        );

        let creator = grant_identity_creation_in_mob(
            &tool_holder,
            "mob-1",
            live(incarnation),
            ["template-a"],
        )
        .expect("a creation grant at the bound incarnation");
        assert!(creator.identity_creation_scope_present("mob-1", incarnation));
        assert_eq!(
            admit_identity_creation_from_template(
                Some(&creator),
                "mob-1",
                incarnation,
                "template-a"
            ),
            Ok(())
        );
        assert_eq!(
            admit_identity_creation_from_template(
                Some(&creator),
                "mob-1",
                incarnation,
                "template-b"
            ),
            CREATION_REFUSED,
            "only the granted templates"
        );
        assert_eq!(
            admit_identity_creation_from_template(
                Some(&creator),
                "mob-2",
                incarnation,
                "template-a"
            ),
            CREATION_REFUSED,
            "only the granted mob"
        );
        let projected = persisted_projection(creator);
        assert_eq!(
            admit_identity_creation_from_template(
                Some(&projected),
                "mob-1",
                incarnation,
                "template-a"
            ),
            CREATION_REFUSED,
            "a persisted projection grants nothing"
        );
    }

    /// Row: identity creation scope reaches only the incarnation of the mob
    /// it was granted for. A later mob under the same id is another
    /// incarnation: the scope admits nothing there, and any grant for the
    /// later mob replaces the binding and drops the scope held for the
    /// earlier one.
    #[test]
    fn a_scope_for_another_mob_incarnation_grants_nothing() {
        let first = MobIncarnationId::mint();
        let later = MobIncarnationId::mint();
        let creator =
            grant_identity_creation_in_mob(&create_only(), "mob-x", live(first), ["template-a"])
                .expect("a creation grant for the first mob");
        assert_eq!(
            admit_identity_creation_from_template(Some(&creator), "mob-x", first, "template-a"),
            Ok(())
        );
        assert_eq!(
            admit_identity_creation_from_template(Some(&creator), "mob-x", later, "template-a"),
            CREATION_REFUSED,
            "scope granted for an earlier mob under the same id"
        );
        assert!(!creator.identity_creation_scope_present("mob-x", later));

        // A manage or spawn grant for the later mob drops the creation
        // scope held for the earlier one.
        for rebound in [
            grant_manage_mob(&creator, "mob-x", live(later)).expect("a manage rebind"),
            grant_spawn_profile_in_mob(&creator, "mob-x", live(later), "worker")
                .expect("a spawn rebind"),
        ] {
            assert!(rebound.identity_creation_scope().get("mob-x").is_none());
            for incarnation in [first, later] {
                assert_eq!(
                    admit_identity_creation_from_template(
                        Some(&rebound),
                        "mob-x",
                        incarnation,
                        "template-a"
                    ),
                    CREATION_REFUSED
                );
            }
        }

        // A creation grant for the later mob admits there only, and drops
        // the manage and spawn scope held for the earlier one.
        let earlier_rights = every_other_operator_right("mob-x", first);
        let rebound =
            grant_identity_creation_in_mob(&earlier_rights, "mob-x", live(later), ["template-a"])
                .expect("a creation rebind");
        assert_eq!(
            admit_identity_creation_from_template(Some(&rebound), "mob-x", later, "template-a"),
            Ok(())
        );
        assert_eq!(
            admit_identity_creation_from_template(Some(&rebound), "mob-x", first, "template-a"),
            CREATION_REFUSED
        );
        assert!(!rebound.managed_mob_scope().contains("mob-x"));
        assert!(rebound.spawn_profile_scope().get("mob-x").is_none());
        assert_eq!(rebound.mob_incarnations().get("mob-x"), Some(&later));
    }

    /// Row: the identity creation scope never leaves the generated seal. A
    /// creator's persisted projection is the projection of the same
    /// authority without the scope, so a reader that knows only the earlier
    /// facts (the wire mirror, which refuses an unknown key) reads it as
    /// before and finds no creation scope. Nothing read back admits a
    /// creation: the projection restores nothing, and facts re-minted from
    /// what it carries hold no creation scope.
    #[test]
    fn a_pre_099_reader_drops_the_scope_and_the_creator_loses_creation() {
        let incarnation = MobIncarnationId::mint();
        let tool_holder = every_other_operator_right("mob-1", incarnation);
        let creator = grant_identity_creation_in_mob(
            &tool_holder,
            "mob-1",
            live(incarnation),
            ["template-a"],
        )
        .expect("a creation grant at the bound incarnation");
        assert_eq!(
            admit_identity_creation_from_template(
                Some(&creator),
                "mob-1",
                incarnation,
                "template-a"
            ),
            Ok(())
        );

        let projection = serde_json::to_value(creator).expect("project the creator");
        assert_eq!(
            projection,
            serde_json::to_value(tool_holder).expect("project the authority without the scope"),
            "the projection carries no identity creation scope"
        );
        let earlier_facts: meerkat_contracts::wire::WireMobToolAuthorityContext =
            serde_json::from_value(projection.clone())
                .expect("a reader of the earlier facts decodes the projection");
        let read_back: MobToolAuthorityContext =
            serde_json::from_value(projection).expect("decode the projection");
        assert!(read_back.identity_creation_scope().is_empty());
        assert!(restore_mob_operator_authority(&read_back).is_err());
        assert_eq!(
            admit_identity_creation_from_template(
                Some(&read_back),
                "mob-1",
                incarnation,
                "template-a"
            ),
            CREATION_REFUSED
        );

        let facts = MobOperatorAuthorityFacts {
            can_create_mobs: earlier_facts.can_create_mobs,
            can_mutate_profiles: earlier_facts.can_mutate_profiles,
            managed_mob_scope: earlier_facts.managed_mob_scope,
            spawn_profile_scope: earlier_facts.spawn_profile_scope,
            ..MobOperatorAuthorityFacts::default()
        };
        let reminted = mint_mob_operator_authority_from_facts(
            &facts,
            "mob-1",
            live(incarnation),
            MobToolCallerProvenance::default(),
            "req-1",
        )
        .expect("the earlier facts re-mint");
        assert!(reminted.can_manage_mob("mob-1", incarnation));
        assert_eq!(
            admit_identity_creation_from_template(
                Some(&reminted),
                "mob-1",
                incarnation,
                "template-a"
            ),
            CREATION_REFUSED,
            "the creator loses creation"
        );
    }

    /// A creation grant at the bound incarnation replaces the templates: a
    /// host whose creation policy changed mints the scope again. An empty
    /// set keeps the binding and allows nothing, and the other scope of the
    /// mob is kept.
    #[test]
    fn an_identity_creation_grant_at_the_bound_incarnation_replaces_the_templates() {
        let incarnation = MobIncarnationId::mint();
        let check = |context: &MobToolAuthorityContext, template: &str| {
            admit_identity_creation_from_template(Some(context), "mob-1", incarnation, template)
        };
        let manager = grant_manage_mob(&create_only(), "mob-1", live(incarnation))
            .expect("manage scope at the incarnation");
        let creator = grant_identity_creation_in_mob(
            &manager,
            "mob-1",
            live(incarnation),
            ["template-a", "template-b"],
        )
        .expect("a creation grant");
        assert_eq!(check(&creator, "template-a"), Ok(()));
        assert_eq!(check(&creator, "template-b"), Ok(()));

        let narrowed =
            grant_identity_creation_in_mob(&creator, "mob-1", live(incarnation), ["template-b"])
                .expect("the policy minted again");
        assert_eq!(check(&narrowed, "template-a"), CREATION_REFUSED);
        assert_eq!(check(&narrowed, "template-b"), Ok(()));
        assert!(narrowed.can_manage_mob("mob-1", incarnation));

        let emptied = grant_identity_creation_in_mob(
            &narrowed,
            "mob-1",
            live(incarnation),
            Vec::<String>::new(),
        )
        .expect("a policy that allows nothing");
        assert_eq!(check(&emptied, "template-b"), CREATION_REFUSED);
        assert!(!emptied.identity_creation_scope_present("mob-1", incarnation));
        assert_eq!(emptied.mob_incarnations().get("mob-1"), Some(&incarnation));
        assert!(emptied.can_manage_mob("mob-1", incarnation));
    }

    /// The identity creation scope survives the generated restore and the
    /// provenance and audit re-mints, as the other scopes do.
    #[test]
    fn restores_and_re_mints_keep_the_identity_creation_scope() {
        let incarnation = MobIncarnationId::mint();
        let creator = grant_identity_creation_in_mob(
            &create_only(),
            "mob-1",
            live(incarnation),
            ["template-a"],
        )
        .expect("a creation grant");
        let creator = with_caller_provenance(
            &creator,
            MobToolCallerProvenance::default().with_mob_id("mob-1"),
        )
        .expect("caller provenance");
        let creator = with_audit_invocation_id(&creator, "audit-1").expect("audit invocation id");
        let restored = restore_mob_operator_authority(&creator).expect("a sealed context restores");
        assert_eq!(
            restored.identity_creation_scope(),
            creator.identity_creation_scope()
        );
        assert_eq!(
            admit_identity_creation_from_template(
                Some(&restored),
                "mob-1",
                incarnation,
                "template-a"
            ),
            Ok(())
        );
    }

    /// The authority a member's build composes before any host edit: the
    /// create-only authority an enabled build resolves, then the default
    /// spawn scope over the profiles of the mob the member runs in.
    fn composed_member_build_authority(incarnation: MobIncarnationId) -> MobToolAuthorityContext {
        let (override_mob, composed) =
            resolve_mob_operator_access(ToolCategoryOverride::Enable, None);
        assert_eq!(override_mob, ToolCategoryOverride::Enable);
        let composed = composed.expect("an enabled build composes generated authority");
        grant_spawn_profiles_in_mob(&composed, "mob-1", live(incarnation), ["lead", "worker"])
            .expect("the default spawn scope")
    }

    /// Row: the positive path. A creator's build mints the identity creation
    /// scope from the host's policy on the authority the build composed, with
    /// the live witness of the mob the creator runs in, and the creation
    /// check admits every template the policy names, in that mob, at that
    /// incarnation. The mint changes nothing else the build composed.
    #[test]
    fn an_agent_with_the_minted_scope_creates_in_its_mob_incarnation() {
        let incarnation = MobIncarnationId::mint();
        let composed = composed_member_build_authority(incarnation);
        let policy_templates = ["research-helper", "reviewer"];
        let creator =
            grant_identity_creation_in_mob(&composed, "mob-1", live(incarnation), policy_templates)
                .expect("the build mints the scope");
        assert!(creator.is_generated_authority_context());
        assert!(creator.identity_creation_scope_present("mob-1", incarnation));
        for template in policy_templates {
            assert_eq!(
                admit_identity_creation_from_template(
                    Some(&creator),
                    "mob-1",
                    incarnation,
                    template
                ),
                Ok(()),
                "{template}"
            );
        }
        assert_eq!(
            creator.spawn_profile_scope(),
            composed.spawn_profile_scope()
        );
        assert_eq!(creator.can_create_mobs(), composed.can_create_mobs());
        assert_eq!(
            creator.can_mutate_profiles(),
            composed.can_mutate_profiles()
        );
        assert_eq!(creator.mob_incarnations(), composed.mob_incarnations());
    }

    /// Row: the identity creation scope is never read from a record. A
    /// creator's session records its authority as a projection; a build that
    /// restores the session from that record restores no authority at all,
    /// composes the member's authority again, and holds no creation scope
    /// until it mints the scope from the policy. Facts re-minted from a
    /// member's durable record carry none either.
    #[test]
    fn a_restored_context_carries_no_scope_until_the_build_mints_it() {
        let incarnation = MobIncarnationId::mint();
        let check = |context: Option<&MobToolAuthorityContext>| {
            admit_identity_creation_from_template(context, "mob-1", incarnation, "template-a")
        };
        let creator = grant_identity_creation_in_mob(
            &composed_member_build_authority(incarnation),
            "mob-1",
            live(incarnation),
            ["template-a"],
        )
        .expect("the first build mints the scope");
        assert_eq!(check(Some(&creator)), Ok(()));

        let record = persisted_projection(creator);
        let (override_mob, restored) =
            resolve_mob_operator_access(ToolCategoryOverride::Enable, Some(record.clone()));
        assert_eq!(override_mob, ToolCategoryOverride::Disable);
        assert!(restored.is_none(), "a record restores no authority");
        assert!(restore_mob_operator_authority(&record).is_err());
        assert_eq!(check(Some(&record)), CREATION_REFUSED);

        let rebuilt = composed_member_build_authority(incarnation);
        assert!(rebuilt.identity_creation_scope().is_empty());
        assert_eq!(check(Some(&rebuilt)), CREATION_REFUSED);

        let reminted = mint_mob_operator_authority_from_facts(
            &MobOperatorAuthorityFacts {
                managed_mob_scope: BTreeSet::from(["mob-1".to_string()]),
                ..MobOperatorAuthorityFacts::default()
            },
            "mob-1",
            live(incarnation),
            MobToolCallerProvenance::default(),
            "req-1",
        )
        .expect("a durable record's facts re-mint");
        assert_eq!(check(Some(&reminted)), CREATION_REFUSED);

        let minted =
            grant_identity_creation_in_mob(&rebuilt, "mob-1", live(incarnation), ["template-a"])
                .expect("the rebuild mints the scope again");
        assert_eq!(check(Some(&minted)), Ok(()));
    }

    /// A creation-only authority holds the identity creation scope for its
    /// templates in its mob, bound to the incarnation it was minted for, and
    /// nothing else: no manage or spawn-profile scope, no create or
    /// profile-mutation authority.
    #[test]
    fn a_creation_only_authority_holds_the_creation_scope_and_nothing_else() {
        let incarnation = MobIncarnationId::mint();
        let creator = identity_creation_only_mob_operator_authority(
            "mob-1",
            live(incarnation),
            ["template-a"],
        )
        .expect("mint the creation-only authority");
        assert!(creator.is_generated_authority_context());
        assert_eq!(
            creator.identity_creation_scope(),
            &BTreeMap::from([(
                "mob-1".to_string(),
                BTreeSet::from(["template-a".to_string()])
            )])
        );
        assert_eq!(
            creator.mob_incarnations(),
            &BTreeMap::from([("mob-1".to_string(), incarnation)])
        );
        assert!(!creator.can_create_mobs());
        assert!(!creator.can_mutate_profiles());
        assert!(!creator.can_run_adaptive_packs());
        assert!(creator.managed_mob_scope().is_empty());
        assert!(creator.spawn_profile_scope().is_empty());
        assert_eq!(
            admit_identity_creation_from_template(
                Some(&creator),
                "mob-1",
                incarnation,
                "template-a"
            ),
            Ok(())
        );
        assert_eq!(
            admit_identity_creation_from_template(
                Some(&creator),
                "mob-1",
                incarnation,
                "template-b"
            ),
            CREATION_REFUSED
        );
        assert_eq!(
            admit_identity_creation_from_template(
                Some(&creator),
                "mob-1",
                MobIncarnationId::mint(),
                "template-a"
            ),
            CREATION_REFUSED
        );
    }
}
