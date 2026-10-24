//! The upper bound on the mob operator authority a placed member is compiled
//! with.
//!
//! A placed member's mob operator tools are upcalls. The controlling host
//! admits each one against the authority facts it recorded when it compiled
//! the member, in the member's committed placed-spawn carrier. A session build
//! hook on the member host does not edit those facts; a ceiling on the spawn
//! request does.

use std::collections::BTreeSet;

use meerkat_contracts::wire::WireMobToolAuthorityContext;
use meerkat_core::service::{LiveMobIncarnation, MobToolAuthorityContext};

use crate::error::OperatorAuthorityCeilingRefusal;
use crate::ids::ProfileName;

/// The most mob operator authority a placed member may hold over its own mob.
///
/// A placed member whose profile enables mob tools and whose spawn request
/// carries no ceiling is compiled with the default authority: spawn scope over
/// every profile of the mob, manage scope over the mob, and create and
/// profile-mutation authority. Manage scope admits every spawn, including the
/// privileged spawn arguments (a runtime mode, a launch mode and a requested
/// tool access policy).
///
/// A ceiling lowers that default and never adds to it:
///
/// - `spawn_profiles`: `None` keeps every profile. `Some(profiles)` keeps only
///   the listed profiles; a profile the mob does not define is ignored.
/// - `manage`: `false` drops manage scope. The member can then spawn only the
///   profiles it holds, never with a privileged spawn argument, and it is
///   refused the operator tools that need manage scope, such as
///   `list_members` and `retire_member`.
/// - `create` and `mutate`: `false` drops create authority and
///   profile-mutation authority.
///
/// Narrowing the spawn profiles needs `manage = false`. Manage scope admits
/// every profile, so a ceiling that sets both is refused with
/// [`OperatorAuthorityCeilingRefusal::SpawnProfilesWithManage`].
///
/// A ceiling binds only a placed member: set on a spawn without a placement,
/// it is refused with [`OperatorAuthorityCeilingRefusal::UnplacedMember`].
///
/// It also bounds every member that member spawns itself (its
/// `spawn_member` and `spawn_many_members` upcalls): the child is compiled
/// with at most the authority recorded for the member that asked for it. A
/// ceiling on the child's own spawn request, or one a spawn customizer sets,
/// can narrow that bound but never widen it, so a narrowed member cannot
/// regain authority through a member it spawns. A member without a ceiling
/// bounds its children by nothing more than the default.
///
/// The narrowed facts are recorded with the member, so the narrowing holds
/// across restarts of the controlling host. A respawn keeps them, unless the
/// respawn request (or a spawn customizer on it) sets a ceiling of its own.
///
/// ```
/// use meerkat_mob::MemberOperatorAuthorityCeiling;
///
/// // Spawn only `worker` members, with no manage scope and no create or
/// // profile-mutation authority.
/// let ceiling = MemberOperatorAuthorityCeiling::unrestricted()
///     .without_manage()
///     .without_create()
///     .without_mutate()
///     .with_spawn_profiles(["worker"]);
/// assert!(!ceiling.manage);
/// assert!(!ceiling.is_unrestricted());
/// assert!(MemberOperatorAuthorityCeiling::unrestricted().is_unrestricted());
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct MemberOperatorAuthorityCeiling {
    /// The profiles the member may spawn in its mob. `None` keeps every
    /// profile.
    pub spawn_profiles: Option<BTreeSet<ProfileName>>,
    /// Whether the member keeps manage scope over its mob.
    pub manage: bool,
    /// Whether the member keeps create authority.
    pub create: bool,
    /// Whether the member keeps profile-mutation authority.
    pub mutate: bool,
}

impl MemberOperatorAuthorityCeiling {
    /// A ceiling that lowers nothing: a member compiled with it holds the
    /// default authority, as with no ceiling.
    #[must_use]
    pub fn unrestricted() -> Self {
        Self {
            spawn_profiles: None,
            manage: true,
            create: true,
            mutate: true,
        }
    }

