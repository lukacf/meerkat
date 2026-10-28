//! Stock rkat's authenticated original source: the in-process ledger of the
//! originals the CLI ingress authenticated and the runtime admitted.
//!
//! The CLI records a *candidate* original immediately before it submits the
//! input that names it. A candidate authenticates only its own submission
//! (its own occurrence) and is never a retained original or a contributor of
//! other work. Exact native acceptance promotes it
//! ([`PendingOriginal::admit`]); a refused or abandoned submission, or a
//! promotion that cannot publish, drops it. Only admitted originals serve as
//! contributors and as retained custody
//! ([`AuthenticatedOriginalSource::retained_custody`]) for every operation
//! of work that names them. While its [`PendingOriginal`] is held, a
//! candidate reads as `AdmissionPending`: the ledger settles it within that
//! one submission, by promoting it or by dropping it, and a dropped
//! candidate reads as `NotRetained` from then on.
//!
//! Custody changes share the grant authority's publication: every promotion,
//! cleanup and release publishes before the ledger changes (publication, then
//! owner lock), so an operation prepared while an original was held is no
//! longer current once that custody changes, and a release that cannot
//! publish does not happen.
//!
//! The ledger is this process's own custody, not a registry: it is never
//! persisted, so after a restart it holds nothing, and work that names an
//! original admitted by an earlier process is refused locally instead of
//! being presented as still held. The stock composition's retained resume
//! reads the same custody: a resume of a run whose originals this ledger
//! holds as admitted can be admitted, one whose original is still a pending
//! candidate is not ready yet, and one whose original it does not hold (an
//! earlier process's, or a dropped candidate) is refused. A host whose
//! delivery owner may hold such a resume announces each promotion or drop
//! (`ContinuationHostBindings::source_custody_changed`), so the resume is
//! asked again.

use std::sync::{Arc, Mutex};

use meerkat_authorization::publication::{
    LocalAuthorizationPublication, LocalPublicationGuard, PublicationError,
};
use meerkat_authorization_contracts::constraints::ResourceDomain;
use meerkat_authorization_contracts::evidence::HistoricalEvidenceRef;
use meerkat_authorization_contracts::work_association::{
    OriginalWorkRef, QualifiedIngressNamespace,
};
use meerkat_core::PrincipalRef;
use meerkat_core::authorization::ControllerReadinessFailure;
use meerkat_core::connection::RealmId;
use meerkat_runtime::input_authority::NativeIngressContext;
use meerkat_runtime::meerkat_machine::{
    AuthenticatedOriginal, AuthenticatedOriginalSource, OriginalDisposition, RetainedCustody,
};

/// Why a custody change did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StockCustodyError {
    /// The shared publication could not publish the change: nothing changed.
    #[error("original custody cannot be changed now")]
    Unavailable,
    /// Another submission of the same original, or another candidate of the
    /// same occurrence, is still pending.
    #[error("another submission of this original or occurrence is pending")]
    CandidatePending,
    /// The ledger holds no such original.
    #[error("the original is not held")]
    NotHeld,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Custody {
    /// Authenticates only its own submission; not yet admitted.
    Candidate,
    /// Admitted by the runtime: a contributor and retained custody.
    Admitted,
}

struct HeldOriginal {
    realm: RealmId,
    ingress: ResourceDomain,
    requester: PrincipalRef,
    original: AuthenticatedOriginal,
    custody: Custody,
}

impl CancelledOriginal {
    fn names(&self, realm: &RealmId, ingress: &ResourceDomain, work: &OriginalWorkRef) -> bool {
        self.realm == *realm && self.ingress == *ingress && self.work == *work
    }
}

impl HeldOriginal {
    fn names(&self, realm: &RealmId, ingress: &ResourceDomain, work: &OriginalWorkRef) -> bool {
        self.realm == *realm && self.ingress == *ingress && self.original.original_work == *work
    }
}

/// The originals this process's CLI ingress authenticated, by ingress
/// namespace, requester and original work.
pub struct StockOriginalLedger {
    publication: LocalAuthorizationPublication,
    held: Mutex<Vec<HeldOriginal>>,
    /// Cancelled originals with their evidence, by realm, ingress domain and
    /// original work. A cancellation outlives the custody rows: releasing or
    /// abandoning a row, or presenting the original again, never clears it.
    cancelled: Mutex<Vec<CancelledOriginal>>,
    /// Test support: the next candidate promotion of THIS ledger fails as if
    /// its publication could not begin (see [`Self::begin_promotion`]).
    #[cfg(any(test, feature = "test-support"))]
    promotion_fault: std::sync::atomic::AtomicBool,
}

struct CancelledOriginal {
    realm: RealmId,
    ingress: ResourceDomain,
    work: OriginalWorkRef,
    evidence: HistoricalEvidenceRef,
}

