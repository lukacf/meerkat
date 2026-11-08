//! Immutable evidence for one selected association's native consent scope.
//! This is only the consent constituent of abandonment recovery. Neither a
//! withdrawal nor a successful owner-loss sweep proves an input finalized,
//! an effect did not occur, a physical resource closed, or G may be restored.

use super::*;
use meerkat_core::{
    ApprovalStatus,
    approval::{ApprovalConsentScope, ApprovalConsentWithdrawalReason},
    session_hosting::{HostingClaim, OwnerLossAttachment},
};

/// The trusted application selects an existing association artifact while it
/// holds the real native claim. This records scope and provenance, not a new
/// membership, permission, native input or hosting proof.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsentAssociation {
    identity: ConsentApplicationIdentity,
    issuer: ConsentIssuerSelection,
    association: SelectedHostArtifact,
    scope: ApprovalConsentScope,
}
impl ConsentAssociation {
    pub fn scope(&self) -> &ApprovalConsentScope {
        &self.scope
    }
    pub fn artifact(&self) -> &SelectedHostArtifact {
        &self.association
    }
}

/// There is deliberately no completed/restore-authorized variant. The future
/// recovery join must independently obtain exact native finalization, known
/// zero-effect evidence for every affected operation, and joined physical
/// cleanup from those actual owners before publishing its own disposition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryJoinStatus {
    AwaitingNativeFinalizationAndPhysicalSettlement,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Disposition {
    schema_version: u32,
    association: ConsentAssociation,
    status: RecoveryJoinStatus,
    // Complete scoped rows, including prior Abandoned history. Never merely
    // the IDs newly changed by the latest native settlement sweep.
    records: Vec<ApprovalRecord>,
}

/// Immutable per-association history, separate from the live issuing file.
/// Serialization carries an artifact reference only. Validate it against the
/// same selected owner/association with `read_disposition` before using it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopedConsentDisposition {
    artifact: SelectedHostArtifact,
}
impl ScopedConsentDisposition {
    pub fn artifact(&self) -> &SelectedHostArtifact {
        &self.artifact
    }
}

impl ApplicationConsentOwner {
    pub fn bind_association(
        &self,
        association: SelectedHostArtifact,
        claim: &HostingClaim,
    ) -> Result<ConsentAssociation, ConsentOwnerError> {
        self.check()?;
        read_artifact(&association)?;
        Ok(ConsentAssociation {
            identity: self.identity().clone(),
            issuer: self.selection.clone(),
            association,
            scope: claim_scope(claim)?,
        })
    }

    /// Row-only settlement under the borrow of a genuine native reservation.
    /// The caller supplies the claim obtained from its actual selected machine;
    /// the association must have been bound to this same session/domain.
    ///
    /// Partial native commits remain committed when another row fails. No
    /// receipt is emitted in that case. A later explicitly requested attempt
    /// reopens current rows. This function never registers/unregisters an
    /// actor and never completes the claim's reservation: only the winning
    /// native registration publication can complete that reservation. Dropping
    /// it here returns the proof so registration still owes its own check.
    pub fn settle_abandoned_scope(
        &self,
        association: &ConsentAssociation,
        claim: &HostingClaim,
    ) -> Result<ScopedConsentDisposition, ConsentOwnerError> {
        self.check_association(association)?;
        if claim_scope(claim)? != association.scope {
            return Err(ConsentOwnerError::WrongClaim);
        }
        let reservation = match claim.reserve_owner_loss() {
            OwnerLossAttachment::Reserved(reservation) => reservation,
            OwnerLossAttachment::Settled | OwnerLossAttachment::InProgress => {
                return Err(ConsentOwnerError::ReservationUnavailable);
            }
        };
        let before = self.scope_records(&association.scope)?;
        if before.is_empty() {
            return Err(ConsentOwnerError::MissingConsent);
        }
        // Do not turn Core's deliberate omission of spent rows into a
        // zero-effect conclusion for a whole association.
        for row in &before {
            reject_spent(row)?;
        }
        self.service.settle_owner_loss(&reservation)?;
        // This census is historical evidence, not an admission fence. A
        // same-lineage producer could issue later work. The future G join
        // must stop admission and independently census native/effect owners.
        let after = self.scope_records(&association.scope)?;
        if before
            .iter()
            .map(|row| &row.approval_id)
            .collect::<Vec<_>>()
            != after.iter().map(|row| &row.approval_id).collect::<Vec<_>>()
        {
            return Err(ConsentOwnerError::InvalidSelection);
        }
        for row in &after {
            abandoned(row, &association.scope)?;
            if self.service.get(&row.approval_id)? != *row {
                return Err(meerkat_core::ApprovalError::StoreConflict {
                    approval_id: row.approval_id.clone(),
                }
                .into());
            }
        }
        self.check_association(association)?;
        let disposition = Disposition {
            schema_version: 1,
            association: association.clone(),
            status: RecoveryJoinStatus::AwaitingNativeFinalizationAndPhysicalSettlement,
            records: after,
        };
        let raw = encode(&disposition)?;
        if raw.len() as u64 > MAX_ARTIFACT_BYTES {
            return Err(ConsentOwnerError::InvalidSelection);
        }
        let artifact = SelectedHostArtifact {
            path: self
                .directory
                .join(format!("disposition-{}.json", hash(&raw))),
            sha256: hash(&raw),
        };
        match create_file(&artifact.path, &raw) {
            Ok(_) => {}
            Err(ConsentOwnerError::Io(error))
                if error.kind() == std::io::ErrorKind::AlreadyExists =>
            {
                if read_artifact(&artifact)? != raw {
                    return Err(ConsentOwnerError::InvalidSelection);
                }
            }
            Err(error) => return Err(error),
        }
        // Repeat both barriers even on exact-existing retry: an earlier
        // write may have produced bytes before its durability barrier failed.
        open_private_file(&artifact.path, false)?.sync_all()?;
        File::open(&self.directory)?.sync_all()?;
        // No native registration or its completion is claimed by this drop.
        drop(reservation);
        Ok(ScopedConsentDisposition { artifact })
    }

