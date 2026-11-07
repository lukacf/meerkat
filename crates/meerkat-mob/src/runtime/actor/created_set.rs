//! The downgrade cycle on the actor (0.9.11; see
//! [`super::super::created_set`]): the purge's and the import's commit
//! steps.

use super::super::created_set::{
    ChangeCommitted, ImportCommit, PurgeCommit, imported_record_kind, imported_row_state,
    purge_refusal,
};
use super::super::identity_change::{
    IdentityChangeRefusal, PurgeRefusedReason, committed_change_events, now_ms, refused,
};
use super::identity_change::ChangeAdmission;
use super::*;
use crate::identity::{
    IdentityAdoptedFrom, IdentityChangeEntryOutcome, IdentityChangeKind, IdentityChangeReceipt,
    IdentityChangeReceiptEntry, IdentityEntered,
};
use crate::machines::mob_machine::MobMachineMutator as _;
use crate::store::{
    CreatedIdentityRowExpectation, CreatedIdentityRowWrite, CreatedRowsPurge, CreatedRowsPurgeWrite,
};

impl MobActor {
    /// Commit the host operator's purge of the mob's rows of schema version
    /// 2 and later in one actor step: MobMachine decides it
    /// (`PurgeIdentityRows`) on a prepared transition, the store's one
    /// transaction removes the rows and writes the receipt only when their
    /// digest is the export's, and only then does the machine commit, which
    /// refuses every later identity change until the mob restarts. A
    /// refusal anywhere removes nothing.
    pub(super) async fn commit_identity_rows_purge(
        &mut self,
        commit: PurgeCommit,
    ) -> Result<ChangeCommitted, MobError> {
        let store = self
            .identity_member
            .clone()
            .ok_or_else(|| refused(IdentityChangeRefusal::StorageUnsupported))?;
        let (receipt_kind, replayed) = self.observe_change_receipt(&commit.operation).await?;
        let prepared = self.prepare_dsl_input_transition(
            mob_dsl::MobMachineInput::PurgeIdentityRows {
                receipt: receipt_kind,
                host_operator: commit.host_operator,
            },
            "purge_identity_rows",
        )?;
        let verdict = prepared
            .transition
            .effects()
            .iter()
            .find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::IdentityRowsPurgeResolved { verdict } => Some(*verdict),
                _ => None,
            })
            .ok_or_else(|| {
                MobError::Internal("MobMachine decided a purge with no verdict".into())
            })?;
        if let Some(refusal) = purge_refusal(verdict, &commit.operation.operation_id) {
            return Err(refused(refusal));
        }
        if verdict == mob_dsl::IdentityRowsPurgeVerdict::Replayed {
            return replayed.map(ChangeCommitted::Replayed).ok_or_else(|| {
                MobError::Internal("MobMachine replayed a purge with no receipt".into())
            });
        }
        if commit.identities.is_empty() {
            return Err(refused(IdentityChangeRefusal::PurgeRefused {
                reason: PurgeRefusedReason::NothingToPurge,
            }));
        }
        let receipt = IdentityChangeReceipt::sealed(
            self.definition.id.clone(),
            commit.operation.operation_id.clone(),
            commit.operation.request_digest.clone(),
            IdentityChangeKind::Purge,
            commit
                .identities
                .iter()
                .map(|identity| {
                    IdentityChangeReceiptEntry::new(
                        identity.clone(),
                        IdentityChangeEntryOutcome::Purged,
                    )
                })
                .collect(),
            now_ms(),
        )
        .and_then(|receipt| {
            receipt.with_attribution(commit.requester.clone(), commit.consent.clone())
        })
        .map_err(|error| MobError::Internal(format!("a purge's receipt: {error}")))?;
        let write = CreatedRowsPurgeWrite {
            receipt: receipt.clone(),
            export_digest: commit.export_digest,
        };
        let operation_id = commit.operation.operation_id;
        self.commit_prepared_dsl_transition_after(prepared, move || async move {
            match store
                .purge_created_identity_rows(&write)
                .await
                .map_err(MobError::from)?
            {
                CreatedRowsPurge::Purged => Ok(()),
                CreatedRowsPurge::Replayed { .. } => Err(MobError::LifecycleOperationPending {
                    intent: "a purge under the same operation committed meanwhile: retry"
                        .to_string(),
                }),
                CreatedRowsPurge::OperationReused { .. } => {
                    Err(refused(IdentityChangeRefusal::OperationReused {
                        operation_id,
                    }))
                }
                CreatedRowsPurge::ExportMismatch { current_digest } => {
                    Err(refused(IdentityChangeRefusal::PurgeRefused {
                        reason: PurgeRefusedReason::ExportMismatch { current_digest },
                    }))
                }
                CreatedRowsPurge::UnreadableRows { identities } => {
                    Err(refused(IdentityChangeRefusal::PurgeRefused {
                        reason: PurgeRefusedReason::UnreadableRows { identities },
                    }))
                }
            }
        })
        .await?;
        Ok(ChangeCommitted::Committed(receipt))
    }

    /// Commit the host operator's import of an export in one actor step:
    /// MobMachine decides the change's admission and each record's verdict
    /// (`ImportIdentityRecord`) on one prepared authority, then declares the
    /// restored records' owned edges (`DeclareOwnedIdentityEdge`); the
    /// change's store transaction writes the restored rows as exported, the
    /// binding rows (origin Created) of the members bound in place and the
    /// receipt, and only then does the machine commit; the roster folds the
    /// binding rows as a replay does, which makes each member a host root.
    /// A record another holder keeps is reported in the receipt; a member
    /// step in flight, and any refusal, writes nothing.
    pub(super) async fn commit_identity_import(
        &mut self,
        commit: ImportCommit,
    ) -> Result<ChangeCommitted, MobError> {
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
            ChangeAdmission::Replayed(receipt) => return Ok(ChangeCommitted::Replayed(receipt)),
            ChangeAdmission::Admitted => {}
        }
        let mob_id = self.definition.id.clone();
        let mut effects = Vec::new();
        let mut entries = Vec::new();
        let mut rows = Vec::new();
        let mut bound_in_place = Vec::new();
        let mut edges = Vec::new();
        for exported in &commit.export.identities {
            let record = &exported.record;
            let identity = &record.agent_identity;
            let dsl_identity = mob_dsl::AgentIdentity::from_domain(identity);
            let current = store.observe_created_identity(&mob_id, identity).await?;
            // No row of schema version 2: a host-adopted intent's row
            // (schema version 1) of the name holds it as much as another
            // row does.
            let row = match current {
                Some(_) => imported_row_state(record, current.as_ref()),
                None => match self
                    .identity
                    .observe_identity_intent(&mob_id, identity)
                    .await?
                {
                    crate::identity::IdentityStoredObservation::Missing => {
                        mob_dsl::ImportedRowState::Absent
                    }
                    _ => mob_dsl::ImportedRowState::Other,
                },
            };
            let transition = authority
                .apply(mob_dsl::MobMachineInput::ImportIdentityRecord {
                    agent_identity: dsl_identity.clone(),
                    kind: imported_record_kind(record),
                    adopted_from_roster: matches!(
                        record.provenance.entered,
                        IdentityEntered::Adopted {
                            from: IdentityAdoptedFrom::Roster,
                            ..
                        }
                    ),
                    row,
                    last_session: exported
                        .session
                        .as_ref()
                        .map(|session| mob_dsl::SessionId::from_domain(&session.session_id)),
                })
                .map_err(|error| MobError::MobMachineRejected {
                    context: "import_identity_record",
                    reason: error.to_string(),
                })?;
            let verdict = transition
                .effects()
                .iter()
                .find_map(|effect| match effect {
                    mob_dsl::MobMachineEffect::IdentityRecordImportResolved { verdict, .. } => {
                        Some(*verdict)
                    }
                    _ => None,
                })
                .ok_or_else(|| {
                    MobError::Internal("MobMachine imported a record with no verdict".into())
                })?;
            use mob_dsl::IdentityImportVerdict as Verdict;
            let outcome = match verdict {
                Verdict::RestoredInPlace | Verdict::RestoredByAdoption => {
                    // A member seated under the name keeps its binding: its
                    // binding row records the Created origin.
                    if authority
                        .state()
                        .identity_to_runtime
                        .contains_key(&dsl_identity)
                    {
                        bound_in_place.push(identity.clone());
                    }
                    IdentityChangeEntryOutcome::Committed
                }
                Verdict::RestoredOwedSeat => IdentityChangeEntryOutcome::Committed,
                Verdict::RestoredPending => IdentityChangeEntryOutcome::PendingPreparation,
                Verdict::RestoredArchived => IdentityChangeEntryOutcome::Archived,
                Verdict::RestoredTombstone => IdentityChangeEntryOutcome::Removed,
                Verdict::Unchanged => IdentityChangeEntryOutcome::Unchanged,
                Verdict::NameTakenMeanwhile => IdentityChangeEntryOutcome::NameTakenMeanwhile,
                Verdict::Unsettled => {
                    return Err(refused(IdentityChangeRefusal::AdmissionUnsettled {
                        identity: identity.clone(),
                    }));
                }
                Verdict::NonAuthoritative => {
                    return Err(refused(IdentityChangeRefusal::CandidateLaunch));
                }
                Verdict::ChangesHeld => return Err(refused(IdentityChangeRefusal::ChangesHeld)),
                Verdict::RowsPurged => return Err(refused(IdentityChangeRefusal::Purged)),
                Verdict::MobNotRunning => {
                    return Err(refused(IdentityChangeRefusal::MobNotRunning));
                }
            };
            let restored = matches!(
                verdict,
                Verdict::RestoredInPlace
                    | Verdict::RestoredOwedSeat
                    | Verdict::RestoredPending
                    | Verdict::RestoredArchived
                    | Verdict::RestoredTombstone
                    | Verdict::RestoredByAdoption
            );
            let mut entry = IdentityChangeReceiptEntry::new(identity.clone(), outcome);
            if let Ok(Some(digest)) = record.spec_digest() {
                entry = entry.with_spec(digest, Some(record.revision));
            }
            entries.push(entry);
            if restored {
                rows.push(CreatedIdentityRowWrite {
                    record: record.clone(),
                    expected: CreatedIdentityRowExpectation::Absent,
                });
                if verdict != Verdict::RestoredTombstone {
                    for edge in &record.owned_edges {
                        edges.push((identity.clone(), edge.peer.clone()));
                    }
                }
            }
            effects.extend(transition.into_effects());
        }
        // The restored records' owned edges, once every record is: an edge's
        // other end may be a record restored after its owner's. The rows
        // keep every edge as exported; MobMachine holds those whose other
        // end is an identity, as a start's recovery of the rows would.
        for (owner, peer) in edges {
            let dsl_owner = mob_dsl::AgentIdentity::from_domain(&owner);
            let declared = authority
                .apply(mob_dsl::MobMachineInput::DeclareOwnedIdentityEdge {
                    edge: mob_dsl::WiringEdge::new(
                        dsl_owner.clone(),
                        mob_dsl::AgentIdentity::from_domain(&peer),
                    ),
                    owner: dsl_owner,
                })
                .map_err(|error| MobError::MobMachineRejected {
                    context: "declare_owned_identity_edge",
                    reason: error.to_string(),
                })?;
            effects.extend(declared.into_effects());
        }
        let events = bound_in_place
            .iter()
            .map(|identity| {
                super::identity_entry_admission::legacy_adoption_row(authority.state(), identity)
                    .map(|row| NewMobEvent {
                        mob_id: mob_id.clone(),
                        timestamp: None,
                        kind: MobEventKind::MemberSessionBindingRecovered(row),
                    })
            })
            .collect::<Result<Vec<_>, MobError>>()?;
        let receipt = IdentityChangeReceipt::sealed(
            mob_id,
            commit.operation.operation_id.clone(),
            commit.operation.request_digest.clone(),
            IdentityChangeKind::Import,
            entries,
            now_ms(),
        )
        .and_then(|receipt| {
            receipt.with_attribution(commit.requester.clone(), commit.consent.clone())
        })
        .map_err(|error| MobError::Internal(format!("an import's receipt: {error}")))?;
        let prepared = PreparedDslInput {
            authority,
            effects,
            phase_changed: false,
        };
        let written = receipt.clone();
        let stored = self
            .commit_prepared_dsl_input_after(prepared, move || async move {
                let commit = store
                    .commit_identity_change(&crate::store::IdentityChangeWrite {
                        receipt: written,
                        rows,
                        events,
                    })
                    .await
                    .map_err(MobError::from)?;
                committed_change_events(commit)
            })
            .await?;
        // The roster folds each binding row as a replay does: the member
        // keeps its binding, and its spawner is cleared.
        {
            let mut roster = self.roster.write().await;
            for event in &stored {
                roster.apply_event(event);
            }
        }
        Ok(ChangeCommitted::Committed(receipt))
    }
}
