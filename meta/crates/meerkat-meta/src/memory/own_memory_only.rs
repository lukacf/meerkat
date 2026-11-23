//! Which agents of a mob are listed their own memory only.
//!
//! A host can name profiles whose agents are given, in the memory index of
//! their build, the records of their own scope alone: no record of their
//! mob's shared scope and no record of the realm's is listed to them, and
//! the store is asked for neither. It is a filter on what an agent is
//! listed. It is not a boundary between tenants: the agent's own records
//! are untouched, it can still propose a record to its mob, and every other
//! reader of the mob's shared memory (the host's calls, a person in the
//! console, the memory steward) reads as before.
//!
//! Who is such an agent is decided from what the host declared and from the
//! mob's own record of who created an agent, never from a name or a label a
//! request can set:
//!
//! - an agent whose declared profile the host listed;
//! - an agent whose declared profile is no profile of its own mob's
//!   definition;
//! - an agent created by such an agent, whatever profile its creation
//!   named, and so on up the agents that created each other;
//! - an agent above which a link cannot be read (a creator that is no
//!   registered agent of the mob, a record that cannot be read, more links
//!   than the walk follows).
//!
//! With no profile listed nothing is withheld and no record is read.

use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use meerkat_mob::{MobDefinition, ProfileName};

use crate::identity_first::{AgentIdentity, DurableAgentSpec};

/// The most links a decision follows up from an agent through the agents
/// that created each other. An agent with more links above it is listed
/// its own memory only.
pub(crate) const MAX_LINEAGE_LINKS: usize = 8;

/// What a mob's own records say is above an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineageAbove {
    /// The host or a person declared the agent: no agent is above it.
    HostDeclared,
    /// An agent created it. `profile` is that agent's declared profile, and
    /// `None` when it cannot be read (the creator is no registered agent of
    /// the mob).
    CreatedBy {
        identity: AgentIdentity,
        profile: Option<ProfileName>,
    },
    /// The mob's records do not say, or cannot be read.
    Unreadable,
}

/// The reader of what is above an agent, from records an agent's request
/// cannot write: the host's declarations and the mob's own record of who
/// created an agent.
#[async_trait]
pub trait TrustedLineage: Send + Sync {
    /// What is above `identity`.
    async fn above(&self, identity: &AgentIdentity) -> LineageAbove;
}

/// Who a record says asked for an agent's creation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RecordedCreator {
    /// The host on one of its surfaces, or a person.
    HostOrPerson,
    /// An agent, by its identity name.
    Agent(String),
    /// A kind of requester this build does not know.
    Unknown,
}

/// Who `requester`, the requester a mob's identity record keeps for the
/// operation that wrote it first, says created the agent.
pub(crate) fn recorded_creator(
    requester: &meerkat_mob::IdentityChangeRequester,
) -> RecordedCreator {
    match requester {
        meerkat_mob::IdentityChangeRequester::Host { .. }
        | meerkat_mob::IdentityChangeRequester::Human { .. } => RecordedCreator::HostOrPerson,
        meerkat_mob::IdentityChangeRequester::Agent { identity } => RecordedCreator::Agent(
            crate::member_comms_id::durable_identity_for_roster_member(identity.as_str()),
        ),
        _ => RecordedCreator::Unknown,
    }
}

/// What a read of an agent's row in the mob's identity store answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RecordRead {
    /// The store holds the agent's row: who it says asked for the agent.
    Row(RecordedCreator),
    /// The store holds no row of the name.
    NoRow,
    /// The row, or the store, cannot be read.
    Unreadable,
}

/// The link above an agent as the mob's records say it, before the profile
/// of the agent that created it is read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RecordedLink {
    /// The host or a person declared the agent.
    HostDeclared,
    /// The agent of this name created it.
    CreatedBy(AgentIdentity),
    /// The records do not say, or cannot be read.
    Unreadable,
}

/// The link above an agent: `read` is the read of its row in the mob's
/// identity store, and `registered` whether the mob's identity runtime
/// registers the agent (from the host's roster or from that store).
pub(crate) fn recorded_link(read: &RecordRead, registered: bool) -> RecordedLink {
    match read {
        RecordRead::Row(RecordedCreator::HostOrPerson) => RecordedLink::HostDeclared,
        RecordRead::Row(RecordedCreator::Agent(name)) => match AgentIdentity::parse(name) {
            Ok(creator) => RecordedLink::CreatedBy(creator),
            Err(_) => RecordedLink::Unreadable,
        },
        // The store holds no row of the name: the agent is the host's
        // roster's, when the runtime registers it.
        RecordRead::NoRow if registered => RecordedLink::HostDeclared,
        RecordRead::Row(RecordedCreator::Unknown) | RecordRead::NoRow | RecordRead::Unreadable => {
            RecordedLink::Unreadable
        }
    }
}

