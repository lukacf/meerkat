//! What a fork-derived member inherits from its source member's build.
//!
//! A durable fork copies its source's transcript, but not the per-build inputs
//! the source's host resolved its tools and instructions from: the source's
//! application context, its application labels, and its retained per-spawn
//! tool overlay. A child built without them is a different agent. HomeCore saw
//! a calendar fork built as a generic domain member: 92 of calendar's 150
//! tools, no calendar, display or picture-schedule tools, no `memory` tool,
//! and a tools block that differed from the forker's, so the child could not
//! reuse the forker's cached prefix either.
//!
//! [`ForkBuildInheritance`] carries those inputs, with the typed
//! [`meerkat_core::ForkBuildSource`] a host resolves the child by, from the
//! source member to the child's seating build. It applies to every
//! fork-derived member: `fork_off` children and other
//! [`MobHandle::fork_member`](super::MobHandle::fork_member) forks, and
//! temporary-council participants forked from a convener's member. Delegate
//! helpers, live-delegation workers and ordinary spawns have their own spec
//! and never carry it.
//!
//! The child keeps its own roster, comms and runtime identity. Only the build
//! inputs are the source's: none of the standard mob member labels, which name
//! a member (see [`crate::build::STANDARD_MOB_MEMBER_LABEL_KEYS`]), passes from
//! the source to the child.
//!
//! # Rebuilds
//!
//! The inputs are fixed when the child is seated and every later rebuild of
//! the child (warm revival, explicit resume, process-restart restore) repeats
//! them from the child's own durable records, so a rebuilt child is built as
//! it was first built, whatever the source has become since:
//!
//! - `fork_source` is persisted with the child's `MemberSpawned` event and
//!   roster entry;
//! - the inherited labels are the child's own persisted roster labels;
//! - the application context is in the child session's durable build state,
//!   which a rebuild that supplies no context carries forward.
//!
//! The per-spawn tool overlay is a process-local dispatcher and cannot be
//! persisted. A rebuild re-derives it from the source instead: while the
//! source is seated in the child's mob, the child gets the source's current
//! retained overlay (after a restart or explicit resume, the one the host's
//! spawn customizer re-supplied for the source). A temporary-council
//! participant's source lives in another mob, so the participant keeps the
//! overlay it retained from its seating, which is its source's. When the
//! source is gone there is no source overlay left, and the child keeps the
//! overlay of its own rebuild recipe; that fallback is logged, never silent.

use std::collections::BTreeMap;
use std::sync::Arc;

use meerkat_core::types::SessionId;
use meerkat_core::{AgentToolDispatcher, ForkBuildSource};

use crate::ids::{AgentIdentity, MobId};
use crate::roster::RosterEntry;

/// Build inputs a fork-derived member inherits from its source member.
///
/// Minted only by the source member's own mob, from its roster entry, its
/// retained per-spawn overlay and the source session's durable build state
/// (see [`MobHandle::fork_build_inheritance`](super::MobHandle::fork_build_inheritance)).
/// A caller therefore cannot name a source it did not fork from. The value is
/// opaque: the runtime applies it to the child's spawn and it is never edited.
///
/// Applying it to a child spawn sets the child's typed fork source and fills
/// the child's application context, labels and per-spawn tool overlay from the
/// source. A value the child's own spawn request states explicitly is kept: the
/// source supplies the default, the request is the override. For labels that
/// holds per key.
#[derive(Clone)]
pub struct ForkBuildInheritance {
    source: ForkBuildSource,
    app_context: Option<serde_json::Value>,
    labels: BTreeMap<String, String>,
    external_tools: Option<Arc<dyn AgentToolDispatcher>>,
}

impl ForkBuildInheritance {
    /// `labels` are the source's roster labels. The standard mob member
    /// labels among them are dropped here: they name the source (in MobKit,
    /// `agent_identity` is the source's durable identity), so a child that
    /// inherited them would claim to be its source.
    pub(crate) fn new(
        source: ForkBuildSource,
        app_context: Option<serde_json::Value>,
        mut labels: BTreeMap<String, String>,
        external_tools: Option<Arc<dyn AgentToolDispatcher>>,
    ) -> Self {
        labels.retain(|key, _| !crate::build::is_standard_mob_member_label(key));
        Self {
            source,
            app_context,
            labels,
            external_tools,
        }
    }