    /// Read immutable history, never a current effect or restore permission.
    /// Later unrelated issuer rows cannot invalidate this per-association
    /// artifact. Historical ordinary per-invocation artifacts are unchanged.
    pub fn read_disposition(
        &self,
        association: &ConsentAssociation,
        selected: &ScopedConsentDisposition,
    ) -> Result<(RecoveryJoinStatus, Vec<ApprovalRecord>), ConsentOwnerError> {
        self.check_association(association)?;
        if selected.artifact.path.parent() != Some(self.directory.as_path())
            || selected.artifact.path.file_name().is_none_or(|name| {
                name != format!("disposition-{}.json", selected.artifact.sha256).as_str()
            })
        {
            return Err(ConsentOwnerError::InvalidSelection);
        }
        let raw = read_artifact(&selected.artifact)?;
        let disposition: Disposition = decode(&raw)?;
        if disposition.schema_version != 1
            || &disposition.association != association
            || disposition.records.is_empty()
            || encode(&disposition)? != raw
        {
            return Err(ConsentOwnerError::InvalidSelection);
        }
        let mut last = None;
        for row in &disposition.records {
            if last.as_ref().is_some_and(|id| id >= &row.approval_id) {
                return Err(ConsentOwnerError::InvalidSelection);
            }
            abandoned(row, &association.scope)?;
            last = Some(row.approval_id.clone());
        }
        Ok((disposition.status, disposition.records))
    }

    fn check_association(&self, association: &ConsentAssociation) -> Result<(), ConsentOwnerError> {
        self.check()?;
        if &association.identity != self.identity() {
            return Err(ConsentOwnerError::WrongApplication);
        }
        if association.issuer != self.selection {
            return Err(ConsentOwnerError::InvalidSelection);
        }
        read_artifact(&association.association)?;
        Ok(())
    }

    fn scope_records(
        &self,
        scope: &ApprovalConsentScope,
    ) -> Result<Vec<ApprovalRecord>, ConsentOwnerError> {
        let mut rows: Vec<_> = self
            .store
            .load_all()?
            .into_iter()
            .filter(|row| {
                row.consent
                    .as_ref()
                    .is_some_and(|consent| consent.scope.as_ref() == Some(scope))
            })
            .collect();
        rows.sort_by(|a, b| a.approval_id.cmp(&b.approval_id));
        if rows
            .windows(2)
            .any(|pair| pair[0].approval_id == pair[1].approval_id)
        {
            return Err(ConsentOwnerError::InvalidSelection);
        }
        // Do not call ApprovalService::get before owner-loss settlement: get
        // refreshes expiry, whereas owner-loss must preserve a lost pending
        // row's deadline and cancel it even after that deadline. The native
        // settlement owns reconciliation; settled rows are checked afterward.
        Ok(rows)
    }
}

