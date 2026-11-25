//! One console read's members: [`MemberCut`].
//!
//! A console read decides every identity record and frame it touches
//! against each runtime's members as listed once in that read: the
//! membership a read uses is the snapshot it takes, the primary mob's
//! members when the read first needs them and the nested mobs' when a
//! decision first reads them. A read is one identities read, one timeline
//! page (so one page of a replay) or one live frame. A member hidden while a
//! read runs is decided as that read's snapshot listed it, and is hidden
//! from the next read, which lists the members again.
//!
//! The decisions apply the same predicates, to the same Core inputs, that
//! [`console_identity_record_visible`], [`resolved_member_visible`] and
//! [`capture_member_lineage`] read for themselves on every call: only how
//! often Core's member list, and each member's console record, are read
//! changes. Those helpers are the same decisions over a cut of their own.

use super::*;

/// The members of one runtime entry for one read (see the module docs), in
/// two listings each taken on first use and then kept: the primary mob's
/// members, and the members of the mobs nested under it (the implicit
/// delegates among them), which only a decision that reads them lists.
pub(super) struct MemberCut {
    entry: RuntimeEntry,
    primary: tokio::sync::OnceCell<Vec<CutMember>>,
    delegates: tokio::sync::OnceCell<Vec<CutMember>>,
}

/// One listed member, with what the read derives from it on first use.
pub(super) struct CutMember {
    pub(super) resolved: ResolvedConsoleMember,
    /// [`identity_record_for_resolved_member`] of the member.
    record: tokio::sync::OnceCell<Option<ConsoleIdentityRecord>>,
    /// [`resolved_member_visible_in`] of the member with its own record.
    visible: tokio::sync::OnceCell<bool>,
}

impl MemberCut {
    pub(super) fn new(entry: &RuntimeEntry) -> Self {
        Self {
            entry: entry.clone(),
            primary: tokio::sync::OnceCell::new(),
            delegates: tokio::sync::OnceCell::new(),
        }
    }

    /// The primary mob's members, listed on the read's first use.
    async fn primary(&self) -> &[CutMember] {
        self.primary
            .get_or_init(|| async {
                Box::pin(primary_member_sources(&self.entry))
                    .await
                    .into_iter()
                    .map(CutMember::new)
                    .collect()
            })
            .await
    }

    /// The nested mobs' members, listed on the read's first use.
    async fn delegates(&self) -> &[CutMember] {
        self.delegates
            .get_or_init(|| async {
                Box::pin(delegate_member_sources(&self.entry))
                    .await
                    .into_iter()
                    .map(CutMember::new)
                    .collect()
            })
            .await
    }

    /// Every member, hidden ones included, as
    /// [`member_sources_for_entry_including_hidden`] lists them: the primary
    /// mob's members, then the nested mobs'.
    pub(super) async fn members(&self) -> impl Iterator<Item = &CutMember> {
        let primary = self.primary().await;
        primary.iter().chain(self.delegates().await)
    }

    /// Whether the entry's policy lists any member of a nested mob.
    fn lists_nested(&self) -> bool {
        self.entry.visibility_policy.include_nested_mob_members()
    }

    /// The members [`member_sources_for_entry`] lists: the primary mob's,
    /// then the members of the nested mobs the console lists that the
    /// policy's mob-level switches do not hide
    /// ([`policy_hides_nested_member`]). Under a policy that lists no nested
    /// member the nested mobs are not listed for it at all, and a council
    /// participant is among them on no policy (see `council`).
    pub(super) async fn unhidden(&self) -> impl Iterator<Item = &CutMember> {
        let primary = self.primary().await;
        let nested: &[CutMember] = if self.lists_nested() {
            self.delegates().await
        } else {
            &[]
        };
        let policy = self.entry.visibility_policy.clone();
        let primary_mob_id = self.entry.runtime.handle().mob_id().to_string();
        let nested = nested.iter().filter(move |member| {
            member.resolved.listed
                && !member.resolved.council_participant
                && !nested_member_unnamed(&member.resolved)
                && !policy_hides_nested_member(
                    policy.as_ref(),
                    &primary_mob_id,
                    &member.resolved.source_mob_id,
                    member.resolved.member.role.as_str(),
                )
        });
        primary.iter().chain(nested)
    }

    /// Whether the listings [`Self::unhidden`] reads are taken already.
    pub(super) fn unhidden_listed(&self) -> bool {
        self.primary.initialized() && (!self.lists_nested() || self.delegates.initialized())
    }
}

impl CutMember {
    fn new(resolved: ResolvedConsoleMember) -> Self {
        Self {
            resolved,
            record: tokio::sync::OnceCell::new(),
            visible: tokio::sync::OnceCell::new(),
        }
    }

