//! What a fork-derived member inherits from its source member's build.
//!
//! A durable fork copies its source's transcript, but not the per-build inputs
//! the source's host resolved its tools and instructions from: the source's
//! application context, its application labels, and its retained per-spawn
//! tool overlay. A child built without them is a different agent: a calendar
//! fork was once built as a generic domain member, missing most of the
//! source's tools (including its calendar tools and the `memory` tool), with a
//! tools block that differed from the forker's, so the child could not reuse
//! the forker's cached prefix either.
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
//! The host's spawn customizer does not run on these rebuilds either
//! ([`rebuild_resume_spec`]): no customizer ran at the child's seating, so a
//! customizer that rewrote the child's labels, application context, auth
//! binding, tool access policy, inherited tool filter or profile and model
//! overrides on a restart would build a different agent (for a host that
//! resolves tools by labels and context, other tools and a lost cache prefix)
//! from the one that was seated. Instructions and the system prompt are not
//! among them: no resume rebuild re-authors those. The
//! one thing a rebuild may take from the customizer is the per-spawn overlay
//! for the child's own identity, and only where the overlay rule below asks
//! for it ([`customizer_own_identity_overlay`]); everything else the customizer
//! returns for that request is discarded.
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
//!   customizer re-supplies an ordinary member's overlay for its own
//!   identity): a child seated with its source's overlay gets the overlay its
//!   source is rebuilt with, restored before the child; a child seated with
//!   the fork caller's overlay gets the customizer's overlay for its own
//!   identity, the only way that overlay can come back.
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
//! - **Ancestor gone or replaced**: the rule is transitive. A grandchild seated
//!   with its source's overlay got, through that source, the overlay of every
//!   in-mob ancestor the source itself followed when the grandchild was
//!   forked. It follows its source only while each of those ancestors is still
//!   the build its fork was taken from, walking `fork_source` up the chain
//!   within the mob until a member whose overlay was its own. Each fork
//!   persists which it got ([`ForkOverlayOrigin::Source`]: an overlay its
//!   source had itself inherited; [`ForkOverlayOrigin::SourceOwn`]: its
//!   source's own, which is an ordinary member's, a fork caller's, a
//!   temporary-council participant's, or the own-identity overlay of a fork
//!   already rebuilt because its own source was gone), so the walk stops where
//!   the overlay entered the chain at the time of that fork, not where the
//!   ancestor's seating-time overlay came from. When a link up the chain is
//!   broken the grandchild is rebuilt like a child whose own source is gone,
//!   and the warning also names the ancestor whose source is missing.
//!
//! A temporary-council participant's source lives in another mob, which this
//! mob cannot observe: in process the participant keeps the overlay it was
//! seated with (its source's); after a restart or explicit resume it gets the
//! customizer's overlay for its own identity.

use std::collections::{BTreeMap, BTreeSet};
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
    /// its source's retained overlay (or with none, like its source), and the
    /// source had itself inherited that overlay from its own in-mob source: a
    /// rebuild of the member follows its source only while that source's own
    /// link up the chain is intact too. Also the decoding of a journal that
    /// does not say which.
    #[default]
    Source,
    /// The fork caller put an overlay on the member's spawn request, and the
    /// member was seated with that overlay instead of its source's.
    Caller,
    /// The member's spawn request stated no overlay, so it was seated with
    /// its source's retained overlay, and that overlay was the source's own:
    /// the source was an ordinary member, a fork seated with its caller's
    /// overlay, a fork whose source is in another mob, or a fork already
    /// rebuilt onto the overlay for its own identity because its own source
    /// was gone or replaced. A rebuild of the member follows its source while
    /// the source is the build it was forked from, whatever became of the
    /// source's own ancestors.
    SourceOwn,
}

impl ForkOverlayOrigin {
    /// Whether this is the default, [`Self::Source`] (serde skips it).
    pub fn is_source(&self) -> bool {
        matches!(self, Self::Source)
    }
}

/// Where a member's retained per-spawn overlay (the one it was last built
/// with, which a fork of it inherits) came from. Process-local, like the
/// overlay; a fork of the member persists it as the fork's
/// [`ForkOverlayOrigin::Source`] or [`ForkOverlayOrigin::SourceOwn`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RetainedOverlayOrigin {
    /// The member's own overlay: an ordinary member's, a fork caller's, a
    /// temporary-council participant's (its source is outside this mob), or
    /// the overlay for its own identity a fork was rebuilt onto because its
    /// source, or an ancestor its overlay came through, was gone or replaced.
    Own,
    /// The overlay the member's in-mob source passed down to it: it was seated
    /// with its source's overlay, or rebuilt following its source.
    Inherited,
}

