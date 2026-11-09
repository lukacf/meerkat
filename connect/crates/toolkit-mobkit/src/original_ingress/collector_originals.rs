//! Current service enrollment published by the canonical collector writer.
//! These rows never become native inputs or retained contributor custody.

use super::*;

#[cfg(test)]
mod contention_tests;
use crate::private_originals::collector_setup::{CollectorRecord, CollectorSetupError};

pub(super) struct PublishedCollector {
    key: toolkit_store::StoreKey,
    record: Option<CollectorRecord>,
    cancelled: Option<HistoricalEvidenceRef>,
}

impl ToolkitOriginalOwner {
    /// The caller holds this owner's source writer and has validated the exact
    /// durable selection, review and native grant. Publication is synchronous;
    /// no native read performs storage or account I/O.
    pub(crate) fn publish_collector_original(
        &self,
        key: toolkit_store::StoreKey,
        record: CollectorRecord,
    ) -> Result<(), CollectorSetupError> {
        if record.revoked
            || record.id.as_str() != key.id
            || record.association.ingress_namespace.realm.to_string() != key.realm
            || record.association.ingress_namespace.ingress != self.ingress_domain
        {
            return Err(CollectorSetupError::Denied);
        }
        {
            let collectors = self
                .collectors
                .lock()
                .map_err(|_| CollectorSetupError::Unavailable)?;
            if collectors
                .iter()
                .any(|entry| entry.key == key && entry.record.as_ref() == Some(&record))
            {
                return Ok(());
            }
        }
        let _publication = self
            .publication
            .begin_owner_change()
            .map_err(|_| CollectorSetupError::Unavailable)?;
        let mut collectors = self
            .collectors
            .lock()
            .map_err(|_| CollectorSetupError::Unavailable)?;
        match collectors.iter_mut().find(|entry| entry.key == key) {
            Some(PublishedCollector {
                record: None,
                cancelled: Some(_),
                ..
            }) => Err(CollectorSetupError::SetupRevoked),
            Some(PublishedCollector {
                record: None,
                cancelled: None,
                ..
            }) => Err(CollectorSetupError::Unavailable),
            Some(entry) if entry.record.as_ref() == Some(&record) => Ok(()),
            Some(_) => Err(CollectorSetupError::Denied),
            None => {
                collectors.push(PublishedCollector {
                    key,
                    record: Some(record),
                    cancelled: None,
                });
                Ok(())
            }
        }
    }

    /// Fence first, including when the durable cancellation result is unknown.
    /// Retrying resolution cannot reactivate this owner's cancelled selection.
    pub(crate) fn fence_collector_original(
        &self,
        key: &toolkit_store::StoreKey,
    ) -> Result<(), CollectorSetupError> {
        let _publication = self
            .publication
            .begin_owner_change()
            .map_err(|_| CollectorSetupError::Unavailable)?;
        let mut collectors = self
            .collectors
            .lock()
            .map_err(|_| CollectorSetupError::Unavailable)?;
        match collectors.iter_mut().find(|entry| &entry.key == key) {
            Some(entry) => entry.record = None,
            None => collectors.push(PublishedCollector {
                key: key.clone(),
                record: None,
                cancelled: None,
            }),
        }
        Ok(())
    }

    pub(crate) fn publish_collector_revocation(
        &self,
        key: &toolkit_store::StoreKey,
        evidence: HistoricalEvidenceRef,
    ) -> Result<(), CollectorSetupError> {
        let _publication = self
            .publication
            .begin_owner_change()
            .map_err(|_| CollectorSetupError::Unavailable)?;
        let mut collectors = self
            .collectors
            .lock()
            .map_err(|_| CollectorSetupError::Unavailable)?;
        let selected = collectors
            .iter_mut()
            .find(|entry| &entry.key == key)
            .ok_or(CollectorSetupError::Unavailable)?;
        if selected.record.is_some() {
            return Err(CollectorSetupError::Denied);
        }
        selected.cancelled = Some(evidence);
        Ok(())
    }

