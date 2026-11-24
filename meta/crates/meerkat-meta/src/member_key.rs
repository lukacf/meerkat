//! The key of a member on a host that serves several mobs, and the default
//! mob stored records fall back to.
//!
//! One host keeps the records of all its mobs in one realm, and two mobs may
//! each have a member of the same name. So a record that belongs to a member
//! names the member by two values: the mob and the identity, each as its
//! own typed value. The mob is never written into the identity string, and
//! no store or policy reads a rendered `<mob>/<identity>` name for meaning.
//!
//! A stored record that carries no mob is the default mob's
//! ([`DefaultMob`]): the mob the launch names. Every record written before
//! records carried a mob has that form, so none is rewritten. A writer names
//! the default mob by leaving `mob_id` out ([`DefaultMob::stored`]): a record
//! written so has the one stored key a read of its member finds. A record
//! that names the default mob by its id is not folded back by every store,
//! so a writer that does not go through [`DefaultMob::stored`] can give a
//! member of the default mob a second stored key that no read of that member
//! finds.

use meerkat_mob::MobId;

use crate::identity_first::AgentIdentity;

/// A member of one of the host's mobs: the mob, and the member's identity in
/// it. Identities are names inside a mob, so the pair, never the identity
/// alone, names a member of a host.
///
/// Not exported yet: the member records name a member by its mob and the
/// name the console knows it by, which is not always an [`AgentIdentity`].
/// The surfaces that address a mob settle the identity type and take this
/// pair into the identity plane.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct MemberKey {
    pub(crate) mob_id: MobId,
    pub(crate) identity: AgentIdentity,
}

#[cfg_attr(not(test), allow(dead_code))]
impl MemberKey {
    #[must_use]
    pub(crate) fn new(mob_id: MobId, identity: AgentIdentity) -> Self {
        Self { mob_id, identity }
    }
}

impl std::fmt::Display for MemberKey {
    /// For logs and messages only. Nothing parses it back.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (mob {})", self.identity, self.mob_id)
    }
}

/// The default mob of a host: the mob its launch names.
///
/// It answers "which mob" for a caller that names none and for a stored
/// record that carries none. A host has exactly one, fixed for the life of
/// its state: the mob in the launch mob's store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultMob(MobId);

impl DefaultMob {
    /// The default mob `mob_id`, as the host states it: the id of its
    /// launch mob. A host may serve several mobs and only its launch mob is
    /// its default mob, so the value comes from the host, never from the
    /// wrapper of a mob a request resolved or a registry adopted.
    #[must_use]
    pub fn new(mob_id: MobId) -> Self {
        Self(mob_id)
    }

    #[must_use]
    pub fn mob_id(&self) -> &MobId {
        &self.0
    }

    /// Whether `mob_id` is the default mob.
    #[must_use]
    pub fn is(&self, mob_id: &MobId) -> bool {
        self.0 == *mob_id
    }

    /// The default mob's member named `identity`.
    #[cfg_attr(not(test), allow(dead_code))]
    #[must_use]
    pub(crate) fn member(&self, identity: AgentIdentity) -> MemberKey {
        MemberKey::new(self.0.clone(), identity)
    }

    /// The mob a stored record names: the mob it carries, or the default
    /// mob when it carries none. An empty value is no mob: the form a
    /// column added to an existing table gives its earlier rows, and the
    /// only place where an empty mob is read so. A typed member (a JSON
    /// record's, a request's) that holds a blank mob is refused where it is
    /// read (`names_a_mob`), before it reaches this.
    #[must_use]
    pub fn resolve(&self, stored: Option<&str>) -> MobId {
        match stored {
            Some(mob_id) if !mob_id.is_empty() => MobId::from(mob_id),
            _ => self.0.clone(),
        }
    }

    /// How a record stores `mob_id`: nothing for the default mob, the mob's
    /// id for any other.
    #[must_use]
    pub fn stored<'a>(&self, mob_id: &'a MobId) -> Option<&'a MobId> {
        (!self.is(mob_id)).then_some(mob_id)
    }
}

/// Whether `mob_id` names a mob: an id that is empty or only white space
/// names none.
///
/// The one rule for a typed member that holds a mob (a JSON record's
/// `mob_id`, a request's): a blank one is refused where it is read, and is
/// never read as the default mob. A member that means the default mob is
/// left out. Only a SQL column's default is read as "no mob"
/// ([`DefaultMob::resolve`]).
#[must_use]
pub(crate) fn names_a_mob(mob_id: &MobId) -> bool {
    !mob_id.as_str().trim().is_empty()
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn default_mob() -> DefaultMob {
        DefaultMob::new(MobId::from("launch"))
    }

    #[test]
    fn a_blank_mob_names_no_mob() {
        for blank in ["", " ", "  ", "\t", "\n "] {
            assert!(!names_a_mob(&MobId::from(blank)), "{blank:?}");
        }
        for named in ["launch", "second", " second ", "a b"] {
            assert!(names_a_mob(&MobId::from(named)), "{named:?}");
        }
    }

    #[test]
    fn a_stored_record_without_a_mob_is_the_default_mobs() {
        let default_mob = default_mob();
        assert_eq!(default_mob.resolve(None), MobId::from("launch"));
        assert_eq!(default_mob.resolve(Some("")), MobId::from("launch"));
        assert_eq!(default_mob.resolve(Some("second")), MobId::from("second"));
    }

    #[test]
    fn the_default_mob_is_stored_as_no_mob() {
        let default_mob = default_mob();
        assert_eq!(default_mob.stored(&MobId::from("launch")), None);
        let second = MobId::from("second");
        assert_eq!(default_mob.stored(&second), Some(&second));
        // A member of the default mob has one stored key: what is stored for
        // it reads back as the same mob.
        for mob in [MobId::from("launch"), MobId::from("second")] {
            assert_eq!(
                default_mob.resolve(default_mob.stored(&mob).map(MobId::as_str)),
                mob
            );
        }
    }

    #[test]
    fn two_mobs_members_of_one_name_are_two_keys() {
        let default_mob = default_mob();
        let identity = AgentIdentity::parse("alice").expect("identity");
        let first = default_mob.member(identity.clone());
        let second = MemberKey::new(MobId::from("second"), identity);
        assert_ne!(first, second);
        assert_eq!(first.identity, second.identity);
    }
}
