//! The actor's operator-instruction steps. MobMachine decides each one:
//!
//! - a write or revoke: its verdict, and for a committed record the journal
//!   row (store-first, before the prepared transition commits);
//! - a delivery's preparation for one owed member: the sealed snapshot the
//!   delivery owner records in the member's session;
//! - a delivery wave's feedback: each member session's record or refusal,
//!   the recorded ones journaled store-first in one row;
//! - a retiring member's release verdict.
//!
//! See `runtime::operator_instruction_delivery` for the delivery owner and
//! the status read.

use super::*;
use crate::operator_instruction::{
    MemberInstructionDelivery, MemberInstructionRelease, MemberInstructionTargetsRecordedEvent,
    OperatorInstructionCommittedEvent, OperatorInstructionDisposition, OperatorInstructionError,
    OperatorInstructionReceipt, OperatorInstructionTarget, OperatorInstructionWrite,
    RecordedMemberInstructionTarget,
};
use crate::runtime::operator_instruction_delivery::{
    MemberInstructionDeliveryOutcome, MemberInstructionPreparation, matched_members,
    member_instruction_record, member_profile,
};
use crate::runtime::placed_member_instruction::{
    placed_member_host, placed_member_instruction_route, release_placed_member_instruction_target,
};
use meerkat_core::{
    InstructionContentDigest, InstructionRevision, InstructionSlot, InstructionWriteError,
    MAX_INSTRUCTION_BODY_BYTES, MAX_OPERATOR_INSTRUCTION_ACTIVE_BYTES,
};

/// How long a destroy waits for one member's operator-instruction release.
const DESTROY_INSTRUCTION_RELEASE_BOUND: std::time::Duration = std::time::Duration::from_secs(10);

/// Realize a member's authorized operator-instruction release within
/// `deadline`, for a retirement (its own deadline) and a destroy
/// ([`DESTROY_INSTRUCTION_RELEASE_BOUND`]). Past the deadline the step fails
/// typed and retryable (`MobError::RetirementInProgress` at stage
/// `operator_instruction_target_release`), its member still seated, and a
/// re-drive or a retried destroy releases the target again. A release that
/// lands answers at once: the bound never delays it.
pub(super) async fn bounded_member_instruction_release(
    deadline: Instant,
    session_id: SessionId,
    realize: impl std::future::Future<Output = Result<(), MobError>>,
) -> Result<(), MobError> {
    match tokio::time::timeout(deadline.saturating_duration_since(Instant::now()), realize).await {
        Ok(result) => result,
        Err(_) => Err(MobError::RetirementInProgress {
            session_id,
            stage: "operator_instruction_target_release".to_string(),
        }),
    }
}

/// What MobMachine answered a write or revoke.
enum WriteVerdict {
    Committed { generation: u64 },
    Duplicate { generation: u64 },
    Refused(OperatorInstructionError),
}

impl MobActor {
    /// A governed runtime (its machine has a native work authorization host)
    /// admits an operator-instruction write only with prepared
    /// authorization, which nothing mints for one yet: a write or revoke is
    /// refused before MobMachine sees it, so nothing is journaled and no
    /// member is owed. The refusal is final; a retry is refused the same
    /// way.
    fn refuse_operator_instruction_write_on_governed_runtime(
        &self,
    ) -> Result<(), OperatorInstructionError> {
        #[cfg(feature = "runtime-adapter")]
        {
            if self
                .runtime_adapter
                .as_ref()
                .is_some_and(|runtime| runtime.has_native_work_authorization_host())
            {
                return Err(OperatorInstructionError::Refused(
                    InstructionWriteError::PreparedAuthorizationRequired,
                ));
            }
        }
        Ok(())
    }

