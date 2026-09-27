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
//! source member to the child's seating build. These seatings carry it:
//!
//! - `fork_off` children, and the children of
//!   [`MobHandle::fork_member`](super::MobHandle::fork_member),
//!   [`MobHandle::fork_member_then_run_bounded`](super::MobHandle::fork_member_then_run_bounded)
//!   and
//!   [`MobHandle::fork_member_then_run_detached`](super::MobHandle::fork_member_then_run_detached);
//! - local temporary-council participants, seated by
//!   [`MobHandle::spawn_attached_forked_participant`](super::MobHandle::spawn_attached_forked_participant)
//!   with an inheritance minted by
//!   [`MobHandle::fork_build_inheritance`](super::MobHandle::fork_build_inheritance).
//!
//! These do not: live-delegation workers
//! ([`MobHandle::fork_member_at_turn_boundary`](super::MobHandle::fork_member_at_turn_boundary)),
//! host-owned council participants (built on their member host, which the
//! in-process overlay cannot reach), respawn successors of a fork child (a new
//! incarnation), delegate helpers and ordinary spawns.
//!
//! The child keeps its own roster, comms and runtime identity. Only the build
//! inputs are the source's: none of the standard mob member labels, which name
//! a member (see [`crate::build::STANDARD_MOB_MEMBER_LABEL_KEYS`]), passes from
//! the source to the child. A host's spawn customizer does not run on these
//! seatings, so it cannot replace or wrap the inherited inputs.
//!
//! # Rebuilds
//!
//! The inputs are fixed when the child is seated, and every later rebuild of
//! the child (warm revival, explicit resume, process-restart restore) repeats
//! them from the child's own durable records:
//!
//! - `fork_source` is persisted with the child's `MemberSpawned` event and
//!   roster entry;
//! - the inherited labels are the child's own persisted roster labels;
//! - the application context is in the child session's durable build state,
//!   which a rebuild that supplies no context carries forward.
//!
//! The per-spawn tool overlay is a process-local dispatcher and cannot be
//! persisted. What is persisted is where the child's seated overlay came from
//! ([`ForkOverlayOrigin`]): the source's retained overlay, or an overlay the
//! fork caller put on the child's spawn request. The actor retains, for every
//! member, the overlay that member was last built with, and that is what a
//! fork of the member inherits. A rebuild of a fork child picks its overlay
//! like this:
//!
//! - **Warm revival** (in process): the child's retained overlay, which is the
//!   one it was built with.
//! - **Explicit resume and process-restart restore** (the host's spawn
//!   customizer re-supplies every member's overlay for its own identity): a
//!   child seated with its source's overlay gets the overlay its source is
//!   rebuilt with, restored before the child; a child seated with the fork
//!   caller's overlay gets the customizer's overlay for its own identity, the
//!   only way that overlay can come back.
//! - **Source gone or replaced**: a child seated with its source's overlay
//!   follows its source only while the source's identity is seated in the
//!   child's mob and bound to the session the child was forked from
//!   (`fork_source.source_session_id`). Once the source is retired, respawned,
//!   repointed to another session, or its identity is reused by another
//!   member, every rebuild of the child (in process or after a restart) gets
//!   the customizer's overlay for the child's own identity (none without a
//!   customizer), and a warning names the missing source. A retired source's
//!   host dispatcher is released from the child's retained overlay at that
//!   rebuild.
//!
//! A temporary-council participant's source lives in another mob, which this
//! mob cannot observe: in process the participant keeps the overlay it was
//! seated with (its source's); after a restart or explicit resume it gets the
//! customizer's overlay for its own identity.

use std::collections::BTreeMap;
use std::sync::Arc;

use meerkat_core::types::SessionId;
use meerkat_core::{AgentToolDispatcher, ForkBuildSource};
use serde::{Deserialize, Serialize};

use crate::error::MobError;
use crate::ids::{AgentIdentity, MobId};
use crate::machines::mob_machine as mob_dsl;
use crate::roster::RosterEntry;

