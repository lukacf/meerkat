//! Rotation targets, the durable rotation record and the same-session
//! successor.
//!
//! MobMachine owns every rotation fact: the pending rotation of an identity
//! (`member_rotation_session_ids`) and its target (`member_rotation_targets`):
//!
//! - a [`RotationTarget::FreshSession`] rotation
//!   ([`super::MobHandle::rotate_member`], a reset or a deletion) retires the
//!   current incarnation, disposes of its session and seats the next
//!   incarnation Dormant on a new session;
//! - a [`RotationTarget::SameSession`] rotation
//!   ([`super::MobHandle::respawn_member`], and
//!   [`super::MobHandle::respawn`] of an identity entry's member) releases
//!   the current incarnation's runtime, keeps its session and seats the next
//!   incarnation Dormant on that same session.
//!
//! Both end in one machine step (`CommitMemberRotation`) whose two journal
//! rows are appended in one batch: the identity is never a non-member. The
//! rotation's start row (`MemberRetirementStarted`) records the rotation
//! ([`MemberRotationRecord`], a field the internal event codec stores and the
//! public event payload never carries), so after a crash the restarted mob
//! records the rotation again (`RecoverMemberRotationIntent`) and finishes it
//! itself, with no host record. A host that keeps its own record of a
//! FreshSession rotation marks the identity host-pending, and the restarted
//! mob leaves that rotation to the host's re-issue; a same-session rotation
//! is always finished by the mob, and keeps a host-pending member's mark, so
//! its successor waits for the host's seat step before it wakes.

use std::collections::BTreeMap;

use meerkat_core::comms::TrustedPeerDescriptor;
use meerkat_core::types::{ContentInput, SessionId};
use serde::{Deserialize, Serialize};

use crate::MobRuntimeMode;
use crate::ids::{AgentIdentity, Generation, ProfileName};
use crate::machines::mob_machine as mob_dsl;

/// Where a rotation seats its next incarnation.
///
/// The handle's entry reads and the journal's rows answer with this type.
/// MobMachine's own state holds the schema enum of the same name and
/// variants (`machines::mob_machine::RotationTarget`); this type is its public
/// projection, which a new machine variant extends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RotationTarget {
    /// A new session: the retiring incarnation's session is disposed of (a
    /// reset, a deletion).
    FreshSession,
    /// The member's own session, which the retirement keeps: a respawn, a
    /// declared role migration, a runtime-mode restamp.
    SameSession,
}

impl RotationTarget {
    pub(crate) fn from_machine(target: mob_dsl::RotationTarget) -> Self {
        match target {
            mob_dsl::RotationTarget::FreshSession => Self::FreshSession,
            mob_dsl::RotationTarget::SameSession => Self::SameSession,
        }
    }

    pub(crate) fn to_machine(self) -> mob_dsl::RotationTarget {
        match self {
            Self::FreshSession => mob_dsl::RotationTarget::FreshSession,
            Self::SameSession => mob_dsl::RotationTarget::SameSession,
        }
    }
}

/// Whether `identity`'s rotation is held for its host, in `state`: the
/// member owes its host's first-boot preparation
/// (`member_host_preparation_causes`, cause `FirstBootRow`) and the
/// rotation that stands for it is a FreshSession one, which the host's own
/// [`super::MobHandle::rotate_member`] finishes (as does
/// [`super::MobHandle::redrive_retirement`]). The one predicate the start's
/// hold, the actor's own re-drive, the re-drive of a caller other than the
/// host and the handle's read decide by.
pub(crate) fn rotation_held_for_host(
    state: &mob_dsl::MobMachineState,
    identity: &mob_dsl::AgentIdentity,
) -> bool {
    state.member_host_preparation_causes.get(identity)
        == Some(&mob_dsl::HostPreparationCause::FirstBootRow)
        && state.member_rotation_targets.get(identity)
            == Some(&mob_dsl::RotationTarget::FreshSession)
}

/// The rotation a member's retirement carries out, as its start row
/// (`MemberRetirementStarted`) records it: the target, the next session and
/// endpoint, and what the next incarnation is seated with.
///
/// It is a replay record. The internal event codec stores it on the start
/// row and the public event payload never carries it; a reader that predates
/// it ignores it, and replays the retirement as a plain one. Only this crate
/// builds one; the retirement start event exposes it read-only.
///
/// A record the codec cannot read fails the journal read, typed, so the
/// whole mob does not resume. While a rollback to 0.9.8 is supported, a later
/// release must not write a record 0.9.8 cannot decode: a new target or a
/// changed field goes under a new key, with a rollback fixture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct MemberRotationRecord {
    pub(crate) target: RotationTarget,
    /// The next incarnation's session: a new one, or (SameSession) the
    /// member's own.
    pub(crate) session_id: SessionId,
    /// The next incarnation's endpoint a FreshSession rotation recorded
    /// (none for network comms before the first wake, and none for a
    /// SameSession rotation: the member keeps its own).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) peer_endpoint: Option<TrustedPeerDescriptor>,
    pub(crate) successor: RotationSuccessor,
}