    /// Commit one operator-instruction write (`SetOperatorInstruction`).
    pub(super) async fn set_operator_instruction(
        &mut self,
        write: OperatorInstructionWrite,
        digest: InstructionContentDigest,
    ) -> Result<OperatorInstructionReceipt, OperatorInstructionError> {
        self.refuse_operator_instruction_write_on_governed_runtime()?;
        let key = write.slot.machine_key();
        let body_bytes = write.body.len() as u64;
        let (target_kind, target_profile, target_member) = write.target.machine_parts();
        let prepared = self.prepare_dsl_signal_transition(
            mob_dsl::MobMachineSignal::SetOperatorInstruction {
                slot: key.clone(),
                revision: write.revision.get(),
                digest: digest.as_str().to_owned(),
                body_bytes,
                target_kind,
                target_profile,
                target_member: target_member.map(mob_dsl::AgentIdentity),
            },
            "set_operator_instruction",
        )?;
        match write_verdict(&prepared.transition, &write.slot, write.revision) {
            WriteVerdict::Committed { generation } => {
                let event = OperatorInstructionCommittedEvent {
                    slot: write.slot.clone(),
                    revision: write.revision,
                    effect: meerkat_core::InstructionEffect::Active,
                    content_sha256: Some(digest.clone()),
                    body_bytes,
                    target: write.target.clone(),
                    generation,
                    body: Some(write.body.clone()),
                };
                let cursor = self
                    .journal_operator_instruction_commit(prepared, event)
                    .await?;
                self.operator_instructions.bodies.record_active(
                    &key,
                    &digest,
                    &write.body,
                    &write.target,
                    cursor,
                );
                self.operator_instructions
                    .bodies
                    .retain_recorded(self.dsl_authority.state());
                Ok(self.operator_instruction_receipt(
                    write.slot,
                    write.revision,
                    OperatorInstructionDisposition::Committed,
                    generation,
                    &key,
                ))
            }
            WriteVerdict::Duplicate { generation } => Ok(self.operator_instruction_receipt(
                write.slot,
                write.revision,
                OperatorInstructionDisposition::Duplicate,
                generation,
                &key,
            )),
            WriteVerdict::Refused(error) => Err(error),
        }
    }

    /// Revoke one operator-instruction slot (`RevokeOperatorInstruction`).
    pub(super) async fn revoke_operator_instruction(
        &mut self,
        slot: InstructionSlot,
        revision: InstructionRevision,
    ) -> Result<OperatorInstructionReceipt, OperatorInstructionError> {
        self.refuse_operator_instruction_write_on_governed_runtime()?;
        let key = slot.machine_key();
        // The record keeps its target: the journal row carries it.
        let target = self.operator_instruction_target_of(&key);
        let prepared = self.prepare_dsl_signal_transition(
            mob_dsl::MobMachineSignal::RevokeOperatorInstruction {
                slot: key.clone(),
                revision: revision.get(),
            },
            "revoke_operator_instruction",
        )?;
        match write_verdict(&prepared.transition, &slot, revision) {
            WriteVerdict::Committed { generation } => {
                let target = target?;
                let event = OperatorInstructionCommittedEvent {
                    slot: slot.clone(),
                    revision,
                    effect: meerkat_core::InstructionEffect::Absent,
                    content_sha256: None,
                    body_bytes: 0,
                    target,
                    generation,
                    body: None,
                };
                self.journal_operator_instruction_commit(prepared, event)
                    .await?;
                self.operator_instructions.bodies.record_revoked(&key);
                self.operator_instructions
                    .bodies
                    .retain_recorded(self.dsl_authority.state());
                Ok(self.operator_instruction_receipt(
                    slot,
                    revision,
                    OperatorInstructionDisposition::Committed,
                    generation,
                    &key,
                ))
            }
            WriteVerdict::Duplicate { generation } => Ok(self.operator_instruction_receipt(
                slot,
                revision,
                OperatorInstructionDisposition::Duplicate,
                generation,
                &key,
            )),
            WriteVerdict::Refused(error) => Err(error),
        }
    }

