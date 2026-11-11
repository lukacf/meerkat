//! Mechanical protected audit payload on the existing input row.
//!
//! Live and rollback clones share pending observations, so rolling back an
//! unrelated native transition cannot erase an observation of an actual call.
//! Store candidates freeze an independent immutable prefix. No drain, accepted
//! flag, settlement state, permission lookup, or additional commit exists here.
//! Only the existing native transaction makes its exact row snapshot durable.
//!
//! A live buffer carries one closed mark. The native archive sets it under
//! the buffer's lock, together with its last check that the stored row holds
//! every record, in the synchronous step where the generated archive removes
//! the row; an archive the machine defers leaves it unset. The mark mirrors
//! that archive, a fact the machine decides, as the ledger removal does, and
//! decides nothing. A closed buffer refuses every later append, so an
//! operation that records after its row was archived is refused as
//! observation unavailable instead of leaving a record no row will write.

use std::sync::{Arc, Mutex, MutexGuard};

use meerkat_authorization_contracts::audit::{
    AuthorizationAuditObservation, AuthorizationAuditSink, NativeAuditContributor,
    StoredAuthorizationAuditObservation,
};
use meerkat_core::authorization::{
    OperationObservationError, OperationRefusalKind, OperationRefused,
};
use meerkat_core::exact_operation::OperationExecutionScope;
use meerkat_core::{InputId, RunId};
use serde::{Deserialize, Serialize};

use crate::input_authority::RetainedInputAuthority;

/// A live buffer's records, and the closed mark the native archive sets.
/// The records' `Debug` is redacted.
#[derive(Debug, Default)]
struct PendingRecords {
    records: Vec<StoredAuthorizationAuditObservation>,
    closed: bool,
}

impl PendingRecords {
    fn open(records: Vec<StoredAuthorizationAuditObservation>) -> Self {
        Self {
            records,
            closed: false,
        }
    }
}

#[derive(Clone)]
enum AuditPayload {
    Pending(Arc<Mutex<PendingRecords>>),
    Frozen(Arc<[StoredAuthorizationAuditObservation]>),
}

/// Observation data only. `Clone` preserves live append custody for native
/// rollback snapshots; `freeze_for_persistence` deliberately does not.
#[derive(Clone)]
pub struct InputAuthorizationAudit(AuditPayload);

impl Default for InputAuthorizationAudit {
    fn default() -> Self {
        Self(AuditPayload::Pending(Arc::new(Mutex::new(
            PendingRecords::default(),
        ))))
    }
}

impl std::fmt::Debug for InputAuthorizationAudit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("InputAuthorizationAudit([REDACTED])")
    }
}

impl InputAuthorizationAudit {
    pub(crate) fn is_empty(&self) -> bool {
        match &self.0 {
            AuditPayload::Pending(records) => records
                .lock()
                .is_ok_and(|pending| pending.records.is_empty()),
            AuditPayload::Frozen(records) => records.is_empty(),
        }
    }

    /// Whether any record is a mandatory-hook withholding, which only stored
    /// input state v6 and later can carry.
    pub(crate) fn has_mandatory_hook_withholding(&self) -> bool {
        let is_withholding = |record: &StoredAuthorizationAuditObservation| {
            matches!(
                record.observation.observation,
                meerkat_authorization_contracts::audit::AuditObservation::MandatoryHookWithheld { .. }
            )
        };
        match &self.0 {
            AuditPayload::Pending(records) => records
                .lock()
                .is_ok_and(|pending| pending.records.iter().any(is_withholding)),
            AuditPayload::Frozen(records) => records.iter().any(is_withholding),
        }
    }

    /// Whether any record carries a refusal read or a policy read's
    /// publication instance, which only stored input state v7 and later can
    /// carry.
    pub(crate) fn has_publication_read(&self) -> bool {
        let carries_read =
            |record: &StoredAuthorizationAuditObservation| match &record.observation.observation {
                meerkat_authorization_contracts::audit::AuditObservation::Refused {
                    read, ..
                } => read.is_some(),
                meerkat_authorization_contracts::audit::AuditObservation::Prepared {
                    policy,
                    ..
                } => policy.publication_instance.is_some(),
                _ => false,
            };
        match &self.0 {
            AuditPayload::Pending(records) => records
                .lock()
                .is_ok_and(|pending| pending.records.iter().any(carries_read)),
            AuditPayload::Frozen(records) => records.iter().any(carries_read),
        }
    }

    /// Freeze exactly the observed prefix for the existing row transaction.
    /// Concurrent appends remain on the live buffer for the next native row
    /// update. This method does not acknowledge or clear either prefix/suffix.
    pub(crate) fn freeze_for_persistence(&self) -> Result<Self, OperationObservationError> {
        match &self.0 {
            AuditPayload::Frozen(records) => Ok(Self(AuditPayload::Frozen(Arc::clone(records)))),
            AuditPayload::Pending(records) => {
                let pending = records.lock().map_err(|_| OperationObservationError)?;
                Ok(Self(AuditPayload::Frozen(pending.records.clone().into())))
            }
        }
    }

