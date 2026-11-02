//! Store-owned identity changes on the actor: a create's plan, its commit
//! step and the host's changes hold (see [`super::super::identity_change`]).
//!
//! The plan probes the change on a fork of the authority: its admission
//! (`AdmitIdentityChange`), its new entries' records
//! (`RecordCreatedIdentityEntry`), and each seat's next generation and
//! admission under created custody, so the seats it plans are the ones the
//! commit stages. The commit step decides it again on one prepared
//! authority, from the facts it observes there (the store's receipt of the
//! operation and the requester's grant, checked current at this final sink):
//! the admission, then the records, then the seats, all or nothing, and
//! writes the change's one store transaction before the machine commits.

use super::super::dormant_seat::DormantSeatPlanEntry;
use super::super::identity_change::{
    ChangeAdmissionFacts, CreateCommit, CreateCommitted, CreatePlan, CreatePlanAnswer,
    CreatePlanEntry, CreatedIdentityPlanItem, EntryChangeCommit, EntryChangeCommitted,
    IdentityChangeCommand, IdentityChangeGrant, IdentityChangeRefusal, IdentityOperation,
    admission_refusal, committed_change_events, create_rows_and_receipt, entry_change_answer,
    entry_change_row, grant_current_at_the_sink, now_ms, record_refusal, refused,
};
use super::dormant_seat::{SeatCommitScope, SeatCommitWrite, SeatPlanMode};
use super::*;
use crate::identity::{IdentityChangeReceipt, IdentityChangeReceiptObservation};
use crate::machines::mob_machine::MobMachineMutator as _;

/// What a change's admission decided on its authority.
enum ChangeAdmission {
    Admitted,
    Replayed(IdentityChangeReceipt),
}

impl MobActor {
    /// Serve one step of a store-owned identity change.
    pub(super) async fn handle_identity_change_command(&mut self, command: IdentityChangeCommand) {
        match command {
            IdentityChangeCommand::PlanCreate {
                operation,
                facts,
                grant,
                decided,
                items,
                reply_tx,
            } => {
                let result = Box::pin(self.plan_store_owned_create(
                    &operation,
                    facts,
                    grant.as_ref(),
                    decided,
                    items,
                ))
                .await;
                let _ = reply_tx.send(result);
            }
            IdentityChangeCommand::CommitCreate { commit, reply_tx } => {
                let result = Box::pin(self.commit_store_owned_create(*commit)).await;
                let _ = reply_tx.send(result);
            }
            IdentityChangeCommand::CommitEntryChange { commit, reply_tx } => {
                let result = Box::pin(self.commit_identity_entry_change(*commit)).await;
                let _ = reply_tx.send(result);
            }
            IdentityChangeCommand::SetHeld { held, reply_tx } => {
                let result = self.apply_dsl_input(
                    mob_dsl::MobMachineInput::SetIdentityChangesHeld { held },
                    "set_identity_changes_held",
                );
                if result.is_ok() {
                    self.publish_machine_state_projection();
                }
                let _ = reply_tx.send(result);
            }
        }
    }

    /// The store's receipt of `operation`, as the machine reads it: none, the
    /// same request's (the receipt itself, which a replay answers), or
    /// another request's.
    async fn observe_change_receipt(
        &self,
        operation: &IdentityOperation,
    ) -> Result<
        (
            mob_dsl::IdentityChangeReceiptKind,
            Option<IdentityChangeReceipt>,
        ),
        MobError,
    > {
        let store = self
            .identity_member
            .as_ref()
            .ok_or_else(|| refused(IdentityChangeRefusal::StorageUnsupported))?;
        let stored = store
            .observe_identity_change_receipt(&self.definition.id, &operation.operation_id)
            .await?;
        Ok(
            match IdentityChangeReceiptObservation::of(stored, &operation.request_digest) {
                IdentityChangeReceiptObservation::Absent => {
                    (mob_dsl::IdentityChangeReceiptKind::Absent, None)
                }
                IdentityChangeReceiptObservation::Same(receipt) => {
                    (mob_dsl::IdentityChangeReceiptKind::Same, Some(receipt))
                }
                IdentityChangeReceiptObservation::Other(_) => {
                    (mob_dsl::IdentityChangeReceiptKind::Other, None)
                }
            },
        )
    }