/// The lineage a mob's own records hold: who the mob's identity store says
/// asked for an agent's creation, and which agents the mob's identity
/// runtime registers (from the host's roster and from that store). No label
/// is read.
///
/// - An agent whose row says the host or a person asked for it, and an
///   agent the host's roster declares, has nothing above it.
/// - An agent whose row says an agent asked for it has that agent above it,
///   with the profile the identity runtime registers for that agent, or
///   none when it registers no such agent (a member that is no registered
///   agent, an agent that is gone).
/// - A row that cannot be read, a row of a kind this build does not know,
///   an agent neither the store nor the roster holds, and a runtime that
///   is gone, was never bound or was given no mob, cannot be read.
///
/// A launch that composes its agents' customizer before its identity
/// runtime exists makes the lineage [`unbound`](Self::unbound) and
/// [`bind`](Self::bind)s it when the runtime stands. Clones share the
/// binding.
#[derive(Clone)]
pub struct RecordedLineage {
    runtime: Arc<std::sync::OnceLock<std::sync::Weak<crate::identity_first::IdentityRuntime>>>,
}

impl RecordedLineage {
    /// The lineage the records of `runtime`'s mob hold.
    #[must_use]
    pub fn of(runtime: &Arc<crate::identity_first::IdentityRuntime>) -> Self {
        Self {
            runtime: Arc::new(std::sync::OnceLock::from(Arc::downgrade(runtime))),
        }
    }

    /// A lineage of no mob yet: until it is bound, every link above every
    /// agent cannot be read.
    #[must_use]
    pub fn unbound() -> Self {
        Self {
            runtime: Arc::new(std::sync::OnceLock::new()),
        }
    }

    /// Bind this lineage, and its clones, to the records of `runtime`'s
    /// mob. The first binding stands: `false` when it was bound before,
    /// and then the lineage still reads the mob it was bound to first. A
    /// caller that gets `false` has two mobs behind one lineage and must
    /// not go on.
    #[must_use = "a lineage that was bound before still reads its first mob"]
    pub fn bind(&self, runtime: &Arc<crate::identity_first::IdentityRuntime>) -> bool {
        self.runtime.set(Arc::downgrade(runtime)).is_ok()
    }
}

#[async_trait]
impl TrustedLineage for RecordedLineage {
    async fn above(&self, identity: &AgentIdentity) -> LineageAbove {
        let Some(runtime) = self.runtime.get().and_then(std::sync::Weak::upgrade) else {
            return LineageAbove::Unreadable;
        };
        let registered = runtime.registered_spec(identity).await.is_ok();
        let read = match runtime.runtime_services().mob_handle() {
            // A runtime that was given no mob has no records to ask.
            None => RecordRead::Unreadable,
            Some(handle) => {
                let member = crate::member_comms_id::mob_member_id(identity.as_str());
                match handle.identity_record(&member).await {
                    Ok(Some(meerkat_mob::CreatedIdentityObservation::Created(record))) => {
                        RecordRead::Row(recorded_creator(&record.provenance.requester))
                    }
                    Ok(None) => RecordRead::NoRow,
                    Ok(Some(_)) | Err(_) => RecordRead::Unreadable,
                }
            }
        };
        match recorded_link(&read, registered) {
            RecordedLink::HostDeclared => LineageAbove::HostDeclared,
            RecordedLink::Unreadable => LineageAbove::Unreadable,
            RecordedLink::CreatedBy(creator) => {
                let profile = runtime
                    .registered_spec(&creator)
                    .await
                    .ok()
                    .map(|registered| registered.spec.profile);
                LineageAbove::CreatedBy {
                    identity: creator,
                    profile,
                }
            }
        }
    }
}

/// The profiles a host listed, and the reader of an agent's lineage.
#[derive(Clone)]
pub struct OwnMemoryOnly {
    profiles: BTreeSet<ProfileName>,
    lineage: Arc<dyn TrustedLineage>,
}

impl std::fmt::Debug for OwnMemoryOnly {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnMemoryOnly")
            .field("profiles", &self.profiles)
            .finish_non_exhaustive()
    }
}

impl OwnMemoryOnly {
    /// The agents of `profiles`, and the agents `lineage` says descend from
    /// them, are listed their own memory only.
    #[must_use]
    pub fn new(profiles: BTreeSet<ProfileName>, lineage: Arc<dyn TrustedLineage>) -> Self {
        Self { profiles, lineage }
    }

