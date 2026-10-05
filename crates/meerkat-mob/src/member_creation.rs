//! Immutable creation facts carried by the canonical member-created journal.
//!
//! These facts prove runtime ancestry, not application permissions. Hosts must
//! intersect every edge with their current access policy. Fork endpoints remain
//! owned by `MemberSpawnedEvent::fork_source`.

use meerkat_core::{MobMemberBinding, SessionId, ops::ToolAccessPolicy};
use serde::{Deserialize, Serialize};

/// A runtime-issued creation token. Restoring a session preserves this token;
/// creating an unrelated member with the same display identity does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MemberCreationId(uuid::Uuid);

impl std::fmt::Display for MemberCreationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl MemberCreationId {
    pub(crate) fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

/// Exact parent facts captured by the source runtime before child activation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberCreationSource {
    pub session_id: SessionId,
    pub member_binding: MobMemberBinding,
    pub creation_id: MemberCreationId,
    /// Effective policy at delegation. `None` means unrestricted. `Inherit`
    /// is never a valid captured policy; resolve it before issuing a witness.
    pub tool_access_policy: Option<ToolAccessPolicy>,
}

/// Creation provenance. Missing old fields are unknown, never proven roots.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MemberCreationProvenance {
    Root,
    Spawn {
        source: MemberCreationSource,
    },
    /// The exact source binding/session is the event's existing `fork_source`.
    Fork {
        source_creation_id: MemberCreationId,
        source_tool_access_policy: Option<ToolAccessPolicy>,
    },
    /// Runtime-authorized respawn of the same logical member. Follow this
    /// exact predecessor to recover ancestry; this is not a delegation edge.
    Successor {
        predecessor_session_id: SessionId,
        predecessor_member_binding: MobMemberBinding,
        predecessor_creation_id: MemberCreationId,
    },
    #[default]
    LegacyUnknown,
}

/// Journal-owned creation identity and provenance.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberCreationRecord {
    /// Absent only for a legacy event without a runtime-issued creation token.
    pub creation_id: Option<MemberCreationId>,
    pub provenance: MemberCreationProvenance,
}

/// Historical exact-session binding. It remains readable after retirement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberCreationSnapshot {
    /// Canonical original member-created event cursor. Explicit successors
    /// retain this cursor; it is never reset by respawn or role migration.
    pub birth_cursor: u64,
    pub session_id: SessionId,
    pub member_binding: MobMemberBinding,
    pub creation: MemberCreationRecord,
    pub fork_source: Option<meerkat_core::ForkBuildSource>,
}

impl MemberCreationSnapshot {
    fn coherent(&self) -> bool {
        let Some(id) = self.creation.creation_id else {
            return self.creation.provenance == MemberCreationProvenance::LegacyUnknown;
        };
        if id.0.is_nil() || self.birth_cursor == 0 {
            return false;
        }
        let resolved = |policy: &Option<ToolAccessPolicy>| {
            policy.as_ref().is_none_or(|policy| {
                meerkat_core::ToolExecutionPolicy::resolve(policy.clone()).is_ok()
            })
        };
        match &self.creation.provenance {
            MemberCreationProvenance::Root => self.fork_source.is_none(),
            MemberCreationProvenance::Spawn { source } => {
                self.fork_source.is_none()
                    && source.session_id != self.session_id
                    && source.creation_id != id
                    && !source.creation_id.0.is_nil()
                    && resolved(&source.tool_access_policy)
            }
            MemberCreationProvenance::Fork {
                source_creation_id,
                source_tool_access_policy,
            } => {
                self.fork_source
                    .as_ref()
                    .is_some_and(|source| source.source_session_id != self.session_id)
                    && *source_creation_id != id
                    && !source_creation_id.0.is_nil()
                    && resolved(source_tool_access_policy)
            }
            MemberCreationProvenance::Successor {
                predecessor_session_id,
                predecessor_member_binding,
                predecessor_creation_id,
            } => {
                self.fork_source.is_none()
                    && predecessor_session_id != &self.session_id
                    && predecessor_creation_id == &id
                    && predecessor_member_binding.mob_id == self.member_binding.mob_id
                    && predecessor_member_binding.member == self.member_binding.member
            }
            MemberCreationProvenance::LegacyUnknown => true,
        }
    }
}

/// Sealed, process-local source witness issued by `MobHandle`. It cannot be
/// reconstructed from agent arguments or deserialized metadata.
#[derive(Debug, Clone)]
pub struct MemberCreationSourceWitness {
    pub(crate) source: Option<MemberCreationSource>,
    pub(crate) successor: bool,
}

impl MemberCreationSourceWitness {
    /// Explicitly record a delegated launch whose parent runtime cannot prove
    /// ancestry. The child may run, but cannot claim inherited authority.
    pub fn unavailable() -> Self {
        Self {
            source: None,
            successor: false,
        }
    }
}