    /// Restore appendable observation payload only when the existing native
    /// recovery owner has accepted the actual input row. Needed for in-memory
    /// stores that return frozen records without a serde round trip.
    pub(crate) fn restore_observations_for_owner(&self) -> Result<Self, OperationObservationError> {
        match &self.0 {
            AuditPayload::Pending(_) => Ok(self.clone()),
            AuditPayload::Frozen(records) => Ok(Self(AuditPayload::Pending(Arc::new(Mutex::new(
                PendingRecords::open(records.to_vec()),
            ))))),
        }
    }

    /// Keep the actual live append handle when the native owner installs a
    /// committed replacement row. Only prefix-compatible histories can be
    /// reconciled; divergent observations are never silently unioned/reordered.
    /// Existing native recovery/CAS authority must accept the row separately.
    pub(crate) fn retain_live_with_committed(
        &self,
        committed: &Self,
    ) -> Result<Self, OperationObservationError> {
        let AuditPayload::Pending(live) = &self.0 else {
            return Err(OperationObservationError);
        };
        if let AuditPayload::Pending(other) = &committed.0
            && Arc::ptr_eq(live, other)
        {
            return Ok(self.clone());
        }
        let frozen = committed.freeze_for_persistence()?;
        let AuditPayload::Frozen(committed) = &frozen.0 else {
            return Err(OperationObservationError);
        };
        let mut pending = live.lock().map_err(|_| OperationObservationError)?;
        let records = &mut pending.records;
        if records.starts_with(committed) {
            return Ok(self.clone());
        }
        if !committed.starts_with(records) {
            return Err(OperationObservationError);
        }
        let previous = records.len();
        records
            .try_reserve(committed.len() - previous)
            .map_err(|_| OperationObservationError)?;
        records.extend_from_slice(&committed[previous..]);
        Ok(self.clone())
    }

    /// Pure comparison for native reconciliation before it accepts a semantic
    /// replacement row. This neither installs a prefix nor drains live data.
    pub(crate) fn verify_committed_prefix_compatible(
        &self,
        committed: &Self,
    ) -> Result<(), OperationObservationError> {
        let live = self.freeze_for_persistence()?;
        let committed = committed.freeze_for_persistence()?;
        let (AuditPayload::Frozen(live), AuditPayload::Frozen(committed)) = (&live.0, &committed.0)
        else {
            return Err(OperationObservationError);
        };
        if live.starts_with(committed) || committed.starts_with(live) {
            Ok(())
        } else {
            Err(OperationObservationError)
        }
    }

    /// This live payload as its row was last durably written, given the
    /// payload the stored row holds. A row write freezes the live records;
    /// observations appended after it stay on the live buffer for the row's
    /// next update, so the live records can run ahead of the stored ones by
    /// such a suffix. When the stored records are a strict prefix of the live
    /// ones, the image is that prefix, frozen; otherwise it is the live
    /// records, frozen, and an exact compare against the stored row refuses
    /// the difference as before. Pure: it neither drains nor acknowledges
    /// either side.
    pub(crate) fn committed_image(&self, stored: &Self) -> Result<Self, OperationObservationError> {
        let live = self.freeze_for_persistence()?;
        let stored = stored.freeze_for_persistence()?;
        let (AuditPayload::Frozen(live_records), AuditPayload::Frozen(stored_records)) =
            (&live.0, &stored.0)
        else {
            return Err(OperationObservationError);
        };
        if stored_records.len() < live_records.len() && live_records.starts_with(stored_records) {
            return Ok(Self(AuditPayload::Frozen(
                live_records[..stored_records.len()].into(),
            )));
        }
        Ok(live)
    }

    /// Whether this payload and `other` hold the same records in the same
    /// order. Pure: it neither drains nor acknowledges either side.
    pub(crate) fn has_same_records(&self, other: &Self) -> Result<bool, OperationObservationError> {
        let left = self.freeze_for_persistence()?;
        let right = other.freeze_for_persistence()?;
        let (AuditPayload::Frozen(left), AuditPayload::Frozen(right)) = (&left.0, &right.0) else {
            return Err(OperationObservationError);
        };
        Ok(left == right)
    }