    /// The rule of a launch that listed `profiles`, read through `lineage`:
    /// none when it listed no profile, so that a launch without the option
    /// builds its agents as before and reads no record.
    #[must_use]
    pub fn listing(
        profiles: BTreeSet<ProfileName>,
        lineage: impl TrustedLineage + 'static,
    ) -> Option<Self> {
        (!profiles.is_empty()).then(|| Self::new(profiles, Arc::new(lineage)))
    }

    /// The profiles the host listed.
    #[must_use]
    pub fn profiles(&self) -> &BTreeSet<ProfileName> {
        &self.profiles
    }

    /// Whether the agent `spec` declares, an agent of the mob `definition`
    /// defines (when the build runs inside one), is listed its own memory
    /// only.
    pub(crate) async fn restricts(
        &self,
        spec: &DurableAgentSpec,
        definition: Option<&MobDefinition>,
    ) -> bool {
        if self.profiles.is_empty() {
            return false;
        }
        if self.profiles.contains(&spec.profile) {
            return true;
        }
        if definition.is_some_and(|definition| !definition.profiles.contains_key(&spec.profile)) {
            return true;
        }
        let mut agent = spec.identity.clone();
        for _ in 0..MAX_LINEAGE_LINKS {
            match self.lineage.above(&agent).await {
                LineageAbove::HostDeclared => return false,
                LineageAbove::Unreadable => return true,
                LineageAbove::CreatedBy { identity, profile } => match profile {
                    Some(profile) if !self.profiles.contains(&profile) => agent = identity,
                    Some(_) | None => return true,
                },
            }
        }
        true
    }
}