impl StockOriginalLedger {
    /// A ledger publishing its custody changes on `publication`, which must be
    /// the publication of the realm's `LocalGrantAuthority`.
    pub fn new(publication: LocalAuthorizationPublication) -> Arc<Self> {
        Arc::new(Self {
            publication,
            held: Mutex::new(Vec::new()),
            cancelled: Mutex::new(Vec::new()),
            #[cfg(any(test, feature = "test-support"))]
            promotion_fault: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// Test support: the next [`PendingOriginal::admit`] of a pending
    /// candidate of THIS ledger gets a publication failure instead of
    /// beginning its owner change, and takes the promotion's ordinary refusal
    /// (the candidate is dropped, nothing publishes). Recording candidates,
    /// cancellation, release, abandonment, an exact replay of an admitted
    /// original and every other ledger leave it armed. One-shot.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn fail_next_promotion_for_test(&self) {
        self.promotion_fault
            .store(true, std::sync::atomic::Ordering::Release);
    }

    /// The promotion's publication begin. Under test support an armed
    /// promotion fault of this ledger replaces exactly this result with a
    /// publication failure; nothing else on the path changes.
    fn begin_promotion(&self) -> Result<LocalPublicationGuard<'_>, PublicationError> {
        #[cfg(any(test, feature = "test-support"))]
        if self
            .promotion_fault
            .swap(false, std::sync::atomic::Ordering::AcqRel)
        {
            return Err(PublicationError::Unavailable);
        }
        self.publication.begin_owner_change()
    }

    /// Record the candidate original `requester` authenticated in
    /// `namespace`, immediately before the input naming it is submitted.
    /// The returned handle must be resolved by the actual native acceptance:
    /// [`PendingOriginal::admit`] after the runtime accepted that exact input
    /// (or deduplicated it to its existing admission); dropping it otherwise
    /// removes the candidate. An original already admitted is kept exactly as
    /// admitted (an exact replay authenticates against it).
    pub fn record_candidate(
        self: &Arc<Self>,
        namespace: &QualifiedIngressNamespace,
        requester: &PrincipalRef,
        original: AuthenticatedOriginal,
    ) -> Result<PendingOriginal, StockCustodyError> {
        let work = original.original_work.clone();
        let _published = self
            .publication
            .begin_owner_change()
            .map_err(|_| StockCustodyError::Unavailable)?;
        let mut held = self.lock();
        if let Some(existing) = held
            .iter()
            .find(|entry| entry.names(&namespace.realm, &namespace.ingress, &work))
        {
            return match existing.custody {
                Custody::Admitted => Ok(PendingOriginal {
                    ledger: Arc::clone(self),
                    realm: namespace.realm.clone(),
                    ingress: namespace.ingress.clone(),
                    work,
                    pending: false,
                }),
                Custody::Candidate => Err(StockCustodyError::CandidatePending),
            };
        }
        // One pending candidate per occurrence: a candidate answers only for
        // its own occurrence, so this keeps that answer bound to the one
        // submission it authenticates and never to another unadmitted
        // original of the same occurrence (for example as a contributor).
        if held.iter().any(|entry| {
            entry.custody == Custody::Candidate
                && entry.realm == namespace.realm
                && entry.ingress == namespace.ingress
                && entry.original.occurrence_scope == original.occurrence_scope
        }) {
            return Err(StockCustodyError::CandidatePending);
        }
        held.push(HeldOriginal {
            realm: namespace.realm.clone(),
            ingress: namespace.ingress.clone(),
            requester: requester.clone(),
            original,
            custody: Custody::Candidate,
        });
        Ok(PendingOriginal {
            ledger: Arc::clone(self),
            realm: namespace.realm.clone(),
            ingress: namespace.ingress.clone(),
            work,
            pending: true,
        })
    }

    /// Cancel the operations of one original this ledger holds (a candidate
    /// or an admitted one), published on the grant publication: its later
    /// operations and any re-admission are refused, while its identity and
    /// controller evidence stay held. The ledger is process memory, so the
    /// cancellation lasts as long as this process's custody does.
    pub fn cancel_operations(
        &self,
        namespace: &QualifiedIngressNamespace,
        original: &OriginalWorkRef,
        evidence: HistoricalEvidenceRef,
    ) -> Result<(), StockCustodyError> {
        let _published = self
            .publication
            .begin_owner_change()
            .map_err(|_| StockCustodyError::Unavailable)?;
        if !self
            .lock()
            .iter()
            .any(|entry| entry.names(&namespace.realm, &namespace.ingress, original))
        {
            return Err(StockCustodyError::NotHeld);
        }
        let mut cancelled = self.cancelled_lock();
        if !cancelled
            .iter()
            .any(|entry| entry.names(&namespace.realm, &namespace.ingress, original))
        {
            cancelled.push(CancelledOriginal {
                realm: namespace.realm.clone(),
                ingress: namespace.ingress.clone(),
                work: original.clone(),
                evidence,
            });
        }
        Ok(())
    }

    fn cancelled_lock(&self) -> std::sync::MutexGuard<'_, Vec<CancelledOriginal>> {
        self.cancelled
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Release the custody of one admitted original: operations prepared
    /// under it are no longer current, and later work naming it is refused.
    pub fn release(
        &self,
        namespace: &QualifiedIngressNamespace,
        original: &OriginalWorkRef,
    ) -> Result<(), StockCustodyError> {
        let _published = self
            .publication
            .begin_owner_change()
            .map_err(|_| StockCustodyError::Unavailable)?;
        self.lock().retain(|entry| {
            !(entry.custody == Custody::Admitted
                && entry.names(&namespace.realm, &namespace.ingress, original))
        });
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<HeldOriginal>> {
        self.held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// One pending candidate, resolved by the actual native acceptance of the
/// input that names it.
pub struct PendingOriginal {
    ledger: Arc<StockOriginalLedger>,
    realm: RealmId,
    ingress: ResourceDomain,
    work: OriginalWorkRef,
    pending: bool,
}

impl PendingOriginal {
    /// Promote the candidate after the runtime actually accepted the input
    /// naming it (or deduplicated it to that admission). If promotion cannot
    /// publish, the candidate is dropped and the original is not held.
    pub fn admit(mut self) -> Result<(), StockCustodyError> {
        if !self.pending {
            return Ok(());
        }
        self.pending = false;
        let published = self.ledger.begin_promotion();
        let mut held = self.ledger.lock();
        match published {
            Ok(_published) => {
                if let Some(entry) = held.iter_mut().find(|entry| {
                    entry.custody == Custody::Candidate
                        && entry.names(&self.realm, &self.ingress, &self.work)
                }) {
                    entry.custody = Custody::Admitted;
                }
                Ok(())
            }
            Err(_) => {
                remove_candidate(&mut held, &self.realm, &self.ingress, &self.work);
                Err(StockCustodyError::Unavailable)
            }
        }
    }
}

impl Drop for PendingOriginal {
    fn drop(&mut self) {
        if !self.pending {
            return;
        }
        // A candidate never served as a contributor or retained custody, so
        // removing it narrows nothing a prepared operation relies on; it is
        // removed even if the publication cannot publish.
        let _published = self.ledger.publication.begin_owner_change();
        remove_candidate(
            &mut self.ledger.lock(),
            &self.realm,
            &self.ingress,
            &self.work,
        );
    }
}

fn remove_candidate(
    held: &mut Vec<HeldOriginal>,
    realm: &RealmId,
    ingress: &ResourceDomain,
    work: &OriginalWorkRef,
) {
    held.retain(|entry| {
        !(entry.custody == Custody::Candidate && entry.names(realm, ingress, work))
    });
}

impl AuthenticatedOriginalSource for StockOriginalLedger {
    /// An admitted original of this requester in this ingress domain, or the
    /// requester's pending candidate for exactly this occurrence (the
    /// submission it authenticates). A candidate never answers for another
    /// occurrence, so it cannot qualify as a contributor of other work.
    fn authenticated_original(
        &self,
        ingress: &NativeIngressContext,
        namespace: &QualifiedIngressNamespace,
        original: &OriginalWorkRef,
    ) -> Result<Option<AuthenticatedOriginal>, ControllerReadinessFailure> {
        Ok(self
            .lock()
            .iter()
            .find(|entry| {
                entry.names(&namespace.realm, &namespace.ingress, original)
                    && entry.requester == *ingress.requester()
                    && (entry.custody == Custody::Admitted
                        || entry.original.occurrence_scope == namespace.occurrence_scope)
            })
            .map(|entry| entry.original.clone()))
    }

    /// The mandate of an admitted original; a candidate of it is pending
    /// until its [`PendingOriginal`] promotes or drops it, within the one
    /// submission that holds it.
    fn retained_custody(
        &self,
        namespace: &QualifiedIngressNamespace,
        original: &OriginalWorkRef,
    ) -> Result<RetainedCustody, ControllerReadinessFailure> {
        let held = self.lock();
        let mut pending = false;
        for entry in held
            .iter()
            .filter(|entry| entry.names(&namespace.realm, &namespace.ingress, original))
        {
            match entry.custody {
                Custody::Admitted => {
                    return Ok(RetainedCustody::Retained(
                        entry.original.authority_basis.clone(),
                    ));
                }
                Custody::Candidate => pending = true,
            }
        }
        Ok(if pending {
            RetainedCustody::AdmissionPending
        } else {
            RetainedCustody::NotRetained
        })
    }

    /// Cancelled once this process cancelled the original, whatever became
    /// of its custody rows; otherwise not operation-cancelled.
    fn original_disposition(
        &self,
        namespace: &QualifiedIngressNamespace,
        original: &OriginalWorkRef,
    ) -> Result<OriginalDisposition, ControllerReadinessFailure> {
        Ok(self
            .cancelled_lock()
            .iter()
            .find(|entry| entry.names(&namespace.realm, &namespace.ingress, original))
            .map_or(OriginalDisposition::Active, |entry| {
                OriginalDisposition::OperationCancelled {
                    evidence: entry.evidence.clone(),
                }
            }))
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests;
