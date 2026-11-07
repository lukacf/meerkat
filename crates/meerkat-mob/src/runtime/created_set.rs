//! The downgrade cycle (0.9.11; `docs/guides/upgrading.mdx`): the export of
//! a mob's created identity set, the host operator's purge of its rows of
//! schema version 2 and later before a downgrade, and the import of the
//! export after the roll-forward.
//!
//! The export ([`super::MobHandle::export_created_identities`]) reads every
//! such row and each identity's session reference: its member's session, or
//! its archived session. The purge
//! ([`super::MobHandle::purge_created_identities`]) is decided by MobMachine
//! (`PurgeIdentityRows`: only the host operator, only while the mob's
//! identity changes are held), and its store transaction removes the rows,
//! and nothing else, only when their digest is the export's; from its
//! commit on, every identity change is refused until the mob restarts. A
//! start that reads no row of a Created entry's name reserves the name
//! (`RecoverRowlessCreatedEntry`): nothing acts on it and nothing seats it
//! until the import restores its row. The import
//! ([`super::MobHandle::import_created_identities`]) is decided record by
//! record (`ImportIdentityRecord`) and writes the restored rows, the binding
//! rows of the members it binds in place and its receipt in one store
//! transaction; the entries it leaves owed their seat are seated after it,
//! Dormant, on their archived session.

use crate::MobError;
use crate::identity::{
    CreatedIdentityObservation, CreatedIdentityPresence, CreatedIdentityRecord,
    CreatedIdentitySetExport, ExportedIdentity, ExportedSessionReference, ExportedSessionState,
    IdentityChangeConsent, IdentityChangeReceipt, IdentityChangeRequester, IdentityPolicyOutcome,
    UnreadableIdentityRow,
};
use crate::ids::AgentIdentity;
use crate::machines::mob_machine as mob_dsl;

use super::identity_change::{
    ChangeAdmissionFacts, ChangeAttribution, IdentityChangeCommand, IdentityChangeConsentInput,
    IdentityChangeGrant, IdentityChangeOutcome, IdentityChangeRefusal,
    IdentityChangeRequesterInput, IdentityOperation, PurgeRefusedReason, now_ms, refused,
};

/// The host operator's purge of a mob's rows of schema version 2 and later
/// ([`super::MobHandle::purge_created_identities`]).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct IdentityRowsPurge {
    pub operation: IdentityOperation,
    pub requester: IdentityChangeRequesterInput,
    /// The digest of the export the rows must match
    /// ([`CreatedIdentitySetExport::export_digest`]).
    pub export_digest: String,
}

impl IdentityRowsPurge {
    #[must_use]
    pub fn new(
        operation: IdentityOperation,
        requester: IdentityChangeRequesterInput,
        export_digest: impl Into<String>,
    ) -> Self {
        Self {
            operation,
            requester,
            export_digest: export_digest.into(),
        }
    }
}

/// How an import treats its export ([`IdentityImport::mode`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum IdentityImportMode {
    /// Re-create each record and re-bind its session: a seated member in
    /// place, an unseated identity Dormant on its archived session (a fresh
    /// session, read as history unavailable, when that session is gone).
    #[default]
    Restore,
}

/// The host operator's import of an export after a roll-forward
/// ([`super::MobHandle::import_created_identities`]).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct IdentityImport {
    pub operation: IdentityOperation,
    pub requester: IdentityChangeRequesterInput,
    pub export: CreatedIdentitySetExport,
    pub mode: IdentityImportMode,
}

impl IdentityImport {
    #[must_use]
    pub fn new(
        operation: IdentityOperation,
        requester: IdentityChangeRequesterInput,
        export: CreatedIdentitySetExport,
    ) -> Self {
        Self {
            operation,
            requester,
            export,
            mode: IdentityImportMode::Restore,
        }
    }
}

/// What a purge's commit step takes.
pub(super) struct PurgeCommit {
    pub(super) operation: IdentityOperation,
    pub(super) export_digest: String,
    pub(super) host_operator: bool,
    pub(super) requester: IdentityChangeRequester,
    pub(super) consent: IdentityChangeConsent,
    /// The identities whose rows the purge removes, as read before the step.
    pub(super) identities: Vec<AgentIdentity>,
}

/// What an import's commit step takes.
pub(super) struct ImportCommit {
    pub(super) operation: IdentityOperation,
    pub(super) facts: ChangeAdmissionFacts,
    pub(super) grant: Option<IdentityChangeGrant>,
    pub(super) decided: bool,
    pub(super) requester: IdentityChangeRequester,
    pub(super) consent: IdentityChangeConsent,
    pub(super) export: CreatedIdentitySetExport,
}