impl RetainedOverlayOrigin {
    /// The origin of the overlay a member is seated with.
    pub(crate) fn at_seating(
        fork_source: Option<&ForkBuildSource>,
        fork_overlay: ForkOverlayOrigin,
        mob_id: &MobId,
    ) -> Self {
        let from_in_mob_source = fork_source
            .and_then(|source| fork_source_in_mob(source, mob_id))
            .is_some();
        if from_in_mob_source && fork_overlay != ForkOverlayOrigin::Caller {
            Self::Inherited
        } else {
            Self::Own
        }
    }
}

/// Every member's retained per-spawn overlay and where it came from, written
/// only by the mob actor (and the restart restore that seeds it) and shared
/// read-only with its handles.
#[derive(Default)]
pub(crate) struct RetainedOverlays {
    tools: BTreeMap<AgentIdentity, Arc<dyn AgentToolDispatcher>>,
    /// Members whose retained overlay (or retained absence of one) is
    /// [`RetainedOverlayOrigin::Inherited`]; every other member's is its own.
    inherited: BTreeSet<AgentIdentity>,
}

impl RetainedOverlays {
    /// `identity`'s retained overlay, if it has one.
    pub(crate) fn get(&self, identity: &AgentIdentity) -> Option<&Arc<dyn AgentToolDispatcher>> {
        self.tools.get(identity)
    }

    /// Where `identity`'s retained overlay came from.
    pub(crate) fn origin(&self, identity: &AgentIdentity) -> RetainedOverlayOrigin {
        if self.inherited.contains(identity) {
            RetainedOverlayOrigin::Inherited
        } else {
            RetainedOverlayOrigin::Own
        }
    }

    /// Record `overlay` (`None`: no overlay) as the one `identity` was
    /// (re)built with, from `origin`.
    pub(crate) fn retain(
        &mut self,
        identity: &AgentIdentity,
        overlay: Option<Arc<dyn AgentToolDispatcher>>,
        origin: RetainedOverlayOrigin,
    ) {
        match overlay {
            Some(tools) => {
                self.tools.insert(identity.clone(), tools);
            }
            None => {
                self.tools.remove(identity);
            }
        }
        match origin {
            RetainedOverlayOrigin::Inherited => {
                self.inherited.insert(identity.clone());
            }
            RetainedOverlayOrigin::Own => {
                self.inherited.remove(identity);
            }
        }
    }