    /// MobMachine's admission of a change on `authority`, from its facts:
    /// the store's receipt, read now, and the requester's grant, checked
    /// current now (a host or human request carries none).
    async fn decide_change_admission<M: mob_dsl::MobMachineMutator>(
        &self,
        authority: &mut M,
        operation: &IdentityOperation,
        facts: ChangeAdmissionFacts,
        grant: Option<&IdentityChangeGrant>,
        decided: bool,
    ) -> Result<ChangeAdmission, MobError> {
        let (receipt, replayed) = self.observe_change_receipt(operation).await?;
        let grant_current = match grant {
            Some(grant) => grant_current_at_the_sink(grant, decided)?,
            None => true,
        };
        let transition = authority
            .apply(mob_dsl::MobMachineInput::AdmitIdentityChange {
                receipt,
                consent: facts.consent,
                permit_binds: facts.permit_binds,
                permit_admits: facts.permit_admits,
                host_operator: facts.host_operator,
                grant_current,
            })
            .map_err(|error| MobError::MobMachineRejected {
                context: "admit_identity_change",
                reason: error.to_string(),
            })?;
        let verdict = transition
            .effects()
            .iter()
            .find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::IdentityChangeAdmissionResolved { verdict } => {
                    Some(*verdict)
                }
                _ => None,
            })
            .ok_or_else(|| {
                MobError::Internal("MobMachine admitted an identity change with no verdict".into())
            })?;
        if let Some(refusal) = admission_refusal(verdict, &operation.operation_id) {
            return Err(refused(refusal));
        }
        match (verdict, replayed) {
            (mob_dsl::IdentityChangeAdmissionKind::Replayed, Some(receipt)) => {
                Ok(ChangeAdmission::Replayed(receipt))
            }
            (mob_dsl::IdentityChangeAdmissionKind::Replayed, None) => Err(MobError::Internal(
                "MobMachine replayed an identity change with no receipt".into(),
            )),
            _ => Ok(ChangeAdmission::Admitted),
        }
    }

    /// Record each new store-owned entry on `authority`, in order; the first
    /// identity MobMachine refuses refuses the change.
    fn record_created_entries<M: mob_dsl::MobMachineMutator>(
        authority: &mut M,
        identities: impl IntoIterator<Item = (AgentIdentity, mob_dsl::UnseatedEntryCause)>,
    ) -> Result<(), MobError> {
        for (agent_identity, cause) in identities {
            let transition = authority
                .apply(mob_dsl::MobMachineInput::RecordCreatedIdentityEntry {
                    agent_identity: mob_dsl::AgentIdentity::from_domain(&agent_identity),
                    cause,
                })
                .map_err(|error| MobError::MobMachineRejected {
                    context: "record_created_identity_entry",
                    reason: error.to_string(),
                })?;
            let verdict = transition
                .effects()
                .iter()
                .find_map(|effect| match effect {
                    mob_dsl::MobMachineEffect::CreatedIdentityEntryRecorded { verdict, .. } => {
                        Some(*verdict)
                    }
                    _ => None,
                })
                .ok_or_else(|| {
                    MobError::Internal(
                        "MobMachine recorded a store-owned entry with no verdict".into(),
                    )
                })?;
            if let Some(refusal) = record_refusal(verdict, &agent_identity) {
                return Err(refusal.map_or_else(|error| error, refused));
            }
        }
        Ok(())
    }

    /// Plan a store-owned create: its admission and its entries' records
    /// probed on a fork, and each identity's seat planned under created
    /// custody as the dormant seat plans one (a spec that owes a kickoff is
    /// embodied by a spawn after the commit).
    async fn plan_store_owned_create(
        &mut self,
        operation: &IdentityOperation,
        facts: ChangeAdmissionFacts,
        grant: Option<&IdentityChangeGrant>,
        decided: bool,
        items: Vec<CreatedIdentityPlanItem>,
    ) -> Result<CreatePlanAnswer, MobError> {
        let mut probe = self.dsl_authority.fork();
        match self
            .decide_change_admission(&mut probe, operation, facts, grant, decided)
            .await?
        {
            ChangeAdmission::Replayed(receipt) => return Ok(CreatePlanAnswer::Replayed(receipt)),
            ChangeAdmission::Admitted => {}
        }
        Self::record_created_entries(
            &mut probe,
            items
                .iter()
                .map(|item| (item.spec.identity.clone(), item.unseated_cause())),
        )?;
        let mut entries = Vec::with_capacity(items.len());
        for item in items {
            if item.unseated_cause() == mob_dsl::UnseatedEntryCause::PreparationPending {
                entries.push(CreatePlanEntry::Preparing);
                continue;
            }
            // The seat's next generation and its admission under created
            // custody, probed on the fork that holds the new entries.
            let dsl_identity = mob_dsl::AgentIdentity::from_domain(&item.spec.identity);
            let generation = probe
                .apply(mob_dsl::MobMachineInput::ComputeRespawnGeneration {
                    agent_identity: dsl_identity.clone(),
                })
                .map_err(|error| {
                    MobError::Internal(format!(
                        "DSL authority prepared batch (compute_respawn_generation) rejected: {error}"
                    ))
                })
                .and_then(|transition| {
                    Self::respawn_generation_from_effects(transition.effects())
                });
            let admission = probe
                .apply(mob_dsl::MobMachineInput::ProbeMemberAdmission {
                    agent_identity: dsl_identity,
                    entry_custody: false,
                    created_custody: true,
                })
                .map_err(|error| {
                    MobError::Internal(format!(
                        "DSL authority prepared batch (probe_member_admission) rejected: {error}"
                    ))
                })
                .and_then(|transition| {
                    Self::member_admission_verdict_from_effects(transition.effects())
                });
            let planned = self
                .plan_dormant_seat(
                    item.spec,
                    SeatPlanMode::Seat,
                    Some(SeatChunkProbe {
                        generation,
                        admission,
                    }),
                )
                .await?;
            entries.push(match planned {
                DormantSeatPlanEntry::Seat(candidate) => CreatePlanEntry::Seat(candidate),
                DormantSeatPlanEntry::Embody(spec) => CreatePlanEntry::Embody(spec),
                DormantSeatPlanEntry::Done(outcome) => {
                    return Err(outcome.into_unseated_error());
                }
                DormantSeatPlanEntry::AlreadySeated { agent_identity, .. } => {
                    return Err(refused(IdentityChangeRefusal::NameTaken {
                        identity: agent_identity,
                        by: super::super::IdentityNameHolder::Identity,
                    }));
                }
            });
        }
        Ok(CreatePlanAnswer::Planned(CreatePlan {
            entries,
            endpoint_source: self.seat_endpoint_source(),
        }))
    }

    /// Commit a store-owned create in one actor step: MobMachine decides its
    /// admission (the requester's grant checked current here, the final
    /// sink), records each new entry and stages each seat on one prepared
    /// authority, all or nothing; the change's store transaction writes its
    /// identities' rows, the seats' journal rows and its receipt; and only
    /// then does the machine commit. A refusal anywhere writes nothing.
    async fn commit_store_owned_create(
        &mut self,
        commit: CreateCommit,
    ) -> Result<CreateCommitted, MobError> {
        let store = self
            .identity_member
            .clone()
            .ok_or_else(|| refused(IdentityChangeRefusal::StorageUnsupported))?;
        let mut authority = self.dsl_authority.prepare_authority();
        match self
            .decide_change_admission(
                &mut authority,
                &commit.operation,
                commit.facts,
                commit.grant.as_ref(),
                commit.decided,
            )
            .await?
        {
            ChangeAdmission::Replayed(receipt) => return Ok(CreateCommitted::Replayed(receipt)),
            ChangeAdmission::Admitted => {}
        }
        Self::record_created_entries(
            &mut authority,
            commit
                .identities
                .iter()
                .map(|(request, cause)| (request.spec.identity.clone(), *cause)),
        )?;
        let (rows, receipt) = create_rows_and_receipt(&self.definition.id, &commit, now_ms())?;
        let CreateCommit { seats, .. } = commit;
        let staged = self
            .stage_seat_commit(authority, seats, SeatCommitScope::AllOrNothing)
            .await?;
        self.finish_seat_commit(
            staged,
            SeatCommitWrite::IdentityChange {
                store,
                rows,
                receipt: receipt.clone(),
            },
        )
        .await?;
        Ok(CreateCommitted::Committed(receipt))
    }

    /// Commit one identity's removal, archive or unarchive in one actor
    /// step: MobMachine decides the change's admission and its verdict on one
    /// prepared authority; the change's store transaction writes its receipt
    /// and the identity's row (the tombstone, the archived row, or the row
    /// present again); and only then does the machine commit. The member's
    /// retirement a Retiring or Archiving verdict owes runs after, on the
    /// handle, and its start row records the mark's cause.
    async fn commit_identity_entry_change(
        &mut self,
        commit: EntryChangeCommit,
    ) -> Result<EntryChangeCommitted, MobError> {
        let store = self
            .identity_member
            .clone()
            .ok_or_else(|| refused(IdentityChangeRefusal::StorageUnsupported))?;
        let mut authority = self.dsl_authority.prepare_authority();
        match self
            .decide_change_admission(
                &mut authority,
                &commit.operation,
                commit.facts,
                commit.grant.as_ref(),
                commit.decided,
            )
            .await?
        {
            ChangeAdmission::Replayed(receipt) => {
                return Ok(EntryChangeCommitted::Replayed(receipt));
            }
            ChangeAdmission::Admitted => {}
        }
        let agent_identity = mob_dsl::AgentIdentity::from_domain(&commit.identity);
        let input = match commit.kind {
            mob_dsl::IdentityEntryChangeKind::Remove => {
                mob_dsl::MobMachineInput::RemoveIdentityEntry {
                    agent_identity: agent_identity.clone(),
                }
            }
            mob_dsl::IdentityEntryChangeKind::Archive => {
                mob_dsl::MobMachineInput::ArchiveIdentityEntry {
                    agent_identity: agent_identity.clone(),
                }
            }
            mob_dsl::IdentityEntryChangeKind::Unarchive => {
                mob_dsl::MobMachineInput::UnarchiveIdentityEntry {
                    agent_identity: agent_identity.clone(),
                }
            }
        };
        let transition = authority
            .apply(input)
            .map_err(|error| MobError::MobMachineRejected {
                context: "identity_entry_change",
                reason: error.to_string(),
            })?;
        let verdict = transition
            .effects()
            .iter()
            .find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::IdentityEntryChangeResolved { verdict, .. } => {
                    Some(*verdict)
                }
                _ => None,
            })
            .ok_or_else(|| {
                MobError::Internal("MobMachine changed an identity with no verdict".into())
            })?;
        let (change, outcome) = entry_change_answer(commit.kind, verdict, &commit.identity)?;
        // The session an archive records: the seated member's, or the
        // unseated entry's archived session record.
        let archived_session = {
            let state = authority.state();
            state
                .member_session_bindings
                .get(&agent_identity)
                .or_else(|| state.identity_entry_archived_sessions.get(&agent_identity))
                .map(|session| SessionId::parse(&session.0))
                .transpose()
                .map_err(|error| {
                    MobError::Internal(format!(
                        "'{}' holds an invalid session: {error}",
                        commit.identity
                    ))
                })?
        };
        let current = store
            .observe_created_identity(&self.definition.id, &commit.identity)
            .await?;
        let at_ms = now_ms();
        let row = entry_change_row(
            &self.definition.id,
            &commit,
            verdict,
            current,
            archived_session,
            at_ms,
        )?;
        let mut entry =
            crate::identity::IdentityChangeReceiptEntry::new(commit.identity.clone(), outcome);
        if let Some(record) = row.as_ref().map(|row| &row.record)
            && let Ok(Some(digest)) = record.spec_digest()
        {
            entry = entry.with_spec(digest, Some(record.revision));
        }
        let receipt = IdentityChangeReceipt::sealed(
            self.definition.id.clone(),
            commit.operation.operation_id.clone(),
            commit.operation.request_digest.clone(),
            change,
            vec![entry],
            at_ms,
        )
        .and_then(|receipt| {
            receipt.with_attribution(commit.requester.clone(), commit.consent.clone())
        })
        .map_err(|error| MobError::Internal(format!("an identity change's receipt: {error}")))?;
        let prepared = PreparedDslInput {
            authority,
            effects: transition.into_effects(),
            phase_changed: false,
        };
        let written = receipt.clone();
        self.commit_prepared_dsl_input_after(prepared, move || async move {
            let commit = store
                .commit_identity_change(&crate::store::IdentityChangeWrite {
                    receipt: written,
                    rows: row.into_iter().collect(),
                    events: Vec::new(),
                })
                .await
                .map_err(MobError::from)?;
            committed_change_events(commit).map(|_| ())
        })
        .await?;
        Ok(EntryChangeCommitted::Committed { receipt, verdict })
    }
}