/// Where a fork-derived member's per-spawn tool overlay came from when it
/// was seated.
///
/// Persisted with the member's `MemberSpawned` event and roster entry, next
/// to its `fork_source`, because the overlay itself is a process-local
/// dispatcher: after a restart or explicit resume this is what decides where
/// the member's overlay is re-derived from (see the module docs). Meaningful
/// only for a fork-derived member; every other member carries the default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ForkOverlayOrigin {
    /// The member's spawn request stated no overlay, so it was seated with
    /// its source's retained overlay (or with none, like its source).
    #[default]
    Source,
    /// The fork caller put an overlay on the member's spawn request, and the
    /// member was seated with that overlay instead of its source's.
    Caller,
}

impl ForkOverlayOrigin {
    /// Whether this is the default, [`Self::Source`] (serde skips it).
    pub fn is_source(&self) -> bool {
        matches!(self, Self::Source)
    }
}

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
        spec.fork_overlay = if spec.external_tools.is_some() {
            ForkOverlayOrigin::Caller
        } else {
            ForkOverlayOrigin::Source
        };
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

/// Order `items` so that every fork-derived member comes after the member it
/// was forked from, when that source is among `items`, keeping the input
/// order otherwise. A rebuild pass that rebuilds members one by one (restart
/// restore, explicit resume) then has each source's overlay before it
/// rebuilds the source's forks, and a fork's forks after the fork.
pub(crate) fn order_fork_sources_first<T>(
    items: &mut [T],
    mob_id: &MobId,
    entry: impl Fn(&T) -> &RosterEntry,
) {
    let sources: BTreeMap<AgentIdentity, Option<AgentIdentity>> = items
        .iter()
        .map(|item| {
            let entry = entry(item);
            (
                entry.agent_identity.clone(),
                entry
                    .fork_source
                    .as_ref()
                    .and_then(|source| fork_source_in_mob(source, mob_id)),
            )
        })
        .collect();
    // Hops to the first ancestor outside `items`, bounded by the item count
    // so a malformed cycle cannot loop.
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
    items.sort_by_key(|item| {
        depths
            .get(&entry(item).agent_identity)
            .copied()
            .unwrap_or(0)
    });
}

/// Why a rebuild of a member seated with its source's overlay cannot follow
/// that source any more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ForkSourceUnavailable {
    /// No member with the source's identity is seated in the mob: the source
    /// was retired.
    Gone,
    /// A member with the source's identity is seated, but bound to another
    /// session than the one the child was forked from (a respawn, a repoint
    /// to a successor session, or a reuse of the identity): another build.
    OtherSession { current: Option<SessionId> },
    /// Restart restore only: the source is seated with the forked session,
    /// but this restore did not rebuild it (it kept its live session), so the
    /// restore has no record of the source's overlay.
    NotRebuilt,
}

impl ForkSourceUnavailable {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Gone => "source_gone",
            Self::OtherSession { .. } => "source_other_session",
            Self::NotRebuilt => "source_not_rebuilt",
        }
    }

    fn current_session(&self) -> Option<&SessionId> {
        match self {
            Self::OtherSession { current } => current.as_ref(),
            Self::Gone | Self::NotRebuilt => None,
        }
    }
}

/// Which overlay a rebuild of `entry` composes (see the module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ForkOverlayRule {
    /// No source overlay to follow: an ordinary member, or a fork whose source
    /// lives in another mob (a temporary-council participant). The member's
    /// own overlay.
    Own,
    /// A fork seated with the fork caller's overlay. The member's own overlay:
    /// in process the retained one, after a restart or explicit resume the
    /// customizer's overlay for its own identity.
    Caller,
    /// A fork seated with its source's overlay, whose source is still the
    /// member and session it was forked from. In process its retained overlay
    /// (the one it was built with); after a restart or explicit resume the
    /// overlay its source was rebuilt with.
    FollowSource(AgentIdentity),
    /// A fork seated with its source's overlay whose source is gone or is
    /// another build. The customizer's overlay for its own identity.
    SourceUnavailable(ForkSourceUnavailable),
}

/// The source's current session binding, from the mob machine's
/// identity-to-session authority.
fn machine_session_binding(
    identity: &AgentIdentity,
    machine_state: &mob_dsl::MobMachineState,
) -> Option<SessionId> {
    machine_state
        .member_session_bindings
        .get(&mob_dsl::AgentIdentity::from_domain(identity))
        .and_then(|session_id| SessionId::parse(&session_id.0).ok())
}