fn claim_scope(claim: &HostingClaim) -> Result<ApprovalConsentScope, ConsentOwnerError> {
    Ok(ApprovalConsentScope {
        domain: claim.domain_ref().ok_or(ConsentOwnerError::WrongClaim)?,
        session_id: claim.session_id().clone(),
    })
}
fn reject_spent(row: &ApprovalRecord) -> Result<(), ConsentOwnerError> {
    if row
        .consent
        .as_ref()
        .is_some_and(|consent| consent.consumed_at.is_some())
    {
        return Err(ConsentOwnerError::Spent(row.approval_id.clone()));
    }
    Ok(())
}
fn abandoned(row: &ApprovalRecord, scope: &ApprovalConsentScope) -> Result<(), ConsentOwnerError> {
    reject_spent(row)?;
    if !matches!(
        row.status,
        ApprovalStatus::Cancelled | ApprovalStatus::Approved
    ) || row.consent.as_ref().is_none_or(|consent| {
        consent.scope.as_ref() != Some(scope)
            || consent.withdrawal.as_ref().is_none_or(|withdrawal| {
                withdrawal.reason != ApprovalConsentWithdrawalReason::Abandoned
            })
    }) {
        return Err(ConsentOwnerError::NotAbandoned(row.approval_id.clone()));
    }
    Ok(())
}

/// Post-native observation only. Constructed after the actual R5 registration
/// completed this claim's owner loss. It holds the complete scoped rows; it
/// does not withdraw again, reserve again or authorize physical adoption.
pub(crate) struct PostNativeConsentCensus {
    pub(crate) association: ConsentAssociation,
    pub(crate) records: Vec<ApprovalRecord>,
}
impl ApplicationConsentOwner {
    pub(crate) fn observe_abandoned_after_native(
        &self,
        association: &ConsentAssociation,
        claim: &HostingClaim,
        proof: &meerkat_core::session_hosting::HostingOwnerLossProof,
        closed_inputs: &std::collections::HashSet<meerkat_core::lifecycle::InputId>,
    ) -> Result<PostNativeConsentCensus, ConsentOwnerError> {
        use meerkat_core::approval::{ApprovalConsentDeliveryState, ApprovalConsentTransition};
        self.check_association(association)?;
        if claim_scope(claim)? != association.scope
            || proof.domain() != &association.scope.domain
            || proof.session_id() != &association.scope.session_id
            || claim.completed_owner_loss().as_ref() != Some(proof)
        {
            return Err(ConsentOwnerError::WrongClaim);
        }
        let records = self.scope_records(&association.scope)?;
        if records.is_empty() {
            return Err(ConsentOwnerError::MissingConsent);
        }
        for row in &records {
            abandoned(row, &association.scope)?;
            let consent = row
                .consent
                .as_ref()
                .ok_or(ConsentOwnerError::MissingConsent)?;
            if !consent.routable()
                || consent
                    .delivery(ApprovalConsentTransition::Withdrawn)
                    .is_none()
                || consent.deliveries.iter().enumerate().any(|(i, delivery)| {
                    consent.deliveries[..i]
                        .iter()
                        .any(|prior| prior.transition == delivery.transition)
                })
            {
                return Err(ConsentOwnerError::NotAbandoned(row.approval_id.clone()));
            }
            for delivery in &consent.deliveries {
                match &delivery.state {
                    ApprovalConsentDeliveryState::Admitted {
                        session_id,
                        input_id,
                    } if session_id == &association.scope.session_id
                        && closed_inputs.iter().any(|id| id.to_string() == *input_id) => {}
                    // A native delivery owner's exact terminal non-delivery,
                    // not a local refusal inferred after an unknown attempt.
                    ApprovalConsentDeliveryState::Failed { .. } => {}
                    _ => return Err(ConsentOwnerError::NotAbandoned(row.approval_id.clone())),
                }
            }
            if self.service.get(&row.approval_id)? != *row {
                return Err(meerkat_core::ApprovalError::StoreConflict {
                    approval_id: row.approval_id.clone(),
                }
                .into());
            }
        }
        if self.scope_records(&association.scope)? != records {
            return Err(ConsentOwnerError::InvalidSelection);
        }
        self.check_association(association)?;
        if claim.completed_owner_loss().as_ref() != Some(proof) {
            return Err(ConsentOwnerError::WrongClaim);
        }
        Ok(PostNativeConsentCensus {
            association: association.clone(),
            records,
        })
    }
}