    /// The member's console record, built on the read's first use.
    pub(super) async fn record(&self) -> Option<&ConsoleIdentityRecord> {
        self.record
            .get_or_init(|| Box::pin(identity_record_for_resolved_member(&self.resolved)))
            .await
            .as_ref()
    }

    /// [`resolved_member_visible_in`] of the member with its own record,
    /// decided on the read's first use. `None` for a member with no record.
    async fn own_record_visible(&self, cut: &MemberCut) -> Option<bool> {
        let record = self.record().await?;
        Some(
            *self
                .visible
                .get_or_init(|| Box::pin(resolved_member_visible_in(cut, &self.resolved, record)))
                .await,
        )
    }
}

/// What one read decides frames with (see the module docs): each identity's
/// visibility as the read first decided it, and each runtime entry's cut.
#[derive(Default)]
pub(super) struct FrameVisibilityRead {
    pub(super) identities: HashMap<(String, String), CachedIdentityVisibility>,
    cuts: HashMap<String, Arc<MemberCut>>,
    /// [`council::council_frames_of_source`] of each (runtime key, captured
    /// source mob) the read decided.
    council_sources: HashMap<(String, String), council::CouncilFrames>,
}

impl FrameVisibilityRead {
    /// Whether the stored `frame` of `entry`'s runtime, with its captured
    /// `provenance`, was written by a council participant. The frame's
    /// source mob is decided once per read.
    pub(super) async fn council_wrote_frame(
        &mut self,
        entry: &RuntimeEntry,
        frame: &ConsoleFrame,
        provenance: &ConsoleFrameMemberProvenance,
    ) -> bool {
        let key = (entry.runtime_key.clone(), provenance.source_mob_id.clone());
        if let Some(decided) = self.council_sources.get(&key) {
            return decided.wrote(frame, provenance);
        }
        let decided = Box::pin(council::council_frames_of_source(
            entry,
            &provenance.source_mob_id,
        ))
        .await;
        let wrote = decided.wrote(frame, provenance);
        self.council_sources.insert(key, decided);
        wrote
    }

    /// Whether the read decided a frame of an identity the identity read
    /// model does not know.
    pub(super) fn decided_unknown_identity(&self) -> bool {
        self.identities
            .values()
            .any(|visibility| matches!(visibility, CachedIdentityVisibility::Missing))
    }

    /// The read's cut of `entry`: one per registration of a runtime key.
    pub(super) fn cut(&mut self, entry: &RuntimeEntry) -> Arc<MemberCut> {
        if let Some(cut) = self.cuts.get(entry.runtime_key.as_str())
            && cut.entry.registration_id == entry.registration_id
        {
            return Arc::clone(cut);
        }
        let cut = Arc::new(MemberCut::new(entry));
        self.cuts
            .insert(entry.runtime_key.clone(), Arc::clone(&cut));
        cut
    }
}

/// [`console_identity_record_visible`] decided against `cut`.
pub(super) async fn console_identity_record_visible_in(
    cut: &MemberCut,
    record: &ConsoleIdentityRecord,
) -> bool {
    if !cut.entry.visibility_policy.identity_visible(record) {
        return false;
    }
    let mut bound_live_member_seen = false;
    let mut wrong_live_projection_seen = false;
    for member in cut.members().await {
        // Associate the live row with this durable record by the approved
        // predicate, not by exact equality with the record's runtime_member_id.
        // That field holds the BINDING (an incarnation), while the row is now the
        // encoded durable identity, so exact equality never matched and a hidden
        // row stopped affecting its own record's visibility - which is how a
        // hidden member stayed inspectable.
        if !live_member_names_record(&member.resolved, record) {
            continue;
        }
        bound_live_member_seen = true;
        let Some(live_record) = member.record().await else {
            continue;
        };
        if live_record.identity != record.identity {
            wrong_live_projection_seen = true;
            continue;
        }
        if raw_resolved_member_visible(&member.resolved, live_record) {
            return true;
        }
    }
    if wrong_live_projection_seen {
        return true;
    }
    !bound_live_member_seen
}

/// Whether the live row `resolved` is a row of `record`. A primary member is
/// one by the identity-level predicate
/// ([`crate::member_comms_id::live_member_names_identity_or_incarnation`]). A
/// nested mob's member is the row of its own record alone, the one that
/// names its mob and name: its bare name never ties it to a record of the
/// primary mob, whatever name the two share.
pub(super) fn live_member_names_record(
    resolved: &ResolvedConsoleMember,
    record: &ConsoleIdentityRecord,
) -> bool {
    if resolved.nested() {
        return crate::console_member_ref::nested_member_identity(
            &resolved.source_mob_id,
            &resolved.runtime_identity,
        )
        .is_some_and(|name| name == record.runtime_member_id);
    }
    crate::member_comms_id::live_member_names_identity_or_incarnation(
        &resolved.runtime_identity,
        &record.identity,
        Some(record.runtime_member_id.as_str()),
    )
}