    // These are synchronous, read-only collection observations. Contention
    // does not change policy: wait for the in-memory holder, but keep poison
    // unavailable. No source mutation/publication writer is acquired here.
    // Nested order is originals -> collectors -> canonical_source; writers
    // finish awaited storage before taking these collection guards.
    pub(super) fn collector_disposition(
        &self,
        namespace: &QualifiedIngressNamespace,
        original: &OriginalWorkRef,
    ) -> Result<Option<OriginalDisposition>, ControllerReadinessFailure> {
        if namespace.ingress != self.ingress_domain
            || original.authority != self.ingress_domain.authority
        {
            return Ok(None);
        }
        // Original IDs are not reserved across the two durable record kinds.
        // An ordinary original always keeps its existing disposition owner.
        crate::no_wait::may_wait("collector.disposition");
        let originals = self
            .originals
            .lock()
            .map_err(|_| ControllerReadinessFailure::PolicyUnavailable)?;
        if originals
            .iter()
            .any(|entry| entry.names(&namespace.realm, original))
        {
            return Ok(None);
        }
        let collectors = self
            .collectors
            .lock()
            .map_err(|_| ControllerReadinessFailure::PolicyUnavailable)?;
        let Some(entry) = collectors.iter().find(|entry| {
            entry.key.realm == namespace.realm.to_string() && entry.key.id == original.work.as_str()
        }) else {
            return Ok(None);
        };
        if let Some(evidence) = &entry.cancelled {
            return Ok(Some(OriginalDisposition::OperationCancelled {
                evidence: evidence.clone(),
            }));
        }
        let record = entry
            .record
            .as_ref()
            .ok_or(ControllerReadinessFailure::PolicyUnavailable)?;
        if record.association.ingress_namespace != *namespace
            || record.association.original_work != *original
        {
            return Err(ControllerReadinessFailure::PolicyUnavailable);
        }
        Ok(Some(OriginalDisposition::Active))
    }

    pub(crate) fn current_collector_record(
        &self,
        association: &InputAuthorityAssociation,
    ) -> Result<Option<CollectorRecord>, ControllerReadinessFailure> {
        let claims = association.candidate();
        let selected =
            self.current_collector_original(&claims.ingress_namespace, &claims.original_work)?;
        let Some(selected) = selected else {
            return Ok(None);
        };
        if &selected != association {
            return Err(ControllerReadinessFailure::PolicyUnavailable);
        }
        let collectors = self
            .collectors
            .lock()
            .map_err(|_| ControllerReadinessFailure::PolicyUnavailable)?;
        collectors
            .iter()
            .filter_map(|entry| entry.record.as_ref())
            .find(|record| record.association == *claims)
            .cloned()
            .map(Some)
            .ok_or(ControllerReadinessFailure::PolicyUnavailable)
    }

    pub(super) fn current_collector_original(
        &self,
        namespace: &QualifiedIngressNamespace,
        original: &OriginalWorkRef,
    ) -> Result<Option<InputAuthorityAssociation>, ControllerReadinessFailure> {
        if namespace.ingress != self.ingress_domain {
            return Ok(None);
        }
        // An admitted original is never reclassified as a service original.
        crate::no_wait::may_wait("collector.current_original");
        let originals = self
            .originals
            .lock()
            .map_err(|_| ControllerReadinessFailure::PolicyUnavailable)?;
        if originals
            .iter()
            .any(|entry| entry.names(&namespace.realm, original))
        {
            return Ok(None);
        }
        let collectors = self
            .collectors
            .lock()
            .map_err(|_| ControllerReadinessFailure::PolicyUnavailable)?;
        let Some(entry) = collectors.iter().find(|entry| {
            entry.key.realm == namespace.realm.to_string()
                && entry.key.id == original.work.as_str()
                && original.authority == self.ingress_domain.authority
        }) else {
            return Ok(None);
        };
        if entry.cancelled.is_some() {
            return Ok(None);
        }
        // A local fence is not evidence of completed durable cancellation.
        let record = entry
            .record
            .as_ref()
            .ok_or(ControllerReadinessFailure::PolicyUnavailable)?;
        if &record.association.ingress_namespace != namespace
            || &record.association.original_work != original
        {
            return Ok(None);
        }
        if !self
            .has_installed_pair_recovery()
            .map_err(|_| ControllerReadinessFailure::PolicyUnavailable)?
        {
            return Err(ControllerReadinessFailure::PolicyUnavailable);
        }
        InputAuthorityAssociation::new(record.association.clone())
            .map(Some)
            .map_err(|_| ControllerReadinessFailure::PolicyUnavailable)
    }
}