/// The rule a rebuild of `entry` follows. `is_seated` reports whether an
/// identity is in the mob's roster; `machine_state` is the mob machine's
/// current state (its session bindings decide whether a seated source is
/// still the build the member was forked from).
pub(crate) fn fork_overlay_rule(
    entry: &RosterEntry,
    mob_id: &MobId,
    is_seated: impl Fn(&AgentIdentity) -> bool,
    machine_state: &mob_dsl::MobMachineState,
) -> ForkOverlayRule {
    let Some(fork_source) = entry.fork_source.as_ref() else {
        return ForkOverlayRule::Own;
    };
    let Some(source) = fork_source_in_mob(fork_source, mob_id) else {
        return ForkOverlayRule::Own;
    };
    if entry.fork_overlay == ForkOverlayOrigin::Caller {
        return ForkOverlayRule::Caller;
    }
    if !is_seated(&source) {
        return ForkOverlayRule::SourceUnavailable(ForkSourceUnavailable::Gone);
    }
    let current = machine_session_binding(&source, machine_state);
    if current.as_ref() != Some(&fork_source.source_session_id) {
        return ForkOverlayRule::SourceUnavailable(ForkSourceUnavailable::OtherSession { current });
    }
    ForkOverlayRule::FollowSource(source)
}

/// Warn that a rebuild of fork-derived member `entry` does not follow its
/// source's overlay, naming the missing source (member and forked session),
/// the source's current session when another one is bound, and why.
pub(crate) fn warn_fork_source_unavailable(
    mob_id: &MobId,
    entry: &RosterEntry,
    reason: &ForkSourceUnavailable,
    own_overlay: bool,
) {
    let Some(fork_source) = entry.fork_source.as_ref() else {
        return;
    };
    tracing::warn!(
        mob_id = %mob_id,
        agent_identity = %entry.agent_identity,
        source_mob_id = %fork_source.source_member.mob_id,
        source_member = %fork_source.source_member.member,
        source_session_id = %fork_source.source_session_id,
        current_source_session_id = ?reason.current_session(),
        reason = reason.as_str(),
        own_overlay,
        "fork-derived member's rebuild cannot follow its source's per-spawn overlay (see \
         reason); it composes the spawn customizer's overlay for its own identity instead \
         (fork_source, labels and application context are still the source's)"
    );
}

/// Warn that a rebuild of fork-derived member `entry`, seated with its fork
/// caller's overlay, gets no overlay back from the spawn customizer.
pub(crate) fn warn_caller_overlay_not_resupplied(mob_id: &MobId, entry: &RosterEntry) {
    let Some(fork_source) = entry.fork_source.as_ref() else {
        return;
    };
    tracing::warn!(
        mob_id = %mob_id,
        agent_identity = %entry.agent_identity,
        source_mob_id = %fork_source.source_member.mob_id,
        source_member = %fork_source.source_member.member,
        source_session_id = %fork_source.source_session_id,
        "fork-derived member was seated with its fork caller's per-spawn overlay, which is not \
         persisted, and the spawn customizer re-supplied none for its identity; it is rebuilt \
         without a per-spawn overlay"
    );
}

impl super::actor::MobActor {
    /// [`fork_overlay_rule`] against this actor's roster and machine state.
    pub(super) async fn fork_overlay_rule(&self, entry: &RosterEntry) -> ForkOverlayRule {
        let roster = self.roster.read().await;
        fork_overlay_rule(
            entry,
            &self.definition.id,
            |identity| roster.get(identity).is_some(),
            self.dsl_authority.state(),
        )
    }

    /// The per-spawn overlay a warm revival of `entry` (bound to
    /// `session_id`) composes.
    ///
    /// The member's retained overlay, which is the one it was built with,
    /// unless it was seated with its source's overlay and that source is gone
    /// or is another build: then the spawn customizer's overlay for the
    /// member's own identity, which also replaces the retained one, so a
    /// retired source's host dispatcher is no longer held for the member.
    pub(super) async fn warm_revival_overlay(
        &self,
        entry: &RosterEntry,
        session_id: &SessionId,
    ) -> Result<Option<Arc<dyn AgentToolDispatcher>>, MobError> {
        match self.fork_overlay_rule(entry).await {
            ForkOverlayRule::Own | ForkOverlayRule::Caller | ForkOverlayRule::FollowSource(_) => {
                Ok(self
                    .per_spawn_external_tools
                    .read()
                    .await
                    .get(&entry.agent_identity)
                    .cloned())
            }
            ForkOverlayRule::SourceUnavailable(reason) => {
                let own = self.own_identity_resume_overlay(entry, session_id)?;
                warn_fork_source_unavailable(&self.definition.id, entry, &reason, own.is_some());
                self.retain_rebuild_overlay(&entry.agent_identity, own.as_ref())
                    .await;
                Ok(own)
            }
        }
    }