/// [`resolved_member_visible`] decided against `cut`, the read `resolved`
/// was listed in.
pub(super) async fn resolved_member_visible_in(
    cut: &MemberCut,
    resolved: &ResolvedConsoleMember,
    record: &ConsoleIdentityRecord,
) -> bool {
    if !raw_resolved_member_visible(resolved, record) {
        return false;
    }
    !Box::pin(live_record_shadowed_by_hidden_durable_binding_in(
        cut, resolved, record,
    ))
    .await
}

/// Whether a live record is shadowed by its durable identity's binding that
/// the policy hides, decided against `cut`, the read `resolved` was listed
/// in.
pub(super) async fn live_record_shadowed_by_hidden_durable_binding_in(
    cut: &MemberCut,
    resolved: &ResolvedConsoleMember,
    record: &ConsoleIdentityRecord,
) -> bool {
    let Some(identity_runtime) = resolved.identity_runtime() else {
        return false;
    };
    let Ok(identity) = crate::identity_first::AgentIdentity::parse(&record.identity) else {
        return false;
    };
    // Identity-level: `resolved.runtime_identity` is the DECODED roster member
    // id, which since the stable-identity lowering is the bare durable
    // identity. The identity's own roster row is never shadowed, so it needs
    // no read of the binding.
    if crate::member_comms_id::live_member_is_identity(
        &resolved.runtime_identity,
        identity.as_str(),
    ) {
        return false;
    }
    let status = match identity_runtime.status(&identity).await {
        Ok(status) => status,
        Err(crate::identity_first::IdentityRuntimeError::UnknownIdentity(_)) => return false,
        // A binding that cannot be read cannot clear the record: it stays
        // hidden, never visible and sendable for want of the read.
        Err(_) => return true,
    };
    // A binding must exist for this status to shadow anything.
    if status.agent_runtime_id.is_none() {
        return false;
    }
    for candidate in cut.unhidden().await {
        // Same identity-level rule: find the candidate whose roster identity IS
        // this status's durable identity, not one whose spelling matches an
        // AgentRuntimeId. A durable identity's row is a primary member's.
        if candidate.resolved.nested()
            || !crate::member_comms_id::live_member_is_identity(
                &candidate.resolved.runtime_identity,
                status.identity.as_str(),
            )
        {
            continue;
        }
        let Some(bound_record) = candidate.record().await else {
            continue;
        };
        if bound_record.identity == record.identity
            && !raw_resolved_member_visible(&candidate.resolved, bound_record)
        {
            return true;
        }
    }
    false
}

/// Whether `frame` belongs to a member the read's `cut` lists hidden and
/// none it lists visible: the members it names by session, runtime member
/// id or identity. A frame that sits under a bare name and was written by a
/// nested mob's member (it names that member's session, or its bare name
/// while no primary member has the name) is a hidden member's too: a nested
/// member is named by its mob and name, so such a frame predates that and
/// its bare name names a primary member only.
pub(super) async fn frame_matches_hidden_member_in(cut: &MemberCut, frame: &ConsoleFrame) -> bool {
    let runtime_member_id = strip_namespace(&frame.identity, &cut.entry.identity_namespace)
        .unwrap_or_else(|| frame.identity.clone());
    let frame_session_id = frame.session_id.as_deref();
    let mut saw_hidden = false;
    let mut saw_visible = false;
    let mut named_bare_by_a_nested_member = false;
    for member in cut.members().await {
        let Some(record) = member.record().await else {
            continue;
        };
        let session_matches =
            frame_session_id.is_some() && record.session_id.as_deref() == frame_session_id;
        let runtime_matches = record.runtime_member_id == runtime_member_id;
        let identity_matches = record.identity == frame.identity;
        if !session_matches && !runtime_matches && !identity_matches {
            if member.resolved.nested() && member.resolved.runtime_identity == runtime_member_id {
                named_bare_by_a_nested_member = true;
            }
            continue;
        }
        if session_matches && !raw_resolved_member_visible(&member.resolved, record) {
            return true;
        }
        if session_matches && member.resolved.nested() && !runtime_matches && !identity_matches {
            return true;
        }
        if member.own_record_visible(cut).await == Some(true) {
            saw_visible = true;
        } else {
            saw_hidden = true;
        }
    }
    (saw_hidden || named_bare_by_a_nested_member) && !saw_visible
}