/// What a purge's or an import's commit step answers.
pub(super) enum ChangeCommitted {
    Committed(IdentityChangeReceipt),
    Replayed(IdentityChangeReceipt),
}

/// The record kind MobMachine's import decides by, from the record's
/// presence.
pub(super) fn imported_record_kind(record: &CreatedIdentityRecord) -> mob_dsl::ImportedRecordKind {
    match record.presence {
        CreatedIdentityPresence::Present => mob_dsl::ImportedRecordKind::Present,
        CreatedIdentityPresence::PendingPreparation { .. } => {
            mob_dsl::ImportedRecordKind::PendingPreparation
        }
        CreatedIdentityPresence::Archived { .. } => mob_dsl::ImportedRecordKind::Archived,
        CreatedIdentityPresence::Removed => mob_dsl::ImportedRecordKind::Tombstone,
    }
}

/// The store's row of an imported record's name, as MobMachine's import
/// reads it: none, the record's own row, or another.
pub(super) fn imported_row_state(
    record: &CreatedIdentityRecord,
    current: Option<&CreatedIdentityObservation>,
) -> mob_dsl::ImportedRowState {
    match current {
        None => mob_dsl::ImportedRowState::Absent,
        Some(CreatedIdentityObservation::Created(row))
            if row.authority_digest == record.authority_digest =>
        {
            mob_dsl::ImportedRowState::Same
        }
        Some(_) => mob_dsl::ImportedRowState::Other,
    }
}

/// `identity`'s session reference in `state`: its member's session, its
/// archived session record, or its archived row's session.
fn session_reference(
    state: &mob_dsl::MobMachineState,
    identity: &AgentIdentity,
    record: &CreatedIdentityRecord,
) -> Result<Option<ExportedSessionReference>, MobError> {
    let dsl_identity = mob_dsl::AgentIdentity::from_domain(identity);
    let parse = |session: &mob_dsl::SessionId| {
        meerkat_core::types::SessionId::parse(&session.0).map_err(|error| {
            MobError::Internal(format!(
                "'{identity}' holds an invalid session reference: {error}"
            ))
        })
    };
    if let Some(session) = state.member_session_bindings.get(&dsl_identity) {
        return Ok(Some(ExportedSessionReference::new(
            parse(session)?,
            ExportedSessionState::Current,
        )));
    }
    if let Some(session) = state.identity_entry_archived_sessions.get(&dsl_identity) {
        return Ok(Some(ExportedSessionReference::new(
            parse(session)?,
            ExportedSessionState::Archived,
        )));
    }
    if let CreatedIdentityPresence::Archived {
        archived_session: Some(session),
    } = &record.presence
    {
        return Ok(Some(ExportedSessionReference::new(
            session.clone(),
            ExportedSessionState::Archived,
        )));
    }
    Ok(None)
}

impl super::MobHandle {
    /// Every row of `self`'s mob the created identity scan reads, by
    /// identity, page by page.
    async fn created_identity_rows(
        &self,
    ) -> Result<std::collections::BTreeMap<AgentIdentity, CreatedIdentityObservation>, MobError>
    {
        let store = self
            .identity_member
            .as_ref()
            .ok_or_else(|| refused(IdentityChangeRefusal::StorageUnsupported))?;
        let mut rows = std::collections::BTreeMap::new();
        let mut after = None;
        loop {
            let page = store
                .scan_created_identity_rows(
                    self.mob_id(),
                    after.as_ref(),
                    crate::store::IDENTITY_INTENT_SCAN_PAGE_MAX,
                )
                .await?;
            rows.extend(page.observations);
            match page.next {
                None => return Ok(rows),
                Some(next) if after.as_ref().is_none_or(|current| &next > current) => {
                    after = Some(next);
                }
                Some(_) => {
                    return Err(MobError::Internal(format!(
                        "the created identity scan of mob '{}' returned a non-advancing cursor",
                        self.mob_id()
                    )));
                }
            }
        }
    }

    /// Export the mob's created identity set (0.9.11, schema
    /// `meerkat.identity.created_set.v1`): every row of schema version 2 and
    /// later (its store-owned identities, archived ones and the tombstones
    /// of removed names), each with its session reference (its member's
    /// session, or its archived session), sealed with the digest a purge
    /// checks. A row this build cannot read is listed by its identity and
    /// evidence digest only. Session contents, memory and decisions are not
    /// in it. It only reads; hold the mob's identity changes first so the
    /// set does not move before the purge.
    ///
    /// Refused typed ([`MobError::IdentityChangeRefused`]) for a custom
    /// storage composition (`storage_unsupported`).
    pub async fn export_created_identities(&self) -> Result<CreatedIdentitySetExport, MobError> {
        let rows = self.created_identity_rows().await?;
        let mut identities = Vec::new();
        let mut unreadable = Vec::new();
        {
            let state = self.machine_state_watch_rx.borrow();
            for (identity, observation) in rows {
                match observation {
                    CreatedIdentityObservation::Created(record) => {
                        let session = session_reference(&state, &identity, &record)?;
                        identities.push(ExportedIdentity::new(record, session));
                    }
                    CreatedIdentityObservation::Unreadable {
                        evidence_digest, ..
                    } => unreadable.push(UnreadableIdentityRow {
                        identity,
                        evidence_digest,
                    }),
                }
            }
        }
        CreatedIdentitySetExport::sealed(self.mob_id().clone(), identities, unreadable)
            .map_err(|error| MobError::Internal(format!("the created identity export: {error}")))
    }