    /// Keep only `profiles` in the member's spawn scope.
    #[must_use]
    pub fn with_spawn_profiles<I, P>(mut self, profiles: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<ProfileName>,
    {
        self.spawn_profiles = Some(profiles.into_iter().map(Into::into).collect());
        self
    }

    /// Drop manage scope over the member's mob.
    #[must_use]
    pub fn without_manage(mut self) -> Self {
        self.manage = false;
        self
    }

    /// Drop create authority.
    #[must_use]
    pub fn without_create(mut self) -> Self {
        self.create = false;
        self
    }

    /// Drop profile-mutation authority.
    #[must_use]
    pub fn without_mutate(mut self) -> Self {
        self.mutate = false;
        self
    }

    /// Whether this ceiling lowers nothing.
    pub fn is_unrestricted(&self) -> bool {
        *self == Self::unrestricted()
    }

    /// Refuse a ceiling whose narrowing could not hold.
    pub(crate) fn check(&self) -> Result<(), OperatorAuthorityCeilingRefusal> {
        if self.spawn_profiles.is_some() && self.manage {
            return Err(OperatorAuthorityCeilingRefusal::SpawnProfilesWithManage);
        }
        Ok(())
    }

    /// Lower `authority`, the default authority a placed member of `mob_id`
    /// is minted with before its manage scope, to this ceiling. Every step is
    /// a generated authority input that only narrows, except the manage grant,
    /// which is the default's own and is made only when the ceiling keeps it.
    pub(crate) fn apply(
        &self,
        authority: &MobToolAuthorityContext,
        mob_id: &str,
        mob_incarnation: LiveMobIncarnation,
    ) -> Result<MobToolAuthorityContext, String> {
        use meerkat_runtime::mob_operator_authority as generated;

        let mut authority = authority.clone();
        if let Some(profiles) = &self.spawn_profiles {
            authority = generated::restrict_spawn_profiles_in_mob(
                &authority,
                mob_id,
                profiles.iter().map(|profile| profile.as_str().to_string()),
            )?;
        }
        if self.manage {
            authority = generated::grant_manage_mob(&authority, mob_id, mob_incarnation)?;
        }
        if !self.create {
            authority = generated::set_create_authority(&authority, false)?;
        }
        if !self.mutate {
            authority = generated::set_profile_mutation(&authority, false)?;
        }
        Ok(authority)
    }

    /// The ceiling that the operator authority facts recorded for a placed
    /// member of `mob_id` keep, or `None` when they keep the whole default.
    ///
    /// The facts hold manage scope only when the ceiling kept it, and a
    /// ceiling that keeps manage scope keeps every profile. Without manage
    /// scope, the recorded spawn profiles are the ceiling: a respawn never
    /// gains a profile its predecessor could not spawn.
    pub(crate) fn recorded_in(facts: &WireMobToolAuthorityContext, mob_id: &str) -> Option<Self> {
        Self::from_recorded_facts(
            mob_id,
            &facts.managed_mob_scope,
            &facts.spawn_profile_scope,
            facts.can_create_mobs,
            facts.can_mutate_profiles,
        )
    }

    /// [`Self::recorded_in`] over the facts a member operator upcall re-mints
    /// its authority from: the bound a member's own spawns inherit.
    pub(crate) fn recorded_in_upcall_facts(
        facts: &meerkat_runtime::mob_operator_authority::MobOperatorAuthorityFacts,
        mob_id: &str,
    ) -> Option<Self> {
        Self::from_recorded_facts(
            mob_id,
            &facts.managed_mob_scope,
            &facts.spawn_profile_scope,
            facts.can_create_mobs,
            facts.can_mutate_profiles,
        )
    }

    fn from_recorded_facts(
        mob_id: &str,
        managed_mob_scope: &BTreeSet<String>,
        spawn_profile_scope: &std::collections::BTreeMap<String, BTreeSet<String>>,
        can_create_mobs: bool,
        can_mutate_profiles: bool,
    ) -> Option<Self> {
        let manage = managed_mob_scope.contains(mob_id);
        let spawn_profiles = (!manage).then(|| {
            spawn_profile_scope
                .get(mob_id)
                .into_iter()
                .flatten()
                .map(|profile| ProfileName::from(profile.as_str()))
                .collect()
        });
        let recorded = Self {
            spawn_profiles,
            manage,
            create: can_create_mobs,
            mutate: can_mutate_profiles,
        };
        (!recorded.is_unrestricted()).then_some(recorded)
    }

    /// The most authority both `self` and `bound` allow: the spawn profiles
    /// both keep, and manage, create and mutate only when both keep them.
    pub(crate) fn intersect(&self, bound: &Self) -> Self {
        let spawn_profiles = match (&self.spawn_profiles, &bound.spawn_profiles) {
            (None, None) => None,
            (Some(profiles), None) | (None, Some(profiles)) => Some(profiles.clone()),
            (Some(own), Some(bounded)) => Some(own.intersection(bounded).cloned().collect()),
        };
        Self {
            spawn_profiles,
            manage: self.manage && bound.manage,
            create: self.create && bound.create,
            mutate: self.mutate && bound.mutate,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;

    fn facts(
        manage: bool,
        create: bool,
        mutate: bool,
        profiles: &[&str],
    ) -> WireMobToolAuthorityContext {
        WireMobToolAuthorityContext {
            principal_token: "token".to_string(),
            can_create_mobs: create,
            can_mutate_profiles: mutate,
            can_run_adaptive_packs: false,
            managed_mob_scope: if manage {
                BTreeSet::from(["mob".to_string()])
            } else {
                BTreeSet::new()
            },
            spawn_profile_scope: BTreeMap::from([(
                "mob".to_string(),
                profiles
                    .iter()
                    .map(|profile| (*profile).to_string())
                    .collect(),
            )]),
            caller_provenance: None,
            audit_invocation_id: None,
        }
    }

    /// Intersecting keeps only what both allow, so a wider ceiling never
    /// widens a bound and a narrower one narrows it.
    #[test]
    fn intersect_keeps_only_what_both_allow() {
        let bound = MemberOperatorAuthorityCeiling::unrestricted()
            .without_manage()
            .with_spawn_profiles(["a", "b"]);
        assert_eq!(
            MemberOperatorAuthorityCeiling::unrestricted().intersect(&bound),
            bound,
            "a ceiling that lowers nothing does not widen the bound"
        );
        assert_eq!(
            MemberOperatorAuthorityCeiling::unrestricted()
                .without_create()
                .intersect(&bound),
            bound.clone().without_create(),
            "a narrower dimension narrows the bound, a wider one does not widen it"
        );
        assert_eq!(
            MemberOperatorAuthorityCeiling::unrestricted()
                .without_manage()
                .with_spawn_profiles(["b", "c"])
                .intersect(&bound),
            MemberOperatorAuthorityCeiling::unrestricted()
                .without_manage()
                .with_spawn_profiles(["b"]),
            "spawn profiles are intersected"
        );
        let manage_only = MemberOperatorAuthorityCeiling::unrestricted().without_mutate();
        assert_eq!(
            manage_only.intersect(&MemberOperatorAuthorityCeiling::unrestricted()),
            manage_only,
            "an unrestricted bound keeps the request's ceiling"
        );
    }

    #[test]
    fn spawn_profiles_with_manage_is_refused() {
        let ceiling = MemberOperatorAuthorityCeiling::unrestricted().with_spawn_profiles(["a"]);
        assert_eq!(
            ceiling.check(),
            Err(OperatorAuthorityCeilingRefusal::SpawnProfilesWithManage)
        );
        assert_eq!(ceiling.without_manage().check(), Ok(()));
        assert_eq!(
            MemberOperatorAuthorityCeiling::unrestricted().check(),
            Ok(())
        );
    }

    /// The default facts (manage over the mob, create, mutate) keep the whole
    /// default, so a respawn of a member spawned without a ceiling compiles
    /// as before.
    #[test]
    fn default_facts_record_no_ceiling() {
        assert_eq!(
            MemberOperatorAuthorityCeiling::recorded_in(
                &facts(true, true, true, &["a", "b"]),
                "mob"
            ),
            None
        );
    }

    #[test]
    fn narrowed_facts_record_their_ceiling() {
        assert_eq!(
            MemberOperatorAuthorityCeiling::recorded_in(&facts(false, true, true, &["a"]), "mob"),
            Some(
                MemberOperatorAuthorityCeiling::unrestricted()
                    .without_manage()
                    .with_spawn_profiles(["a"])
            )
        );
        assert_eq!(
            MemberOperatorAuthorityCeiling::recorded_in(
                &facts(true, false, false, &["a", "b"]),
                "mob"
            ),
            Some(
                MemberOperatorAuthorityCeiling::unrestricted()
                    .without_create()
                    .without_mutate()
            )
        );
        // Manage scope over another mob is not manage scope over this one.
        assert_eq!(
            MemberOperatorAuthorityCeiling::recorded_in(&facts(true, true, true, &["a"]), "other"),
            Some(
                MemberOperatorAuthorityCeiling::unrestricted()
                    .without_manage()
                    .with_spawn_profiles(Vec::<ProfileName>::new())
            )
        );
    }
}