    /// Journal a committed record, then commit its prepared transition: a
    /// record the journal does not hold is never in the machine, and one it
    /// holds always is. An append that answers an error may still have
    /// landed (a store whose commit outlived its acknowledgement): the
    /// journal after the append's floor decides. A landed row commits the
    /// transition, so the next write never reuses its generation (a replay
    /// keeps the first row of a generation and drops a later one as stale);
    /// a row proven absent drops it; a journal that cannot be read stops
    /// the actor for a cold recovery, which replays whatever landed.
    /// Answers the cursor of the row the journal holds.
    async fn journal_operator_instruction_commit(
        &mut self,
        prepared: PreparedDslTransition,
        event: OperatorInstructionCommittedEvent,
    ) -> Result<u64, OperatorInstructionError> {
        let desired = MobEventKind::OperatorInstructionCommitted(event);
        let cursor_floor = self
            .events
            .latest_cursor()
            .await
            .map_err(|error| OperatorInstructionError::Mob(MobError::from(error)))?;
        let append = self
            .events
            .append(NewMobEvent {
                mob_id: self.definition.id.clone(),
                timestamp: None,
                kind: desired.clone(),
            })
            .await;
        let cursor = match append {
            Ok(appended) => appended.cursor,
            Err(error) => {
                match super::reconcile_exact_mob_event_after_cursor(
                    self.events.as_ref(),
                    &self.definition.id,
                    cursor_floor,
                    &desired,
                )
                .await
                {
                    Ok(Some(landed)) => {
                        tracing::warn!(
                            error = %error,
                            "an operator instruction row landed although its append answered an \
                             error; committing it"
                        );
                        landed.cursor
                    }
                    Ok(None) => return Err(OperatorInstructionError::Mob(MobError::from(error))),
                    Err(reconcile_error) => {
                        return Err(OperatorInstructionError::Mob(
                            self.durable_uncertainty_fail_stop
                                .record(MobError::Internal(format!(
                                    "an operator instruction append failed ({error}) and the \
                                     journal could not tell whether it landed; the actor stops \
                                     for a cold recovery: {reconcile_error}"
                                ))),
                        ));
                    }
                }
            }
        };
        if let Err(error) = self.commit_prepared_dsl_transition(prepared) {
            return Err(OperatorInstructionError::Mob(
                self.durable_uncertainty_fail_stop
                    .record(MobError::Internal(format!(
                        "an operator instruction row is journaled but MobMachine did not commit \
                         it; the actor stops for a cold recovery: {error}"
                    ))),
            ));
        }
        Ok(cursor)
    }

    fn operator_instruction_target_of(
        &self,
        key: &str,
    ) -> Result<OperatorInstructionTarget, OperatorInstructionError> {
        let state = self.dsl_authority.state();
        let kind = state
            .operator_instruction_target_kind
            .get(key)
            .copied()
            .unwrap_or_default();
        let profile = state
            .operator_instruction_target_profile
            .get(key)
            .map_or("", String::as_str);
        let member = state
            .operator_instruction_target_member
            .get(key)
            .map(|member| member.0.as_str());
        OperatorInstructionTarget::from_machine_parts(kind, profile, member)
            .map_err(OperatorInstructionError::Mob)
    }

    /// The bodies `agent_identity`'s snapshot reads: the shared records'
    /// held bodies, and the member's own, read from their journal rows. A
    /// member holds few records of its own, so this reads few rows whatever
    /// the mob holds.
    pub(super) async fn member_instruction_bodies(
        &self,
        agent_identity: &AgentIdentity,
    ) -> Result<crate::runtime::operator_instruction_delivery::OperatorInstructionBodies, MobError>
    {
        let reads = self
            .operator_instructions
            .bodies
            .member_journal_reads(self.dsl_authority.state(), agent_identity);
        let read = crate::runtime::operator_instruction_delivery::read_journaled_bodies(
            self.events.as_ref(),
            reads,
        )
        .await?;
        Ok(self.operator_instructions.bodies.for_member(read))
    }

    fn operator_instruction_receipt(
        &self,
        slot: InstructionSlot,
        revision: InstructionRevision,
        disposition: OperatorInstructionDisposition,
        generation: u64,
        key: &str,
    ) -> OperatorInstructionReceipt {
        OperatorInstructionReceipt {
            mob_id: self.definition.id.clone(),
            slot,
            revision,
            disposition,
            generation,
            matched_members: matched_members(self.dsl_authority.state(), key),
        }
    }

    /// Prepare one owed member's delivery
    /// (`PrepareMemberInstructionDelivery`): the sealed snapshot at the
    /// machine's generation for the member's bound session, when the machine
    /// delivers now. A placed member's delivery carries its route to its
    /// member host, from the same machine state. The member's own bodies
    /// are read from their journal rows here; the actor serves no other
    /// command meanwhile, so the snapshot is of one machine state.
    pub(super) async fn prepare_member_instruction_delivery(
        &mut self,
        agent_identity: &AgentIdentity,
    ) -> Result<MemberInstructionPreparation, MobError> {
        self.prepare_member_instruction_delivery_with(agent_identity, None)
            .await
    }