/// [`capture_member_lineage`] read from `cut`, the read `resolved` was
/// listed in.
pub(super) async fn capture_member_lineage_in(
    cut: &MemberCut,
    resolved: &ResolvedConsoleMember,
    record: &ConsoleIdentityRecord,
) -> Vec<ConsoleFrameLineageLink> {
    let namespace = resolved.entry.identity_namespace.as_str();
    let mut lineage = vec![lineage_link_for_record(
        record,
        namespace,
        resolved.member.role.as_str(),
        resolved.member.labels.get("role").map(String::as_str),
    )];
    let mut visited = BTreeSet::from([record.identity.clone()]);
    let mut child_labels = record.labels.clone();
    while lineage.len() < CAPTURED_LINEAGE_DEPTH {
        let Some((parent, bound_session)) = crate::console_spawn::lineage_parent(&child_labels)
            .map(|parent| {
                (
                    parent.identity.to_string(),
                    parent.bound_session.map(str::to_string),
                )
            })
        else {
            break;
        };
        let parent_identity = apply_namespace(&parent, namespace);
        if !visited.insert(parent_identity.clone()) {
            break;
        }
        let mut confirmed = None;
        if let Some(bound_session) = bound_session.as_deref() {
            for candidate in cut.members().await {
                let roster_id = candidate.resolved.member.agent_identity.as_str();
                let alias_name = crate::member_comms_id::runtime_alias_str(roster_id);
                let alias: &str = alias_name.as_ref();
                let resolved_identity = crate::member_comms_id::resolve_member_identity(
                    &candidate.resolved.handle,
                    &candidate.resolved.member.agent_identity,
                    &candidate.resolved.member.labels,
                );
                let durable = resolved_identity.as_deref().unwrap_or(alias);
                // A nested mob's member is also named by its mob and name:
                // the spelling the spawn registry gives a nested spawner.
                let console_name = candidate.resolved.console_name();
                if roster_id != parent
                    && alias != parent
                    && durable != parent
                    && console_name.as_deref() != Some(parent.as_str())
                {
                    continue;
                }
                let Some(candidate_record) = candidate.record().await else {
                    continue;
                };
                if candidate_record.session_id.as_deref() == Some(bound_session) {
                    confirmed = Some((
                        candidate_record.clone(),
                        candidate.resolved.member.role.to_string(),
                        candidate.resolved.member.labels.get("role").cloned(),
                    ));
                    break;
                }
            }
        }
        match confirmed {
            Some((parent_record, role, own_role_label)) => {
                lineage.push(lineage_link_for_record(
                    &parent_record,
                    namespace,
                    &role,
                    own_role_label.as_deref(),
                ));
                child_labels = parent_record.labels;
            }
            None => {
                lineage.push(ConsoleFrameLineageLink {
                    identity: parent_identity,
                    agent_id: None,
                    role: None,
                    labels: None,
                });
                break;
            }
        }
    }
    lineage
}

/// [`capture_member_provenance`] with its lineage read from `cut`.
pub(super) async fn capture_member_provenance_in(
    inner: &AggregatorInner,
    cut: &MemberCut,
    resolved: &ResolvedConsoleMember,
    record: &ConsoleIdentityRecord,
) -> ConsoleFrameMemberProvenance {
    let lineage = Box::pin(capture_member_lineage_in(cut, resolved, record)).await;
    retain_member_provenance_with_lineage(inner, resolved, record, lineage)
}

/// Test seam: how often each runtime registration's members were listed.
#[cfg(test)]
pub(super) mod listings {
    use std::collections::HashMap;
    use std::sync::{LazyLock, Mutex, PoisonError};

    /// One of a runtime's two member listings.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub(in crate::console_aggregator) enum Listing {
        /// The primary mob's members.
        Primary,
        /// The members of the mobs nested under the primary mob.
        Delegates,
    }

    static LISTINGS: LazyLock<Mutex<HashMap<(uuid::Uuid, Listing), usize>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));

    /// One `listing` of the members of `registration_id`'s runtime.
    pub(in crate::console_aggregator) fn record(registration_id: uuid::Uuid, listing: Listing) {
        *LISTINGS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry((registration_id, listing))
            .or_default() += 1;
    }

    /// The `listing`s of `registration_id`'s runtime so far.
    pub(in crate::console_aggregator) fn count(
        registration_id: uuid::Uuid,
        listing: Listing,
    ) -> usize {
        LISTINGS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&(registration_id, listing))
            .copied()
            .unwrap_or_default()
    }
}