    /// The per-spawn overlay an explicit-resume rebuild of `entry` composes,
    /// given `own`, the spawn customizer's overlay for its own identity.
    /// Sources are rebuilt before their forks in the same pass, so a source's
    /// retained overlay is the one it is rebuilt with.
    pub(super) async fn recustomized_rebuild_overlay(
        &self,
        entry: &RosterEntry,
        own: Option<Arc<dyn AgentToolDispatcher>>,
    ) -> Option<Arc<dyn AgentToolDispatcher>> {
        match self.fork_overlay_rule(entry).await {
            ForkOverlayRule::Own => own,
            ForkOverlayRule::Caller => {
                if own.is_none() {
                    warn_caller_overlay_not_resupplied(&self.definition.id, entry);
                }
                own
            }
            ForkOverlayRule::FollowSource(source) => self
                .per_spawn_external_tools
                .read()
                .await
                .get(&source)
                .cloned(),
            ForkOverlayRule::SourceUnavailable(reason) => {
                warn_fork_source_unavailable(&self.definition.id, entry, &reason, own.is_some());
                own
            }
        }
    }

    /// Record `overlay` as the one `identity` was (re)built with: what a
    /// later warm revival of the member composes and what a fork of it
    /// inherits.
    pub(super) async fn retain_rebuild_overlay(
        &self,
        identity: &AgentIdentity,
        overlay: Option<&Arc<dyn AgentToolDispatcher>>,
    ) {
        let mut retained = self.per_spawn_external_tools.write().await;
        match overlay {
            Some(tools) => {
                retained.insert(identity.clone(), Arc::clone(tools));
            }
            None => {
                retained.remove(identity);
            }
        }
    }

    /// The spawn customizer's per-spawn overlay for `entry`'s own identity,
    /// asked exactly as a restart restore asks it (a `Resume` of the member's
    /// bound session); `None` without a customizer.
    fn own_identity_resume_overlay(
        &self,
        entry: &RosterEntry,
        session_id: &SessionId,
    ) -> Result<Option<Arc<dyn AgentToolDispatcher>>, MobError> {
        let Some(customizer) = self.spawn_member_customizer.as_ref() else {
            return Ok(None);
        };
        let mut spec = member_resume_spec(entry, session_id);
        customizer.customize_spawn(
            &super::handle::SpawnCustomizationContext {
                mob_id: self.definition.id.clone(),
                spawn_source: super::handle::SpawnSource::Resume,
                spawner_identity: None,
                spawner_runtime_id: None,
                requested_profile: spec.role_name.clone(),
            },
            &mut spec,
        )?;
        Ok(spec.external_tools)
    }
}

/// The resume request a rebuild of `entry` on its bound session
/// `session_id` hands to the host's spawn customizer (restart restore,
/// explicit resume, and a fork child's own-identity overlay).
pub(crate) fn member_resume_spec(
    entry: &RosterEntry,
    session_id: &SessionId,
) -> super::handle::SpawnMemberSpec {
    let mut spec =
        super::handle::SpawnMemberSpec::new(entry.role.clone(), entry.agent_identity.clone());
    spec.launch_mode = crate::launch::MemberLaunchMode::Resume {
        bridge_session_id: session_id.clone(),
        resume_from_role: None,
    };
    spec.runtime_mode = Some(entry.runtime_mode);
    spec.labels = Some(entry.labels.clone());
    spec.override_profile = entry.effective_profile_override.clone();
    spec.model_override = entry.effective_model_override.clone();
    spec.spawned_by = entry.spawned_by.clone();
    spec.fork_job = entry.fork_job.clone();
    spec
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
