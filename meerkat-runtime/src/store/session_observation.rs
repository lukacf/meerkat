//! Exact body/authority pairs for committed session reads.

use std::sync::Arc;

use meerkat_core::Session;
use meerkat_core::session::{
    CallbackBatchIdentity, CallbackBatchObservationError, StagedCallbackResultsObservation,
};
use meerkat_core::session_store::VerifiedSessionHeadMaterialization;

use super::{
    CommittedWholeBlobSnapshot, HeadCanonicalStoreAuthority, RuntimeSessionAuthority,
    RuntimeStoreError,
};

/// A verified body paired with its original physical authority, not a promise
/// that the authority remains current after this read.
#[derive(Debug)]
pub struct CommittedSessionBodyObservation {
    session: Arc<Session>,
    authority: RuntimeSessionAuthority,
}

impl CommittedSessionBodyObservation {
    pub fn from_whole_blob(snapshot: CommittedWholeBlobSnapshot) -> Self {
        let (session, _, authority) = snapshot.into_parts();
        Self {
            session,
            authority: RuntimeSessionAuthority::WholeBlob(authority),
        }
    }

    pub fn from_head_canonical(
        authority: HeadCanonicalStoreAuthority,
        materialized: VerifiedSessionHeadMaterialization,
    ) -> Result<Self, RuntimeStoreError> {
        if materialized.head() != authority.boundary_head()
            || materialized.session().id() != authority.session_id()
        {
            return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: authority.session_id().to_string(),
                detail: "verified session body differs from the observed committed head".into(),
            });
        }
        Ok(Self {
            session: Arc::clone(materialized.session()),
            authority: RuntimeSessionAuthority::HeadCanonical(authority),
        })
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn authority(&self) -> &RuntimeSessionAuthority {
        &self.authority
    }

    pub fn into_session(self) -> Session {
        Arc::unwrap_or_clone(self.session)
    }

    pub fn observe_callback_results(
        &self,
        target: &CallbackBatchIdentity,
    ) -> Result<CommittedCallbackResultsObservation, CallbackBatchObservationError> {
        let results = self.session.observe_staged_callback_results(target)?;
        Ok(CommittedCallbackResultsObservation {
            authority: self.authority.clone(),
            target: target.clone(),
            results,
        })
    }
}

/// Ordinary-owner callback content bound to the physical body that produced
/// it. The generated continuation commit must compare this exact predecessor;
/// a newer unrelated authority cannot be substituted for it.
#[derive(Debug)]
pub struct CommittedCallbackResultsObservation {
    authority: RuntimeSessionAuthority,
    target: CallbackBatchIdentity,
    results: StagedCallbackResultsObservation,
}

impl CommittedCallbackResultsObservation {
    pub fn authority(&self) -> &RuntimeSessionAuthority {
        &self.authority
    }

    pub fn target(&self) -> &CallbackBatchIdentity {
        &self.target
    }

    pub fn results(&self) -> &StagedCallbackResultsObservation {
        &self.results
    }
}