    /// Purge the mob's rows of schema version 2 and later before a
    /// downgrade (0.9.11): the host operator only, while the mob's identity
    /// changes are held, under the digest of the export it took
    /// ([`Self::export_created_identities`]). One store transaction removes
    /// every such row (store-owned identities, archived ones, tombstones)
    /// and writes the purge's receipt (kind `purge`), only when the rows are
    /// the export's: never a session, a journal row or a host-adopted
    /// intent. From its commit on, every identity change of the mob is
    /// refused ([`IdentityChangeRefusal::Purged`]) until it restarts. A
    /// retry with the same operation and request answers
    /// [`IdentityChangeOutcome::Replayed`].
    ///
    /// Refused typed ([`IdentityChangeRefusal::PurgeRefused`], nothing
    /// removed): a requester that is not the host operator, changes not
    /// held, an export the rows no longer match, a row this build cannot
    /// read, or no row to purge; and as every change: another request under
    /// the operation, a purge already made, a candidate composition, a mob
    /// that is not running, a custom storage composition, and more rows
    /// than one change commits ([`IdentityChangeRefusal::BatchTooLarge`]).
    pub async fn purge_created_identities(
        &self,
        purge: IdentityRowsPurge,
    ) -> Result<IdentityChangeOutcome, MobError> {
        let IdentityRowsPurge {
            operation,
            requester,
            export_digest,
        } = purge;
        if self.identity_member.is_none() {
            return Err(refused(IdentityChangeRefusal::StorageUnsupported));
        }
        let invalid = |problem: String| refused(IdentityChangeRefusal::InvalidRequest { problem });
        if operation.operation_id.trim().is_empty() || operation.request_digest.trim().is_empty() {
            return Err(invalid(
                "a change needs an operation id and a request digest".to_string(),
            ));
        }
        if export_digest.trim().is_empty() {
            return Err(invalid("a purge needs its export's digest".to_string()));
        }
        let host_operator = ChangeAttribution::is_host_operator(&requester);
        // The purge is the host operator's own step: its receipt records the
        // host's allow rule.
        let ChangeAttribution {
            requester, consent, ..
        } = ChangeAttribution::of(
            requester,
            IdentityChangeConsentInput::Policy(IdentityPolicyOutcome::Allow),
            &operation,
        )?;
        let identities: Vec<AgentIdentity> =
            self.created_identity_rows().await?.into_keys().collect();
        if identities.len() > crate::identity::IDENTITY_CHANGE_BATCH_MAX {
            return Err(refused(IdentityChangeRefusal::BatchTooLarge {
                max: crate::identity::IDENTITY_CHANGE_BATCH_MAX,
            }));
        }
        let commit = PurgeCommit {
            operation,
            export_digest,
            host_operator,
            requester,
            consent,
            identities,
        };
        let committed = self
            .send_actor_command(|reply_tx| {
                super::MobCommand::IdentityChange(Box::new(IdentityChangeCommand::CommitPurge {
                    commit: Box::new(commit),
                    reply_tx,
                }))
            })
            .await??;
        Ok(match committed {
            ChangeCommitted::Committed(receipt) => IdentityChangeOutcome::Committed { receipt },
            ChangeCommitted::Replayed(receipt) => IdentityChangeOutcome::Replayed { receipt },
        })
    }