    /// [`Self::prepare_member_instruction_delivery`] over bodies the caller
    /// already holds for the member (`own_bodies`: the shared records' and
    /// the member's own, read from their journal rows off the actor), so
    /// this step reads no journal row. A record of the member written since
    /// those rows were read has no body there: the snapshot cannot be built,
    /// the step answers the error, and the member stays owed.
    pub(super) async fn prepare_member_instruction_delivery_with(
        &mut self,
        agent_identity: &AgentIdentity,
        own_bodies: Option<
            crate::runtime::operator_instruction_delivery::OperatorInstructionBodies,
        >,
    ) -> Result<MemberInstructionPreparation, MobError> {
        let transition = self.apply_dsl_signal_collect_transition(
            mob_dsl::MobMachineSignal::PrepareMemberInstructionDelivery {
                agent_identity: mob_dsl::AgentIdentity::from_domain(agent_identity),
            },
            "prepare_member_instruction_delivery",
        )?;
        let prepared = transition.effects().iter().find_map(|effect| match effect {
            mob_dsl::MobMachineEffect::MemberInstructionDeliveryPrepared {
                session_id,
                generation,
                ..
            } => Some((session_id.clone(), *generation)),
            _ => None,
        });
        let Some((session_id, generation)) = prepared else {
            return Ok(MemberInstructionPreparation::NotNow);
        };
        let session_id = SessionId::parse(&session_id.0).map_err(|error| {
            MobError::Internal(format!(
                "MobMachine prepared an operator instruction delivery for member \
                 {agent_identity} on a session id that is not one: {error}"
            ))
        })?;
        let bodies = match own_bodies {
            Some(bodies) => bodies,
            None => self.member_instruction_bodies(agent_identity).await?,
        };
        let state = self.dsl_authority.state();
        let mut record = member_instruction_record(
            state,
            &bodies,
            None,
            &self.definition.id,
            agent_identity,
            &member_profile(state, agent_identity),
        )?;
        record.generation = generation;
        let route = placed_member_instruction_route(
            state,
            &self.definition.id,
            agent_identity,
            &session_id,
        )?;
        let delivery = MemberInstructionDelivery::authorized(session_id, record);
        Ok(match route {
            Some(route) => MemberInstructionPreparation::DeliverPlaced { delivery, route },
            None => MemberInstructionPreparation::Deliver(delivery),
        })
    }

    /// Feed one delivery wave's outcomes back to MobMachine
    /// (`RecordMemberInstructionTarget`, `RefuseMemberInstructionTarget`) in
    /// one prepared step. The members the machine records are journaled in
    /// one row before the step commits, so a replay closes exactly the
    /// obligations the machine closed.
    pub(super) async fn settle_member_instruction_deliveries(
        &mut self,
        outcomes: Vec<MemberInstructionDeliveryOutcome>,
    ) -> Result<(), MobError> {
        let mut authority = self.dsl_authority.prepare_authority();
        let mut effects = Vec::new();
        let mut recorded = Vec::new();
        for outcome in outcomes {
            let signal = match outcome {
                MemberInstructionDeliveryOutcome::Recorded {
                    agent_identity,
                    session_id,
                    generation,
                } => mob_dsl::MobMachineSignal::RecordMemberInstructionTarget {
                    agent_identity: mob_dsl::AgentIdentity::from_domain(&agent_identity),
                    session_id: mob_dsl::SessionId::from_domain(&session_id),
                    generation,
                },
                MemberInstructionDeliveryOutcome::Blocked {
                    agent_identity,
                    session_id,
                    generation,
                    block,
                } => mob_dsl::MobMachineSignal::RefuseMemberInstructionTarget {
                    agent_identity: mob_dsl::AgentIdentity::from_domain(&agent_identity),
                    session_id: mob_dsl::SessionId::from_domain(&session_id),
                    generation,
                    block,
                },
            };
            let transition = authority.apply_signal(signal).map_err(|error| {
                MobError::Internal(format!(
                    "MobMachine refused operator instruction delivery feedback: {error}"
                ))
            })?;
            for effect in transition.effects() {
                if let mob_dsl::MobMachineEffect::MemberInstructionTargetRecorded {
                    agent_identity,
                    generation,
                    ..
                } = effect
                {
                    recorded.push(RecordedMemberInstructionTarget {
                        agent_identity: AgentIdentity::from(agent_identity.0.as_str()),
                        generation: *generation,
                    });
                }
            }
            effects.extend(transition.into_effects());
        }
        let prepared = PreparedDslInput {
            authority,
            effects,
            phase_changed: false,
        };
        if recorded.is_empty() {
            return self.commit_prepared_dsl_input(prepared);
        }
        let events = Arc::clone(&self.events);
        let mob_id = self.definition.id.clone();
        self.commit_prepared_dsl_input_after(prepared, || async move {
            events
                .append(NewMobEvent {
                    mob_id,
                    timestamp: None,
                    kind: MobEventKind::MemberInstructionTargetsRecorded(
                        MemberInstructionTargetsRecordedEvent { members: recorded },
                    ),
                })
                .await
                .map(|_| ())
                .map_err(MobError::from)
        })
        .await
    }

