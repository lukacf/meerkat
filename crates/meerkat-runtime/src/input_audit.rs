//! Mechanical protected audit payload on the existing input row.
//!
//! Live and rollback clones share pending observations, so rolling back an
//! unrelated native transition cannot erase an observation of an actual call.
//! Store candidates freeze an independent immutable prefix. No drain, accepted
//! flag, settlement state, permission lookup, or additional commit exists here.
//! Only the existing native transaction makes its exact row snapshot durable.

use std::sync::{Arc, Mutex};

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

#[derive(Clone)]
enum AuditPayload {
    Pending(Arc<Mutex<Vec<StoredAuthorizationAuditObservation>>>),
    Frozen(Arc<[StoredAuthorizationAuditObservation]>),
}

/// Observation data only. `Clone` preserves live append custody for native
/// rollback snapshots; `freeze_for_persistence` deliberately does not.
#[derive(Clone)]
pub struct InputAuthorizationAudit(AuditPayload);

impl Default for InputAuthorizationAudit {
    fn default() -> Self {
        Self(AuditPayload::Pending(Arc::new(Mutex::new(Vec::new()))))
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
            AuditPayload::Pending(records) => {
                records.lock().is_ok_and(|records| records.is_empty())
            }
            AuditPayload::Frozen(records) => records.is_empty(),
        }
    }

    /// Freeze exactly the observed prefix for the existing row transaction.
    /// Concurrent appends remain on the live buffer for the next native row
    /// update. This method does not acknowledge or clear either prefix/suffix.
    pub(crate) fn freeze_for_persistence(&self) -> Result<Self, OperationObservationError> {
        match &self.0 {
            AuditPayload::Frozen(records) => Ok(Self(AuditPayload::Frozen(Arc::clone(records)))),
            AuditPayload::Pending(records) => {
                let records = records.lock().map_err(|_| OperationObservationError)?;
                Ok(Self(AuditPayload::Frozen(records.clone().into())))
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
                records.to_vec(),
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
        let mut records = live.lock().map_err(|_| OperationObservationError)?;
        if records.starts_with(committed) {
            return Ok(self.clone());
        }
        if !committed.starts_with(&records) {
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

    /// Derive a sink from this actual admitted row, never from an input-id map
    /// supplied by the operation. The caller must obtain scope/run/originals
    /// from the current native owner while it holds that row's custody.
    pub(crate) fn bind(
        &self,
        row_input_id: &InputId,
        execution_scope: OperationExecutionScope,
        run_id: RunId,
        originals: &[RetainedInputAuthority],
    ) -> Result<Arc<dyn AuthorizationAuditSink>, OperationRefused> {
        let OperationExecutionScope::RuntimeInput {
            canonical_input_id, ..
        } = &execution_scope
        else {
            return Err(malformed());
        };
        if canonical_input_id != row_input_id
            || originals.is_empty()
            || !originals.iter().any(|item| item.input_id() == row_input_id)
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
        Ok(Self(AuditPayload::Pending(Arc::new(Mutex::new(records)))))
    }
}

struct NativeInputAuditSink {
    records: Arc<Mutex<Vec<StoredAuthorizationAuditObservation>>>,
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
        let mut records = self.records.lock().map_err(|_| OperationObservationError)?;
        records
            .try_reserve(1)
            .map_err(|_| OperationObservationError)?;
        records.push(StoredAuthorizationAuditObservation {
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
mod tests;
