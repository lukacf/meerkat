//! One console read's members: [`MemberCut`].
//!
//! A console read decides every identity record and frame it touches
//! against each runtime's members as listed once in that read: the
//! membership a read uses is the snapshot it takes, the primary mob's
//! members when the read first needs them and the implicit delegates' when
//! a decision first reads them. A read is one identities read, one timeline
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
/// members, and the implicit delegates' members, which only a decision that
/// reads them lists.
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

    /// The implicit delegates' members, listed on the read's first use.
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

    /// Whether the entry's policy lists the implicit delegates' members.
    fn lists_delegates(&self) -> bool {
        self.entry
            .visibility_policy
            .include_implicit_delegate_members()
    }

    /// Every member, hidden ones included, as
    /// [`member_sources_for_entry_including_hidden`] lists them: the primary
    /// mob's members, then the implicit delegates'.
    pub(super) async fn members(&self) -> impl Iterator<Item = &CutMember> {
        let primary = self.primary().await;
        primary.iter().chain(self.delegates().await)
    }

    /// The members [`member_sources_for_entry`] lists: the primary mob's,
    /// then the implicit delegates' only while the policy includes them. The
    /// delegates are not listed for it otherwise.
    pub(super) async fn unhidden(&self) -> impl Iterator<Item = &CutMember> {
        let primary = self.primary().await;
        let delegates: &[CutMember] = if self.lists_delegates() {
            self.delegates().await
        } else {
            &[]
        };
        primary.iter().chain(delegates)
    }

    /// Whether the listings [`Self::unhidden`] reads are taken already.
    pub(super) fn unhidden_listed(&self) -> bool {
        self.primary.initialized() && (!self.lists_delegates() || self.delegates.initialized())
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
}

impl FrameVisibilityRead {
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
        if !crate::member_comms_id::live_member_names_identity_or_incarnation(
            &member.resolved.runtime_identity,
            &record.identity,
            Some(record.runtime_member_id.as_str()),
        ) {
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
    let Some(identity_runtime) = resolved.entry.identity_runtime.as_ref() else {
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
        // AgentRuntimeId.
        if !crate::member_comms_id::live_member_is_identity(
            &candidate.resolved.runtime_identity,
            status.identity.as_str(),
        ) {
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
/// id or identity.
pub(super) async fn frame_matches_hidden_member_in(cut: &MemberCut, frame: &ConsoleFrame) -> bool {
    let runtime_member_id = strip_namespace(&frame.identity, &cut.entry.identity_namespace)
        .unwrap_or_else(|| frame.identity.clone());
    let frame_session_id = frame.session_id.as_deref();
    let mut saw_hidden = false;
    let mut saw_visible = false;
    for member in cut.members().await {
        let Some(record) = member.record().await else {
            continue;
        };
        let session_matches =
            frame_session_id.is_some() && record.session_id.as_deref() == frame_session_id;
        let runtime_matches = record.runtime_member_id == runtime_member_id;
        let identity_matches = record.identity == frame.identity;
        if !session_matches && !runtime_matches && !identity_matches {
            continue;
        }
        if session_matches && !raw_resolved_member_visible(&member.resolved, record) {
            return true;
        }
        if member.own_record_visible(cut).await == Some(true) {
            saw_visible = true;
        } else {
            saw_hidden = true;
        }
    }
    saw_hidden && !saw_visible
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
    )];
    let mut visited = BTreeSet::from([record.identity.clone()]);
    let mut child_labels = record.labels.clone();
    while lineage.len() < CAPTURED_LINEAGE_DEPTH {
        let Some(parent) = child_labels
            .get(crate::console_spawn::SPAWNED_BY_LABEL)
            .map(|parent| parent.trim().to_string())
            .filter(|parent| !parent.is_empty())
        else {
            break;
        };
        let parent_identity = apply_namespace(&parent, namespace);
        if !visited.insert(parent_identity.clone()) {
            break;
        }
        let bound_session = child_labels
            .get(crate::console_spawn::SPAWNED_BY_SESSION_LABEL)
            .map(|session| session.trim().to_string())
            .filter(|session| !session.is_empty());
        let mut confirmed = None;
        if let Some(bound_session) = bound_session.as_deref() {
            for candidate in cut.members().await {
                let roster_id = candidate.resolved.member.agent_identity.as_str();
                let alias_name = crate::member_comms_id::runtime_alias_str(roster_id);
                let alias: &str = alias_name.as_ref();
                let durable = crate::member_comms_id::durable_identity_label(
                    &candidate.resolved.member.labels,
                )
                .unwrap_or(alias);
                if roster_id != parent && alias != parent && durable != parent {
                    continue;
                }
                let Some(candidate_record) = candidate.record().await else {
                    continue;
                };
                if candidate_record.session_id.as_deref() == Some(bound_session) {
                    confirmed = Some((
                        candidate_record.clone(),
                        candidate.resolved.member.role.to_string(),
                    ));
                    break;
                }
            }
        }
        match confirmed {
            Some((parent_record, role)) => {
                lineage.push(lineage_link_for_record(&parent_record, namespace, &role));
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
        /// The implicit delegate mobs' members.
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