    /// Carry the member's snapshot in the session its build mints
    /// (`operator_instruction_delivery::carry_member_instruction_birth`).
    /// The member's own bodies are read from their journal rows only when
    /// the build carries a snapshot.
    pub(super) async fn carry_member_instruction_birth(
        &self,
        request: &mut meerkat_core::service::CreateSessionRequest,
        agent_identity: &AgentIdentity,
        profile_name: &str,
    ) -> Result<(), MobError> {
        if !crate::runtime::operator_instruction_delivery::birth_carries_member_instructions(
            request,
            self.dsl_authority.state(),
        ) {
            return Ok(());
        }
        let bodies = self.member_instruction_bodies(agent_identity).await?;
        crate::runtime::operator_instruction_delivery::carry_member_instruction_birth(
            request,
            self.dsl_authority.state(),
            &bodies,
            None,
            &self.definition.id,
            agent_identity,
            profile_name,
        )
    }

    /// The snapshot a placed member a `Fresh` spawn mints is born with, for
    /// its materialize (V7): its whole snapshot at the mob's generation,
    /// which its member host records in the session it mints.
    pub(super) async fn member_instruction_birth_target(
        &self,
        agent_identity: &AgentIdentity,
        profile_name: &str,
    ) -> Result<crate::runtime::bridge_protocol::BridgeInstructionTarget, MobError> {
        let bodies = self.member_instruction_bodies(agent_identity).await?;
        crate::runtime::operator_instruction_delivery::member_instruction_record(
            self.dsl_authority.state(),
            &bodies,
            None,
            &self.definition.id,
            agent_identity,
            profile_name,
        )
        .map(|record| {
            crate::runtime::bridge_protocol::BridgeInstructionTarget::from_record(&record)
        })
    }

    /// The release verdict of a retiring member
    /// (`ResolveMemberInstructionRelease`) for the session it retires from:
    /// the release to realize, when MobMachine authorizes one. A member
    /// without a session has no target to release. A placed member's roster
    /// reference may not copy its session: its session is the one MobMachine
    /// binds it to (or moved to its pending retirement).
    pub(super) fn resolve_member_instruction_release(
        &mut self,
        agent_identity: &AgentIdentity,
        session_id: Option<&SessionId>,
        disposal: mob_dsl::MemberSessionDisposal,
    ) -> Result<Option<MemberInstructionRelease>, MobError> {
        let session_id = match session_id {
            Some(session_id) => session_id.clone(),
            None => match self.placed_member_machine_session(agent_identity) {
                Some(session_id) => session_id,
                None => return Ok(None),
            },
        };
        let session_id = &session_id;
        let transition = self.apply_dsl_signal_collect_transition(
            mob_dsl::MobMachineSignal::ResolveMemberInstructionRelease {
                agent_identity: mob_dsl::AgentIdentity::from_domain(agent_identity),
                session_id: mob_dsl::SessionId::from_domain(session_id),
                disposal,
            },
            "resolve_member_instruction_release",
        )?;
        let authorized = transition.effects().iter().find_map(|effect| match effect {
            mob_dsl::MobMachineEffect::MemberInstructionReleaseAuthorized {
                session_id, ..
            } => Some(session_id.clone()),
            _ => None,
        });
        let Some(session_id) = authorized else {
            return Ok(None);
        };
        let session_id = SessionId::parse(&session_id.0).map_err(|error| {
            MobError::Internal(format!(
                "MobMachine authorized an operator instruction release for member \
                 {agent_identity} on a session id that is not one: {error}"
            ))
        })?;
        Ok(Some(MemberInstructionRelease::authorized(
            session_id,
            self.definition.id.clone(),
        )))
    }

