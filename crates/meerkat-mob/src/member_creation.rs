//! Immutable creation facts carried by the canonical member-created journal.
//!
//! These facts prove runtime ancestry, not application permissions. Hosts must
//! intersect every edge with their current access policy. Fork endpoints remain
//! owned by `MemberSpawnedEvent::fork_source`.

use meerkat_core::{MobMemberBinding, SessionId};
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
    },
    /// Runtime-authorized respawn of the same logical member. Follow this
    /// exact predecessor to recover ancestry; this is not a delegation edge.
    Successor {
        predecessor_session_id: SessionId,
        predecessor_member_binding: MobMemberBinding,
        predecessor_creation_id: MemberCreationId,
    },
    /// This creation happened under the current runtime, but its ancestry
    /// could not be proved. Consumers must not infer inherited authority.
    Unproven,
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
        match &self.creation.provenance {
            MemberCreationProvenance::Root => self.fork_source.is_none(),
            MemberCreationProvenance::Spawn { source } => {
                self.fork_source.is_none()
                    && source.session_id != self.session_id
                    && source.creation_id != id
                    && !source.creation_id.0.is_nil()
            }
            MemberCreationProvenance::Fork { source_creation_id } => {
                self.fork_source
                    .as_ref()
                    .is_some_and(|source| source.source_session_id != self.session_id)
                    && *source_creation_id != id
                    && !source_creation_id.0.is_nil()
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
            MemberCreationProvenance::Unproven | MemberCreationProvenance::LegacyUnknown => true,
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

/// Host construction and agent ingress are explicit even without a witness.
/// Only trusted host constructors issue `HostRoot`; agent ingress replaces it.
#[derive(Debug, Clone)]
pub(crate) enum MemberCreationOrigin {
    HostRoot,
    Source(MemberCreationSourceWitness),
    Unproven,
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
    /// The source's creation facts exist but cannot be trusted: a changed or
    /// stale binding, missing or disagreeing metadata, or conflicting history.
    /// This is a fault, never a legitimate absence.
    #[error("member creation ancestry unavailable: {0}")]
    Unavailable(&'static str),
    /// There are no creation facts to capture, and that is expected.
    #[error("member creation facts absent: {0}")]
    Absent(MemberCreationAbsence),
}

/// Why a source legitimately has no creation facts to capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MemberCreationAbsence {
    /// The dispatch carried no origin session (a host or upcall call).
    #[error("the call has no origin session")]
    OwnerlessDispatch,
    /// The source service keeps no durable session metadata.
    #[error("the source service has no persisted metadata authority")]
    NonDurableService,
    /// The source is a session that is not bound to any mob member.
    #[error("the source session is not a mob member")]
    SourceNotAMember,
    /// The source member predates creation tokens; its ancestry is unknown.
    #[error("the source member predates creation tokens")]
    LegacyCreation,
}

/// How a creation-source capture resolved for spawn admission.
///
/// Creation facts are optional and confer no permission, so no outcome
/// changes spawn admission: only `Captured` proves a source, and both other
/// outcomes record the child as unproven. They stay distinct so a fault is
/// never mistaken for a legitimate absence.
#[derive(Debug)]
pub enum CreationSourceCapture {
    /// The source's sealed creation facts.
    Captured(MemberCreationSourceWitness),
    /// The source legitimately has no creation facts.
    Absent(MemberCreationAbsence),
    /// Reading the source's creation facts failed.
    CaptureFailed(MemberCreationError),
}

impl CreationSourceCapture {
    /// Classify the result of a source capture.
    pub fn classify(result: Result<MemberCreationSourceWitness, MemberCreationError>) -> Self {
        match result {
            Ok(witness) => Self::Captured(witness),
            Err(MemberCreationError::Absent(absence)) => Self::Absent(absence),
            Err(error) => Self::CaptureFailed(error),
        }
    }

    /// The witness a spawn is admitted with. A failed capture is traced at
    /// error level with its cause; the child is still admitted and recorded
    /// as unproven, never as a root.
    pub fn into_admitted_witness(self, operation: &str) -> MemberCreationSourceWitness {
        match self {
            Self::Captured(witness) => witness,
            Self::Absent(_) => MemberCreationSourceWitness::unavailable(),
            Self::CaptureFailed(error) => {
                tracing::error!(
                    operation,
                    capture = "failed",
                    error = %error,
                    "member creation source capture failed; the child is recorded as unproven"
                );
                MemberCreationSourceWitness::unavailable()
            }
        }
    }
}

/// Read index of immutable journal facts. Only committed event projection
/// populates this index; it is never a second persistence or write authority.
#[derive(Debug, Default)]
pub(crate) struct MemberCreationProjection {
    // Shared only by internal canonical replay/commit projections. Public
    // roster snapshots are rebuilt from entries with independent history.
    // No await or external call occurs under this lock.
    entries:
        std::sync::RwLock<std::collections::BTreeMap<uuid::Uuid, Option<MemberCreationSnapshot>>>,
}

impl MemberCreationProjection {
    pub(crate) fn recover_binding(
        &self,
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
            self.poison(session_id);
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
            self.poison(session_id);
            return;
        }
        self.insert(snapshot);
    }

    pub(crate) fn observe(&self, event: &crate::MobEvent) {
        let crate::MobEventKind::MemberSpawned(spawned) = &event.kind else {
            return;
        };
        let Some(session_id) = spawned
            .bridge_member_ref()
            .and_then(crate::event::MemberRef::bridge_session_id)
        else {
            return;
        };
        let Ok(previous) = self.get(session_id) else {
            return;
        };
        let Ok(Some(mut snapshot)) = select_creation_snapshot(
            std::slice::from_ref(event),
            &event.mob_id,
            session_id,
            previous,
        ) else {
            self.poison(session_id);
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
                self.poison(session_id);
                return;
            };
            snapshot.birth_cursor = previous.birth_cursor;
        }
        self.insert(snapshot);
    }

    fn poison(&self, session_id: &SessionId) {
        if let Ok(mut entries) = self.entries.write() {
            entries.insert(session_id.0, None);
        }
        // A poisoned lock stays poisoned and every read reports the fault.
    }

    fn insert(&self, snapshot: MemberCreationSnapshot) {
        let Ok(mut entries) = self.entries.write() else {
            return;
        };
        match entries.entry(snapshot.session_id.0) {
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
        let entries = self.entries.read().map_err(|_| {
            MemberCreationError::Unavailable("member creation history lock poisoned")
        })?;
        match entries.get(&session_id.0) {
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
    previous: Option<MemberCreationSnapshot>,
) -> Result<Option<MemberCreationSnapshot>, MemberCreationError> {
    let mut found = previous;
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
        let mut candidate = MemberCreationSnapshot {
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
        if let Some(previous) = found.as_ref()
            && candidate.fork_source.is_none()
            && previous.session_id == candidate.session_id
            && previous.member_binding.mob_id == candidate.member_binding.mob_id
            && previous.member_binding.member == candidate.member_binding.member
            && previous.creation == candidate.creation
            && matches!(
                previous.creation.provenance,
                MemberCreationProvenance::Fork { .. }
            )
        {
            // A plain exact-session resume may omit the launch's fork field.
            // Retain the first canonical endpoint for the same creation;
            // never manufacture one or replace a conflicting supplied one.
            candidate.fork_source.clone_from(&previous.fork_source);
        }
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
        let index = MemberCreationProjection::default();
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
    fn missing_successor_ancestor_and_unproven_without_token_fail_closed() {
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
        let index = MemberCreationProjection::default();
        index.observe(&event);
        assert!(index.get(&session).is_err());

        let session = SessionId::new();
        let mut event = created(&session, MemberCreationId::new());
        let MobEventKind::MemberSpawned(spawned) = &mut event.kind else {
            unreachable!()
        };
        spawned.creation = MemberCreationRecord {
            creation_id: None,
            provenance: MemberCreationProvenance::Unproven,
        };
        index.observe(&event);
        assert!(index.get(&session).is_err());
    }

    #[test]
    fn legacy_creation_is_unknown_and_role_migration_keeps_original_creation_binding() {
        let record: MemberCreationRecord = Default::default();
        assert!(record.creation_id.is_none());
        assert_eq!(record.provenance, MemberCreationProvenance::LegacyUnknown);
        assert_ne!(record.provenance, MemberCreationProvenance::Unproven);
        let session = SessionId::new();
        let original = created(&session, MemberCreationId::new());
        let mut migrated = original.clone();
        let MobEventKind::MemberSpawned(spawned) = &mut migrated.kind else {
            unreachable!()
        };
        spawned.role = ProfileName::from("analyst");
        let index = MemberCreationProjection::default();
        index.observe(&original);
        index.observe(&migrated);
        assert_eq!(
            index.get(&session).unwrap().unwrap().member_binding.role,
            "worker"
        );
    }

    #[test]
    fn fork_reseat_retains_original_endpoint_and_rejects_conflicting_or_missing_proof() {
        let session = SessionId::new();
        let mut original = created(&session, MemberCreationId::new());
        let source = meerkat_core::ForkBuildSource::new(
            MobMemberBinding {
                mob_id: "test".into(),
                role: "worker".into(),
                member: "parent".into(),
            },
            SessionId::new(),
        );
        let MobEventKind::MemberSpawned(spawned) = &mut original.kind else {
            unreachable!()
        };
        spawned.creation.provenance = MemberCreationProvenance::Fork {
            source_creation_id: MemberCreationId::new(),
        };
        spawned.fork_source = Some(source.clone());
        let mut reseated = original.clone();
        reseated.cursor = 9;
        let MobEventKind::MemberSpawned(spawned) = &mut reseated.kind else {
            unreachable!()
        };
        spawned.role = ProfileName::from("analyst");
        spawned.fork_source = None;

        let index = MemberCreationProjection::default();
        index.observe(&original);
        index.observe(&reseated);
        let retained = index.get(&session).unwrap().unwrap();
        assert_eq!(retained.birth_cursor, original.cursor);
        assert_eq!(retained.member_binding.role, "worker");
        assert_eq!(retained.fork_source, Some(source.clone()));

        let missing_original = MemberCreationProjection::default();
        missing_original.observe(&reseated);
        assert!(missing_original.get(&session).is_err());

        let mut different_creation = reseated.clone();
        let MobEventKind::MemberSpawned(spawned) = &mut different_creation.kind else {
            unreachable!()
        };
        spawned.creation.provenance = MemberCreationProvenance::Fork {
            source_creation_id: MemberCreationId::new(),
        };
        let changed_source = MemberCreationProjection::default();
        changed_source.observe(&original);
        changed_source.observe(&different_creation);
        assert!(changed_source.get(&session).is_err());

        let MobEventKind::MemberSpawned(spawned) = &mut reseated.kind else {
            unreachable!()
        };
        spawned.fork_source = Some(meerkat_core::ForkBuildSource::new(
            source.source_member,
            SessionId::new(),
        ));
        index.observe(&reseated);
        assert!(index.get(&session).is_err());
        index.observe(&original);
        assert!(index.get(&session).is_err());
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
            source_creation_id,
        } if source_creation_id == source_id)
        );
    }
}