    /// The typed source the child's build will carry.
    pub fn source(&self) -> &ForkBuildSource {
        &self.source
    }

    /// The application context of the source's current build, verbatim.
    pub fn app_context(&self) -> Option<&serde_json::Value> {
        self.app_context.as_ref()
    }

    /// The source's application labels: its roster labels without the
    /// standard mob member labels (`mob_id`, `role`, `profile_name`,
    /// `meerkat_id`, `agent_identity`), which name the source itself. The
    /// child's roster entry carries only what the child's own spawn request
    /// states for those keys, never the source's values, and the child's build
    /// stamps them from the child's own member binding.
    pub fn labels(&self) -> &BTreeMap<String, String> {
        &self.labels
    }

    /// Whether the source carries a retained per-spawn tool overlay.
    pub fn has_external_tools(&self) -> bool {
        self.external_tools.is_some()
    }

    /// Whether this inheritance was minted for the fork of exactly
    /// `source_identity`'s session `source_session_id`.
    pub(crate) fn names_fork_of(
        &self,
        source_identity: &AgentIdentity,
        source_session_id: &SessionId,
    ) -> bool {
        self.source.source_member.member == source_identity.as_str()
            && &self.source.source_session_id == source_session_id
    }

    /// Apply to the child's spawn request (see the type docs).
    pub(crate) fn apply_to(self, spec: &mut super::handle::SpawnMemberSpec) {
        let Self {
            source,
            app_context,
            labels,
            external_tools,
        } = self;
        spec.fork_source = Some(source);
        if spec.context.is_none() {
            spec.context = app_context;
        }
        if !labels.is_empty() {
            let mut merged = labels;
            if let Some(requested) = spec.labels.take() {
                merged.extend(requested);
            }
            spec.labels = Some(merged);
        }
        if spec.external_tools.is_none() {
            spec.external_tools = external_tools;
        }
    }
}

/// The member `fork_source` names, when that source is a member of mob
/// `mob_id`: the member a rebuild of the fork-derived member re-derives its
/// per-spawn overlay from. A source in another mob (a temporary-council
/// participant's convener member) is not.
pub(crate) fn fork_source_in_mob(
    fork_source: &ForkBuildSource,
    mob_id: &MobId,
) -> Option<AgentIdentity> {
    (fork_source.source_member.mob_id == mob_id.as_str())
        .then(|| AgentIdentity::from(fork_source.source_member.member.as_str()))
}

/// Order `entries` so that every fork-derived member comes after the member
/// it was forked from, when that source is among `entries`, keeping the input
/// order otherwise. A restore that rebuilds members one by one then has each
/// source's overlay before it rebuilds the source's forks.
pub(crate) fn order_fork_sources_first(entries: &mut [RosterEntry], mob_id: &MobId) {
    let sources: BTreeMap<AgentIdentity, Option<AgentIdentity>> = entries
        .iter()
        .map(|entry| {
            (
                entry.agent_identity.clone(),
                entry
                    .fork_source
                    .as_ref()
                    .and_then(|source| fork_source_in_mob(source, mob_id)),
            )
        })
        .collect();
    // Hops to the first ancestor outside `entries`, bounded by the entry
    // count so a malformed cycle cannot loop.
    let depth = |identity: &AgentIdentity| {
        let mut depth = 0_usize;
        let mut current = identity;
        while let Some(Some(source)) = sources.get(current) {
            if !sources.contains_key(source) || depth >= sources.len() {
                break;
            }
            depth += 1;
            current = source;
        }
        depth
    };
    let depths: BTreeMap<AgentIdentity, usize> = sources
        .keys()
        .map(|identity| (identity.clone(), depth(identity)))
        .collect();
    entries.sort_by_key(|entry| depths.get(&entry.agent_identity).copied().unwrap_or(0));
}