/// The start check of the profiles a host listed: each name is a profile
/// of `definition`, the launch mob's definition, and is listed once.
///
/// # Errors
/// A text that names the option and the name, when a name is no profile of
/// the definition or is listed twice.
pub fn own_memory_only_profiles_of(
    definition: &MobDefinition,
    names: &[String],
) -> Result<BTreeSet<ProfileName>, String> {
    let mut profiles = BTreeSet::new();
    for name in names {
        let profile = ProfileName::from(name.as_str());
        if !definition.profiles.contains_key(&profile) {
            let known = definition
                .profiles
                .keys()
                .map(ProfileName::as_str)
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "own_memory_only_profiles names {name:?}, which is no profile of the mob {} \
                 (its profiles: {known})",
                definition.id
            ));
        }
        if !profiles.insert(profile) {
            return Err(format!(
                "own_memory_only_profiles names the profile {name:?} more than once"
            ));
        }
    }
    Ok(profiles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_mob::IdentityChangeRequester;
    use std::error::Error;

    fn definition() -> Result<MobDefinition, Box<dyn Error>> {
        Ok(MobDefinition::from_toml(
            r#"
[mob]
id = "alpha"

[profiles.guest]
model = "gpt-5.5"

[profiles.staff]
model = "gpt-5.5"
"#,
        )?)
    }

    #[test]
    fn a_recorded_requester_says_who_created_the_agent() -> Result<(), Box<dyn Error>> {
        assert_eq!(
            recorded_creator(&IdentityChangeRequester::Host {
                surface: "host".to_string(),
            }),
            RecordedCreator::HostOrPerson
        );
        assert_eq!(
            recorded_creator(&IdentityChangeRequester::Human {
                principal: "person:one".to_string(),
            }),
            RecordedCreator::HostOrPerson
        );
        let creator = meerkat_mob::AgentIdentity::from("staff");
        assert_eq!(
            recorded_creator(&IdentityChangeRequester::Agent { identity: creator }),
            RecordedCreator::Agent("staff".to_string())
        );
        Ok(())
    }

    #[test]
    fn listed_profiles_are_checked_against_the_mob_definition() -> Result<(), Box<dyn Error>> {
        let definition = definition()?;
        let checked = own_memory_only_profiles_of(&definition, &["guest".to_string()])?;
        assert_eq!(
            checked.into_iter().collect::<Vec<_>>(),
            vec![ProfileName::from("guest")]
        );
        assert!(own_memory_only_profiles_of(&definition, &[])?.is_empty());
        let Err(refusal) = own_memory_only_profiles_of(&definition, &["gest".to_string()]) else {
            return Err("a name no profile has is refused".into());
        };
        for part in ["own_memory_only_profiles", "gest", "alpha"] {
            assert!(refusal.contains(part), "{part}: {refusal}");
        }
        let Err(refusal) =
            own_memory_only_profiles_of(&definition, &["guest".to_string(), "guest".to_string()])
        else {
            return Err("a name listed twice is refused".into());
        };
        assert!(refusal.contains("guest"), "{refusal}");
        Ok(())
    }

    fn agent(name: &str) -> Result<AgentIdentity, Box<dyn Error>> {
        AgentIdentity::parse(name).map_err(|err| {
            std::io::Error::other(format!("test identity {name} should parse: {err}")).into()
        })
    }

    /// The link above an agent, from the read of its row and from whether
    /// the mob's identity runtime registers it.
    #[test]
    fn the_mobs_records_say_what_is_above_an_agent() -> Result<(), Box<dyn Error>> {
        // A row that says the host or a person asked for the agent.
        for registered in [true, false] {
            assert_eq!(
                recorded_link(&RecordRead::Row(RecordedCreator::HostOrPerson), registered),
                RecordedLink::HostDeclared
            );
        }
        // A row that says an agent asked for it.
        assert_eq!(
            recorded_link(
                &RecordRead::Row(RecordedCreator::Agent("staff".to_string())),
                true
            ),
            RecordedLink::CreatedBy(agent("staff")?)
        );
        // No row: an agent the runtime registers is the host's roster's.
        assert_eq!(
            recorded_link(&RecordRead::NoRow, true),
            RecordedLink::HostDeclared
        );
        Ok(())
    }

    /// What the records do not say cannot be read: a row of a requester
    /// kind this build does not know, a creator whose name is no agent
    /// name, an agent neither the store nor the roster holds, a failed read.
    #[test]
    fn what_the_mobs_records_do_not_say_cannot_be_read() {
        for (read, registered) in [
            (RecordRead::Row(RecordedCreator::Unknown), true),
            (RecordRead::Row(RecordedCreator::Agent(String::new())), true),
            (RecordRead::NoRow, false),
            (RecordRead::Unreadable, true),
            (RecordRead::Unreadable, false),
        ] {
            assert_eq!(
                recorded_link(&read, registered),
                RecordedLink::Unreadable,
                "{read:?}, registered {registered}"
            );
        }
    }

    /// A lineage in which the host declared every agent.
    struct AllHostDeclared;

    #[async_trait]
    impl TrustedLineage for AllHostDeclared {
        async fn above(&self, _identity: &AgentIdentity) -> LineageAbove {
            LineageAbove::HostDeclared
        }
    }

    fn spec_of(name: &str, profile: &str) -> Result<DurableAgentSpec, Box<dyn Error>> {
        Ok(DurableAgentSpec {
            identity: agent(name)?,
            profile: ProfileName::from(profile),
            addressability: Default::default(),
            display_name: None,
            labels: Default::default(),
            context: None,
            additional_instructions: Vec::new(),
            initial_message: None,
            runtime_mode_override: None,
            backend: None,
            binding: None,
            placement: None,
        })
    }

    /// A lineage that no launch bound to a mob's records reads no link:
    /// with a profile listed, an agent of an unlisted profile is listed its
    /// own memory only until the lineage is bound.
    #[tokio::test]
    async fn a_lineage_bound_to_no_mob_reads_no_link() -> Result<(), Box<dyn Error>> {
        let lineage = RecordedLineage::unbound();
        assert_eq!(
            lineage.above(&agent("clerk")?).await,
            LineageAbove::Unreadable
        );
        let definition = definition()?;
        let rule = OwnMemoryOnly::new(
            BTreeSet::from([ProfileName::from("guest")]),
            Arc::new(lineage.clone()),
        );
        assert!(
            rule.restricts(&spec_of("clerk", "staff")?, Some(&definition))
                .await,
            "an unlisted profile above which no link can be read"
        );
        let none_listed = OwnMemoryOnly::new(BTreeSet::new(), Arc::new(lineage));
        assert!(
            !none_listed
                .restricts(&spec_of("clerk", "staff")?, Some(&definition))
                .await,
            "no profile is listed"
        );
        Ok(())
    }

    /// With a profile listed, an agent whose declared profile is no profile
    /// of its own mob's definition is listed its own memory only; an agent
    /// of an unlisted profile the definition has is not, and with no
    /// profile listed neither is.
    #[tokio::test]
    async fn an_agent_whose_profile_its_mob_does_not_define_is_listed_its_own_memory_only()
    -> Result<(), Box<dyn Error>> {
        let definition = definition()?;
        let rule = OwnMemoryOnly::new(
            BTreeSet::from([ProfileName::from("guest")]),
            Arc::new(AllHostDeclared),
        );
        assert!(
            rule.restricts(&spec_of("stray", "visitor")?, Some(&definition))
                .await,
            "a profile the mob's definition does not hold"
        );
        assert!(
            !rule
                .restricts(&spec_of("clerk", "staff")?, Some(&definition))
                .await,
            "an unlisted profile of the definition"
        );
        let none_listed = OwnMemoryOnly::new(BTreeSet::new(), Arc::new(AllHostDeclared));
        assert!(
            !none_listed
                .restricts(&spec_of("stray", "visitor")?, Some(&definition))
                .await,
            "no profile is listed"
        );
        Ok(())
    }
}