    /// The session MobMachine binds a placed member to, or the one its
    /// releasing retire moved to the runtime's pending retirement; `None`
    /// for a member that is not placed.
    fn placed_member_machine_session(&self, agent_identity: &AgentIdentity) -> Option<SessionId> {
        let state = self.dsl_authority.state();
        let identity = mob_dsl::AgentIdentity::from_domain(agent_identity);
        if !state.member_placement.contains_key(&identity) {
            return None;
        }
        let session = state.member_session_bindings.get(&identity).or_else(|| {
            state
                .identity_to_runtime
                .get(&identity)
                .and_then(|runtime| state.runtime_retire_pending_sessions.get(runtime))
        })?;
        SessionId::parse(&session.0).ok()
    }

    /// A destroy's release of one member's operator-instruction target,
    /// before the member's seat clears. The wait is bounded: a destroy never
    /// waits long on a session's turn boundary. A release that does not land
    /// (refused, failed or past its bound) fails the member's destroy step
    /// typed, so the member stays seated and MobMachine keeps its facts, and
    /// a retried destroy decides and realizes the same release again.
    pub(super) async fn release_member_instruction_target_for_destroy(
        &mut self,
        agent_identity: &AgentIdentity,
        session_id: Option<&SessionId>,
        disposal: mob_dsl::MemberSessionDisposal,
    ) -> Result<(), MobError> {
        let Some(release) =
            self.resolve_member_instruction_release(agent_identity, session_id, disposal)?
        else {
            return Ok(());
        };
        let released_session = release.session_id().clone();
        let realize = self.member_instruction_release_future(agent_identity, release);
        bounded_member_instruction_release(
            Instant::now() + DESTROY_INSTRUCTION_RELEASE_BOUND,
            released_session,
            realize,
        )
        .await
    }

    /// Realize a retiring member's authorized release. The member's delivery
    /// lock is taken first, so a delivery in flight lands before the release
    /// and none after it. A target this mob does not own, or a service that
    /// keeps no durable document, has nothing to release. A placed member's
    /// target is released on its member host, over the supervisor bridge.
    pub(super) fn member_instruction_release_future(
        &self,
        agent_identity: &AgentIdentity,
        release: MemberInstructionRelease,
    ) -> impl std::future::Future<Output = Result<(), MobError>> + 'static {
        let service = Arc::clone(&self.session_service);
        let lock = self
            .operator_instructions
            .delivery
            .member_lock(agent_identity);
        let placed = placed_member_host(self.dsl_authority.state(), agent_identity);
        let bridge = Arc::clone(&self.supervisor_bridge);
        let identity = agent_identity.clone();
        async move {
            // A delivery prepared before this release writes nothing after
            // it: the release moves the member's epoch on under its lock.
            let mut epoch = lock.lock().await;
            *epoch = epoch.wrapping_add(1);
            let session_id = release.session_id().clone();
            match placed {
                Some(Ok(host)) => {
                    return release_placed_member_instruction_target(
                        &bridge,
                        &host,
                        release.mob_id(),
                        &identity,
                        &session_id,
                    )
                    .await;
                }
                // A placed member whose host has no current binding cannot
                // be reached: nothing is released, and the retirement goes
                // on (a revoked host released its members with their
                // sessions).
                Some(Err(error)) => {
                    tracing::warn!(
                        session_id = %session_id,
                        error = %error,
                        "the retiring placed member's host cannot be reached: its operator \
                         instruction target is not released"
                    );
                    return Ok(());
                }
                None => {}
            }
            match service.release_member_instruction_target(release).await {
                Ok(_)
                | Err(
                    meerkat_core::service::SessionInstructionError::Refused(
                        InstructionWriteError::OwnedBySession
                        | InstructionWriteError::OwnedByOtherMob,
                    )
                    | meerkat_core::service::SessionInstructionError::Session(
                        meerkat_core::service::SessionError::NotFound { .. },
                    ),
                ) => Ok(()),
                // A session whose recorded target or instruction history does
                // not read holds no target this release can clear: as for a
                // session that is gone, there is nothing to release, so a
                // retirement or a destroy never waits on it.
                Err(meerkat_core::service::SessionInstructionError::Refused(
                    InstructionWriteError::Malformed(error),
                )) => {
                    tracing::warn!(
                        session_id = %session_id,
                        error = %error,
                        "the member session's operator instruction state does not read: there \
                         is no target to release"
                    );
                    Ok(())
                }
                // A service that keeps no durable document holds no target to
                // release; so does a session service wrapper that does not
                // forward the release (its members' deliveries were blocked
                // the same way).
                Err(meerkat_core::service::SessionInstructionError::DurabilityUnavailable) => {
                    tracing::warn!(
                        session_id = %session_id,
                        "the mob session service released no operator instruction target: it \
                         keeps no durable session document, or a wrapper does not forward \
                         release_member_instruction_target"
                    );
                    Ok(())
                }
                Err(error) => Err(MobError::Internal(format!(
                    "the retiring member's operator instruction target could not be released: \
                     {error}"
                ))),
            }
        }
    }
}