impl super::actor::MobActor {
    /// The per-spawn tool overlay a rebuild of `entry` composes, given `own`,
    /// the overlay the rebuild recipe carries for `entry` itself.
    ///
    /// An ordinary member gets `own`. A fork-derived member is rebuilt with
    /// its source's inputs (see the module docs): while its source is seated
    /// in this mob, the source's current retained overlay (`None` when the
    /// source has none, exactly like the source). A source in another mob (a
    /// temporary-council participant's convener member) is not this mob's to
    /// read, so the member gets `own`, the overlay it retained from its
    /// seating (its source's). A source that is gone left no overlay to
    /// re-derive from, so the member gets `own` too, and that is logged.
    pub(super) async fn fork_rebuild_overlay(
        &self,
        entry: &RosterEntry,
        own: Option<Arc<dyn AgentToolDispatcher>>,
    ) -> Option<Arc<dyn AgentToolDispatcher>> {
        let Some(fork_source) = entry.fork_source.as_ref() else {
            return own;
        };
        let Some(source) = fork_source_in_mob(fork_source, &self.definition.id) else {
            return own;
        };
        if self.roster.read().await.get(&source).is_some() {
            return self
                .per_spawn_external_tools
                .read()
                .await
                .get(&source)
                .cloned();
        }
        tracing::warn!(
            mob_id = %self.definition.id,
            agent_identity = %entry.agent_identity,
            source_mob_id = %fork_source.source_member.mob_id,
            source_member = %fork_source.source_member.member,
            own_overlay = own.is_some(),
            "fork-derived member's source is no longer seated in its mob; its rebuild keeps \
             its own per-spawn overlay instead of the source's (fork_source, labels and \
             application context are still the source's)"
        );
        own
    }
}