    /// Forget `identity`'s overlay (its lifetime ended), releasing the host
    /// dispatcher.
    pub(crate) fn remove(&mut self, identity: &AgentIdentity) {
        self.tools.remove(identity);
        self.inherited.remove(identity);
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
    pub(crate) creation_source: Option<crate::MemberCreationSourceWitness>,
    app_context: Option<serde_json::Value>,
    labels: BTreeMap<String, String>,
    external_tools: Option<Arc<dyn AgentToolDispatcher>>,
    /// Where the source's retained overlay came from: what a child seated
    /// with it persists as its [`ForkOverlayOrigin`].
    external_tools_origin: RetainedOverlayOrigin,
}

impl ForkBuildInheritance {
    /// `labels` are the source's roster labels. The standard mob member
    /// labels among them are dropped here: they name the source (in MobKit,
    /// `agent_identity` is the source's durable identity), so a child that
    /// inherited them would claim to be its source. `external_tools` is the
    /// source's retained overlay, from `external_tools_origin`.
    pub(crate) fn new(
        source: ForkBuildSource,
        app_context: Option<serde_json::Value>,
        mut labels: BTreeMap<String, String>,
        external_tools: Option<Arc<dyn AgentToolDispatcher>>,
        external_tools_origin: RetainedOverlayOrigin,
    ) -> Self {
        labels.retain(|key, _| !crate::build::is_standard_mob_member_label(key));
        Self {
            source,
            creation_source: None,
            app_context,
            labels,
            external_tools,
            external_tools_origin,
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
            creation_source,
            app_context,
            labels,
            external_tools,
            external_tools_origin,
        } = self;
        spec.fork_source = Some(source);
        spec.creation_origin = creation_source.map_or(
            crate::member_creation::MemberCreationOrigin::Unproven,
            crate::member_creation::MemberCreationOrigin::Source,
        );
        spec.fork_overlay = match (spec.external_tools.is_some(), external_tools_origin) {
            (true, _) => ForkOverlayOrigin::Caller,
            (false, RetainedOverlayOrigin::Inherited) => ForkOverlayOrigin::Source,
            (false, RetainedOverlayOrigin::Own) => ForkOverlayOrigin::SourceOwn,
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

/// Why a fork-derived member's link to the member it was forked from is
/// broken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ForkLinkBroken {
    /// No member with the source's identity is seated in the mob: the source
    /// was retired.
    Gone,
    /// A member with the source's identity is seated, but bound to another
    /// session than the one the fork was taken from (a respawn, a repoint to a
    /// successor session, or a reuse of the identity): another build.
    OtherSession { current: Option<SessionId> },
}

impl ForkLinkBroken {
    fn current_session(&self) -> Option<&SessionId> {
        match self {
            Self::OtherSession { current } => current.as_ref(),
            Self::Gone => None,
        }
    }
}

/// Why a rebuild of a member seated with its source's overlay cannot follow
/// that source any more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ForkSourceUnavailable {
    /// The member's own source is gone or is another build.
    Source(ForkLinkBroken),
    /// The member's source is still the build it was forked from, but the
    /// overlay that source passed on came to it from further up the in-mob
    /// fork chain, and a link there is broken: `ancestor` (the source, or one
    /// of its own in-mob ancestors) followed its source's overlay, and that
    /// source is gone or is another build (`link`).
    Ancestor {
        ancestor: AgentIdentity,
        link: ForkLinkBroken,
    },
    /// Restart restore only: the source is seated with the forked session,
    /// but this restore did not rebuild it (it kept its live session), so the
    /// restore has no record of the source's overlay.
    NotRebuilt,
}

impl ForkSourceUnavailable {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Source(ForkLinkBroken::Gone) => "source_gone",
            Self::Source(ForkLinkBroken::OtherSession { .. }) => "source_other_session",
            Self::Ancestor {
                link: ForkLinkBroken::Gone,
                ..
            } => "ancestor_source_gone",
            Self::Ancestor {
                link: ForkLinkBroken::OtherSession { .. },
                ..
            } => "ancestor_source_other_session",
            Self::NotRebuilt => "source_not_rebuilt",
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

/// Whether `link` (a fork-derived member's `fork_source`, naming in-mob
/// member `source`) still leads to the build the fork was taken from: `source`
/// is seated and bound to the forked session. The seated source's roster entry
/// when it does.
fn follow_fork_link<'r>(
    source: &AgentIdentity,
    link: &ForkBuildSource,
    seated: &impl Fn(&AgentIdentity) -> Option<&'r RosterEntry>,
    machine_state: &mob_dsl::MobMachineState,
) -> Result<&'r RosterEntry, ForkLinkBroken> {
    let Some(source_entry) = seated(source) else {
        return Err(ForkLinkBroken::Gone);
    };
    let current = machine_session_binding(source, machine_state);
    if current.as_ref() != Some(&link.source_session_id) {
        return Err(ForkLinkBroken::OtherSession { current });
    }
    Ok(source_entry)
}

/// The in-mob member whose overlay `entry` followed when it was built, with
/// the fork link to it, if its overlay came from its source: `None` for an
/// ordinary member, a fork seated with its caller's overlay, and a fork whose
/// source is in another mob (a temporary-council participant), whose overlays
/// are their own.
fn followed_link<'e>(
    entry: &'e RosterEntry,
    mob_id: &MobId,
) -> Option<(AgentIdentity, &'e ForkBuildSource)> {
    if entry.fork_overlay == ForkOverlayOrigin::Caller {
        return None;
    }
    let link = entry.fork_source.as_ref()?;
    fork_source_in_mob(link, mob_id).map(|source| (source, link))
}