    /// Import an export after a roll-forward (0.9.11, mode
    /// [`IdentityImportMode::Restore`]): the host operator only, under
    /// imported consent; idempotent by its operation. Each record is
    /// re-created and its session re-bound, decided record by record by
    /// MobMachine: a local member seated under its name, bound to a
    /// session, keeps its runtime, generation and session and becomes the
    /// store-owned identity again (a Created entry whose row was purged,
    /// or a plain member the older build left); an unseated identity is
    /// seated again, Dormant, on its archived session (the exported
    /// reference when the mob's journal holds none; a fresh session, read
    /// as history unavailable, when that session is gone); a pending, an
    /// archived and a removed name's record are restored as recorded; and
    /// a record of an adoption from a roster whose roster declared the name
    /// again returns it to store custody by the adoption's step. A name
    /// another holder keeps (a roster, a member, another row) is left as it
    /// is and reported (receipt outcome `name_taken_meanwhile`); a record
    /// whose row the store holds already is `unchanged`. The restored rows,
    /// the binding rows of the members bound in place and the receipt (kind
    /// `import`) are written in one store transaction.
    ///
    /// Refused typed, with nothing written: a requester that is not the
    /// host operator or another consent than imported
    /// ([`IdentityChangeRefusal::ImportRefused`]), an export of another mob
    /// or one that does not validate, more records than one change commits,
    /// a member step in flight for a name
    /// ([`IdentityChangeRefusal::AdmissionUnsettled`], retryable), a purge
    /// the mob has not restarted since, and every refusal of a change's
    /// admission.
    pub async fn import_created_identities(
        &self,
        import: IdentityImport,
        consent: IdentityChangeConsentInput<'_>,
    ) -> Result<IdentityChangeOutcome, MobError> {
        let IdentityImport {
            operation,
            requester,
            export,
            mode: IdentityImportMode::Restore,
        } = import;
        if self.identity_member.is_none() {
            return Err(refused(IdentityChangeRefusal::StorageUnsupported));
        }
        let invalid = |problem: String| refused(IdentityChangeRefusal::InvalidRequest { problem });
        if operation.operation_id.trim().is_empty() || operation.request_digest.trim().is_empty() {
            return Err(invalid(
                "a change needs an operation id and a request digest".to_string(),
            ));
        }
        if !ChangeAttribution::is_host_operator(&requester)
            || !matches!(consent, IdentityChangeConsentInput::Imported(_))
        {
            return Err(refused(IdentityChangeRefusal::ImportRefused));
        }
        export
            .validate()
            .map_err(|error| invalid(format!("the export: {error}")))?;
        if export.mob_id != *self.mob_id() {
            return Err(invalid(format!(
                "the export is of mob '{}', not '{}'",
                export.mob_id,
                self.mob_id()
            )));
        }
        if export.identities.is_empty() {
            return Err(invalid("the export holds no record to restore".to_string()));
        }
        if export.identities.len() > crate::identity::IDENTITY_CHANGE_BATCH_MAX {
            return Err(refused(IdentityChangeRefusal::BatchTooLarge {
                max: crate::identity::IDENTITY_CHANGE_BATCH_MAX,
            }));
        }
        let consent_facts = ChangeAttribution::facts_of(consent, &operation, true);
        let ChangeAttribution {
            requester,
            grant,
            decided,
            consent,
        } = ChangeAttribution::of(requester, consent, &operation)?;
        let commit = ImportCommit {
            facts: consent_facts.at(now_ms()),
            operation,
            grant,
            decided,
            requester,
            consent,
            export,
        };
        let committed = self
            .send_actor_command(|reply_tx| {
                super::MobCommand::IdentityChange(Box::new(IdentityChangeCommand::CommitImport {
                    commit: Box::new(commit),
                    reply_tx,
                }))
            })
            .await??;
        Ok(match committed {
            ChangeCommitted::Committed(receipt) => {
                // The entries the import left owed their seat are seated from
                // their rows, as a start seats them, and their owned edges
                // realized.
                self.reseat_owed_created_entries().await;
                self.realize_owned_identity_edges().await;
                IdentityChangeOutcome::Committed { receipt }
            }
            ChangeCommitted::Replayed(receipt) => IdentityChangeOutcome::Replayed { receipt },
        })
    }
}

/// The refusal of a purge MobMachine's verdict names (`None` for a purge it
/// lets go on, and for a replay).
pub(super) fn purge_refusal(
    verdict: mob_dsl::IdentityRowsPurgeVerdict,
    operation_id: &str,
) -> Option<IdentityChangeRefusal> {
    use mob_dsl::IdentityRowsPurgeVerdict as Verdict;
    match verdict {
        Verdict::Purged | Verdict::Replayed => None,
        Verdict::NonAuthoritative => Some(IdentityChangeRefusal::CandidateLaunch),
        Verdict::OperationReused => Some(IdentityChangeRefusal::OperationReused {
            operation_id: operation_id.to_string(),
        }),
        Verdict::RowsPurged => Some(IdentityChangeRefusal::Purged),
        Verdict::NotHostOperator => Some(IdentityChangeRefusal::PurgeRefused {
            reason: PurgeRefusedReason::NotHostOperator,
        }),
        Verdict::ChangesNotHeld => Some(IdentityChangeRefusal::PurgeRefused {
            reason: PurgeRefusedReason::ChangesNotHeld,
        }),
        Verdict::MobNotRunning => Some(IdentityChangeRefusal::MobNotRunning),
    }
}
