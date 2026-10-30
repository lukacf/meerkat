//! Created identity entries at a start: the recovery of the identity store's
//! created entry rows, and the seat of a created entry owed its seat on its
//! archived session. See [`super::identity_entry`] for the entry concept and
//! its reads.
//!
//! MobMachine decides every fact (`RecoverCreatedIdentityEntry`). After the
//! journal replay and before the start's member plan, the shell passes it
//! each row of the identity store's created identity scan
//! ([`crate::store::MobIdentityStore::scan_created_identities_page`]), with
//! the row's presence and the session the identity's member was last bound
//! to in the replayed journal. A store read that fails fails the start: a
//! name the store may reserve is never read as free. The replay of a member
//! row whose origin token this build does not know reserves the name as
//! unreadable through the same signal.
//!
//! A created entry owed its seat (a build that predates created entries
//! retired its member) is seated again once the mob runs, Dormant, on its
//! archived session, under created custody, with the role, runtime mode,
//! labels, overrides, continuity and wiring of its member's last spawn row:
//! the same member comes back on the same session, history kept, and wakes
//! on its first delivery. An entry with no archived session, or whose
//! archived session cannot be revived, stays owed its seat, and its reads
//! say so.

use std::collections::BTreeMap;

use meerkat_core::types::SessionId;

use crate::MobError;
use crate::event::{MemberSpawnedEvent, MobEvent, MobEventKind};
use crate::identity::{CreatedIdentityObservation, CreatedIdentityPresence};
use crate::ids::{AgentIdentity, MobId};
use crate::machines::mob_machine as mob_dsl;
use crate::store::MobIdentityStore;

/// The presence MobMachine's recovery reads from one created entry row.
fn recovered_presence(observation: &CreatedIdentityObservation) -> mob_dsl::RecoveredPresence {
    match observation {
        CreatedIdentityObservation::Created(record) => match record.presence {
            CreatedIdentityPresence::Present => mob_dsl::RecoveredPresence::Present,
            CreatedIdentityPresence::PendingPreparation { .. } => {
                mob_dsl::RecoveredPresence::PresentUnseated
            }
            CreatedIdentityPresence::Removed => mob_dsl::RecoveredPresence::Tombstone,
        },
        CreatedIdentityObservation::Unreadable { .. } => mob_dsl::RecoveredPresence::Unreadable,
    }
}

/// Whether MobMachine recovers created entries in the authority's phase: a
/// running or stopped mob. A completed or destroyed mob acts on no member.
fn recovers_in_phase(authority: &mob_dsl::MobMachineAuthority) -> bool {
    matches!(
        authority.state().lifecycle_phase,
        mob_dsl::MobPhase::Running | mob_dsl::MobPhase::Stopped
    )
}

/// The session each identity's member was last bound to in `events`: its
/// last spawn row's session, or a later binding row's.
fn last_bound_sessions(mob_id: &MobId, events: &[MobEvent]) -> BTreeMap<AgentIdentity, SessionId> {
    let mut sessions = BTreeMap::new();
    for event in events.iter().filter(|event| &event.mob_id == mob_id) {
        match &event.kind {
            MobEventKind::MemberSpawned(spawned) => {
                if let Some(session_id) = spawned
                    .bridge_member_ref()
                    .and_then(crate::event::MemberRef::bridge_session_id)
                {
                    sessions.insert(spawned.agent_identity.clone(), session_id.clone());
                }
            }
            MobEventKind::MemberSessionBindingRecovered(recovered) => {
                if let Some(session_id) = recovered.bridge_session_id() {
                    sessions.insert(recovered.agent_identity.clone(), session_id.clone());
                }
            }
            _ => {}
        }
    }
    sessions
}

/// Recover `mob_id`'s created entry rows into MobMachine, one
/// `RecoverCreatedIdentityEntry` per row of the identity store's created
/// identity scan, after the replay of `epoch_events` and before the start's
/// member plan.
pub(super) async fn recover_created_identity_entries(
    authority: &mut mob_dsl::MobMachineAuthority,
    store: &dyn MobIdentityStore,
    mob_id: &MobId,
    epoch_events: &[MobEvent],
) -> Result<(), MobError> {
    if !recovers_in_phase(authority) {
        return Ok(());
    }
    let mut last_sessions = None;
    let mut after = None;
    loop {
        let page = store
            .scan_created_identities_page(
                mob_id,
                after.as_ref(),
                crate::store::IDENTITY_INTENT_SCAN_PAGE_MAX,
            )
            .await?;
        for (identity, observation) in &page.observations {
            let last_session = last_sessions
                .get_or_insert_with(|| last_bound_sessions(mob_id, epoch_events))
                .get(identity)
                .map(mob_dsl::SessionId::from_domain);
            super::builder::apply_seeded_mob_signal(
                authority,
                mob_dsl::MobMachineSignal::RecoverCreatedIdentityEntry {
                    agent_identity: mob_dsl::AgentIdentity::from_domain(identity),
                    presence: recovered_presence(observation),
                    last_session,
                },
                "recover_created_identity_entry",
            )?;
        }
        match page.next {
            None => return Ok(()),
            Some(next) if after.as_ref().is_none_or(|current| &next > current) => {
                after = Some(next);
            }
            Some(_) => {
                return Err(MobError::Internal(format!(
                    "the created identity scan of mob '{mob_id}' returned a non-advancing cursor"
                )));
            }
        }
    }
}