/// The rule a rebuild of `entry` follows. `seated` looks an identity up in
/// the mob's roster; `machine_state` is the mob machine's current state (its
/// session bindings decide whether a seated source is still the build the
/// member was forked from).
///
/// A member seated with its source's overlay follows the source only while
/// the source, and every in-mob ancestor the source's own overlay came
/// through, is still the build its fork was taken from (see the module docs).
/// Each member's persisted [`ForkOverlayOrigin`] says how far up the chain
/// its overlay came from: the walk crosses the link to a member's source, and
/// stops above it when the member was seated with that source's own overlay
/// ([`ForkOverlayOrigin::SourceOwn`]).
pub(crate) fn fork_overlay_rule<'r>(
    entry: &RosterEntry,
    mob_id: &MobId,
    seated: impl Fn(&AgentIdentity) -> Option<&'r RosterEntry>,
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
    let source_entry = match follow_fork_link(&source, fork_source, &seated, machine_state) {
        Ok(source_entry) => source_entry,
        Err(link) => {
            return ForkOverlayRule::SourceUnavailable(ForkSourceUnavailable::Source(link));
        }
    };
    // Walk the chain the source's overlay came down, while the member below
    // took an overlay its source had itself inherited. `visited` bounds the
    // walk should a malformed lineage ever loop.
    let mut visited = BTreeSet::from([entry.agent_identity.clone()]);
    let mut below = entry.fork_overlay;
    let mut current = source_entry;
    while below == ForkOverlayOrigin::Source && visited.insert(current.agent_identity.clone()) {
        let Some((next, link)) = followed_link(current, mob_id) else {
            break;
        };
        match follow_fork_link(&next, link, &seated, machine_state) {
            Ok(next_entry) => {
                below = current.fork_overlay;
                current = next_entry;
            }
            Err(link) => {
                return ForkOverlayRule::SourceUnavailable(ForkSourceUnavailable::Ancestor {
                    ancestor: current.agent_identity.clone(),
                    link,
                });
            }
        }
    }
    ForkOverlayRule::FollowSource(source)
}