    /// Hold this payload for a native archive's last check: a live buffer
    /// stays locked, so no record can be appended, until the hold is
    /// dropped. A frozen payload has no buffer to hold.
    pub(crate) fn hold_for_archive(
        &self,
    ) -> Result<HeldInputAuthorizationAudit<'_>, OperationObservationError> {
        match &self.0 {
            AuditPayload::Pending(records) => records
                .lock()
                .map(|pending| HeldInputAuthorizationAudit(HeldPayload::Pending(pending)))
                .map_err(|_| OperationObservationError),
            AuditPayload::Frozen(records) => {
                Ok(HeldInputAuthorizationAudit(HeldPayload::Frozen(records)))
            }
        }
    }

    /// Derive a sink from this actual admitted row, never from an input-id map
    /// supplied by the operation. The caller must obtain scope/run/originals
    /// from the current native owner while it holds that row's custody.
    pub(crate) fn bind(
        &self,
        row_input_id: &InputId,
        execution_scope: OperationExecutionScope,
        run_id: RunId,
        originals: &[RetainedInputAuthority],
        retained_resume: bool,
    ) -> Result<Arc<dyn AuthorizationAuditSink>, OperationRefused> {
        let OperationExecutionScope::RuntimeInput {
            canonical_input_id, ..
        } = &execution_scope
        else {
            return Err(malformed());
        };
        // A resume of retained work is bound under the originals it
        // resumes; it is never an original itself.
        if canonical_input_id != row_input_id
            || originals.is_empty()
            || (!retained_resume && !originals.iter().any(|item| item.input_id() == row_input_id))
        {
            return Err(malformed());
        }
        let AuditPayload::Pending(records) = &self.0 else {
            return Err(malformed());
        };
        let contributors = originals
            .iter()
            .map(|original| {
                let association = original.association().candidate();
                NativeAuditContributor {
                    input_id: original.input_id().clone(),
                    requester: association.requester.clone(),
                    logical_executor: association.logical_executor.clone(),
                    represented_subject: association.represented_subject.clone(),
                }
            })
            .collect::<Vec<_>>()
            .into();
        Ok(Arc::new(NativeInputAuditSink {
            records: Arc::clone(records),
            execution_scope,
            run_id,
            contributors,
        }))
    }
}

impl Serialize for InputAuthorizationAudit {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let frozen = self
            .freeze_for_persistence()
            .map_err(serde::ser::Error::custom)?;
        let AuditPayload::Frozen(records) = &frozen.0 else {
            return Err(serde::ser::Error::custom("audit snapshot unavailable"));
        };
        records.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for InputAuthorizationAudit {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let records = Vec::<StoredAuthorizationAuditObservation>::deserialize(deserializer)?;
        // Loading historical observations does not acknowledge a live pending
        // buffer, infer completion, authorize recovery, or mint a sink.
        Ok(Self(AuditPayload::Pending(Arc::new(Mutex::new(
            PendingRecords::open(records),
        )))))
    }
}

/// A row's audit payload held for a native archive's last check (see
/// [`InputAuthorizationAudit::hold_for_archive`]).
pub(crate) struct HeldInputAuthorizationAudit<'a>(HeldPayload<'a>);

enum HeldPayload<'a> {
    Pending(MutexGuard<'a, PendingRecords>),
    Frozen(&'a [StoredAuthorizationAuditObservation]),
}

impl HeldInputAuthorizationAudit<'_> {
    /// Whether the held records are exactly `committed`'s, in order:
    /// whether the stored row holds every record. `committed` is a frozen
    /// payload (records read from or written to the stored row); a live one
    /// is refused, as it could be this very buffer.
    pub(crate) fn holds_only(
        &self,
        committed: &InputAuthorizationAudit,
    ) -> Result<bool, OperationObservationError> {
        let AuditPayload::Frozen(committed) = &committed.0 else {
            return Err(OperationObservationError);
        };
        Ok(match &self.0 {
            HeldPayload::Pending(pending) => pending.records.as_slice() == &committed[..],
            HeldPayload::Frozen(records) => *records == &committed[..],
        })
    }

    /// Mark the held buffer closed, as the native archive removes its row:
    /// every later append is refused. A frozen payload has no buffer.
    pub(crate) fn close(&mut self) {
        if let HeldPayload::Pending(pending) = &mut self.0 {
            pending.closed = true;
        }
    }
}

struct NativeInputAuditSink {
    records: Arc<Mutex<PendingRecords>>,
    execution_scope: OperationExecutionScope,
    run_id: RunId,
    contributors: Arc<[NativeAuditContributor]>,
}

impl AuthorizationAuditSink for NativeInputAuditSink {
    fn append(
        &self,
        observation: AuthorizationAuditObservation,
    ) -> Result<(), OperationObservationError> {
        if observation.execution_scope != self.execution_scope
            || observation.run_id.as_ref() != Some(&self.run_id)
        {
            return Err(OperationObservationError);
        }
        let mut pending = self.records.lock().map_err(|_| OperationObservationError)?;
        // The row was archived: no write of it would ever carry the record.
        if pending.closed {
            return Err(OperationObservationError);
        }
        pending
            .records
            .try_reserve(1)
            .map_err(|_| OperationObservationError)?;
        pending.records.push(StoredAuthorizationAuditObservation {
            contributors: Arc::clone(&self.contributors),
            observation,
        });
        Ok(())
    }
}

fn malformed() -> OperationRefused {
    OperationRefused::new(OperationRefusalKind::MalformedFacts)
}

#[cfg(test)]
pub(crate) mod tests;