/// What a rotation seats its next incarnation with: enough for the restarted
/// mob to plan the same incarnation again (its profile authority is granted
/// again under the recovered intent). What only the process that started the
/// rotation held (a per-spawn tools overlay, a compaction curator, a dispatch
/// admission, a shell environment) is the spawn customizer's after a restart,
/// as for any rebuild.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct RotationSuccessor {
    pub(crate) generation: Generation,
    pub(crate) profile: ProfileName,
    pub(crate) runtime_mode: MobRuntimeMode,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) labels: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "crate::event::is_ephemeral_continuity")]
    pub(crate) continuity_intent: super::SpawnContinuityIntent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) effective_profile_override: Option<crate::profile::Profile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) effective_model_override: Option<String>,
    #[serde(default, skip_serializing_if = "crate::DefinitionWiring::is_applies")]
    pub(crate) definition_wiring: crate::DefinitionWiring,
    /// The owed kickoff's prompt (a rotation's initial message).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) kickoff_prompt: Option<ContentInput>,
    /// The objective the owed kickoff resolves under, when the rotation
    /// owes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) objective_id: Option<meerkat_core::interaction::ObjectiveId>,
    /// The build inputs the next incarnation's unborn session's first build
    /// applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) seat_build_inputs: Option<super::SeatBuildInputs>,
    /// The member whose turn created the member (a same-session successor
    /// keeps its spawner).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) spawned_by: Option<AgentIdentity>,
    /// The role the kept session was last built under, when a same-session
    /// successor takes another one (a declared role migration): its next
    /// build declares it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) resume_from_role: Option<ProfileName>,
}

impl MemberRotationRecord {
    /// The rotation's target.
    #[must_use]
    pub fn target(&self) -> RotationTarget {
        self.target
    }

    /// The session the next incarnation is seated on.
    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    /// The next incarnation's generation.
    #[must_use]
    pub fn next_generation(&self) -> Generation {
        self.successor.generation
    }

    /// The machine form of the record's endpoint.
    pub(crate) fn machine_peer_endpoint(&self) -> Option<mob_dsl::MemberPeerEndpoint> {
        self.peer_endpoint
            .as_ref()
            .map(mob_dsl::MemberPeerEndpoint::from)
    }

    /// Decode a stored record. A record this build cannot read (a damaged
    /// one, or a later release's target) fails the decode: its retirement
    /// never replays as a plain one, which would end the member (and, for a
    /// same-session rotation, archive its conversation).
    pub(crate) fn from_stored(value: serde_json::Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(value)
    }
}

/// A same-session successor of a member ([`super::MobHandle::respawn_member`]).
///
/// The member's current incarnation is retired and its next one is seated
/// Dormant at the next generation on the SAME session, in one machine step:
/// its conversation, comms identity and wiring edges are kept. The next
/// incarnation keeps the member's labels, spawner, wiring exemption and
/// continuity, and its profile and runtime mode unless this request names
/// others (a declared role migration, a runtime-mode restamp).
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct MemberRespawn {
    /// A message the next incarnation runs once as its first turn on the
    /// kept session (an owed kickoff: it wakes the member).
    pub initial_message: Option<ContentInput>,
    /// The next incarnation's profile; `None` keeps the member's.
    pub profile: Option<ProfileName>,
    /// The next incarnation's runtime mode; `None` keeps the member's.
    pub runtime_mode: Option<MobRuntimeMode>,
    /// Wake the next incarnation before answering
    /// ([`super::MobHandle::ensure_member_live`]); otherwise it stays Dormant
    /// until its next delivery or owed kickoff wakes it.
    pub wake: bool,
}

impl MemberRespawn {
    /// A successor with the member's own profile and runtime mode, no
    /// initial message, left Dormant.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Run `message` once as the next incarnation's first turn.
    #[must_use]
    pub fn with_initial_message(mut self, message: impl Into<ContentInput>) -> Self {
        self.initial_message = Some(message.into());
        self
    }

    /// Seat the next incarnation with `profile`.
    #[must_use]
    pub fn with_profile(mut self, profile: impl Into<ProfileName>) -> Self {
        self.profile = Some(profile.into());
        self
    }

    /// Seat the next incarnation with `runtime_mode`.
    #[must_use]
    pub fn with_runtime_mode(mut self, runtime_mode: MobRuntimeMode) -> Self {
        self.runtime_mode = Some(runtime_mode);
        self
    }

    /// Wake the next incarnation before answering.
    #[must_use]
    pub fn with_wake(mut self, wake: bool) -> Self {
        self.wake = wake;
        self
    }
}

/// The answer of [`super::MobHandle::release_member_at_scope`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ReleaseOutcome {
    /// The scope's incarnation is current: its materialization in this
    /// process is released (or nothing of it was materialized).
    Released,
    /// The scope names an incarnation that is not current (another runtime
    /// id, fence or session, another incarnation of the mob, or no member):
    /// nothing is released.
    Stale,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_targets_project_the_machine_enum_both_ways() {
        for target in [RotationTarget::FreshSession, RotationTarget::SameSession] {
            assert_eq!(RotationTarget::from_machine(target.to_machine()), target);
        }
        assert_eq!(
            serde_json::to_value(RotationTarget::SameSession).ok(),
            Some(serde_json::json!("same_session"))
        );
    }

    #[test]
    fn an_unreadable_stored_record_fails_its_decode() {
        assert!(
            MemberRotationRecord::from_stored(serde_json::json!({
                "target": "a_later_target",
                "session_id": "00000000-0000-4000-8000-000000000001",
                "successor": {}
            }))
            .is_err()
        );
    }
}