/// Warn that a rebuild of fork-derived member `entry` does not follow its
/// source's overlay, naming the missing source (member and forked session)
/// and why (`reason`). When the member's own source is another build,
/// `current_source_session_id` is that source's current session. When the
/// broken link is further up the chain, `ancestor_member` names the ancestor
/// whose own source is gone or is another build, and
/// `ancestor_current_source_session_id` that source's current session.
pub(crate) fn warn_fork_source_unavailable(
    mob_id: &MobId,
    entry: &RosterEntry,
    reason: &ForkSourceUnavailable,
    own_overlay: bool,
) {
    let Some(fork_source) = entry.fork_source.as_ref() else {
        return;
    };
    const MESSAGE: &str = "fork-derived member's rebuild cannot follow its source's per-spawn \
                           overlay (see reason); it composes the spawn customizer's overlay for \
                           its own identity instead (fork_source, labels and application context \
                           are still the ones it was seated with)";
    match reason {
        ForkSourceUnavailable::Ancestor { ancestor, link } => tracing::warn!(
            mob_id = %mob_id,
            agent_identity = %entry.agent_identity,
            source_mob_id = %fork_source.source_member.mob_id,
            source_member = %fork_source.source_member.member,
            source_session_id = %fork_source.source_session_id,
            reason = reason.as_str(),
            ancestor_member = %ancestor,
            ancestor_current_source_session_id = ?link.current_session(),
            own_overlay,
            "{MESSAGE}"
        ),
        ForkSourceUnavailable::Source(link) => tracing::warn!(
            mob_id = %mob_id,
            agent_identity = %entry.agent_identity,
            source_mob_id = %fork_source.source_member.mob_id,
            source_member = %fork_source.source_member.member,
            source_session_id = %fork_source.source_session_id,
            current_source_session_id = ?link.current_session(),
            reason = reason.as_str(),
            own_overlay,
            "{MESSAGE}"
        ),
        ForkSourceUnavailable::NotRebuilt => tracing::warn!(
            mob_id = %mob_id,
            agent_identity = %entry.agent_identity,
            source_mob_id = %fork_source.source_member.mob_id,
            source_member = %fork_source.source_member.member,
            source_session_id = %fork_source.source_session_id,
            reason = reason.as_str(),
            own_overlay,
            "{MESSAGE}"
        ),
    }
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
            |identity| roster.get(identity),
            self.dsl_authority.state(),
        )
    }

    /// The per-spawn overlay a warm revival of `entry` (bound to
    /// `session_id`) composes.
    ///
    /// The member's retained overlay, which is the one it was built with,
    /// unless it was seated with its source's overlay and that source, or an
    /// in-mob ancestor the source's overlay came through, is gone or is
    /// another build: then the spawn customizer's overlay for the member's own
    /// identity, which also replaces the retained one, so a retired source's
    /// host dispatcher is no longer held for the member.
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
                let own = customizer_own_identity_overlay(
                    &self.definition.id,
                    self.spawn_member_customizer.as_ref(),
                    entry,
                    session_id,
                )?;
                warn_fork_source_unavailable(&self.definition.id, entry, &reason, own.is_some());
                self.retain_rebuild_overlay(
                    &entry.agent_identity,
                    own.as_ref(),
                    RetainedOverlayOrigin::Own,
                )
                .await;
                Ok(own)
            }
        }
    }

    /// The per-spawn overlay an explicit-resume rebuild of `entry` on
    /// `session_id` composes, given `spec`, its rebuild request
    /// ([`rebuild_resume_spec`]), and where it comes from. Sources are rebuilt
    /// before their forks in the same pass, so a source's retained overlay is
    /// the one it is rebuilt with.
    pub(super) async fn recustomized_rebuild_overlay(
        &self,
        entry: &RosterEntry,
        session_id: &SessionId,
        spec: &super::handle::SpawnMemberSpec,
    ) -> Result<(Option<Arc<dyn AgentToolDispatcher>>, RetainedOverlayOrigin), MobError> {
        let own = || {
            rebuild_own_overlay(
                &self.definition.id,
                self.spawn_member_customizer.as_ref(),
                entry,
                session_id,
                spec,
            )
        };
        let overlay = match self.fork_overlay_rule(entry).await {
            ForkOverlayRule::Own => own()?,
            ForkOverlayRule::Caller => {
                let own = own()?;
                if own.is_none() {
                    warn_caller_overlay_not_resupplied(&self.definition.id, entry);
                }
                own
            }
            ForkOverlayRule::FollowSource(source) => {
                let inherited = self
                    .per_spawn_external_tools
                    .read()
                    .await
                    .get(&source)
                    .cloned();
                return Ok((inherited, RetainedOverlayOrigin::Inherited));
            }
            ForkOverlayRule::SourceUnavailable(reason) => {
                let own = own()?;
                warn_fork_source_unavailable(&self.definition.id, entry, &reason, own.is_some());
                own
            }
        };
        Ok((overlay, RetainedOverlayOrigin::Own))
    }

    /// Record `overlay` (from `origin`) as the one `identity` was (re)built
    /// with: what a later warm revival of the member composes and what a fork
    /// of it inherits.
    pub(super) async fn retain_rebuild_overlay(
        &self,
        identity: &AgentIdentity,
        overlay: Option<&Arc<dyn AgentToolDispatcher>>,
        origin: RetainedOverlayOrigin,
    ) {
        self.per_spawn_external_tools
            .write()
            .await
            .retain(identity, overlay.cloned(), origin);
    }
}

/// The context the host's spawn customizer is asked a rebuild's resume
/// request `spec` in.
fn resume_customization_context(
    mob_id: &MobId,
    spec: &super::handle::SpawnMemberSpec,
) -> super::handle::SpawnCustomizationContext {
    super::handle::SpawnCustomizationContext {
        mob_id: mob_id.clone(),
        spawn_source: super::handle::SpawnSource::Resume,
        spawner_identity: None,
        spawner_runtime_id: None,
        requested_profile: spec.role_name.clone(),
    }
}

/// The resume request a rebuild (process-restart restore, explicit resume) of
/// `entry` on its bound session `session_id` builds from.
///
/// An ordinary member's request is [`member_resume_spec`] as the host's spawn
/// `customizer` makes it (`SpawnSource::Resume`), unchanged. A fork-derived
/// member's (one carrying `fork_source`: a fork child or a local
/// temporary-council participant) is [`member_resume_spec`] itself, its own
/// durable records: no customizer ran at its seating, so none runs on its
/// rebuilds, which repeat its first build (see the module docs). Its
/// per-spawn overlay is the overlay rule's ([`rebuild_own_overlay`] when the
/// rule asks for its own).
///
/// The customizer is asked through
/// [`SpawnMemberCustomizer::customize_resume`](super::handle::SpawnMemberCustomizer::customize_resume)
/// with `durable`, what the member's session (`durable.session_id`) holds.
pub(crate) fn rebuild_resume_spec(
    mob_id: &MobId,
    customizer: Option<&Arc<dyn super::handle::SpawnMemberCustomizer>>,
    entry: &RosterEntry,
    durable: &super::handle::ResumedMemberView,
) -> Result<super::handle::SpawnMemberSpec, MobError> {
    let mut spec = member_resume_spec(entry, &durable.session_id);
    if entry.fork_source.is_some() {
        return Ok(spec);
    }
    if let Some(customizer) = customizer {
        customizer.customize_resume(
            &resume_customization_context(mob_id, &spec),
            durable,
            &mut spec,
        )?;
    }
    Ok(spec)
}

