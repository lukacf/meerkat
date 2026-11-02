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
//! retired its member, or a kickoff's spawn after its create failed) is
//! seated again once the mob runs, Dormant, under created custody, from its
//! row's spec when the row carries one: on its archived session (the same
//! member comes back on the same session, history kept, and wakes on its
//! first delivery), on a fresh session with the notice when that session is
//! gone from the store, or on a fresh one when it has none; a spec that owes
//! a kickoff is embodied. A row written before rows carried their spec seats
//! the entry with the role, runtime mode, overrides, continuity, wiring and
//! roster labels of its member's last spawn row, as any revive keeps its
//! member's spec, and only on its archived session: an entry with none, or
//! whose archived session cannot be revived, stays owed its seat, and its
//! reads say so.
//!
//! The start also carries out what the rows keep owed: the role migration
//! of a store-owned update whose restamp a crash prevented, and the
//! descriptive inputs (context, instructions) a seat whose session no build
//! created carries to its first build, set from the row's current spec.

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
            CreatedIdentityPresence::Archived { .. } => mob_dsl::RecoveredPresence::Archived,
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

/// What a start carries out from the store-owned rows it recovered: each
/// present identity's spec (a seat whose session no build created takes its
/// descriptive inputs from it), and the identities whose row keeps a role
/// migration its restamp has not committed.
#[derive(Debug, Default)]
pub(super) struct RecoveredCreatedRows {
    pub(super) specs: BTreeMap<AgentIdentity, crate::identity::CreatedIdentitySpec>,
    pub(super) role_migrations_owed: std::collections::BTreeSet<AgentIdentity>,
}