impl std::fmt::Debug for ForkBuildInheritance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ForkBuildInheritance")
            .field("source", &self.source)
            .field("app_context", &self.app_context.is_some())
            .field("labels", &self.labels)
            .field("external_tools", &self.external_tools.is_some())
            .finish()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::ids::ProfileName;
    use crate::runtime::SpawnMemberSpec;

    struct NoTools;

    #[async_trait::async_trait]
    impl AgentToolDispatcher for NoTools {
        fn tools(&self) -> Arc<[Arc<meerkat_core::ToolDef>]> {
            Vec::new().into()
        }

        async fn dispatch(
            &self,
            call: meerkat_core::ToolCallView<'_>,
        ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
            Err(meerkat_core::ToolError::not_found(call.name))
        }
    }

    fn source() -> ForkBuildSource {
        ForkBuildSource::new(
            meerkat_core::MobMemberBinding {
                mob_id: "home".to_string(),
                role: "domain".to_string(),
                member: "domain-calendar".to_string(),
            },
            SessionId::new(),
        )
    }

    fn inheritance(tools: Option<Arc<dyn AgentToolDispatcher>>) -> ForkBuildInheritance {
        ForkBuildInheritance::new(
            source(),
            Some(serde_json::json!({"domain": "calendar"})),
            BTreeMap::from([
                ("domain".to_string(), "calendar".to_string()),
                ("tier".to_string(), "gold".to_string()),
            ]),
            tools,
        )
    }

    #[test]
    fn a_bare_child_spec_takes_every_source_input() {
        let tools: Arc<dyn AgentToolDispatcher> = Arc::new(NoTools);
        let inheritance = inheritance(Some(Arc::clone(&tools)));
        let expected_source = inheritance.source().clone();
        let mut spec = SpawnMemberSpec::new(ProfileName::from("domain"), "child");

        inheritance.apply_to(&mut spec);

        assert_eq!(spec.fork_source, Some(expected_source));
        assert_eq!(
            spec.context,
            Some(serde_json::json!({"domain": "calendar"}))
        );
        assert_eq!(
            spec.labels,
            Some(BTreeMap::from([
                ("domain".to_string(), "calendar".to_string()),
                ("tier".to_string(), "gold".to_string()),
            ]))
        );
        assert!(
            spec.external_tools
                .as_ref()
                .is_some_and(|applied| Arc::ptr_eq(applied, &tools)),
            "the child carries the source's exact overlay dispatcher"
        );
    }

    #[test]
    fn explicit_child_inputs_override_the_source_defaults() {
        let source_tools: Arc<dyn AgentToolDispatcher> = Arc::new(NoTools);
        let child_tools: Arc<dyn AgentToolDispatcher> = Arc::new(NoTools);
        let mut spec = SpawnMemberSpec::new(ProfileName::from("domain"), "child");
        spec.context = Some(serde_json::json!({"domain": "override"}));
        spec.labels = Some(BTreeMap::from([("tier".to_string(), "silver".to_string())]));
        spec.external_tools = Some(Arc::clone(&child_tools));

        inheritance(Some(source_tools)).apply_to(&mut spec);

        assert_eq!(
            spec.context,
            Some(serde_json::json!({"domain": "override"}))
        );
        assert_eq!(
            spec.labels,
            Some(BTreeMap::from([
                ("domain".to_string(), "calendar".to_string()),
                ("tier".to_string(), "silver".to_string()),
            ])),
            "the source supplies unlabelled keys; the request wins per key"
        );
        assert!(
            spec.external_tools
                .as_ref()
                .is_some_and(|applied| Arc::ptr_eq(applied, &child_tools))
        );
        assert!(spec.fork_source.is_some());
    }

    /// Regression (identity confusion): MobKit stamps its durable identity
    /// into a member's roster labels (`agent_identity: "domain:calendar"`,
    /// `profile_name`), and meerkat's standard labels name a member too. A
    /// fork child that inherited them claimed its source's identity.
    #[test]
    fn inherited_labels_drop_every_standard_mob_member_label() {
        let mut source_labels = BTreeMap::from([
            ("domain".to_string(), "calendar".to_string()),
            ("tier".to_string(), "gold".to_string()),
        ]);
        for key in crate::build::STANDARD_MOB_MEMBER_LABEL_KEYS {
            source_labels.insert(key.to_string(), format!("source-{key}"));
        }
        source_labels.insert("agent_identity".to_string(), "domain:calendar".to_string());
        let inheritance = ForkBuildInheritance::new(source(), None, source_labels, None);
        let app_labels = BTreeMap::from([
            ("domain".to_string(), "calendar".to_string()),
            ("tier".to_string(), "gold".to_string()),
        ]);
        assert_eq!(inheritance.labels(), &app_labels);

        let mut bare = SpawnMemberSpec::new(ProfileName::from("domain"), "child");
        inheritance.clone().apply_to(&mut bare);
        assert_eq!(
            bare.labels,
            Some(app_labels.clone()),
            "the child's roster labels carry none of the source's member-naming labels"
        );

        let mut own = SpawnMemberSpec::new(ProfileName::from("domain"), "child");
        own.labels = Some(BTreeMap::from([(
            "agent_identity".to_string(),
            "domain:calendar-fork".to_string(),
        )]));
        inheritance.apply_to(&mut own);
        let mut expected = app_labels;
        expected.insert(
            "agent_identity".to_string(),
            "domain:calendar-fork".to_string(),
        );
        assert_eq!(
            own.labels,
            Some(expected),
            "a value the child's own request states for a standard key is the child's"
        );
    }

    #[test]
    fn names_fork_of_requires_the_exact_source_member_and_session() {
        let inheritance = inheritance(None);
        let session = inheritance.source().source_session_id.clone();
        assert!(inheritance.names_fork_of(&AgentIdentity::from("domain-calendar"), &session));
        assert!(!inheritance.names_fork_of(&AgentIdentity::from("domain-gmail"), &session));
        assert!(
            !inheritance.names_fork_of(&AgentIdentity::from("domain-calendar"), &SessionId::new())
        );
    }
}