/// What `session_id` holds for a resume rebuild: one read of its persisted
/// metadata. A failed read is a typed error, never an absent identity.
///
/// Built in its own frame: the restore and explicit-resume loops that await
/// it are among the largest debug poll frames in the crate, and the inline
/// metadata view would add to every one of them.
pub(crate) fn load_resumed_member_view<'a>(
    session_service: &'a dyn super::session_service::MobSessionService,
    session_id: &'a SessionId,
) -> meerkat_runtime::stack_relief::OwnFrameFuture<
    'a,
    Result<super::handle::ResumedMemberView, MobError>,
> {
    meerkat_runtime::stack_relief::box_in_own_frame(move || {
        read_resumed_member_view(session_service, session_id)
    })
}

async fn read_resumed_member_view(
    session_service: &dyn super::session_service::MobSessionService,
    session_id: &SessionId,
) -> Result<super::handle::ResumedMemberView, MobError> {
    let metadata = session_service
        .load_persisted_session_metadata(session_id)
        .await
        .map_err(MobError::SessionError)?;
    Ok(super::handle::ResumedMemberView::new(
        session_id.clone(),
        metadata
            .and_then(|view| view.session_metadata)
            .map(|metadata| metadata.llm_identity()),
    ))
}

/// The provider-params override the customizer gives `entry`'s rebuild on a
/// successor session, asked again with the successor's `durable` view after a
/// restore or explicit resume moved the member off its absent bound session.
pub(crate) fn successor_resume_provider_params(
    mob_id: &MobId,
    customizer: Option<&Arc<dyn super::handle::SpawnMemberCustomizer>>,
    entry: &RosterEntry,
    durable: &super::handle::ResumedMemberView,
) -> Result<Option<meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>, MobError> {
    Ok(rebuild_resume_spec(mob_id, customizer, entry, durable)?.resume_provider_params)
}

/// Apply a resume rebuild's field-scoped provider-params override to the
/// profile it builds with: the value becomes the resumed identity's provider
/// params (`ResumeOverrideField::ProviderParams`), persisted with it.
pub(crate) fn apply_resume_provider_params(
    profile: &mut crate::profile::Profile,
    params: Option<&meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
) {
    let Some(params) = params else {
        return;
    };
    profile.provider_params = Some(params.clone());
    if !profile
        .resume_overrides
        .contains(&crate::profile::ResumeOverrideField::ProviderParams)
    {
        profile
            .resume_overrides
            .push(crate::profile::ResumeOverrideField::ProviderParams);
    }
}

/// The host spawn customizer's per-spawn overlay for `entry`'s own identity,
/// asked exactly as a rebuild asks it (a `Resume` of the member's bound
/// session `session_id`). Everything else the customizer puts on that request
/// is discarded. `None` without a customizer.
pub(crate) fn customizer_own_identity_overlay(
    mob_id: &MobId,
    customizer: Option<&Arc<dyn super::handle::SpawnMemberCustomizer>>,
    entry: &RosterEntry,
    session_id: &SessionId,
) -> Result<Option<Arc<dyn AgentToolDispatcher>>, MobError> {
    let Some(customizer) = customizer else {
        return Ok(None);
    };
    let mut spec = member_resume_spec(entry, session_id);
    customizer.customize_spawn(&resume_customization_context(mob_id, &spec), &mut spec)?;
    Ok(spec.external_tools)
}