/// Recover `mob_id`'s created entry rows into MobMachine, one
/// `RecoverCreatedIdentityEntry` per row of the identity store's created
/// identity scan, after the replay of `epoch_events` and before the start's
/// member plan; and answer what the start carries out from them.
pub(super) async fn recover_created_identity_entries(
    authority: &mut mob_dsl::MobMachineAuthority,
    store: &dyn MobIdentityStore,
    mob_id: &MobId,
    epoch_events: &[MobEvent],
) -> Result<RecoveredCreatedRows, MobError> {
    let mut recovered = RecoveredCreatedRows::default();
    if !recovers_in_phase(authority) {
        return Ok(recovered);
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
            if let CreatedIdentityObservation::Created(record) = observation
                && record.presence == CreatedIdentityPresence::Present
                && let Some(spec) = &record.spec
            {
                recovered.specs.insert(identity.clone(), spec.clone());
                if record.pending_role_migration.is_some() {
                    recovered.role_migrations_owed.insert(identity.clone());
                }
            }
            // An archived row names its archived session itself; any other
            // row's revival record is its member's last bound session.
            let archived_session = match observation {
                CreatedIdentityObservation::Created(record) => match &record.presence {
                    CreatedIdentityPresence::Archived {
                        archived_session: Some(session),
                    } => Some(mob_dsl::SessionId::from_domain(session)),
                    _ => None,
                },
                CreatedIdentityObservation::Unreadable { .. } => None,
            };
            let last_session = archived_session.or_else(|| {
                last_sessions
                    .get_or_insert_with(|| last_bound_sessions(mob_id, epoch_events))
                    .get(identity)
                    .map(mob_dsl::SessionId::from_domain)
            });
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
            None => return Ok(recovered),
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
/// last spawn row's role, runtime mode, overrides, continuity, wiring and
/// roster labels, as any revive keeps its member's spec, resuming
/// `session_id`, its archived session, under created custody.
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

/// The created entries owed their seat that a start seats again, in
/// identity order, each with its archived session record if it has one.
/// None when the mob is not running, its destroy is admitted, or the
/// composition is a candidate.
fn owed_created_seats(state: &mob_dsl::MobMachineState) -> Vec<(AgentIdentity, Option<SessionId>)> {
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
        let archived = match state
            .identity_entry_archived_sessions
            .get(identity)
            .map(|archived| SessionId::parse(&archived.0))
            .transpose()
        {
            Ok(archived) => archived,
            Err(error) => {
                tracing::warn!(
                    agent_identity = %identity.0,
                    error = %error,
                    "a created entry owed its seat holds an invalid archived session"
                );
                continue;
            }
        };
        owed.push((AgentIdentity::from(identity.0.as_str()), archived));
    }
    owed
}

impl super::MobHandle {
    /// Carry out at a start what the store-owned rows keep owed (see the
    /// module docs): the entries owed their seat
    /// ([`Self::reseat_owed_created_entries`]), then, off the caller's path,
    /// each role migration whose restamp a crash prevented, in its own
    /// declared apply. Never fails its caller: what it cannot carry out
    /// stays owed, and the failure is logged.
    pub(super) async fn carry_out_store_owned_rows_at_start(
        &self,
        role_migrations_owed: std::collections::BTreeSet<AgentIdentity>,
    ) {
        self.reseat_owed_created_entries().await;
        if role_migrations_owed.is_empty() {
            return;
        }
        // The mob's own obligation, whoever started it.
        let owner = self.non_owning().with_command_authority(
            crate::control_policy::CommandAuthority::principal(
                crate::control_policy::MobControlPrincipal::Owner,
            ),
        );
        crate::tokio::spawn(async move {
            for identity in role_migrations_owed {
                if let Err(error) = owner.redeclare_store_owned_identity(&identity).await {
                    tracing::warn!(
                        mob_id = %owner.mob_id(),
                        agent_identity = %identity,
                        error = %error,
                        "a store-owned identity's role migration was not carried out"
                    );
                }
            }
        });
    }

    /// Seat every created entry owed its seat again, Dormant, under created
    /// custody (see the module docs): after a start that runs the mob, and
    /// after a stopped mob's resume. An entry whose row carries its spec is
    /// seated from it; one whose row predates its spec, from its last spawn
    /// row, on its archived session. A mob that is not running, and a
    /// candidate composition, seat nothing. Never fails its caller: an entry
    /// it cannot seat stays owed its seat, and the failure is logged.
    pub(super) async fn reseat_owed_created_entries(&self) {
        self.redrive_owed_entry_endings();
        let owed = owed_created_seats(&self.machine_state_watch_rx.borrow());
        if owed.is_empty() {
            return;
        }
        // The mob's own obligation, whoever started or resumed it.
        let owner = self.non_owning().with_command_authority(
            crate::control_policy::CommandAuthority::principal(
                crate::control_policy::MobControlPrincipal::Owner,
            ),
        );
        let mut legacy = Vec::new();
        for (identity, archived) in owed {
            let spec = if self.identity_member.is_some() {
                match self.store_owned_spec(&identity).await {
                    Ok(spec) => spec,
                    Err(error) => {
                        tracing::warn!(
                            mob_id = %self.mob_id(),
                            agent_identity = %identity,
                            error = %error,
                            "a created entry owed its seat: its row could not be read"
                        );
                        continue;
                    }
                }
            } else {
                None
            };
            match (spec, archived) {
                (Some(spec), _) => {
                    if let Err(error) =
                        Box::pin(owner.seat_store_owned_from_spec(&identity, &spec)).await
                    {
                        tracing::warn!(
                            mob_id = %self.mob_id(),
                            agent_identity = %identity,
                            error = %error,
                            "a created entry owed its seat was not seated from its row's spec"
                        );
                    }
                }
                (None, Some(session_id)) => legacy.push((identity, session_id)),
                (None, None) => {}
            }
        }
        if legacy.is_empty() {
            return;
        }
        let owed = legacy;
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

    /// Finish the removals and archives MobMachine holds owed (recovered at
    /// a start from the identities' rows, or left by a removal whose
    /// retirement did not run): retire the member of each, with the mark's
    /// cause, off the caller's path, each retirement in its own task to its
    /// end. A retirement that fails leaves the mark, and the next start, or
    /// a retry of the change, retires the member again.
    pub(super) fn redrive_owed_entry_endings(&self) {
        let owed = super::identity_entry::owed_entry_endings(&self.machine_state_watch_rx.borrow());
        for identity in owed {
            // The mob's own obligation, whoever started it.
            let owner = self.non_owning().with_command_authority(
                crate::control_policy::CommandAuthority::principal(
                    crate::control_policy::MobControlPrincipal::Owner,
                ),
            );
            crate::tokio::spawn(async move {
                if let Err(error) = owner.retire(identity.clone()).await {
                    tracing::warn!(
                        mob_id = %owner.mob_id(),
                        agent_identity = %identity,
                        error = %error,
                        "an identity's owed ending did not retire its member"
                    );
                }
            });
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
                record(CreatedIdentityPresence::PendingPreparation {
                    attempt: 3,
                    constructed: false,
                }),
                mob_dsl::RecoveredPresence::PresentUnseated,
            ),
            (
                record(CreatedIdentityPresence::Removed),
                mob_dsl::RecoveredPresence::Tombstone,
            ),
            (
                record(CreatedIdentityPresence::Archived {
                    archived_session: None,
                }),
                mob_dsl::RecoveredPresence::Archived,
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