/// Reserve `identity`'s name as unreadable at the replay of a member row
/// whose origin token this build does not know: nothing acts on it.
pub(super) fn reserve_unknown_origin_name(
    authority: &mut mob_dsl::MobMachineAuthority,
    identity: &AgentIdentity,
) -> Result<(), MobError> {
    if !recovers_in_phase(authority) {
        return Ok(());
    }
    super::builder::apply_seeded_mob_signal(
        authority,
        mob_dsl::MobMachineSignal::RecoverCreatedIdentityEntry {
            agent_identity: mob_dsl::AgentIdentity::from_domain(identity),
            presence: mob_dsl::RecoveredPresence::Unreadable,
            last_session: None,
        },
        "recover_unknown_identity_entry_origin",
    )
}

/// The spec that seats a created entry owed its seat again: its member's
/// last spawn row's role, runtime mode, labels, overrides, continuity and
/// wiring, resuming `session_id`, its archived session, under created
/// custody.
fn reseat_spec(row: &MemberSpawnedEvent, session_id: SessionId) -> super::SpawnMemberSpec {
    let mut spec = super::SpawnMemberSpec::new(row.role.clone(), row.agent_identity.clone());
    spec.runtime_mode = Some(row.runtime_mode);
    spec.labels = Some(row.labels.clone());
    spec.override_profile = row.effective_profile_override.clone();
    spec.model_override = row.effective_model_override.clone();
    spec.definition_wiring = row.definition_wiring;
    spec.continuity_intent = row.continuity_intent.clone();
    spec.launch_mode = crate::launch::MemberLaunchMode::Resume {
        bridge_session_id: session_id,
        resume_from_role: None,
    };
    spec.created_custody = true;
    spec
}

/// The created entries owed their seat that a start seats again: each one's
/// archived session record, in identity order. None when the mob is not
/// running, its destroy is admitted, or the composition is a candidate.
fn owed_created_seats(state: &mob_dsl::MobMachineState) -> Vec<(AgentIdentity, SessionId)> {
    if state.lifecycle_phase != mob_dsl::MobPhase::Running
        || state.destroy_admitted
        || state.composition_authority != mob_dsl::CompositionAuthorityKind::Authoritative
    {
        return Vec::new();
    }
    let mut owed = Vec::new();
    for (identity, cause) in &state.identity_entry_unseated {
        if *cause != mob_dsl::UnseatedEntryCause::SeatOwed
            || state.identity_entry_origins.get(identity)
                != Some(&mob_dsl::IdentityEntryOrigin::Created)
            || state.identity_entry_unreadable.contains(identity)
        {
            continue;
        }
        let Some(archived) = state.identity_entry_archived_sessions.get(identity) else {
            continue;
        };
        match SessionId::parse(&archived.0) {
            Ok(session_id) => owed.push((AgentIdentity::from(identity.0.as_str()), session_id)),
            Err(error) => tracing::warn!(
                agent_identity = %identity.0,
                error = %error,
                "a created entry owed its seat holds an invalid archived session"
            ),
        }
    }
    owed
}