/// A rebuild's overlay for `entry`'s own identity, where the overlay rule asks
/// for it, given `spec`, the rebuild request [`rebuild_resume_spec`] made: an
/// ordinary member's is the overlay the customizer put on its request; a
/// fork-derived member's request is not customized, so its own overlay is
/// [`customizer_own_identity_overlay`], asked only now.
pub(crate) fn rebuild_own_overlay(
    mob_id: &MobId,
    customizer: Option<&Arc<dyn super::handle::SpawnMemberCustomizer>>,
    entry: &RosterEntry,
    session_id: &SessionId,
    spec: &super::handle::SpawnMemberSpec,
) -> Result<Option<Arc<dyn AgentToolDispatcher>>, MobError> {
    if entry.fork_source.is_some() {
        customizer_own_identity_overlay(mob_id, customizer, entry, session_id)
    } else {
        Ok(spec.external_tools.clone())
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
            .field("external_tools_origin", &self.external_tools_origin)
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
            RetainedOverlayOrigin::Own,
        )
    }

    /// A child seated with its source's overlay persists whether that overlay
    /// was the source's own or one the source had itself inherited: the
    /// source's current overlay, not its seating-time lineage, is what the
    /// child got.
    #[test]
    fn the_child_records_where_its_sources_overlay_came_from() {
        for (origin, expected) in [
            (RetainedOverlayOrigin::Own, ForkOverlayOrigin::SourceOwn),
            (RetainedOverlayOrigin::Inherited, ForkOverlayOrigin::Source),
        ] {
            let mut inheritance = inheritance(None);
            inheritance.external_tools_origin = origin;
            let mut bare = SpawnMemberSpec::new(ProfileName::from("domain"), "child");
            inheritance.clone().apply_to(&mut bare);
            assert_eq!(bare.fork_overlay, expected, "{origin:?}");

            let mut own = SpawnMemberSpec::new(ProfileName::from("domain"), "child");
            own.external_tools = Some(Arc::new(NoTools));
            inheritance.apply_to(&mut own);
            assert_eq!(own.fork_overlay, ForkOverlayOrigin::Caller, "{origin:?}");
        }
    }

    #[test]
    fn retained_overlays_track_origin_independently_of_the_overlay() {
        let identity = AgentIdentity::from("fork-child");
        let mut retained = RetainedOverlays::default();
        assert_eq!(retained.origin(&identity), RetainedOverlayOrigin::Own);
        retained.retain(&identity, None, RetainedOverlayOrigin::Inherited);
        assert!(retained.get(&identity).is_none());
        assert_eq!(retained.origin(&identity), RetainedOverlayOrigin::Inherited);
        retained.retain(
            &identity,
            Some(Arc::new(NoTools)),
            RetainedOverlayOrigin::Own,
        );
        assert!(retained.get(&identity).is_some());
        assert_eq!(retained.origin(&identity), RetainedOverlayOrigin::Own);
        retained.retain(&identity, None, RetainedOverlayOrigin::Inherited);
        retained.remove(&identity);
        assert!(retained.get(&identity).is_none());
        assert_eq!(retained.origin(&identity), RetainedOverlayOrigin::Own);
    }

    #[test]
    fn seating_origin_is_inherited_only_from_an_in_mob_source_overlay() {
        let home = MobId::from("home");
        let other = MobId::from("convener");
        let source = source();
        for (fork_source, fork_overlay, mob_id, expected) in [
            (
                None,
                ForkOverlayOrigin::Source,
                &home,
                RetainedOverlayOrigin::Own,
            ),
            (
                Some(&source),
                ForkOverlayOrigin::Source,
                &home,
                RetainedOverlayOrigin::Inherited,
            ),
            (
                Some(&source),
                ForkOverlayOrigin::SourceOwn,
                &home,
                RetainedOverlayOrigin::Inherited,
            ),
            (
                Some(&source),
                ForkOverlayOrigin::Caller,
                &home,
                RetainedOverlayOrigin::Own,
            ),
            (
                Some(&source),
                ForkOverlayOrigin::Source,
                &other,
                RetainedOverlayOrigin::Own,
            ),
        ] {
            assert_eq!(
                RetainedOverlayOrigin::at_seating(fork_source, fork_overlay, mob_id),
                expected,
                "{fork_source:?} {fork_overlay:?} {mob_id}"
            );
        }
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
        let inheritance = ForkBuildInheritance::new(
            source(),
            None,
            source_labels,
            None,
            RetainedOverlayOrigin::Own,
        );
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

    /// A seated roster entry for `member` with `fork_source`.
    fn roster_entry(member: &str, fork_source: Option<ForkBuildSource>) -> RosterEntry {
        let identity = AgentIdentity::from(member);
        let mut roster = crate::roster::Roster::new();
        roster.add(crate::roster::RosterAddEntry {
            agent_identity: identity.clone(),
            generation: crate::ids::Generation::INITIAL,
            fence_token: crate::ids::FenceToken::new(1),
            agent_runtime_id: crate::ids::AgentRuntimeId::initial(identity.clone()),
            role: ProfileName::from("domain"),
            runtime_mode: crate::MobRuntimeMode::TurnDriven,
            member_ref: crate::event::MemberRef::from_bridge_session_id(SessionId::new()),
            peer_id: None,
            transport_public_key: None,
            direct_member_fence: None,
            labels: BTreeMap::from([("domain".to_string(), "calendar".to_string())]),
            effective_profile_override: None,
            effective_model_override: None,
            spawned_by: None,
            fork_job: None,
            fork_source,
            fork_overlay: ForkOverlayOrigin::Source,
        });
        roster.get(&identity).cloned().expect("the entry is seated")
    }

    /// Rewrites every input of a resume request it can reach, for the
    /// member's own identity.
    struct RewritingCustomizer;

    impl super::super::handle::SpawnMemberCustomizer for RewritingCustomizer {
        fn customize_spawn(
            &self,
            ctx: &super::super::handle::SpawnCustomizationContext,
            spec: &mut SpawnMemberSpec,
        ) -> Result<(), MobError> {
            assert_eq!(ctx.spawn_source, super::super::handle::SpawnSource::Resume);
            spec.labels
                .get_or_insert_with(BTreeMap::new)
                .insert("rewritten".to_string(), spec.identity.to_string());
            spec.context = Some(serde_json::json!({"rewritten_for": spec.identity.as_str()}));
            spec.additional_instructions = Some(vec!["record memories".to_string()]);
            spec.model_override = Some("rewritten-model".to_string());
            spec.external_tools = Some(Arc::new(NoTools));
            Ok(())
        }
    }

    #[test]
    fn only_an_ordinary_members_rebuild_request_is_customized() {
        let mob_id = MobId::from("home");
        let customizer: Arc<dyn super::super::handle::SpawnMemberCustomizer> =
            Arc::new(RewritingCustomizer);
        let session = SessionId::new();

        let ordinary = roster_entry("domain-gmail", None);
        let spec = rebuild_resume_spec(
            &mob_id,
            Some(&customizer),
            &ordinary,
            &super::super::handle::ResumedMemberView::new(session.clone(), None),
        )
        .unwrap();
        assert_eq!(
            spec.context,
            Some(serde_json::json!({"rewritten_for": "domain-gmail"}))
        );
        assert!(spec.labels.as_ref().unwrap().contains_key("rewritten"));
        assert_eq!(spec.model_override.as_deref(), Some("rewritten-model"));
        assert!(
            rebuild_own_overlay(&mob_id, Some(&customizer), &ordinary, &session, &spec)
                .unwrap()
                .is_some(),
            "an ordinary member's own overlay is the one on its customized request"
        );

        let council_source = ForkBuildSource::new(
            meerkat_core::MobMemberBinding {
                mob_id: "convener".to_string(),
                role: "domain".to_string(),
                member: "domain-calendar".to_string(),
            },
            SessionId::new(),
        );
        for (member, fork_source) in [
            ("fork-child", source()),
            ("council-participant", council_source),
        ] {
            let entry = roster_entry(member, Some(fork_source));
            let spec = rebuild_resume_spec(
                &mob_id,
                Some(&customizer),
                &entry,
                &super::super::handle::ResumedMemberView::new(session.clone(), None),
            )
            .unwrap();
            let own_records = member_resume_spec(&entry, &session);
            assert_eq!(spec.labels, own_records.labels, "'{member}' labels");
            assert_eq!(spec.labels, Some(entry.labels.clone()));
            assert_eq!(spec.context, None, "'{member}' context is carried forward");
            assert_eq!(
                spec.additional_instructions, None,
                "'{member}' instructions"
            );
            assert_eq!(spec.model_override, None, "'{member}' model override");
            assert!(spec.external_tools.is_none(), "'{member}' overlay");
            assert!(
                rebuild_own_overlay(&mob_id, Some(&customizer), &entry, &session, &spec)
                    .unwrap()
                    .is_some(),
                "'{member}' can still take the customizer's overlay for its own identity"
            );
            assert!(
                rebuild_own_overlay(&mob_id, None, &entry, &session, &spec)
                    .unwrap()
                    .is_none(),
                "'{member}' has no own overlay without a customizer"
            );
        }
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