/// A failed provenance read is not evidence of a root or an absent ancestor.
#[derive(Debug, thiserror::Error)]
pub enum MemberCreationError {
    #[error("member creation storage read failed: {0}")]
    Storage(#[from] crate::store::MobStoreError),
    #[error("member creation runtime read failed: {0}")]
    Runtime(#[from] crate::MobError),
    #[error("member creation session read failed: {0}")]
    Session(#[from] meerkat_core::service::SessionError),
    #[error("member creation ancestry unavailable: {0}")]
    Unavailable(&'static str),
}

/// Read index of immutable journal facts. Only committed event projection
/// populates this index; it is never a second persistence or write authority.
#[derive(Debug, Clone, Default)]
pub(crate) struct MemberCreationProjection {
    entries: std::collections::BTreeMap<uuid::Uuid, Option<MemberCreationSnapshot>>,
}

impl MemberCreationProjection {
    pub(crate) fn recover_binding(
        &mut self,
        mob_id: &crate::MobId,
        recovered: &crate::event::MemberSessionBindingRecoveredEvent,
        previous: Option<&crate::roster::RosterEntry>,
    ) {
        let Some(session_id) = recovered.bridge_session_id() else {
            return;
        };
        let snapshot = previous
            .filter(|entry| entry.agent_runtime_id == recovered.agent_runtime_id)
            .and_then(|entry| entry.bridge_session_id())
            .and_then(|source_session| self.get(source_session).ok().flatten());
        let Some(previous) = snapshot else {
            self.entries.insert(session_id.0, None);
            return;
        };
        if previous.session_id == *session_id {
            return;
        }
        let creation = match previous.creation.creation_id {
            Some(creation_id) => MemberCreationRecord {
                creation_id: Some(creation_id),
                provenance: MemberCreationProvenance::Successor {
                    predecessor_session_id: previous.session_id.clone(),
                    predecessor_member_binding: previous.member_binding.clone(),
                    predecessor_creation_id: creation_id,
                },
            },
            None => MemberCreationRecord::default(),
        };
        let snapshot = MemberCreationSnapshot {
            birth_cursor: previous.birth_cursor,
            session_id: session_id.clone(),
            member_binding: previous.member_binding,
            creation,
            fork_source: None,
        };
        if snapshot.member_binding.mob_id != mob_id.as_str()
            || snapshot.member_binding.member != recovered.agent_identity.as_str()
            || !snapshot.coherent()
        {
            self.entries.insert(session_id.0, None);
            return;
        }
        self.insert(snapshot);
    }

    pub(crate) fn observe(&mut self, event: &crate::MobEvent) {
        let crate::MobEventKind::MemberSpawned(spawned) = &event.kind else {
            return;
        };
        let Some(session_id) = spawned
            .bridge_member_ref()
            .and_then(crate::event::MemberRef::bridge_session_id)
        else {
            return;
        };
        let Ok(Some(mut snapshot)) =
            select_creation_snapshot(std::slice::from_ref(event), &event.mob_id, session_id)
        else {
            self.entries.insert(session_id.0, None);
            return;
        };
        if let MemberCreationProvenance::Successor {
            predecessor_session_id,
            predecessor_member_binding,
            predecessor_creation_id,
        } = &snapshot.creation.provenance
        {
            let previous = self.get(predecessor_session_id).ok().flatten();
            let Some(previous) = previous.filter(|previous| {
                previous.creation.creation_id == Some(*predecessor_creation_id)
                    && previous.member_binding == *predecessor_member_binding
                    && previous.birth_cursor < event.cursor
            }) else {
                self.entries.insert(session_id.0, None);
                return;
            };
            snapshot.birth_cursor = previous.birth_cursor;
        }
        self.insert(snapshot);
    }

    fn insert(&mut self, snapshot: MemberCreationSnapshot) {
        match self.entries.entry(snapshot.session_id.0) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(Some(snapshot));
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                if entry.get().as_ref().is_some_and(|previous| {
                    previous.member_binding.mob_id != snapshot.member_binding.mob_id
                        || previous.member_binding.member != snapshot.member_binding.member
                        || previous.creation != snapshot.creation
                        || previous.fork_source != snapshot.fork_source
                }) {
                    entry.insert(None);
                }
            }
        }
    }

    pub(crate) fn get(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<MemberCreationSnapshot>, MemberCreationError> {
        match self.entries.get(&session_id.0) {
            Some(Some(snapshot)) => Ok(Some(snapshot.clone())),
            Some(None) => Err(MemberCreationError::Unavailable(
                "conflicting session history",
            )),
            None => Ok(None),
        }
    }
}

pub(crate) fn select_creation_snapshot(
    events: &[crate::MobEvent],
    mob_id: &crate::MobId,
    session_id: &SessionId,
) -> Result<Option<MemberCreationSnapshot>, MemberCreationError> {
    let mut found: Option<MemberCreationSnapshot> = None;
    for event in events {
        if &event.mob_id != mob_id {
            continue;
        }
        let crate::MobEventKind::MemberSpawned(spawned) = &event.kind else {
            continue;
        };
        if spawned
            .bridge_member_ref()
            .and_then(crate::event::MemberRef::bridge_session_id)
            != Some(session_id)
        {
            continue;
        }
        if spawned.agent_runtime_id.identity != spawned.agent_identity
            || spawned.agent_runtime_id.generation != spawned.generation
        {
            return Err(MemberCreationError::Unavailable(
                "incoherent member incarnation",
            ));
        }
        let candidate = MemberCreationSnapshot {
            birth_cursor: event.cursor,
            session_id: session_id.clone(),
            member_binding: MobMemberBinding {
                mob_id: mob_id.to_string(),
                role: spawned.role.to_string(),
                member: spawned.agent_identity.to_string(),
            },
            creation: spawned.creation.clone(),
            fork_source: spawned.fork_source.clone(),
        };
        if !candidate.coherent() {
            return Err(MemberCreationError::Unavailable(
                "incoherent creation proof",
            ));
        }
        if let Some(previous) = found.as_ref() {
            // Role migration may re-seat the exact same member/session. Its
            // immutable creation proof stays the original journal binding.
            if previous.member_binding.member != candidate.member_binding.member
                || previous.creation != candidate.creation
                || previous.fork_source != candidate.fork_source
            {
                return Err(MemberCreationError::Unavailable(
                    "conflicting session history",
                ));
            }
        } else {
            found = Some(candidate);
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentIdentity, Generation, MobEvent, MobEventKind, MobId, ProfileName};

    fn created(session: &SessionId, id: MemberCreationId) -> MobEvent {
        let identity = AgentIdentity::from("worker");
        let mut spawned = crate::event::MemberSpawnedEvent::new(
            identity.clone(),
            Generation::INITIAL,
            crate::FenceToken::new(1),
            crate::AgentRuntimeId::initial(identity),
            ProfileName::from("worker"),
        )
        .with_bridge_member_ref(Some(crate::event::MemberRef::from_bridge_session_id(
            session.clone(),
        )));
        spawned.creation = MemberCreationRecord {
            creation_id: Some(id),
            provenance: MemberCreationProvenance::Root,
        };
        MobEvent {
            cursor: 1,
            timestamp: chrono::Utc::now(),
            mob_id: MobId::from("test"),
            kind: MobEventKind::MemberSpawned(spawned),
        }
    }

    #[test]
    fn creation_projection_retains_retired_and_reset_ancestry_and_distinguishes_name_reuse() {
        let first = SessionId::new();
        let second = SessionId::new();
        let first_id = MemberCreationId::new();
        let second_id = MemberCreationId::new();
        let mut roster = crate::roster::Roster::project(&[created(&first, first_id)]);
        let mut reset = created(&second, second_id);
        reset.kind = MobEventKind::MobReset;
        roster.apply(&reset);
        assert!(roster.is_empty());
        roster.apply(&created(&second, second_id));
        assert_eq!(
            roster
                .creation_history
                .get(&first)
                .unwrap()
                .unwrap()
                .creation
                .creation_id,
            Some(first_id)
        );
        assert_eq!(
            roster
                .creation_history
                .get(&second)
                .unwrap()
                .unwrap()
                .creation
                .creation_id,
            Some(second_id)
        );
        assert_ne!(first_id, second_id);
    }

    #[test]
    fn conflicting_session_history_is_poisoned_and_cannot_be_repaired_by_later_duplicate() {
        let session = SessionId::new();
        let original = created(&session, MemberCreationId::new());
        let mut index = MemberCreationProjection::default();
        index.observe(&original);
        index.observe(&created(&session, MemberCreationId::new()));
        index.observe(&original);
        assert!(matches!(
            index.get(&session),
            Err(MemberCreationError::Unavailable(_))
        ));
    }

    #[test]
    fn recovered_binding_preserves_original_birth_and_refuses_stale_incarnation() {
        let old_session = SessionId::new();
        let new_session = SessionId::new();
        let id = MemberCreationId::new();
        let original = created(&old_session, id);
        let mut roster = crate::roster::Roster::project(&[original]);
        let identity = AgentIdentity::from("worker");
        let recovered = crate::event::MemberSessionBindingRecoveredEvent::new(
            identity.clone(),
            crate::AgentRuntimeId::initial(identity.clone()),
            new_session.clone(),
        );
        let mut event = created(&new_session, id);
        event.cursor = 9;
        event.kind = MobEventKind::MemberSessionBindingRecovered(recovered);
        roster.apply(&event);
        let proof = roster.creation_history.get(&new_session).unwrap().unwrap();
        assert_eq!(proof.birth_cursor, 1);
        assert_eq!(proof.creation.creation_id, Some(id));
        assert!(
            matches!(proof.creation.provenance, MemberCreationProvenance::Successor { predecessor_session_id, .. } if predecessor_session_id == old_session)
        );

        let stale_session = SessionId::new();
        event.kind = MobEventKind::MemberSessionBindingRecovered(
            crate::event::MemberSessionBindingRecoveredEvent::new(
                identity.clone(),
                crate::AgentRuntimeId::new(identity, Generation::new(42)),
                stale_session.clone(),
            ),
        );
        roster.apply(&event);
        assert!(roster.creation_history.get(&stale_session).is_err());
    }

    #[test]
    fn missing_successor_ancestor_and_unresolved_policy_fail_closed() {
        let session = SessionId::new();
        let id = MemberCreationId::new();
        let mut event = created(&session, id);
        let MobEventKind::MemberSpawned(spawned) = &mut event.kind else {
            unreachable!()
        };
        spawned.creation.provenance = MemberCreationProvenance::Successor {
            predecessor_session_id: SessionId::new(),
            predecessor_member_binding: MobMemberBinding {
                mob_id: "test".into(),
                role: "worker".into(),
                member: "worker".into(),
            },
            predecessor_creation_id: id,
        };
        let mut index = MemberCreationProjection::default();
        index.observe(&event);
        assert!(index.get(&session).is_err());

        let session = SessionId::new();
        let mut event = created(&session, MemberCreationId::new());
        let MobEventKind::MemberSpawned(spawned) = &mut event.kind else {
            unreachable!()
        };
        spawned.creation.provenance = MemberCreationProvenance::Spawn {
            source: MemberCreationSource {
                session_id: SessionId::new(),
                member_binding: MobMemberBinding {
                    mob_id: "test".into(),
                    role: "worker".into(),
                    member: "parent".into(),
                },
                creation_id: MemberCreationId::new(),
                tool_access_policy: Some(ToolAccessPolicy::Inherit),
            },
        };
        index.observe(&event);
        assert!(index.get(&session).is_err());
    }

    #[test]
    fn legacy_creation_is_unknown_and_role_migration_keeps_original_creation_binding() {
        let record: MemberCreationRecord = Default::default();
        assert!(record.creation_id.is_none());
        assert_eq!(record.provenance, MemberCreationProvenance::LegacyUnknown);
        let session = SessionId::new();
        let original = created(&session, MemberCreationId::new());
        let mut migrated = original.clone();
        let MobEventKind::MemberSpawned(spawned) = &mut migrated.kind else {
            unreachable!()
        };
        spawned.role = ProfileName::from("analyst");
        let mut index = MemberCreationProjection::default();
        index.observe(&original);
        index.observe(&migrated);
        assert_eq!(
            index.get(&session).unwrap().unwrap().member_binding.role,
            "worker"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn creation_proof_survives_stored_codec_and_replay_with_one_fork_endpoint() {
        let source_session = SessionId::new();
        let child_session = SessionId::new();
        let source_id = MemberCreationId::new();
        let source = created(&source_session, source_id);
        let mut child = created(&child_session, MemberCreationId::new());
        let MobEventKind::MemberSpawned(spawned) = &mut child.kind else {
            unreachable!()
        };
        spawned.agent_identity = AgentIdentity::from("fork");
        spawned.agent_runtime_id = crate::AgentRuntimeId::initial(spawned.agent_identity.clone());
        spawned.creation.provenance = MemberCreationProvenance::Fork {
            source_creation_id: source_id,
            source_tool_access_policy: Some(ToolAccessPolicy::ReadOnly),
        };
        spawned.fork_source = Some(meerkat_core::ForkBuildSource::new(
            MobMemberBinding {
                mob_id: "test".into(),
                role: "worker".into(),
                member: "worker".into(),
            },
            source_session.clone(),
        ));
        let restored: Vec<_> = [source, child]
            .iter()
            .map(|event| {
                crate::event::decode_stored_mob_event(
                    &crate::event::encode_stored_mob_event(event).unwrap(),
                )
                .unwrap()
            })
            .collect();
        let roster = crate::roster::Roster::project(&restored);
        let snapshot = roster
            .creation_history
            .get(&child_session)
            .unwrap()
            .unwrap();
        assert_eq!(
            snapshot.fork_source.unwrap().source_session_id,
            source_session
        );
        assert!(
            matches!(snapshot.creation.provenance, MemberCreationProvenance::Fork {
            source_creation_id, source_tool_access_policy: Some(ToolAccessPolicy::ReadOnly),
        } if source_creation_id == source_id)
        );
    }
}