/// MobMachine's verdict on a write or revoke, from its prepared transition.
fn write_verdict(
    transition: &mob_dsl::MobMachineTransition,
    slot: &InstructionSlot,
    revision: InstructionRevision,
) -> WriteVerdict {
    for effect in transition.effects() {
        match effect {
            mob_dsl::MobMachineEffect::OperatorInstructionCommitted { generation, .. } => {
                return WriteVerdict::Committed {
                    generation: *generation,
                };
            }
            mob_dsl::MobMachineEffect::OperatorInstructionDuplicate { generation, .. } => {
                return WriteVerdict::Duplicate {
                    generation: *generation,
                };
            }
            mob_dsl::MobMachineEffect::OperatorInstructionRefused {
                refusal,
                current_revision,
                ..
            } => {
                return WriteVerdict::Refused(refusal_error(
                    slot,
                    revision,
                    *refusal,
                    *current_revision,
                ));
            }
            _ => {}
        }
    }
    WriteVerdict::Refused(OperatorInstructionError::Mob(MobError::Internal(
        "MobMachine answered an operator instruction write with no verdict".to_string(),
    )))
}

fn refusal_error(
    slot: &InstructionSlot,
    revision: InstructionRevision,
    refusal: mob_dsl::OperatorInstructionRefusal,
    current_revision: Option<u64>,
) -> OperatorInstructionError {
    use mob_dsl::OperatorInstructionRefusal as Refusal;
    let refused = |error| OperatorInstructionError::Refused(error);
    match refusal {
        Refusal::StaleRevision => refused(InstructionWriteError::StaleRevision {
            revision,
            current: current_revision
                .and_then(|current| InstructionRevision::new(current).ok())
                .unwrap_or(revision),
        }),
        Refusal::RevisionConflict => refused(InstructionWriteError::RevisionConflict { revision }),
        Refusal::UnknownSlot => refused(InstructionWriteError::UnknownSlot { slot: slot.clone() }),
        Refusal::SlotLimitExceeded => refused(InstructionWriteError::SlotLimitExceeded),
        Refusal::BodyTooLarge => refused(InstructionWriteError::BodyTooLarge {
            max_bytes: MAX_INSTRUCTION_BODY_BYTES,
        }),
        Refusal::ActiveSetBudgetExceeded => {
            refused(InstructionWriteError::ActiveSetBudgetExceeded {
                max_bytes: MAX_OPERATOR_INSTRUCTION_ACTIVE_BYTES,
            })
        }
        Refusal::MobTerminal => OperatorInstructionError::MobTerminal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bound on a member's release, which a retirement takes from its own
    /// deadline: a release that has not landed by the deadline fails the
    /// step typed and retryable at the release stage, and one that lands
    /// answers its own result at once.
    #[tokio::test]
    async fn a_release_past_its_deadline_fails_typed_and_retryable() {
        let session_id = SessionId::new();
        let deadline = Instant::now();
        let error = bounded_member_instruction_release(
            deadline,
            session_id.clone(),
            std::future::pending::<Result<(), MobError>>(),
        )
        .await
        .expect_err("a release that never lands is bounded by its deadline");
        assert!(
            matches!(
                &error,
                MobError::RetirementInProgress { session_id: released, stage }
                    if released == &session_id && stage == "operator_instruction_target_release"
            ),
            "the step fails typed and retryable: {error:?}"
        );
        bounded_member_instruction_release(deadline, session_id, std::future::ready(Ok(())))
            .await
            .expect("a release that lands answers at once, past its deadline or not");
    }
}
