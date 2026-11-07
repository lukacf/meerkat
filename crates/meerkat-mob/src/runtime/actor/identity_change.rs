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
    CreatePlanEntry, CreatedEntryFacts, CreatedIdentityPlanItem, EntryChangeCommit,
    EntryChangeCommitted, IdentityChangeCommand, IdentityChangeGrant, IdentityChangeRefusal,
    IdentityGrantRequest, IdentityOperation, admission_refusal, change_target_profile,
    committed_change_events, create_rows_and_receipt, entry_change_answer, entry_change_row,
    entry_change_stale, grant_covers_the_commit, grant_current_at_the_sink, now_ms, record_refusal,
    refused,
};
use super::dormant_seat::{SeatCommitScope, SeatCommitWrite, SeatPlanMode};
use super::*;
use crate::identity::{IdentityChangeReceipt, IdentityChangeReceiptObservation};
use crate::machines::mob_machine::MobMachineMutator as _;

/// What a change's admission decided on its authority.
pub(super) enum ChangeAdmission {
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
                grant_request,
                items,
                reply_tx,
            } => {
                let result = Box::pin(self.plan_store_owned_create(
                    &operation,
                    facts,
                    grant.as_ref(),
                    decided,
                    &grant_request,
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
            IdentityChangeCommand::Settle { commit, reply_tx } => {
                let result = Box::pin(self.settle_identity_preparation_step(*commit)).await;
                let _ = reply_tx.send(result);
            }
            IdentityChangeCommand::Unlist { identity, reply_tx } => {
                let result = Box::pin(self.unlist_identity_entry(&identity)).await;
                let _ = reply_tx.send(result);
            }
            #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
            IdentityChangeCommand::BeginConstruction { request, reply_tx } => {
                let result = Box::pin(self.begin_identity_construction(*request)).await;
                let _ = reply_tx.send(result);
            }
            #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
            IdentityChangeCommand::AdoptConstruction {
                identity,
                construction,
                reply_tx,
            } => {
                let _ = reply_tx.send(self.adopt_identity_construction(identity, *construction));
            }
            #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
            IdentityChangeCommand::EndConstruction {
                identity,
                attempt,
                completion,
                reply_tx,
            } => {
                let result =
                    Box::pin(self.end_identity_construction(identity, attempt, completion)).await;
                let _ = reply_tx.send(result);
            }
            #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
            IdentityChangeCommand::AbortConstruction {
                identity,
                attempt,
                reply_tx,
            } => {
                let _ = reply_tx.send(self.abort_identity_construction_step(identity, attempt));
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
    /// the store's receipt, read now, the requester's grant, checked current
    /// now (a host or human request carries none), and, for a store-owned
    /// update, the stale refusal its row read answers (`None` when its spec
    /// is current, and for every other change).
    pub(super) async fn decide_change_admission<M: mob_dsl::MobMachineMutator>(
        &self,
        authority: &mut M,
        operation: &IdentityOperation,
        facts: ChangeAdmissionFacts,
        grant: Option<&IdentityChangeGrant>,
        decided: bool,
        stale: Option<IdentityChangeRefusal>,
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
                spec_current: stale.is_none(),
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
        if let Some(refusal) = admission_refusal(verdict, &operation.operation_id, stale) {
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

    /// Record each new store-owned entry on `authority`, in order, from its
    /// request's facts; the first identity MobMachine refuses refuses the
    /// change. Answers the cause MobMachine recorded each entry unseated for
    /// (`created_entry_cause`), read on `authority` through `state`, in
    /// order.
    fn record_created_entries<M: mob_dsl::MobMachineMutator>(
        authority: &mut M,
        state: fn(&M) -> &mob_dsl::MobMachineState,
        identities: impl IntoIterator<Item = (AgentIdentity, CreatedEntryFacts)>,
    ) -> Result<Vec<mob_dsl::UnseatedEntryCause>, MobError> {
        let mut causes = Vec::new();
        for (agent_identity, facts) in identities {
            let dsl_identity = mob_dsl::AgentIdentity::from_domain(&agent_identity);
            let transition = authority
                .apply(mob_dsl::MobMachineInput::RecordCreatedIdentityEntry {
                    agent_identity: dsl_identity.clone(),
                    preparation_required: facts.preparation_required,
                    placement: facts.placement,
                    held_creation: facts.held_creation,
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
                return Err(refused(refusal));
            }
            let cause = state(authority)
                .identity_entry_unseated
                .get(&dsl_identity)
                .copied()
                .ok_or_else(|| {
                    MobError::Internal(format!(
                        "MobMachine recorded '{agent_identity}' with no unseated cause"
                    ))
                })?;
            causes.push(cause);
        }
        Ok(causes)
    }

    /// Plan a store-owned create: its admission (and the cover of its specs,
    /// `grant_request`, by an agent's grant) and its entries' records probed
    /// on a fork, and each identity's seat planned under created custody as
    /// the dormant seat plans one (a spec that owes a kickoff is embodied by
    /// a spawn after the commit).
    async fn plan_store_owned_create(
        &mut self,
        operation: &IdentityOperation,
        facts: ChangeAdmissionFacts,
        grant: Option<&IdentityChangeGrant>,
        decided: bool,
        grant_request: &IdentityGrantRequest,
        items: Vec<CreatedIdentityPlanItem>,
    ) -> Result<CreatePlanAnswer, MobError> {
        let mut probe = self.dsl_authority.fork();
        match self
            .decide_change_admission(&mut probe, operation, facts, grant, decided, None)
            .await?
        {
            ChangeAdmission::Replayed(receipt) => return Ok(CreatePlanAnswer::Replayed(receipt)),
            ChangeAdmission::Admitted => {}
        }
        grant_covers_the_commit(grant, Some(grant_request))?;
        let causes = Self::record_created_entries(
            &mut probe,
            mob_dsl::MobMachineAuthority::state,
            items
                .iter()
                .map(|item| (item.spec.identity.clone(), item.facts)),
        )?;
        let mut entries = Vec::with_capacity(items.len());
        for (item, cause) in items.into_iter().zip(causes) {
            // Only an entry owed its seat is seated by the commit step: one
            // awaiting its host's preparation, hosted on its first use or
            // held for its host's held protocol is created by its host.
            if cause != mob_dsl::UnseatedEntryCause::SeatOwed {
                entries.push(CreatePlanEntry::Unseated);
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
    /// sink, and covering the specs committed), records each new entry and
    /// stages each seat on one prepared authority, all or nothing; the
    /// change's store transaction writes its identities' rows, the seats'
    /// journal rows and its receipt; and only then does the machine commit.
    /// A refusal anywhere writes nothing.
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
                None,
            )
            .await?
        {
            ChangeAdmission::Replayed(receipt) => return Ok(CreateCommitted::Replayed(receipt)),
            ChangeAdmission::Admitted => {}
        }
        grant_covers_the_commit(
            commit.grant.as_ref(),
            Some(&IdentityGrantRequest::for_specs(
                commit.identities.iter().map(|(request, _)| &request.spec),
            )),
        )?;
        let causes = Self::record_created_entries(
            &mut authority,
            mob_dsl::MobMachinePreparedAuthority::state,
            commit
                .identities
                .iter()
                .map(|(request, facts)| (request.spec.identity.clone(), *facts)),
        )?;
        let (rows, receipt) =
            create_rows_and_receipt(&self.definition.id, &commit, &causes, now_ms())?;
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
    /// present again); and only then does the machine commit. The row is
    /// read first: a change decided against a spec the row no longer holds
    /// is stale, which the admission answers after a replay. An agent's
    /// grant must cover the identity's current profile (its row's, or its
    /// member's). The member's retirement a Retiring or Archiving verdict
    /// owes runs after, on the handle, and its start row records the mark's
    /// cause. A removal that ended a held construction in its step
    /// (`Removed`) sends the abort to the construction's hold once
    /// committed, and answers the report of the hold's end.
    async fn commit_identity_entry_change(
        &mut self,
        commit: EntryChangeCommit,
    ) -> Result<EntryChangeCommitted, MobError> {
        let store = self
            .identity_member
            .clone()
            .ok_or_else(|| refused(IdentityChangeRefusal::StorageUnsupported))?;
        let current = store
            .observe_created_identity(&self.definition.id, &commit.identity)
            .await?;
        let stale = entry_change_stale(
            &commit.identity,
            commit.expected_spec_digest.as_deref(),
            current.as_ref(),
        );
        let mut authority = self.dsl_authority.prepare_authority();
        match self
            .decide_change_admission(
                &mut authority,
                &commit.operation,
                commit.facts,
                commit.grant.as_ref(),
                commit.decided,
                stale,
            )
            .await?
        {
            ChangeAdmission::Replayed(receipt) => {
                return Ok(EntryChangeCommitted::Replayed(receipt));
            }
            ChangeAdmission::Admitted => {}
        }
        let agent_identity = mob_dsl::AgentIdentity::from_domain(&commit.identity);
        // The construction held for the identity before the step: a removal
        // that ends the entry ends it in the same step.
        let held_attempt = authority
            .state()
            .identity_entry_constructions_held
            .get(&agent_identity)
            .copied();
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
        // The identity's open construction, held or activating, which the
        // refusal of an archive awaiting its preparation names.
        let open_attempt = {
            let state = authority.state();
            state
                .identity_entry_constructions_held
                .get(&agent_identity)
                .or_else(|| {
                    state
                        .identity_entry_constructions_activating
                        .get(&agent_identity)
                })
                .copied()
        };
        let (change, outcome) =
            entry_change_answer(commit.kind, verdict, &commit.identity, open_attempt)?;
        // An expected spec digest names a store-owned identity's row spec: a
        // roster-owned identity's spec is its host roster's to check.
        let row_spec = match &current {
            Some(crate::identity::CreatedIdentityObservation::Created(row)) => row.spec.as_ref(),
            _ => None,
        };
        if commit.expected_spec_digest.is_some() && row_spec.is_none() {
            return Err(refused(IdentityChangeRefusal::InvalidRequest {
                problem: format!(
                    "'{}' holds no store-owned spec to check the expected digest against: \
                     its host's roster checks its spec",
                    commit.identity
                ),
            }));
        }
        grant_covers_the_commit(
            commit.grant.as_ref(),
            change_target_profile(authority.state(), &commit.identity, row_spec)
                .map(IdentityGrantRequest::for_identity)
                .as_ref(),
        )?;
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
        // The removal of an unseated entry ends the identity at once
        // (MobMachine names it, `IdentityEnded`): its scoped tool grants end
        // before the change's store transaction and the step's commit. A
        // failure fails the change, which writes nothing.
        self.end_tool_grants_of_ended_identities(&prepared.effects, "it was removed")
            .await?;
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
        // The removal ended the held construction with its entry: its hold
        // aborts, and its member is never published.
        let ended_construction = match (verdict, held_attempt) {
            (mob_dsl::IdentityEntryChangeVerdict::Removed, Some(attempt)) => {
                self.abort_hold_ended_by_removal(&commit.identity, attempt)
            }
            _ => None,
        };
        Ok(EntryChangeCommitted::Committed {
            receipt,
            verdict,
            ended_construction,
        })
    }
}