impl super::MobHandle {
    /// Seat every created entry owed its seat again, Dormant, on its
    /// archived session, under created custody (see the module docs): after
    /// a start that runs the mob, and after a stopped mob's resume. A mob
    /// that is not running, and a candidate composition, seat nothing. Never
    /// fails its caller: an entry it cannot seat stays owed its seat, and
    /// the failure is logged.
    pub(super) async fn reseat_owed_created_entries(&self) {
        let owed = owed_created_seats(&self.machine_state_watch_rx.borrow());
        if owed.is_empty() {
            return;
        }
        let rows = match self.last_spawn_rows().await {
            Ok(rows) => rows,
            Err(error) => {
                tracing::warn!(
                    mob_id = %self.mob_id(),
                    error = %error,
                    "created entries owed their seat: the journal read failed"
                );
                return;
            }
        };
        let mut specs = Vec::with_capacity(owed.len());
        for (identity, session_id) in owed {
            match rows.get(&identity) {
                Some(row) => specs.push(reseat_spec(row, session_id)),
                None => tracing::warn!(
                    mob_id = %self.mob_id(),
                    agent_identity = %identity,
                    "a created entry owed its seat has no spawn row"
                ),
            }
        }
        // The mob's own obligation, whoever started or resumed it.
        let owner = self.non_owning().with_command_authority(
            crate::control_policy::CommandAuthority::principal(
                crate::control_policy::MobControlPrincipal::Owner,
            ),
        );
        match owner.seat_dormant_members(specs).await {
            Ok(outcomes) => {
                for outcome in outcomes {
                    match outcome {
                        super::DormantSeatOutcome::Seated { .. }
                        | super::DormantSeatOutcome::AlreadySeated { .. } => {}
                        other => {
                            let identity = other.agent_identity().clone();
                            let error = other.into_unseated_error();
                            tracing::warn!(
                                mob_id = %self.mob_id(),
                                agent_identity = %identity,
                                error = %error,
                                "a created entry owed its seat was not seated"
                            );
                        }
                    }
                }
            }
            Err(error) => tracing::warn!(
                mob_id = %self.mob_id(),
                error = %error,
                "created entries owed their seat: the seat failed"
            ),
        }
    }

    /// Every identity's last spawn row in this mob's current epoch.
    async fn last_spawn_rows(
        &self,
    ) -> Result<BTreeMap<AgentIdentity, MemberSpawnedEvent>, MobError> {
        let events = self.events.replay_all().await.map_err(MobError::from)?;
        let mob_events: Vec<&MobEvent> = events
            .iter()
            .filter(|event| &event.mob_id == self.mob_id())
            .collect();
        let epoch_start = mob_events
            .iter()
            .rposition(|event| matches!(event.kind, MobEventKind::MobReset))
            .map_or(0, |position| position + 1);
        let mut rows = BTreeMap::new();
        for event in &mob_events[epoch_start..] {
            if let MobEventKind::MemberSpawned(spawned) = &event.kind {
                rows.insert(spawned.agent_identity.clone(), spawned.clone());
            }
        }
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::MemberRef;
    use crate::ids::{AgentRuntimeId, FenceToken, Generation, ProfileName};

    /// The presence each created row passes to MobMachine.
    #[test]
    fn a_created_row_passes_its_presence() {
        let record = |presence| {
            CreatedIdentityObservation::Created(
                crate::identity::CreatedIdentityRecord::sealed(
                    MobId::from("mob"),
                    AgentIdentity::from("created"),
                    1,
                    presence,
                    crate::identity::CreatedIdentityProvenance::new("create-1", 1),
                )
                .expect("a sealed row"),
            )
        };
        for (observation, presence) in [
            (
                record(CreatedIdentityPresence::Present),
                mob_dsl::RecoveredPresence::Present,
            ),
            (
                record(CreatedIdentityPresence::PendingPreparation { attempt: 3 }),
                mob_dsl::RecoveredPresence::PresentUnseated,
            ),
            (
                record(CreatedIdentityPresence::Removed),
                mob_dsl::RecoveredPresence::Tombstone,
            ),
            (
                CreatedIdentityObservation::Unreadable {
                    evidence_digest: "sha256:0".to_string(),
                    detail: "a later build's row".to_string(),
                },
                mob_dsl::RecoveredPresence::Unreadable,
            ),
        ] {
            assert_eq!(
                recovered_presence(&observation),
                presence,
                "{observation:?}"
            );
        }
    }

    /// The last session of an identity is its last binding row's, in the
    /// mob's own journal.
    #[test]
    fn the_last_bound_session_is_the_last_binding_rows() {
        let mob = MobId::from("mob");
        let identity = AgentIdentity::from("member");
        let first = SessionId::new();
        let second = SessionId::new();
        let spawned = |generation: u64, session: &SessionId| {
            MobEventKind::MemberSpawned(
                MemberSpawnedEvent::new(
                    identity.clone(),
                    Generation::new(generation),
                    FenceToken::new(generation + 1),
                    AgentRuntimeId::new(identity.clone(), Generation::new(generation)),
                    ProfileName::from("worker"),
                )
                .with_bridge_member_ref(Some(MemberRef::from_bridge_session_id(session.clone()))),
            )
        };
        let event = |cursor: u64, mob_id: &MobId, kind: MobEventKind| MobEvent {
            cursor,
            timestamp: chrono::Utc::now(),
            mob_id: mob_id.clone(),
            kind,
        };
        let events = [
            event(1, &mob, spawned(0, &first)),
            event(2, &mob, spawned(1, &second)),
            event(3, &MobId::from("other"), spawned(2, &first)),
        ];
        assert_eq!(
            last_bound_sessions(&mob, &events).get(&identity),
            Some(&second)
        );
    }
}
