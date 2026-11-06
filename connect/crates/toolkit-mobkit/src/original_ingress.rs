//! Trusted Toolkit application ingress for native governed work.
//!
//! This module is a Rust host-composition boundary, never a JSON/RPC decoder.
//! The application authenticates its transport and owns immutable original
//! evidence. Native grants own current permission. Original operation cancellation
//! is recorded by this source owner and read by the native disposition hook. All host
//! policy mutations share this owner's publication; source reads perform no I/O.

use meerkat_authorization::{
    clock::LocalAuthorizationClock,
    grant_policy::OperationPolicyOwner,
    grants::{
        GrantDurabilityError, GrantOwnerStore, GrantRefusal, LocalGrantAuthority,
        LocalGrantConfiguration,
    },
    publication::{LocalAuthorizationPublication, PublicationError},
};
use meerkat_authorization_contracts::{
    constraints::{ExecutionRestrictions, ResourceDomain},
    evidence::{EvidenceId, HistoricalEvidenceRef},
    grant::GrantLineageRef,
    grant_mutation::ControllerGrantMutationCustody,
    work_association::{
        InputAuthorityAssociation, OriginalWorkRef, QualifiedIngressNamespace, WorkAuthorityBasis,
    },
};
use meerkat_core::{
    ControllerModelClient, PrincipalRef, authorization::ControllerReadinessFailure,
};
use meerkat_runtime::{
    Input, RuntimeDriverError,
    identifiers::IdempotencyKey,
    input_authority::NativeIngressContext,
    meerkat_machine::{
        AuthenticatedOriginal, AuthenticatedOriginalSource, NativeGrantWorkConfiguration,
        NativeRetainedResumeCheck, OriginalDisposition, RetainedCustody, ScopedGrantComposition,
        ScopedInvocation, bind_scoped_original,
    },
};
use std::sync::{Arc, Mutex, Weak};

pub(crate) mod shared_entry_diagnostics;

pub(crate) mod custody_attempt;
mod service_set_shape;
pub(crate) use service_set_shape::ServiceSetBindingTargets;

mod collector_originals;
pub(crate) mod helper_originals;
mod historical_admission;
mod retained_resume;
pub(crate) mod shared_entry;
pub(crate) mod workflow_custody;
pub(crate) mod workflow_member;

pub(crate) fn controller_only_basis(basis: &WorkAuthorityBasis) -> bool {
    matches!(basis, WorkAuthorityBasis::TargetedGrantLineages { base: None, targets } if targets.is_empty())
}

/// Immutable application-owned evidence, not accepted native input or permission.
/// Only this writer can return the process-local handle used for explicit
/// task-grant revocation. Durable original cancellation uses the source record.
pub struct RetainedToolkitOriginal {
    association: InputAuthorityAssociation,
    custody: AuthenticatedOriginal,
    /// Established by the exact retained access decision, never lineage shape.
    grant_binding: OriginalGrantBinding,
    // Same-owner evidence retained before the original association CAS. It is
    // never reconstructed from a legacy row or a native input receipt.
    first_binding: Option<Arc<toolkit_store::StoredRecord>>,
}

impl RetainedToolkitOriginal {
    /// Persistable historical claims. Reading them establishes no authentication,
    /// accepted input, current grant or authority to reconstruct this handle.
    pub fn association(&self) -> &InputAuthorityAssociation {
        &self.association
    }

    pub(crate) fn first_binding_for(&self, key: &toolkit_store::StoreKey) -> bool {
        self.first_binding
            .as_ref()
            .is_some_and(|record| &record.key == key)
    }

    /// Historical first-write evidence for settlement of this already bound
    /// original. This does not establish a current route or new admission.
    pub(crate) fn first_binding_record(&self) -> Option<&toolkit_store::StoredRecord> {
        self.first_binding.as_deref()
    }
}

impl std::fmt::Debug for RetainedToolkitOriginal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RetainedToolkitOriginal([REDACTED])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum OriginalWriterError {
    #[error("original custody is unavailable")]
    Unavailable,
    #[error("original evidence conflicts with retained custody")]
    Conflict,
    #[error("original belongs to another ingress or owner")]
    WrongOwner,
    #[error("original authority selection is unsupported")]
    UnsupportedMandate,
    #[error("native operation grant was refused")]
    Grant(GrantRefusal),
    #[error("the native setup operation is not permitted")]
    OperationDenied,
    #[error("original operation was durably cancelled")]
    OperationCancelled,
}

/// Current source custody, settled only by the actual native submission owner.
/// Durable original bindings remain separate historical evidence.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SourceCustody {
    // Exact enrolled historical source; no native restoration has published custody.
    RestorationUnknown,
    Candidate,
    Admitted,
}

/// Canonical callback observation made under the existing source writer.
/// Historical enrollment is uncertainty, never native admission or a new
/// pending owner. The association and attempt are validated before publication.
pub(crate) enum CanonicalBindingObservation {
    Unretained,
    Cancelled(Arc<toolkit_store::StoredRecord>),
    Historical {
        association: InputAuthorityAssociation,
        attempt: custody_attempt::CustodyAttempt,
    },
}

/// A canonical binding observation's cancellation record and historical
/// custody decision, as a binding's publication applies them.
type BindingObserved = (
    Option<Arc<toolkit_store::StoredRecord>>,
    Option<custody_attempt::CustodyDecision>,
);

impl From<Option<Arc<toolkit_store::StoredRecord>>> for CanonicalBindingObservation {
    fn from(record: Option<Arc<toolkit_store::StoredRecord>>) -> Self {
        match record {
            Some(record) => Self::Cancelled(record),
            None => Self::Unretained,
        }
    }
}

impl CanonicalBindingObservation {
    pub(crate) fn historical(
        association: &InputAuthorityAssociation,
        attempt: Option<&custody_attempt::CustodyAttempt>,
    ) -> Result<Self, OriginalWriterError> {
        match attempt {
            Some(attempt) => {
                attempt.validate_binding(association)?;
                Ok(Self::Historical {
                    association: association.clone(),
                    attempt: attempt.clone(),
                })
            }
            None => Ok(Self::Unretained),
        }
    }

    pub(crate) fn first_binding_record(
        self,
    ) -> Result<Option<Arc<toolkit_store::StoredRecord>>, OriginalWriterError> {
        match self {
            Self::Unretained => Ok(None),
            Self::Cancelled(record) => Ok(Some(record)),
            Self::Historical { .. } => Err(OriginalWriterError::Conflict),
        }
    }
}

/// Task provenance is separate from native lineage. A restored task without
/// its decision owner cannot claim an issuer; a standing grant never acquires
/// task revocation authority merely because it has a leaf.
#[derive(Clone, PartialEq, Eq)]
enum OriginalGrantBinding {
    ControllerOnly,
    /// Per-operation class authority is retained by its canonical admission.
    /// It does not confer task revocation or setup helper authority.
    ClassCallbacks,
    /// Finite native targets selected for this exact authenticated original.
    NativeTargets,
    Standing,
    Task {
        issuer: Option<PrincipalRef>,
    },
    /// An exact reviewed set requires all members only on first admission.
    ServiceSet {
        issuer: PrincipalRef,
    },
    /// An internal helper has its own native mandate. Its contributor does
    /// not give this writer permission to revoke or reuse a task grant.
    Helper,
}

struct HeldOriginal {
    retained: Arc<RetainedToolkitOriginal>,
    source_custody: Option<SourceCustody>,
    // The actual bounded settlement, never reconstructed from durable IDs.
    custody_settlement: Option<Weak<crate::private_originals::custody_recovery::CustodyHandoff>>,
    // Actual native client supplied with the bound input. Its availability is
    // governed by this same candidate/admitted custody, never selection data.
    controller: Option<ControllerModelClient>,
    service_set_admission: Option<Arc<ServiceSetAdmission>>,
    operation: OriginalOperationDisposition,
}

/// Final canonical preparation for the existing native first-authentication
/// callback. This is not admission or durable source authority.
struct ServiceSetAdmission {
    publication: meerkat_authorization::publication::LocalPublicationStamp,
    expires_at_ms: u64,
}

/// An uncertain cancellation cannot be treated as Open. This disposition
/// belongs to the existing original owner, independently of source admission.
#[derive(Clone, PartialEq, Eq)]
enum OriginalOperationDisposition {
    Open,
    CancellationPending,
    Cancelled {
        record: Arc<toolkit_store::StoredRecord>,
    },
}

/// Retain a validated unbound source in this same owner collection. Before
/// first binding it may own an entered write or cancellation, never Candidate
/// or admitted contributor custody. The variant name predates binding writes.
enum OriginalRecord {
    Bound(HeldOriginal),
    UnboundCancellation {
        realm: meerkat_core::RealmId,
        original: OriginalWorkRef,
        source_record: Arc<toolkit_store::StoredRecord>,
        operation: OriginalOperationDisposition,
        first_binding: Option<InputAuthorityAssociation>,
        withdrawn_association: Option<InputAuthorityAssociation>,
    },
}

impl OriginalRecord {
    fn bound(&self) -> Option<&HeldOriginal> {
        match self {
            Self::Bound(held) => Some(held),
            Self::UnboundCancellation { .. } => None,
        }
    }
    fn bound_mut(&mut self) -> Option<&mut HeldOriginal> {
        match self {
            Self::Bound(held) => Some(held),
            Self::UnboundCancellation { .. } => None,
        }
    }
    fn names(&self, realm: &meerkat_core::RealmId, original: &OriginalWorkRef) -> bool {
        match self {
            Self::Bound(held) => {
                let c = held.association.candidate();
                &c.ingress_namespace.realm == realm && &c.original_work == original
            }
            Self::UnboundCancellation {
                realm: held_realm,
                original: held_original,
                ..
            } => held_realm == realm && held_original == original,
        }
    }
    fn operation_mut(&mut self) -> &mut OriginalOperationDisposition {
        match self {
            Self::Bound(held) => &mut held.operation,
            Self::UnboundCancellation { operation, .. } => operation,
        }
    }
}

impl std::ops::Deref for HeldOriginal {
    type Target = RetainedToolkitOriginal;
    fn deref(&self) -> &Self::Target {
        &self.retained
    }
}

impl HeldOriginal {
    fn names(&self, namespace: &QualifiedIngressNamespace, original: &OriginalWorkRef) -> bool {
        let claims = self.association.candidate();
        claims.ingress_namespace.realm == namespace.realm
            && claims.ingress_namespace.ingress == namespace.ingress
            && &claims.original_work == original
    }
}

/// Canonical source rows selected by this same writer for one cancellation.
/// The caller validates the immutable link; these coordinates confer no grant.
pub(crate) struct OriginalCancellationSource {
    pub realm: meerkat_core::RealmId,
    pub original: OriginalWorkRef,
    pub record: Arc<toolkit_store::StoredRecord>,
    pub association: Option<InputAuthorityAssociation>,
}

/// The exact native-bound input before any Candidate publication. Dropping this
/// value changes no source custody. Staging rechecks the current source owner.
pub(crate) struct PreparedToolkitOriginal {
    owner: Arc<ToolkitOriginalOwner>,
    original: Arc<RetainedToolkitOriginal>,
    input: Input,
    controller: ControllerModelClient,
}

impl PreparedToolkitOriginal {
    pub(crate) fn input(&self) -> &Input {
        &self.input
    }

    /// Return only already admitted custody. A concurrent release cannot turn
    /// this observation into a new Candidate with an unpersisted first input.
    pub(crate) fn admitted(
        &self,
    ) -> Result<Option<(Input, PendingToolkitOriginal)>, RuntimeDriverError> {
        self.owner
            .admitted_guard(&self.original)
            .map(|pending| pending.map(|pending| (self.input.clone(), pending)))
            .map_err(|_| RuntimeDriverError::ValidationFailed {
                reason: "Toolkit admitted original is unavailable".into(),
            })
    }

    pub(crate) fn stage(self) -> Result<(Input, PendingToolkitOriginal), RuntimeDriverError> {
        let pending = self
            .owner
            .record_candidate_with_controller(&self.original, Some(self.controller))
            .map_err(|_| RuntimeDriverError::ValidationFailed {
                reason: "Toolkit original candidate is unavailable".into(),
            })?;
        Ok((self.input, pending))
    }

    /// Rebind the same still-owned candidate after a proven native refusal.
    /// The caller excludes recovery and validates the exact retained Input;
    /// this method neither creates nor replaces a custody guard.
    pub(crate) fn rebind_candidate(
        self,
        pending: &PendingToolkitOriginal,
    ) -> Result<Input, OriginalWriterError> {
        if !pending.pending
            || !Arc::ptr_eq(&self.owner, &pending.owner)
            || !Arc::ptr_eq(&self.original, &pending.original)
        {
            return Err(OriginalWriterError::WrongOwner);
        }
        self.owner.check_bound_source(&self.original)?;
        let _publication = self
            .owner
            .publication
            .begin_owner_change()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let mut originals = self
            .owner
            .originals
            .lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let claims = self.original.association().candidate();
        let namespace = &claims.ingress_namespace;
        if claims.contributing_work.iter().any(|contributor| {
            !originals
                .iter()
                .filter_map(OriginalRecord::bound)
                .any(|held| {
                    held.source_custody == Some(SourceCustody::Admitted)
                        && held.operation == OriginalOperationDisposition::Open
                        && held.names(namespace, contributor)
                        && held.association.candidate().requester == claims.requester
                })
        }) {
            return Err(OriginalWriterError::Conflict);
        }
        let held = originals
            .iter_mut()
            .filter_map(OriginalRecord::bound_mut)
            .find(|held| Arc::ptr_eq(&held.retained, &self.original))
            .ok_or(OriginalWriterError::WrongOwner)?;
        if held.operation != OriginalOperationDisposition::Open
            || held.source_custody != Some(SourceCustody::Candidate)
        {
            return Err(OriginalWriterError::Conflict);
        }
        held.controller = Some(self.controller);
        Ok(self.input)
    }
}

// One live adapter per source owner. The published store already owns this
// source's publication Arc, so the reverse store reference must remain weak.
struct CanonicalSourceBinding {
    store: Weak<dyn toolkit_store::ToolkitStore>,
    scope: toolkit_store::StoreScope,
    installed_pair: bool,
}

/// A candidate held alongside its exact bound input until native admission.
/// This is the source owner's custody guard, not a native admission receipt.
/// The Meta binder adapter retains it through tracked submission, calls
/// `admit` only on fresh native acceptance, and drops the retry guard on native
/// deduplication or definite pre-admission refusal. Unresolved admission retains
/// custody. A fresh TurnTicket is not acknowledgement. This type owns source
/// custody; native admission remains the runtime owner's decision.
pub struct PendingToolkitOriginal {
    owner: Arc<ToolkitOriginalOwner>,
    original: Arc<RetainedToolkitOriginal>,
    pending: bool,
}

impl PendingToolkitOriginal {
    /// Intrinsic state of this actual guard, not a native admission observation.
    /// A later input may carry an admitted-only guard without staging Candidate.
    pub(crate) fn is_first_candidate(&self) -> bool {
        self.pending
    }

    pub(crate) fn enroll_settlement(
        &self,
        settlement: &Arc<crate::private_originals::custody_recovery::CustodyHandoff>,
    ) -> Result<(), OriginalWriterError> {
        if !self.pending || !settlement.enrolled() {
            return Ok(());
        }
        let _publication = self
            .owner
            .publication
            .begin_owner_change()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let mut originals = self
            .owner
            .originals
            .lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let held = originals
            .iter_mut()
            .filter_map(OriginalRecord::bound_mut)
            .find(|held| Arc::ptr_eq(&held.retained, &self.original))
            .ok_or(OriginalWriterError::WrongOwner)?;
        if held.source_custody != Some(SourceCustody::Candidate) {
            return Err(OriginalWriterError::Conflict);
        }
        held.custody_settlement = Some(Arc::downgrade(settlement));
        Ok(())
    }

    /// Settle this source candidate after exact native admission. An error is
    /// a post-admission source-publication failure, never evidence that the
    /// accepted native work was refused. Its completion/ticket stays native.
    pub fn admit(mut self) -> Result<Arc<RetainedToolkitOriginal>, OriginalWriterError> {
        if !self.pending {
            return Ok(self.original.clone());
        }
        self.pending = false;
        let publication = self.owner.publication.begin_owner_change();
        let mut originals = self
            .owner
            .originals
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let held = originals
            .iter_mut()
            .filter_map(OriginalRecord::bound_mut)
            .find(|entry| Arc::ptr_eq(&entry.retained, &self.original))
            .ok_or(OriginalWriterError::WrongOwner)?;
        if held.source_custody != Some(SourceCustody::Candidate) {
            return Err(OriginalWriterError::Conflict);
        }
        match publication {
            Ok(_publication) => {
                held.source_custody = Some(SourceCustody::Admitted);
                Ok(self.original.clone())
            }
            Err(_) => {
                // The candidate never served as a contributor. Its durable
                // association remains evidence for explicit native replay.
                held.source_custody = None;
                held.controller = None;
                Err(OriginalWriterError::Unavailable)
            }
        }
    }
}

impl Drop for PendingToolkitOriginal {
    fn drop(&mut self) {
        if !self.pending {
            return;
        }
        // Withdrawal changes no admitted contributor. As in the native-owned
        // stock reference, remove an unadmitted candidate even if publication
        // is unavailable, while retaining its immutable durable binding.
        let _publication = self.owner.publication.begin_owner_change();
        let mut originals = self
            .owner
            .originals
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(held) = originals
            .iter_mut()
            .filter_map(OriginalRecord::bound_mut)
            .find(|entry| Arc::ptr_eq(&entry.retained, &self.original))
            && held.source_custody == Some(SourceCustody::Candidate)
        {
            held.source_custody = None;
            held.controller = None;
        }
    }
}

/// The source owner's actual local state. It holds the same native grant Arc
/// that the host installs through ScopedGrantComposition and Meta's E1 seam.
/// The mutation mutex serializes application writes only. Native reads never
/// acquire it, and it is not a cancellation or permission owner.
pub struct ToolkitOriginalOwner {
    ingress_domain: ResourceDomain,
    publication: LocalAuthorizationPublication,
    clock: Arc<dyn LocalAuthorizationClock>,
    grants: Arc<LocalGrantAuthority>,
    mutation: tokio::sync::Mutex<()>,
    #[cfg(test)]
    source_writer_wait: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    #[cfg(test)]
    record_change_probe: Mutex<Option<Arc<dyn toolkit_store::RecordChangePublication>>>,
    // A fixture's hold of the source writer, taken just before the next
    // helper admission takes the writer.
    #[cfg(test)]
    helper_admission_writer_hold: Mutex<Option<std::time::Duration>>,
    // A helper admission's writer and source checks, in order, beside that
    // fixture holder's release of the writer.
    #[cfg(test)]
    helper_admission_events: Mutex<Vec<&'static str>>,
    originals: Mutex<Vec<OriginalRecord>>,
    // Published by the same canonical collector writer, never agent custody.
    collectors: Mutex<Vec<collector_originals::PublishedCollector>>,
    canonical_source: Mutex<Option<CanonicalSourceBinding>>,
    // Ephemeral native-send to Controller-decision custody, never permission.
    shared_entries: Mutex<Vec<Arc<shared_entry::SharedEntry>>>,
    // Restrictive selections captured by the real registration owner.
    workflow_members: Mutex<Vec<workflow_member::SelectedWorkflowMember>>,
}

impl toolkit_store::RecordChangePublication for ToolkitOriginalOwner {
    fn begin_record_change(
        &self,
    ) -> toolkit_store::StoreResult<Box<dyn toolkit_store::RecordChangeGuard + '_>> {
        #[cfg(test)]
        {
            // One fixture pause after the real SQLite transaction validated
            // its row, before the unchanged native publication begins. No
            // observer mutex or replacement publication spans the callback.
            let probe = self.record_change_probe.lock().unwrap().take();
            if let Some(probe) = probe {
                let _probe = probe
                    .begin_record_change()
                    .expect("test publication probe failed");
            }
        }
        // The canonical SQLite owner invokes this before a validated UPDATE.
        // This is the same publication used by native grants and source
        // cancellation, not a second record or permission owner.
        Ok(Box::new(RecordPublicationGuard {
            _guard: self
                .publication
                .begin_owner_change()
                .map_err(|_| toolkit_store::StoreError::Synchronization)?,
        }))
    }
}

struct RecordPublicationGuard<'a> {
    _guard: meerkat_authorization::publication::LocalPublicationGuard<'a>,
}
impl toolkit_store::RecordChangeGuard for RecordPublicationGuard<'_> {}

impl ToolkitOriginalOwner {
    /// Observe the next actual SQLite record change without replacing this
    /// owner's publication or store. Only test builds have this pause seam.
    #[cfg(test)]
    pub(crate) fn observe_next_record_change_for_test(
        &self,
        probe: Arc<dyn toolkit_store::RecordChangePublication>,
    ) {
        assert!(
            self.record_change_probe
                .lock()
                .unwrap()
                .replace(probe)
                .is_none()
        );
    }

    /// Restore the exact owner once. A missing/corrupt/leased store is an error,
    /// never a reason to initialize a replacement or translate a sealed root.
    pub fn restore(
        store: Arc<dyn GrantOwnerStore>,
        clock: Arc<dyn LocalAuthorizationClock>,
        ingress_domain: ResourceDomain,
    ) -> Result<Self, GrantDurabilityError> {
        let publication = LocalAuthorizationPublication::new();
        let grants = LocalGrantAuthority::restore(store, publication.clone(), clock.clone())?;
        Ok(Self::from_native(
            grants,
            publication,
            clock,
            ingress_domain,
        ))
    }

    /// Explicit first-use setup, selected by the application's existing setup
    /// decision and authenticated administration. Never called after a failed
    /// restore. Native create refuses an occupied store. Root/controller grant
    /// issuance remains a separate call on this actual native owner.
    pub fn initialize(
        configuration: LocalGrantConfiguration,
        store: Arc<dyn GrantOwnerStore>,
        clock: Arc<dyn LocalAuthorizationClock>,
        ingress_domain: ResourceDomain,
    ) -> Result<Self, GrantDurabilityError> {
        let publication = LocalAuthorizationPublication::new();
        let grants = LocalGrantAuthority::new_durable(
            configuration,
            store,
            publication.clone(),
            clock.clone(),
        )?;
        Ok(Self::from_native(
            grants,
            publication,
            clock,
            ingress_domain,
        ))
    }

    fn from_native(
        grants: LocalGrantAuthority,
        publication: LocalAuthorizationPublication,
        clock: Arc<dyn LocalAuthorizationClock>,
        ingress_domain: ResourceDomain,
    ) -> Self {
        Self {
            ingress_domain,
            publication,
            clock,
            grants: Arc::new(grants),
            mutation: tokio::sync::Mutex::new(()),
            #[cfg(test)]
            source_writer_wait: Mutex::new(None),
            #[cfg(test)]
            record_change_probe: Mutex::new(None),
            #[cfg(test)]
            helper_admission_writer_hold: Mutex::new(None),
            #[cfg(test)]
            helper_admission_events: Mutex::new(Vec::new()),
            originals: Mutex::new(Vec::new()),
            collectors: Mutex::new(Vec::new()),
            canonical_source: Mutex::new(None),
            shared_entries: Mutex::new(Vec::new()),
            workflow_members: Mutex::new(Vec::new()),
        }
    }

    /// Claim only after format and constructor inputs have been validated.
    /// A returned adapter may be Arc-cloned, but a second constructor cannot
    /// create independent retained vectors under this same source authority.
    pub(crate) fn bind_canonical_source(
        &self,
        store: &Arc<dyn toolkit_store::ToolkitStore>,
        scope: &toolkit_store::StoreScope,
    ) -> Result<(), OriginalWriterError> {
        let mut selected = self
            .canonical_source
            .lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        if let Some(existing) = selected.as_ref() {
            let existing_store = existing
                .store
                .upgrade()
                .ok_or(OriginalWriterError::Unavailable)?;
            return Err(
                if Arc::ptr_eq(&existing_store, store) && existing.scope == *scope {
                    OriginalWriterError::Conflict
                } else {
                    OriginalWriterError::WrongOwner
                },
            );
        }
        *selected = Some(CanonicalSourceBinding {
            store: Arc::downgrade(store),
            scope: scope.clone(),
            installed_pair: false,
        });
        Ok(())
    }

    /// Existing low-level sources without a private adapter remain unchanged.
    /// Once an adapter selected its canonical store, helper reads and writes
    /// must use that same object and scope; dead selection is not rebinding.
    pub(crate) fn check_canonical_source(
        &self,
        store: &dyn toolkit_store::ToolkitStore,
        scope: &toolkit_store::StoreScope,
    ) -> Result<(), OriginalWriterError> {
        let selected = self
            .canonical_source
            .lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let Some(selected) = selected.as_ref() else {
            return Ok(());
        };
        let selected_store = selected
            .store
            .upgrade()
            .ok_or(OriginalWriterError::Unavailable)?;
        let same_store = std::ptr::eq(
            selected_store.as_ref() as *const dyn toolkit_store::ToolkitStore as *const (),
            store as *const dyn toolkit_store::ToolkitStore as *const (),
        );
        if same_store && selected.scope == *scope {
            Ok(())
        } else {
            Err(OriginalWriterError::WrongOwner)
        }
    }

    // Only the actual installed openers call this after opening both selected
    // nonsymlink stores under the exclusive native grant lease. It is not a
    // general durable-store capability or a runtime-start side effect.
    pub(crate) fn select_installed_pair_recovery(
        &self,
        store: &dyn toolkit_store::ToolkitStore,
        scope: &toolkit_store::StoreScope,
    ) -> Result<(), OriginalWriterError> {
        self.check_canonical_source(store, scope)?;
        let mut selected = self
            .canonical_source
            .lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let selected = selected.as_mut().ok_or(OriginalWriterError::WrongOwner)?;
        selected.installed_pair = true;
        Ok(())
    }

    pub(crate) fn has_installed_pair_recovery(&self) -> Result<bool, OriginalWriterError> {
        let selected = self
            .canonical_source
            .lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let Some(selected) = selected.as_ref() else {
            return Ok(false);
        };
        if selected.store.upgrade().is_none() {
            return Err(OriginalWriterError::Unavailable);
        }
        Ok(selected.installed_pair)
    }

    // Foreground provenance is retained before the first binding write. A cold
    // census must never steal an entry merely because Candidate is not staged
    // yet, or its tracked binder is still in an entered store operation.
    pub(crate) fn has_live_first_binding(
        &self,
        key: &toolkit_store::StoreKey,
    ) -> Result<bool, OriginalWriterError> {
        let originals = self
            .originals
            .lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        Ok(originals.iter().any(|record| match record {
            OriginalRecord::Bound(held) => held.retained.first_binding_for(key),
            OriginalRecord::UnboundCancellation {
                source_record,
                first_binding,
                ..
            } => first_binding.is_some() && &source_record.key == key,
        }))
    }

    pub fn grants(&self) -> Arc<LocalGrantAuthority> {
        self.grants.clone()
    }
    pub fn publication(&self) -> LocalAuthorizationPublication {
        self.publication.clone()
    }
    /// A status read may omit an already terminal canonical row, but must not
    /// hide an unresolved cancellation in this existing process owner. Absence
    /// after restart supplies no authority and creates no replacement custody.
    pub(crate) fn check_status_cancellation(
        &self,
        namespace: &QualifiedIngressNamespace,
        original: &OriginalWorkRef,
        cancelled: bool,
        association: Option<&InputAuthorityAssociation>,
        source_record: &toolkit_store::StoredRecord,
    ) -> Result<(), OriginalWriterError> {
        if namespace.ingress != self.ingress_domain {
            return Err(OriginalWriterError::WrongOwner);
        }
        let originals = self
            .originals
            .try_lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let Some(record) = originals
            .iter()
            .find(|entry| entry.names(&namespace.realm, original))
        else {
            return Ok(());
        };
        let operation = match record {
            OriginalRecord::Bound(held) => {
                if association != Some(held.retained.association()) {
                    return Err(OriginalWriterError::Conflict);
                }
                &held.operation
            }
            OriginalRecord::UnboundCancellation {
                source_record: retained,
                operation,
                ..
            } => {
                if !crate::private_originals::same_source_origin(retained, source_record) {
                    return Err(OriginalWriterError::Conflict);
                }
                operation
            }
        };
        match operation {
            OriginalOperationDisposition::Open if !cancelled => Ok(()),
            OriginalOperationDisposition::Cancelled { record }
                if cancelled && record.as_ref() == source_record =>
            {
                Ok(())
            }
            OriginalOperationDisposition::CancellationPending => {
                Err(OriginalWriterError::Unavailable)
            }
            _ => Err(OriginalWriterError::Conflict),
        }
    }
    pub fn clock(&self) -> Arc<dyn LocalAuthorizationClock> {
        self.clock.clone()
    }
    pub fn ingress_domain(&self) -> &ResourceDomain {
        &self.ingress_domain
    }

    /// Compose the native configuration with this exact grant/source owner.
    /// The application supplies its actual policies constructed against this
    /// publication. The one-use E1 handoff consumes the returned configuration;
    /// no independently supplied grant/clock/bootstrap pair can be mixed here.
    pub fn configuration(
        self: &Arc<Self>,
        invocation: ScopedInvocation,
        operation_owner: Arc<dyn OperationPolicyOwner>,
        retained_resume: Option<Arc<NativeRetainedResumeCheck>>,
    ) -> NativeGrantWorkConfiguration {
        let mut configuration = ScopedGrantComposition {
            grants: self.grants.clone(),
            ingress_domain: self.ingress_domain.clone(),
            originals: self.clone(),
            invocation,
            operation_owner,
            retained_resume,
        }
        .into_configuration();
        let ordinary = configuration.ingress.clone();
        let owner = Arc::downgrade(self);
        configuration.ingress = Arc::new(move |runtime, input, ingress, association| {
            let owner = owner.upgrade().ok_or_else(workflow_member::unavailable)?;
            owner.check_workflow_member(runtime, association, ingress.controller_client())?;
            ordinary(runtime, input, ingress, association)
        });
        configuration
    }

    /// Create an original-specific child under the host-selected operation base.
    /// The independently authenticated source facts must agree with all claimed
    /// original fields before issuance. Caller identity is authenticated by the
    /// application, not by accepting these arguments.
    ///
    /// `persist_new_original` is the application's existing durable original-key
    /// compare-and-insert. It stores the complete final association, refuses an
    /// already-bound or uncertain original, and returns only after its promised
    /// durability. It does not submit native input. No publication/source lock
    /// is held across this callback. Its result is the exact durable cancellation record, or None for Open,
    /// read in the same serialized write. Existing originals use restore/replay,
    /// never this new-issuance path.
    ///
    /// Order: issue child, durably bind source original, retain local evidence,
    /// return handle. A crash or refused insert can leave an unused child. Such
    /// a child has no published source original and cannot pass scoped ingress.
    /// Restart first reads the canonical source binding; a new child is safe
    /// only if that owner proves no binding was committed. No business effect
    /// or native replay result is inferred from internal issuance.
    #[allow(clippy::too_many_arguments)]
    pub fn enroll_new_authenticated(
        &self,
        association: InputAuthorityAssociation,
        source_custody: AuthenticatedOriginal,
        authenticated_requester: &PrincipalRef,
        authenticated_ingress_actor: &PrincipalRef,
        authenticated_grant_issuer: &PrincipalRef,
        child_id: EvidenceId,
        requested: ExecutionRestrictions,
        persist_new_original: impl FnOnce(
            &InputAuthorityAssociation,
        ) -> Result<
            Option<Arc<toolkit_store::StoredRecord>>,
            OriginalWriterError,
        >,
    ) -> Result<Arc<RetainedToolkitOriginal>, OriginalWriterError> {
        let _mutation = self
            .mutation
            .try_lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let (final_association, final_custody) = self.prepare_new_authenticated(
            association,
            source_custody,
            authenticated_requester,
            authenticated_ingress_actor,
            authenticated_grant_issuer,
            child_id,
            requested,
        )?;
        let cancelled_record = persist_new_original(&final_association)?;
        self.publish_task(
            final_association,
            final_custody,
            authenticated_grant_issuer.clone(),
            cancelled_record,
        )
    }

    /// The same original writer with an asynchronous durable store callback.
    /// Only the writer mutex spans storage I/O; native source reads never take
    /// it, and no native publication guard is retained across the await.
    #[allow(clippy::too_many_arguments)]
    pub async fn enroll_new_authenticated_async<F, Fut>(
        &self,
        association: InputAuthorityAssociation,
        source_custody: AuthenticatedOriginal,
        authenticated_requester: &PrincipalRef,
        authenticated_ingress_actor: &PrincipalRef,
        authenticated_grant_issuer: &PrincipalRef,
        child_id: EvidenceId,
        requested: ExecutionRestrictions,
        persist_new_original: F,
    ) -> Result<Arc<RetainedToolkitOriginal>, OriginalWriterError>
    where
        F: FnOnce(InputAuthorityAssociation) -> Fut,
        Fut: std::future::Future<
                Output = Result<Option<Arc<toolkit_store::StoredRecord>>, OriginalWriterError>,
            >,
    {
        let _mutation = self
            .mutation
            .try_lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let (final_association, final_custody) = self.prepare_new_authenticated(
            association,
            source_custody,
            authenticated_requester,
            authenticated_ingress_actor,
            authenticated_grant_issuer,
            child_id,
            requested,
        )?;
        let cancelled_record = persist_new_original(final_association.clone()).await?;
        self.publish_task(
            final_association,
            final_custody,
            authenticated_grant_issuer.clone(),
            cancelled_record,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_new_authenticated(
        &self,
        association: InputAuthorityAssociation,
        source_custody: AuthenticatedOriginal,
        authenticated_requester: &PrincipalRef,
        authenticated_ingress_actor: &PrincipalRef,
        authenticated_grant_issuer: &PrincipalRef,
        child_id: EvidenceId,
        requested: ExecutionRestrictions,
    ) -> Result<(InputAuthorityAssociation, AuthenticatedOriginal), OriginalWriterError> {
        self.check_source(
            &association,
            &source_custody,
            authenticated_requester,
            authenticated_ingress_actor,
        )?;
        self.require_absent(&association)?;
        let claims = association.candidate();
        let WorkAuthorityBasis::GrantLineage { lineage } = &claims.authority_basis else {
            return Err(OriginalWriterError::UnsupportedMandate);
        };
        if lineage.len()
            >= meerkat_authorization_contracts::work_association::MAX_ASSOCIATION_REFERENCES
        {
            return Err(OriginalWriterError::UnsupportedMandate);
        }
        let parent = lineage
            .last()
            .ok_or(OriginalWriterError::UnsupportedMandate)?;
        self.grants
            .resolve_lineage(
                lineage,
                authenticated_grant_issuer,
                claims.represented_subject.as_ref(),
            )
            .map_err(OriginalWriterError::Grant)?;
        let child = self
            .grants
            .issue_child(
                authenticated_grant_issuer,
                parent,
                child_id,
                claims.logical_executor.clone(),
                requested,
            )
            .map_err(OriginalWriterError::Grant)?;
        let mut final_lineage = lineage.clone();
        final_lineage.push(child);
        let mut final_claims = claims.clone();
        final_claims.authority_basis = WorkAuthorityBasis::GrantLineage {
            lineage: final_lineage,
        };
        let final_association = InputAuthorityAssociation::new(final_claims)
            .map_err(|_| OriginalWriterError::Conflict)?;
        let mut final_custody = source_custody;
        final_custody.authority_basis = final_association.candidate().authority_basis.clone();
        self.check_original_child(&final_association)?;
        Ok((final_association, final_custody))
    }

    /// Retain a first conversation with no selected operation lineage. The
    /// complete controller lineage is independently resolved for the executor;
    /// it never becomes an operation grant or an empty selected agent mandate.
    /// The durable callback observes the canonical original before publication,
    /// including on cold rebind. It cannot replace an earlier association.
    pub(crate) async fn bind_controller_only_async<F, Fut>(
        &self,
        association: InputAuthorityAssociation,
        source_custody: AuthenticatedOriginal,
        authenticated_requester: &PrincipalRef,
        authenticated_ingress_actor: &PrincipalRef,
        persist_or_observe: F,
    ) -> Result<Arc<RetainedToolkitOriginal>, OriginalWriterError>
    where
        F: FnOnce(InputAuthorityAssociation) -> Fut,
        Fut: std::future::Future<Output = Result<CanonicalBindingObservation, OriginalWriterError>>,
    {
        let _mutation = self.source_writer().await;
        self.check_source(
            &association,
            &source_custody,
            authenticated_requester,
            authenticated_ingress_actor,
        )?;
        self.refuse_cancelled_binding(&association)?;
        let claims = association.candidate();
        if !controller_only_basis(&claims.authority_basis) {
            return Err(OriginalWriterError::UnsupportedMandate);
        }
        self.grants
            .resolve_controller_lineage(
                &claims.controller_grant_lineage,
                &claims.logical_executor,
                claims.represented_subject.as_ref(),
            )
            .map_err(OriginalWriterError::Grant)?
            .check_current()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let observation = persist_or_observe(association.clone()).await?;
        self.publish_binding(
            association,
            source_custody,
            OriginalGrantBinding::ControllerOnly,
            observation,
        )
    }

    /// Class callbacks use only independently selected target lineages. The
    /// canonical source callback authenticates the immutable admission and
    /// exact native references under this writer, including on replay. No
    /// operation target becomes a base or a setup/task grant.
    pub(crate) async fn bind_class_callbacks_async<F, Fut>(
        &self,
        association: InputAuthorityAssociation,
        source_custody: AuthenticatedOriginal,
        authenticated_requester: &PrincipalRef,
        authenticated_ingress_actor: &PrincipalRef,
        persist_or_observe: F,
    ) -> Result<Arc<RetainedToolkitOriginal>, OriginalWriterError>
    where
        F: FnOnce(InputAuthorityAssociation) -> Fut,
        Fut: std::future::Future<Output = Result<CanonicalBindingObservation, OriginalWriterError>>,
    {
        let _mutation = self.source_writer().await;
        self.check_source(
            &association,
            &source_custody,
            authenticated_requester,
            authenticated_ingress_actor,
        )?;
        self.refuse_cancelled_binding(&association)?;
        let claims = association.candidate();
        let WorkAuthorityBasis::TargetedGrantLineages {
            base: None,
            targets,
        } = &claims.authority_basis
        else {
            return Err(OriginalWriterError::UnsupportedMandate);
        };
        if targets.iter().any(|row| !matches!(row.target,
            meerkat_authorization_contracts::work_association::GrantTarget::RegisteredToolBundleOperation { .. }
            | meerkat_authorization_contracts::work_association::GrantTarget::McpServer { .. })) {
            return Err(OriginalWriterError::UnsupportedMandate);
        }
        self.grants
            .resolve_controller_lineage(
                &claims.controller_grant_lineage,
                &claims.logical_executor,
                claims.represented_subject.as_ref(),
            )
            .map_err(OriginalWriterError::Grant)?
            .check_current()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let observation = persist_or_observe(association.clone()).await?;
        self.publish_binding(
            association,
            source_custody,
            OriginalGrantBinding::ClassCallbacks,
            observation,
        )
    }

    /// The canonical source validates every selected native row and the
    /// unchanged independent class/setup basis. Only source-specific native
    /// children take the original-isolation check; class callbacks retain
    /// their existing admission-owned sharing semantics.
    pub(crate) async fn bind_native_targets_async<F, Fut>(
        &self,
        association: InputAuthorityAssociation,
        source_custody: AuthenticatedOriginal,
        requester: &PrincipalRef,
        actor: &PrincipalRef,
        selected: &[meerkat_authorization_contracts::work_association::GrantTarget],
        persist_or_observe: F,
    ) -> Result<Arc<RetainedToolkitOriginal>, OriginalWriterError>
    where
        F: FnOnce(InputAuthorityAssociation) -> Fut,
        Fut: std::future::Future<Output = Result<CanonicalBindingObservation, OriginalWriterError>>,
    {
        use meerkat_authorization_contracts::work_association::GrantTarget;
        let _writer = self.source_writer().await;
        self.check_source(&association, &source_custody, requester, actor)?;
        self.refuse_cancelled_binding(&association)?;
        let claims = association.candidate();
        let WorkAuthorityBasis::TargetedGrantLineages { targets, .. } = &claims.authority_basis
        else {
            return Err(OriginalWriterError::UnsupportedMandate);
        };
        if selected.is_empty()
            || selected.windows(2).any(|pair| pair[0] >= pair[1])
            || selected.iter().any(|target| {
                !matches!(
                    target,
                    GrantTarget::Peer { .. }
                        | GrantTarget::PeerDirectory
                        | GrantTarget::Memory { .. }
                )
            })
        {
            return Err(OriginalWriterError::UnsupportedMandate);
        }
        let isolated = targets
            .iter()
            .filter(|row| selected.contains(&row.target))
            .cloned()
            .collect::<Vec<_>>();
        if isolated.len() != selected.len() {
            return Err(OriginalWriterError::Conflict);
        }
        let mut projected = claims.clone();
        projected.authority_basis = WorkAuthorityBasis::TargetedGrantLineages {
            base: None,
            targets: isolated,
        };
        let projected =
            InputAuthorityAssociation::new(projected).map_err(|_| OriginalWriterError::Conflict)?;
        {
            let originals = self
                .originals
                .try_lock()
                .map_err(|_| OriginalWriterError::Unavailable)?;
            Self::check_selected_lineage_isolation(&projected, &originals, Some(&association))?;
        }
        self.grants
            .resolve_controller_lineage(
                &claims.controller_grant_lineage,
                &claims.logical_executor,
                claims.represented_subject.as_ref(),
            )
            .map_err(OriginalWriterError::Grant)?
            .check_current()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let observation = persist_or_observe(association.clone()).await?;
        self.publish_binding(
            association,
            source_custody,
            OriginalGrantBinding::NativeTargets,
            observation,
        )
    }

    /// Bind or cold-rebind the actual selected access decision. A task issuer
    /// is supplied only after that decision proves this exact original; None
    /// means standing access, not an unknown task issuer. The durable callback
    /// must re-read the canonical row while this writer mutex is held. Its
    /// cancelled disposition cannot be taken from a pre-lock snapshot.
    pub(crate) async fn bind_issued_access_async<F, Fut>(
        &self,
        association: InputAuthorityAssociation,
        source_custody: AuthenticatedOriginal,
        authenticated_requester: &PrincipalRef,
        authenticated_ingress_actor: &PrincipalRef,
        task_issuer: Option<&PrincipalRef>,
        persist_or_observe: F,
    ) -> Result<Arc<RetainedToolkitOriginal>, OriginalWriterError>
    where
        F: FnOnce(InputAuthorityAssociation) -> Fut,
        Fut: std::future::Future<Output = Result<CanonicalBindingObservation, OriginalWriterError>>,
    {
        let _mutation = self.source_writer().await;
        self.check_source(
            &association,
            &source_custody,
            authenticated_requester,
            authenticated_ingress_actor,
        )?;
        self.refuse_cancelled_binding(&association)?;
        let binding = match task_issuer {
            Some(issuer) => {
                self.check_task_leaf(&association)?;
                self.check_original_child(&association)?;
                OriginalGrantBinding::Task {
                    issuer: Some(issuer.clone()),
                }
            }
            None => {
                self.check_standing_lineage(&association)?;
                OriginalGrantBinding::Standing
            }
        };
        let observation = persist_or_observe(association.clone()).await?;
        self.publish_binding(association, source_custody, binding, observation)
    }

    /// Bind the exact independently commissioned G base and setup MCP target.
    /// The planning callback verifies the actual late-helper provenance and
    /// immutable full basis under this same source writer. This remains Task
    /// custody; it is not an approved service-set or a new authority owner.
    pub(crate) async fn bind_workflow_access_async<F, Fut>(
        &self,
        association: InputAuthorityAssociation,
        custody: AuthenticatedOriginal,
        requester: &PrincipalRef,
        actor: &PrincipalRef,
        issuer: &PrincipalRef,
        persist_or_observe: F,
    ) -> Result<Arc<RetainedToolkitOriginal>, OriginalWriterError>
    where
        F: FnOnce(InputAuthorityAssociation) -> Fut,
        Fut: std::future::Future<Output = Result<CanonicalBindingObservation, OriginalWriterError>>,
    {
        let _writer = self.source_writer().await;
        self.check_source(&association, &custody, requester, actor)?;
        self.refuse_cancelled_binding(&association)?;
        let claims = association.candidate();
        let (_, target) =
            crate::installed_host::operator_setup::workflow_mcp::split(&claims.authority_basis)
                .map_err(|_| OriginalWriterError::UnsupportedMandate)?;
        if target.is_none() {
            return Err(OriginalWriterError::UnsupportedMandate);
        }
        for lineage in claims.authority_basis.grant_lineages() {
            let grant = self
                .grants
                .resolve_lineage(
                    lineage,
                    &claims.logical_executor,
                    claims.represented_subject.as_ref(),
                )
                .map_err(OriginalWriterError::Grant)?;
            if grant.restrictions().delegation_depth
                != meerkat_authorization_contracts::constraints::DelegationDepth::remaining(0)
            {
                return Err(OriginalWriterError::UnsupportedMandate);
            }
            grant
                .check_current()
                .map_err(|_| OriginalWriterError::Unavailable)?;
        }
        self.check_original_child(&association)?;
        let observation = persist_or_observe(association.clone()).await?;
        self.publish_binding(
            association,
            custody,
            OriginalGrantBinding::Task {
                issuer: Some(issuer.clone()),
            },
            observation,
        )
    }

    fn check_task_leaf(
        &self,
        association: &InputAuthorityAssociation,
    ) -> Result<(), OriginalWriterError> {
        let c = association.candidate();
        let lineage = match &c.authority_basis {
            WorkAuthorityBasis::GrantLineage { lineage } => lineage.as_slice(),
            WorkAuthorityBasis::TargetedGrantLineages { .. } => {
                let find = c
                    .authority_basis
                    .lineage_for(Some(
                        &crate::installed_host::operator_setup::setup_targets::find_target(),
                    ))
                    .ok_or(OriginalWriterError::UnsupportedMandate)?;
                // The canonical callback validates the complete issued target
                // selection and exact child identities before publication. A
                // cold structural rebind must not resolve an unrelated target.
                // First issuance checks depth/currentness; native operation
                // entry checks the target actually used on each invocation.
                return if find.len() >= 2 {
                    Ok(())
                } else {
                    Err(OriginalWriterError::UnsupportedMandate)
                };
            }
            _ => return Err(OriginalWriterError::UnsupportedMandate),
        };
        let resolved = self
            .grants
            .resolve_lineage(lineage, &c.logical_executor, c.represented_subject.as_ref())
            .map_err(OriginalWriterError::Grant)?;
        if resolved.restrictions().delegation_depth
            != meerkat_authorization_contracts::constraints::DelegationDepth::remaining(0)
        {
            return Err(OriginalWriterError::UnsupportedMandate);
        }
        resolved
            .check_current()
            .map_err(|_| OriginalWriterError::Unavailable)
    }

    /// Bind one source-owned public-service set. Its callback must match every
    /// target and catalog lineage to the immutable reviewed source while this
    /// writer is held. The versioned caller supplies the exact configured setup
    /// targets separately from the public service targets.
    pub(crate) async fn bind_issued_service_set_async<F, Fut>(
        &self,
        association: InputAuthorityAssociation,
        source_custody: AuthenticatedOriginal,
        authenticated_requester: &PrincipalRef,
        authenticated_ingress_actor: &PrincipalRef,
        task_issuer: &PrincipalRef,
        selected_targets: ServiceSetBindingTargets<'_>,
        persist_or_observe: F,
    ) -> Result<Arc<RetainedToolkitOriginal>, OriginalWriterError>
    where
        F: FnOnce(InputAuthorityAssociation) -> Fut,
        Fut: std::future::Future<Output = Result<CanonicalBindingObservation, OriginalWriterError>>,
    {
        let _mutation = self.source_writer().await;
        self.check_source(
            &association,
            &source_custody,
            authenticated_requester,
            authenticated_ingress_actor,
        )?;
        self.refuse_cancelled_binding(&association)?;
        let claims = association.candidate();
        let WorkAuthorityBasis::TargetedGrantLineages {
            base: Some(_),
            targets,
        } = &claims.authority_basis
        else {
            return Err(OriginalWriterError::UnsupportedMandate);
        };
        selected_targets.check(
            &targets
                .iter()
                .map(|target| &target.target)
                .collect::<Vec<_>>(),
        )?;
        for lineage in claims.authority_basis.grant_lineages() {
            let resolved = self
                .grants
                .resolve_lineage(
                    lineage,
                    &claims.logical_executor,
                    claims.represented_subject.as_ref(),
                )
                .map_err(OriginalWriterError::Grant)?;
            if resolved.restrictions().delegation_depth
                != meerkat_authorization_contracts::constraints::DelegationDepth::remaining(0)
            {
                return Err(OriginalWriterError::UnsupportedMandate);
            }
            resolved
                .check_current()
                .map_err(|_| OriginalWriterError::Unavailable)?;
        }
        self.check_original_child(&association)?;
        let observation = persist_or_observe(association.clone()).await?;
        self.publish_binding(
            association,
            source_custody,
            OriginalGrantBinding::ServiceSet {
                issuer: task_issuer.clone(),
            },
            observation,
        )
    }

    /// Install only the exact candidate's final preparation. This does not
    /// publish semantic source state, so it cannot invalidate the stamp whose
    /// canonical reads it represents. A missing or stale guard refuses entry.
    pub(crate) fn prepare_service_set_admission(
        &self,
        original: &Arc<RetainedToolkitOriginal>,
        publication: meerkat_authorization::publication::LocalPublicationStamp,
        expires_at_ms: u64,
    ) -> Result<(), OriginalWriterError> {
        let mut originals = self
            .originals
            .try_lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let held = originals
            .iter_mut()
            .filter_map(OriginalRecord::bound_mut)
            .find(|held| Arc::ptr_eq(&held.retained, original))
            .ok_or(OriginalWriterError::WrongOwner)?;
        if !matches!(held.grant_binding, OriginalGrantBinding::ServiceSet { .. })
            || held.source_custody != Some(SourceCustody::Candidate)
            || held.operation != OriginalOperationDisposition::Open
            || held.service_set_admission.is_some()
        {
            return Err(OriginalWriterError::Conflict);
        }
        publication
            .check_current()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        held.service_set_admission = Some(Arc::new(ServiceSetAdmission {
            publication,
            expires_at_ms,
        }));
        Ok(())
    }

    /// Reconstruct an acknowledged original from the actual application's
    /// retained source record, including its exact writer-issued child binding.
    /// Public JSON, audit copies and old Toolkit sealed roots are not this
    /// caller. The source owner must verify that the child was reserved solely
    /// for this original; native lineage resolution does not infer that fact.
    /// Revoked, expired or foreign native references remain refusals.
    /// This restores only immutable source evidence. It does not make the original
    /// a contributor, restore an accepted native input or report replay success.
    /// Cold contributor custody requires exact native admission/replay settlement;
    /// a stored association alone can never produce it. The callback reads the
    /// actual durable cancellation state under this writer mutex; a stale
    /// pre-lock snapshot is not a valid callback implementation.
    pub fn restore_authenticated(
        &self,
        association: InputAuthorityAssociation,
        source_custody: AuthenticatedOriginal,
        authenticated_requester: &PrincipalRef,
        authenticated_ingress_actor: &PrincipalRef,
        read_operation_cancelled: impl FnOnce() -> Result<
            Option<Arc<toolkit_store::StoredRecord>>,
            OriginalWriterError,
        >,
    ) -> Result<Arc<RetainedToolkitOriginal>, OriginalWriterError> {
        self.check_source(
            &association,
            &source_custody,
            authenticated_requester,
            authenticated_ingress_actor,
        )?;
        let _mutation = self
            .mutation
            .try_lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        self.refuse_cancelled_binding(&association)?;
        self.check_original_child(&association)?;
        let cancelled_record = read_operation_cancelled()?;
        self.publish_binding(
            association,
            source_custody,
            OriginalGrantBinding::Task { issuer: None },
            cancelled_record.into(),
        )
    }

    fn check_source(
        &self,
        association: &InputAuthorityAssociation,
        source: &AuthenticatedOriginal,
        requester: &PrincipalRef,
        actor: &PrincipalRef,
    ) -> Result<(), OriginalWriterError> {
        let claims = association.candidate();
        if claims.ingress_namespace.ingress != self.ingress_domain
            || &claims.requester != requester
            || &claims.ingress_actor != actor
        {
            return Err(OriginalWriterError::WrongOwner);
        }
        if source.original_work != claims.original_work
            || source.occurrence_scope != claims.ingress_namespace.occurrence_scope
            || source.original_authentication != claims.original_authentication
            || source.root_event != claims.root_event
            || source.contributing_work != claims.contributing_work
            || source.target != claims.target
            || source.authority_basis != claims.authority_basis
        {
            return Err(OriginalWriterError::Conflict);
        }
        Ok(())
    }

    fn check_original_child(
        &self,
        association: &InputAuthorityAssociation,
    ) -> Result<(), OriginalWriterError> {
        let claims = association.candidate();
        let lineages = claims.authority_basis.grant_lineages().collect::<Vec<_>>();
        if lineages.is_empty() || lineages.iter().any(|lineage| lineage.len() < 2) {
            return Err(OriginalWriterError::UnsupportedMandate);
        }
        // Preserve the legacy single-lineage reconstruction check. Targeted
        // reconstruction checks currentness only when its operation is used.
        if let WorkAuthorityBasis::GrantLineage { lineage } = &claims.authority_basis {
            self.grants
                .resolve_lineage(
                    lineage,
                    &claims.logical_executor,
                    claims.represented_subject.as_ref(),
                )
                .map_err(OriginalWriterError::Grant)?;
        }
        let originals = self
            .originals
            .try_lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        Self::check_task_lineage_isolation(association, &originals)
    }

    fn check_task_lineage_isolation(
        association: &InputAuthorityAssociation,
        originals: &[OriginalRecord],
    ) -> Result<(), OriginalWriterError> {
        Self::check_selected_lineage_isolation(association, originals, None)
    }

    fn check_selected_lineage_isolation(
        association: &InputAuthorityAssociation,
        originals: &[OriginalRecord],
        replay: Option<&InputAuthorityAssociation>,
    ) -> Result<(), OriginalWriterError> {
        let claims = association.candidate();
        let lineages = claims.authority_basis.grant_lineages().collect::<Vec<_>>();
        if lineages.is_empty() || lineages.iter().any(|lineage| lineage.len() < 2) {
            return Err(OriginalWriterError::UnsupportedMandate);
        }
        // Every independently constrained target child is reserved to this
        // original. Structural replay does not resolve unrelated revocation.
        for (index, lineage) in lineages.iter().enumerate() {
            let child = lineage
                .last()
                .ok_or(OriginalWriterError::UnsupportedMandate)?;
            if claims.controller_grant_lineage.contains(child)
                || lineages
                    .iter()
                    .enumerate()
                    .any(|(other, path)| other != index && path.contains(child))
            {
                return Err(OriginalWriterError::UnsupportedMandate);
            }
        }
        for existing in originals.iter().filter_map(OriginalRecord::bound) {
            if existing.association == *association || replay == Some(&existing.association) {
                continue;
            }
            let old_claims = existing.association.candidate();
            for lineage in &lineages {
                let child = lineage
                    .last()
                    .ok_or(OriginalWriterError::UnsupportedMandate)?;
                if old_claims.controller_grant_lineage.contains(child)
                    || existing
                        .custody
                        .authority_basis
                        .grant_lineages()
                        .any(|old| old.contains(child))
                {
                    return Err(OriginalWriterError::Conflict);
                }
            }
            if matches!(
                existing.grant_binding,
                OriginalGrantBinding::Task { .. }
                    | OriginalGrantBinding::ServiceSet { .. }
                    | OriginalGrantBinding::NativeTargets
            ) {
                for old in Self::original_scoped_lineages(existing) {
                    if old.last().is_some_and(|child| {
                        claims.controller_grant_lineage.contains(child)
                            || lineages.iter().any(|lineage| lineage.contains(child))
                    }) {
                        return Err(OriginalWriterError::Conflict);
                    }
                }
            }
        }
        Ok(())
    }

    // Native target children belong to this original. Independent callback
    // children remain admission-owned even when carried by the same input.
    fn original_scoped_lineages(existing: &HeldOriginal) -> Vec<&[GrantLineageRef]> {
        if matches!(existing.grant_binding, OriginalGrantBinding::NativeTargets)
            && let WorkAuthorityBasis::TargetedGrantLineages { targets, .. } =
                &existing.custody.authority_basis
        {
            use meerkat_authorization_contracts::work_association::GrantTarget;
            return targets
                .iter()
                .filter(|row| {
                    matches!(
                        row.target,
                        GrantTarget::Peer { .. }
                            | GrantTarget::PeerDirectory
                            | GrantTarget::Memory { .. }
                    )
                })
                .map(|row| row.lineage.as_slice())
                .collect();
        }
        existing.custody.authority_basis.grant_lineages().collect()
    }

    fn require_absent(
        &self,
        association: &InputAuthorityAssociation,
    ) -> Result<(), OriginalWriterError> {
        self.refuse_cancelled_binding(association)?;
        if self.find_exact(association)?.is_some() {
            return Err(OriginalWriterError::Conflict);
        }
        Ok(())
    }

    fn refuse_cancelled_binding(
        &self,
        association: &InputAuthorityAssociation,
    ) -> Result<(), OriginalWriterError> {
        self.refuse_cancelled_binding_with(association, CollectionAccess::Try)
    }

    fn refuse_cancelled_binding_with(
        &self,
        association: &InputAuthorityAssociation,
        access: CollectionAccess,
    ) -> Result<(), OriginalWriterError> {
        let originals = self.source_collection(access).inspect_err(|_| {
            shared_entry_diagnostics::parent_refusal(if self.originals.is_poisoned() {
                shared_entry_diagnostics::ParentCause::OriginalsPoisoned
            } else {
                shared_entry_diagnostics::ParentCause::OriginalsWouldBlock
            });
        })?;
        Self::refuse_cancelled_binding_in(&originals, association)
    }

    /// [`Self::refuse_cancelled_binding`] where the caller must not wait:
    /// `Ok(false)` when the source collection is held by another owner's
    /// short section and nothing was checked.
    fn try_refuse_cancelled_binding(
        &self,
        association: &InputAuthorityAssociation,
    ) -> Result<bool, OriginalWriterError> {
        let originals = match self.originals.try_lock() {
            Ok(originals) => originals,
            Err(std::sync::TryLockError::WouldBlock) => return Ok(false),
            Err(std::sync::TryLockError::Poisoned(_)) => {
                shared_entry_diagnostics::parent_refusal(
                    shared_entry_diagnostics::ParentCause::OriginalsPoisoned,
                );
                return Err(OriginalWriterError::Unavailable);
            }
        };
        Self::refuse_cancelled_binding_in(&originals, association).map(|()| true)
    }

    fn refuse_cancelled_binding_in(
        originals: &[OriginalRecord],
        association: &InputAuthorityAssociation,
    ) -> Result<(), OriginalWriterError> {
        let c = association.candidate();
        if let Some(record) = originals
            .iter()
            .find(|entry| entry.names(&c.ingress_namespace.realm, &c.original_work))
        {
            let operation = match record {
                OriginalRecord::Bound(held) => &held.operation,
                OriginalRecord::UnboundCancellation {
                    operation,
                    first_binding,
                    ..
                } => {
                    if first_binding
                        .as_ref()
                        .is_some_and(|first| first != association)
                    {
                        shared_entry_diagnostics::parent_refusal(
                            shared_entry_diagnostics::ParentCause::FirstBindingConflict,
                        );
                        return Err(OriginalWriterError::Conflict);
                    }
                    operation
                }
            };
            return match operation {
                OriginalOperationDisposition::Open => Ok(()),
                OriginalOperationDisposition::CancellationPending => {
                    shared_entry_diagnostics::parent_refusal(
                        shared_entry_diagnostics::ParentCause::CancellationPending,
                    );
                    Err(OriginalWriterError::Unavailable)
                }
                OriginalOperationDisposition::Cancelled { .. } => {
                    shared_entry_diagnostics::parent_refusal(
                        shared_entry_diagnostics::ParentCause::Cancelled,
                    );
                    Err(OriginalWriterError::OperationCancelled)
                }
            };
        }
        Ok(())
    }

    fn find_exact(
        &self,
        association: &InputAuthorityAssociation,
    ) -> Result<Option<Arc<RetainedToolkitOriginal>>, OriginalWriterError> {
        let originals = self
            .originals
            .try_lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let claims = association.candidate();
        for existing in originals.iter().filter_map(OriginalRecord::bound) {
            let old = existing.association.candidate();
            if old.ingress_namespace.realm == claims.ingress_namespace.realm
                && old.ingress_namespace.ingress == claims.ingress_namespace.ingress
                && old.original_work == claims.original_work
            {
                return if existing.association == *association {
                    Ok(Some(existing.retained.clone()))
                } else {
                    Err(OriginalWriterError::Conflict)
                };
            }
        }
        Ok(None)
    }

    fn check_standing_lineage(
        &self,
        association: &InputAuthorityAssociation,
    ) -> Result<(), OriginalWriterError> {
        let claims = association.candidate();
        let WorkAuthorityBasis::GrantLineage { lineage } = &claims.authority_basis else {
            return Err(OriginalWriterError::UnsupportedMandate);
        };
        self.grants
            .resolve_lineage(
                lineage,
                &claims.logical_executor,
                claims.represented_subject.as_ref(),
            )
            .map_err(OriginalWriterError::Grant)?;
        let originals = self
            .originals
            .try_lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        for existing in originals.iter().filter_map(OriginalRecord::bound) {
            if existing.association == *association {
                continue;
            }
            if matches!(
                existing.grant_binding,
                OriginalGrantBinding::Task { .. }
                    | OriginalGrantBinding::ServiceSet { .. }
                    | OriginalGrantBinding::NativeTargets
            ) && Self::original_scoped_lineages(existing)
                .into_iter()
                .any(|old| {
                    old.last().is_some_and(|child| {
                        lineage.contains(child) || claims.controller_grant_lineage.contains(child)
                    })
                })
            {
                return Err(OriginalWriterError::Conflict);
            }
        }
        Ok(())
    }

    fn publish_task(
        &self,
        association: InputAuthorityAssociation,
        source_custody: AuthenticatedOriginal,
        issuer: PrincipalRef,
        cancelled_record: Option<Arc<toolkit_store::StoredRecord>>,
    ) -> Result<Arc<RetainedToolkitOriginal>, OriginalWriterError> {
        self.publish_binding(
            association,
            source_custody,
            OriginalGrantBinding::Task {
                issuer: Some(issuer),
            },
            cancelled_record.into(),
        )
    }

    /// Called under the existing source mutation writer immediately before the
    /// first canonical association CAS. The caller validated this exact unbound
    /// row; native's tracked foreground owns the entered asynchronous write.
    ///
    /// Inside its owner change the collection is only tried. With
    /// `contended_until`, a collection held by another owner's short section
    /// ends the change with nothing changed, is waited for outside any change
    /// until that deadline, and the change begins again
    /// ([`Self::change_with_collection_within`]); the in-memory record is
    /// applied at most once, and the CAS after it is the caller's. Without it
    /// a held collection is unavailable at once, as before.
    pub(crate) fn retain_first_binding_write(
        &self,
        store: &dyn toolkit_store::ToolkitStore,
        scope: &toolkit_store::StoreScope,
        expected_key: &toolkit_store::StoreKey,
        source_record: Arc<toolkit_store::StoredRecord>,
        association: &InputAuthorityAssociation,
        contended_until: Option<std::time::Instant>,
    ) -> Result<(), OriginalWriterError> {
        self.check_canonical_source(store, scope)?;
        // Each trusted caller derives its exact canonical key. Helper rows use
        // their own namespace within the selected application's source scope.
        if source_record.key.realm != scope.realm || source_record.key != *expected_key {
            return Err(OriginalWriterError::WrongOwner);
        }
        let claims = association.candidate();
        let retain = |originals: &mut Vec<OriginalRecord>| -> Result<(), OriginalWriterError> {
            if let Some(existing) = originals
                .iter()
                .find(|entry| entry.names(&claims.ingress_namespace.realm, &claims.original_work))
            {
                return match existing {
                    OriginalRecord::UnboundCancellation {
                        source_record: prior,
                        operation: OriginalOperationDisposition::Open,
                        first_binding: Some(first),
                        ..
                    } if first == association
                        && prior.key == source_record.key
                        && (crate::private_originals::same_source_origin(
                            prior,
                            &source_record,
                        ) || helper_originals::same_unbound_source(prior, &source_record)) =>
                    {
                        Ok(())
                    }
                    OriginalRecord::UnboundCancellation {
                        operation: OriginalOperationDisposition::Cancelled { .. },
                        ..
                    } => Err(OriginalWriterError::OperationCancelled),
                    _ => Err(OriginalWriterError::Unavailable),
                };
            }
            originals.push(OriginalRecord::UnboundCancellation {
                realm: claims.ingress_namespace.realm.clone(),
                original: claims.original_work.clone(),
                source_record: source_record.clone(),
                operation: OriginalOperationDisposition::Open,
                first_binding: Some(association.clone()),
                withdrawn_association: None,
            });
            Ok(())
        };
        match contended_until {
            Some(deadline) => {
                self.change_with_collection_within(deadline, std::thread::sleep, || {}, retain)
            }
            None => {
                let _publication = self
                    .publication
                    .begin_owner_change()
                    .map_err(|_| OriginalWriterError::Unavailable)?;
                let mut originals = self
                    .originals
                    .try_lock()
                    .map_err(|_| OriginalWriterError::Unavailable)?;
                retain(&mut originals)
            }
        }
    }

    /// Publish a validated, durable withdrawal in the existing source owner.
    /// Cold readback grants no controller or contributor custody and cannot
    /// reopen this first attempt. Cancellation remains an independent fact.
    pub(crate) fn retain_cold_withdrawal(
        &self,
        association: InputAuthorityAssociation,
        row: Arc<toolkit_store::StoredRecord>,
        cancelled: bool,
    ) -> Result<(), OriginalWriterError> {
        let claims = association.candidate();
        if claims.ingress_namespace.ingress != self.ingress_domain {
            return Err(OriginalWriterError::WrongOwner);
        }
        let _publication = self
            .publication
            .begin_owner_change()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let mut originals = self
            .originals
            .try_lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        if let Some(existing) = originals
            .iter_mut()
            .find(|entry| entry.names(&claims.ingress_namespace.realm, &claims.original_work))
        {
            match existing {
                OriginalRecord::Bound(held) => {
                    if held.association != association {
                        return Err(OriginalWriterError::Conflict);
                    }
                    held.source_custody = None;
                    held.custody_settlement = None;
                    if cancelled {
                        held.operation = OriginalOperationDisposition::Cancelled { record: row };
                    }
                }
                OriginalRecord::UnboundCancellation {
                    source_record,
                    first_binding,
                    withdrawn_association,
                    operation,
                    ..
                } => {
                    if source_record.key != row.key
                        || first_binding.is_some()
                        || withdrawn_association
                            .as_ref()
                            .is_some_and(|prior| prior != &association)
                    {
                        return Err(OriginalWriterError::Conflict);
                    }
                    if cancelled {
                        *operation = OriginalOperationDisposition::Cancelled {
                            record: row.clone(),
                        };
                    }
                    *source_record = row;
                    *withdrawn_association = Some(association);
                }
            }
        } else {
            originals.push(OriginalRecord::UnboundCancellation {
                realm: claims.ingress_namespace.realm.clone(),
                original: claims.original_work.clone(),
                operation: if cancelled {
                    OriginalOperationDisposition::Cancelled {
                        record: row.clone(),
                    }
                } else {
                    OriginalOperationDisposition::Open
                },
                source_record: row,
                first_binding: None,
                withdrawn_association: Some(association),
            });
        }
        Ok(())
    }

    fn publish_binding(
        &self,
        association: InputAuthorityAssociation,
        source_custody: AuthenticatedOriginal,
        grant_binding: OriginalGrantBinding,
        observation: CanonicalBindingObservation,
    ) -> Result<Arc<RetainedToolkitOriginal>, OriginalWriterError> {
        let observed = Self::binding_observation(&association, observation)?;
        // The existing writer mutex spans durable observation and this write.
        // Native publication is acquired only after storage I/O has finished.
        let _publication = self
            .publication
            .begin_owner_change()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let mut originals = self
            .originals
            .try_lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        Self::publish_into(
            &mut originals,
            association,
            source_custody,
            grant_binding,
            observed,
        )
    }

    /// [`Self::publish_binding`] for a caller that holds no owner change of
    /// the shared publication and may wait until `deadline`: a source
    /// collection held by another owner's short section ends the change with
    /// no fact changed, is waited for outside any change, and the change
    /// begins again ([`Self::change_with_collection_within`]). Past
    /// `deadline` it is unavailable, as before. The publication is made at
    /// most once. The collection's holders never wait for a lock the caller
    /// holds: the source writer and the publication writer are always taken
    /// before the collection, never under it.
    fn publish_binding_within(
        &self,
        association: InputAuthorityAssociation,
        source_custody: AuthenticatedOriginal,
        grant_binding: OriginalGrantBinding,
        observation: CanonicalBindingObservation,
        deadline: std::time::Instant,
    ) -> Result<Arc<RetainedToolkitOriginal>, OriginalWriterError> {
        let observed = Self::binding_observation(&association, observation)?;
        let mut publication = Some((association, source_custody, grant_binding, observed));
        self.change_with_collection_within(
            deadline,
            std::thread::sleep,
            || {},
            |originals| {
                let (association, source_custody, grant_binding, observed) =
                    publication.take().ok_or(OriginalWriterError::Unavailable)?;
                Self::publish_into(
                    originals,
                    association,
                    source_custody,
                    grant_binding,
                    observed,
                )
            },
        )
    }

    /// A canonical binding observation's cancellation record and historical
    /// custody decision, validated against `association`.
    fn binding_observation(
        association: &InputAuthorityAssociation,
        observation: CanonicalBindingObservation,
    ) -> Result<BindingObserved, OriginalWriterError> {
        Ok(match observation {
            CanonicalBindingObservation::Unretained => (None, None),
            CanonicalBindingObservation::Cancelled(record) => (Some(record), None),
            CanonicalBindingObservation::Historical {
                association: observed,
                attempt,
            } => {
                if &observed != association {
                    return Err(OriginalWriterError::Conflict);
                }
                attempt.validate_binding(association)?;
                (None, Some(attempt.decision()))
            }
        })
    }

    /// Publish a binding into the owner's held collection, inside an open
    /// owner change of the shared publication. Never waits.
    fn publish_into(
        originals: &mut Vec<OriginalRecord>,
        association: InputAuthorityAssociation,
        source_custody: AuthenticatedOriginal,
        grant_binding: OriginalGrantBinding,
        (cancelled_record, historical_decision): BindingObserved,
    ) -> Result<Arc<RetainedToolkitOriginal>, OriginalWriterError> {
        let claims = association.candidate();
        let mut first_binding = None;
        let mut first_index = None;
        if let Some(index) = originals
            .iter()
            .position(|entry| entry.names(&claims.ingress_namespace.realm, &claims.original_work))
            && let OriginalRecord::UnboundCancellation {
                source_record,
                operation,
                first_binding: prepared,
                ..
            } = &originals[index]
        {
            match operation {
                OriginalOperationDisposition::Open if prepared.as_ref() == Some(&association) => {
                    first_binding = Some(source_record.clone());
                    first_index = Some(index);
                }
                OriginalOperationDisposition::Cancelled { .. } => {
                    return Err(OriginalWriterError::OperationCancelled);
                }
                _ => return Err(OriginalWriterError::Unavailable),
            }
        }
        if let Some(existing) = originals
            .iter_mut()
            .filter_map(OriginalRecord::bound_mut)
            .find(|held| {
                held.association.candidate().ingress_namespace.realm
                    == claims.ingress_namespace.realm
                    && held.association.candidate().original_work == claims.original_work
            })
        {
            if existing.association != association {
                return Err(OriginalWriterError::Conflict);
            }
            if existing.grant_binding != grant_binding
                && !(matches!(grant_binding, OriginalGrantBinding::Task { issuer: None })
                    && matches!(
                        existing.grant_binding,
                        OriginalGrantBinding::Task { .. } | OriginalGrantBinding::ServiceSet { .. }
                    ))
            {
                return Err(OriginalWriterError::Conflict);
            }
            // A binder's read-Open result cannot reconcile an in-flight or
            // indeterminate cancellation. Only that cancellation may settle it.
            if existing.operation == OriginalOperationDisposition::CancellationPending {
                return Err(OriginalWriterError::Unavailable);
            }
            if let Some(record) = cancelled_record {
                existing.operation = OriginalOperationDisposition::Cancelled { record };
            }
            return if matches!(
                existing.operation,
                OriginalOperationDisposition::Cancelled { .. }
            ) {
                Err(OriginalWriterError::OperationCancelled)
            } else {
                // A failed/stopped census publication cannot keep exact terminal
                // withdrawal unknown after this serialized canonical readback.
                // Other warm custody, including explicit release, is untouched.
                if existing.source_custody == Some(SourceCustody::RestorationUnknown)
                    && historical_decision == Some(custody_attempt::CustodyDecision::Withdrawn)
                {
                    existing.source_custody = None;
                    existing.custody_settlement = None;
                    existing.controller = None;
                }
                Ok(existing.retained.clone())
            };
        }
        // Preserve actual first-write custody and every existing Bound state.
        // Only a newly reconstructed, canonically enrolled historical binding
        // is unknown until native-backed restoration publishes admission.
        let restored_custody = (matches!(
            historical_decision,
            Some(
                custody_attempt::CustodyDecision::Pending
                    | custody_attempt::CustodyDecision::Promoted
            )
        ) && first_binding.is_none())
        .then_some(SourceCustody::RestorationUnknown);
        let retained = Arc::new(RetainedToolkitOriginal {
            association,
            custody: source_custody,
            grant_binding,
            first_binding,
        });
        let operation_cancelled = cancelled_record.is_some();
        let bound = OriginalRecord::Bound(HeldOriginal {
            retained: retained.clone(),
            source_custody: restored_custody,
            custody_settlement: None,
            controller: None,
            service_set_admission: None,
            operation: match cancelled_record {
                Some(record) => OriginalOperationDisposition::Cancelled { record },
                None => OriginalOperationDisposition::Open,
            },
        });
        match first_index {
            Some(index) => originals[index] = bound,
            None => originals.push(bound),
        }
        if operation_cancelled {
            Err(OriginalWriterError::OperationCancelled)
        } else {
            Ok(retained)
        }
    }

    /// Validate this exact canonical source, then fence it before an awaited
    /// cancellation write can become indeterminate. No publication guard spans
    /// I/O. Error or dropped future leaves Pending, without erasing immutable
    /// association/controller/admission facts. Only exact cancellation
    /// persistence/reconciliation can publish Cancelled with its actual stored
    /// row evidence. The native disposition callback projects that exact row
    /// under this source owner without store I/O.
    pub(crate) async fn record_operation_cancelled<V, VFut, F, Fut>(
        &self,
        realm: &meerkat_core::RealmId,
        original: &OriginalWorkRef,
        validate_source: V,
        persist: F,
    ) -> Result<(), OriginalWriterError>
    where
        V: FnOnce(
            Option<InputAuthorityAssociation>,
            Option<Arc<toolkit_store::StoredRecord>>,
        ) -> VFut,
        VFut: std::future::Future<
                Output = Result<Arc<toolkit_store::StoredRecord>, OriginalWriterError>,
            >,
        F: FnOnce(Arc<toolkit_store::StoredRecord>) -> Fut,
        Fut: std::future::Future<
                Output = Result<Arc<toolkit_store::StoredRecord>, OriginalWriterError>,
            >,
    {
        let _mutation = self.bounded_source_writer().await?;
        // Compare the selected event with this canonical original before any
        // publication changes. The writer mutex keeps these immutable facts
        // coupled to the later Pending and durable cancellation writes.
        let (binding, prior_source) = {
            let originals = self.wait_for_collection()?;
            match originals.iter().find(|entry| entry.names(realm, original)) {
                Some(OriginalRecord::Bound(held)) => (
                    Some(held.association.clone()),
                    match &held.operation {
                        OriginalOperationDisposition::Cancelled { record } => Some(record.clone()),
                        _ => None,
                    },
                ),
                Some(OriginalRecord::UnboundCancellation { source_record, .. }) => {
                    (None, Some(source_record.clone()))
                }
                None => (None, None),
            }
        };
        let source_record = validate_source(binding.clone(), prior_source).await?;
        let associations = binding.into_iter().collect::<Vec<_>>();
        let entries = self.change_with_collection(|originals| {
            if let Some(record) = originals
                .iter_mut()
                .find(|entry| entry.names(realm, original))
            {
                if !matches!(
                    record.operation_mut(),
                    OriginalOperationDisposition::Cancelled { .. }
                ) {
                    *record.operation_mut() = OriginalOperationDisposition::CancellationPending;
                }
            } else {
                originals.push(OriginalRecord::UnboundCancellation {
                    realm: realm.clone(),
                    original: original.clone(),
                    source_record: source_record.clone(),
                    operation: OriginalOperationDisposition::CancellationPending,
                    first_binding: None,
                    withdrawn_association: None,
                });
            }
            self.close_shared_entries(&associations)
        })?;
        let cancellation_record = persist(source_record).await?;
        self.change_with_collection(|originals| {
            let record = originals
                .iter_mut()
                .find(|entry| entry.names(realm, original))
                .ok_or(OriginalWriterError::Unavailable)?;
            *record.operation_mut() = OriginalOperationDisposition::Cancelled {
                record: cancellation_record.clone(),
            };
            Ok(())
        })?;
        drop(_mutation);
        Self::drain_shared_entries(entries).await
    }

    /// Serialize the existing canonical selection of one still-unbound source
    /// with this owner's first binding and cancellation. The ordinary async
    /// caller waits on the existing source writer; native callbacks do not.
    /// No human interaction runs under this custody, and the callback must not
    /// enter this owner again.
    /// Absence is not permission: the callback still validates the retained
    /// decision, current native parent and exact source row before its CAS.
    pub(crate) async fn with_unbound_source_selection<T, F, Fut>(
        &self,
        realm: &meerkat_core::RealmId,
        original: &OriginalWorkRef,
        select: F,
    ) -> Result<T, OriginalWriterError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = T>,
    {
        let _mutation = self.source_writer().await;
        {
            let originals = self
                .originals
                .try_lock()
                .map_err(|_| OriginalWriterError::Unavailable)?;
            if let Some(record) = originals
                .iter()
                .find(|record| record.names(realm, original))
            {
                return Err(match record {
                    OriginalRecord::Bound(_) => OriginalWriterError::Conflict,
                    OriginalRecord::UnboundCancellation { operation, .. } => match operation {
                        OriginalOperationDisposition::Cancelled { .. } => {
                            OriginalWriterError::OperationCancelled
                        }
                        _ => OriginalWriterError::Unavailable,
                    },
                });
            }
        }
        Ok(select().await)
    }

    /// Serialize creation of a follow-up source with cancellation of its exact
    /// admitted parent. The callback still owns durable conditional publication.
    /// This check does not reuse the parent's operation grant for the new input.
    /// Ordinary async callers wait before rechecking the parent. Callers must
    /// not already hold this writer, and callbacks must not reacquire it.
    /// No synchronous native-owner callback may enter this waiting boundary.
    pub(crate) async fn retain_followup_source<T, F, Fut>(
        &self,
        parent: &InputAuthorityAssociation,
        publish: F,
    ) -> Result<T, OriginalWriterError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T, OriginalWriterError>>,
    {
        shared_entry_diagnostics::begin_followup(parent);
        let _mutation = self.source_writer().await;
        // Under the source writer and outside any owner change: a short hold
        // of the source collection is waited for, within the bound.
        shared_entry_diagnostics::check_parent(|| {
            self.refuse_cancelled_binding_with(parent, CollectionAccess::Wait)
        })
        .inspect_err(|error| {
            eprintln!(
                r#"{{"event":"toolkit_shared_dispatch_refused","stage":"followup.parent_current"}}"#
            );
            shared_entry_diagnostics::emit(
                "followup.parent_current",
                Some(shared_entry_diagnostics::writer_kind(*error)),
                None,
            );
        })?;
        {
            let originals = self.wait_for_collection().inspect_err(|_| {
                eprintln!(r#"{{"event":"toolkit_shared_dispatch_refused","stage":"followup.originals_lock","kind":"unavailable"}}"#);
            })?;
            if !originals
                .iter()
                .filter_map(OriginalRecord::bound)
                .any(|held| {
                    held.association == *parent
                        && held.source_custody == Some(SourceCustody::Admitted)
                })
            {
                eprintln!(
                    r#"{{"event":"toolkit_shared_dispatch_refused","stage":"followup.admitted_parent","kind":"unavailable"}}"#
                );
                return Err(OriginalWriterError::Unavailable);
            }
        }
        shared_entry_diagnostics::publish_entered();
        publish().await.inspect_err(|_| {
            eprintln!(
                r#"{{"event":"toolkit_shared_dispatch_refused","stage":"followup.publish"}}"#
            );
        })
    }

    /// Failure reconciliation shares the source-publication writer. The exact
    /// admitted parent's identity must still be retained, but its cancellation
    /// or terminality must not prevent cleanup of unpublished service grants.
    /// This callback cannot admit a new input or reopen the parent operation.
    pub(crate) async fn reconcile_followup_source<T, F, Fut>(
        &self,
        parent: &InputAuthorityAssociation,
        reconcile: F,
    ) -> Result<T, OriginalWriterError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = T>,
    {
        let _mutation = self
            .mutation
            .try_lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        {
            let originals = self
                .originals
                .try_lock()
                .map_err(|_| OriginalWriterError::Unavailable)?;
            if !originals
                .iter()
                .filter_map(OriginalRecord::bound)
                .any(|held| {
                    held.association == *parent
                        && held.source_custody == Some(SourceCustody::Admitted)
                })
            {
                return Err(OriginalWriterError::Unavailable);
            }
        }
        Ok(reconcile().await)
    }

    /// Reconcile physical cleanup only for this owner's exact completed
    /// cancellation. A cold tombstone remains a tombstone: no admitted original,
    /// controller custody or open operation is constructed by this callback.
    pub(crate) async fn reconcile_cancelled_source<T, F, Fut>(
        &self,
        source: &OriginalCancellationSource,
        reconcile: F,
    ) -> Result<T, OriginalWriterError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = T>,
    {
        let _mutation = self
            .mutation
            .try_lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        {
            let originals = self
                .originals
                .try_lock()
                .map_err(|_| OriginalWriterError::Unavailable)?;
            let selected = originals
                .iter()
                .find(|entry| entry.names(&source.realm, &source.original))
                .ok_or(OriginalWriterError::Unavailable)?;
            let operation = match selected {
                OriginalRecord::Bound(held) => {
                    if source.association.as_ref() != Some(&held.association) {
                        return Err(OriginalWriterError::Conflict);
                    }
                    &held.operation
                }
                OriginalRecord::UnboundCancellation {
                    source_record,
                    operation,
                    ..
                } => {
                    if !crate::private_originals::same_source_origin(source_record, &source.record)
                    {
                        return Err(OriginalWriterError::Conflict);
                    }
                    operation
                }
            };
            if !matches!(operation, OriginalOperationDisposition::Cancelled { record }
                if record.as_ref() == source.record.as_ref())
            {
                return Err(OriginalWriterError::Unavailable);
            }
        }
        Ok(reconcile().await)
    }

    /// Fence every exact linked source under the existing publication before
    /// the first durable write. An interrupted write leaves all members pending.
    /// Recovery must reopen their canonical parent link before binding again.
    pub(crate) async fn record_linked_operations_cancelled<V, VFut, F, Fut>(
        &self,
        validate_sources: V,
        mut persist: F,
    ) -> Result<Vec<Arc<toolkit_store::StoredRecord>>, OriginalWriterError>
    where
        V: FnOnce() -> VFut,
        VFut: std::future::Future<
                Output = Result<Vec<OriginalCancellationSource>, OriginalWriterError>,
            >,
        F: FnMut(Arc<toolkit_store::StoredRecord>) -> Fut,
        Fut: std::future::Future<
                Output = Result<Arc<toolkit_store::StoredRecord>, OriginalWriterError>,
            >,
    {
        let _mutation = self.bounded_source_writer().await?;
        let sources = validate_sources().await?;
        if sources.is_empty() {
            return Err(OriginalWriterError::WrongOwner);
        }
        let settled;
        {
            let originals = self.wait_for_collection()?;
            let mut all_settled = true;
            // Validate the entire group before any publication. The mutation
            // lock keeps group membership and immutable bindings coupled to
            // the later repair. A durable flag alone is insufficient.
            for (index, source) in sources.iter().enumerate() {
                if sources[..index]
                    .iter()
                    .any(|prior| prior.realm == source.realm && prior.original == source.original)
                {
                    return Err(OriginalWriterError::Conflict);
                }
                let operation = match originals
                    .iter()
                    .find(|entry| entry.names(&source.realm, &source.original))
                {
                    Some(OriginalRecord::Bound(held)) => {
                        if source.association.as_ref() != Some(&held.association) {
                            return Err(OriginalWriterError::Conflict);
                        }
                        Some(&held.operation)
                    }
                    Some(OriginalRecord::UnboundCancellation {
                        source_record,
                        operation,
                        ..
                    }) => {
                        if !crate::private_originals::same_source_origin(
                            source_record,
                            &source.record,
                        ) {
                            return Err(OriginalWriterError::Conflict);
                        }
                        Some(operation)
                    }
                    None => None,
                };
                all_settled &= matches!(operation,
                    Some(OriginalOperationDisposition::Cancelled { record })
                        if record.as_ref() == source.record.as_ref());
            }
            // Same exact cancellation evidence and same settled owner facts:
            // there is no change to invalidate unrelated authorization reads.
            settled = all_settled;
        }
        let associations = sources
            .iter()
            .filter_map(|source| source.association.clone())
            .collect::<Vec<_>>();
        if settled {
            let entries = self.close_shared_entries(&associations)?;
            drop(_mutation);
            Self::drain_shared_entries(entries).await?;
            return Ok(sources.into_iter().map(|source| source.record).collect());
        }
        let entries = self.change_with_collection(|originals| {
            for source in &sources {
                if let Some(record) = originals
                    .iter_mut()
                    .find(|entry| entry.names(&source.realm, &source.original))
                {
                    if !matches!(
                        record.operation_mut(),
                        OriginalOperationDisposition::Cancelled { .. }
                    ) {
                        *record.operation_mut() = OriginalOperationDisposition::CancellationPending;
                    }
                } else {
                    originals.push(OriginalRecord::UnboundCancellation {
                        realm: source.realm.clone(),
                        original: source.original.clone(),
                        source_record: source.record.clone(),
                        operation: OriginalOperationDisposition::CancellationPending,
                        first_binding: None,
                        withdrawn_association: None,
                    });
                }
            }
            // Close the same final-entry gates at validated Pending, before
            // any durable cancellation await. A lost write never reopens them.
            self.close_shared_entries(&associations)
        })?;
        // The canonical parent is first. This ordering makes a partially
        // persisted cancellation visible to every linked source on cold bind.
        let mut committed = Vec::with_capacity(sources.len());
        for source in &sources {
            committed.push(persist(source.record.clone()).await?);
        }
        self.change_with_collection(|originals| {
            for (source, record) in sources.iter().zip(&committed) {
                let current = originals
                    .iter_mut()
                    .find(|entry| entry.names(&source.realm, &source.original))
                    .ok_or(OriginalWriterError::Unavailable)?;
                *current.operation_mut() = OriginalOperationDisposition::Cancelled {
                    record: record.clone(),
                };
            }
            Ok(())
        })?;
        drop(_mutation);
        Self::drain_shared_entries(entries).await?;
        Ok(committed)
    }

    /// Bind the final input/controller and hold its candidate until the native
    /// submission owner settles the paired guard. The actual native door owns
    /// admission, replay, attachment and completion. It must not drop this guard
    /// at binder return or promote it merely because a ticket was allocated.
    pub fn bind_authenticated_input(
        self: &Arc<Self>,
        original: &Arc<RetainedToolkitOriginal>,
        selected_session: &meerkat_core::SessionId,
        input: Input,
        event_key: IdempotencyKey,
        transport_authentication: HistoricalEvidenceRef,
        controller: ControllerModelClient,
    ) -> Result<(Input, PendingToolkitOriginal), RuntimeDriverError> {
        self.prepare_authenticated_input(
            original,
            selected_session,
            input,
            event_key,
            transport_authentication,
            controller,
        )?
        .stage()
    }

    /// Prepare native binding without making this source a Candidate. The
    /// caller retains this value under its existing foreground operation and
    /// may persist exact evidence before consuming it through `stage`.
    pub(crate) fn prepare_authenticated_input(
        self: &Arc<Self>,
        original: &Arc<RetainedToolkitOriginal>,
        selected_session: &meerkat_core::SessionId,
        input: Input,
        event_key: IdempotencyKey,
        transport_authentication: HistoricalEvidenceRef,
        controller: ControllerModelClient,
    ) -> Result<PreparedToolkitOriginal, RuntimeDriverError> {
        self.try_prepare_authenticated_input(
            original,
            selected_session,
            input,
            event_key,
            transport_authentication,
            controller,
        )?
        .ok_or_else(|| RuntimeDriverError::ValidationFailed {
            reason: "Toolkit original is not retained by this source owner".into(),
        })
    }

    /// [`Self::prepare_authenticated_input`] for a native binder inside
    /// Core's governed bind, which must not wait: `Ok(None)` when the source
    /// collection is held by another owner's short section and nothing was
    /// prepared.
    pub(crate) fn try_prepare_authenticated_input(
        self: &Arc<Self>,
        original: &Arc<RetainedToolkitOriginal>,
        selected_session: &meerkat_core::SessionId,
        input: Input,
        event_key: IdempotencyKey,
        transport_authentication: HistoricalEvidenceRef,
        controller: ControllerModelClient,
    ) -> Result<Option<PreparedToolkitOriginal>, RuntimeDriverError> {
        let retained = self.try_require_retained(original).map_err(|_| {
            RuntimeDriverError::ValidationFailed {
                reason: "Toolkit original is not retained by this source owner".into(),
            }
        })?;
        if !retained {
            return Ok(None);
        }
        let claims = original.association.candidate();
        if claims.target.logical_runtime.as_str()
            != meerkat_runtime::LogicalRuntimeId::for_session(selected_session).to_string()
        {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "Toolkit original targets another native session".into(),
            });
        }
        let bound = bind_scoped_original(
            input,
            original.association.clone(),
            event_key,
            claims.requester.clone(),
            claims.ingress_actor.clone(),
            claims.ingress_namespace.realm.clone(),
            transport_authentication,
            controller.clone(),
        )?;
        Ok(Some(PreparedToolkitOriginal {
            owner: self.clone(),
            original: original.clone(),
            input: bound,
            controller,
        }))
    }

    #[cfg(test)]
    pub(crate) fn record_candidate(
        self: &Arc<Self>,
        original: &Arc<RetainedToolkitOriginal>,
    ) -> Result<PendingToolkitOriginal, OriginalWriterError> {
        self.record_candidate_with_controller(original, None)
    }

    fn admitted_guard(
        self: &Arc<Self>,
        original: &Arc<RetainedToolkitOriginal>,
    ) -> Result<Option<PendingToolkitOriginal>, OriginalWriterError> {
        // An admitted source needs no new candidate or controller mutation.
        // Observe its real custody without invalidating unrelated preparations.
        // The read guard ends inside the closure; any slow-path write still
        // takes the publication writer before the collection and rechecks facts.
        let mut collection_unavailable = false;
        let admitted = self.publication.observe(|| {
            let originals = match self.originals.lock() {
                Ok(originals) => originals,
                Err(_) => {
                    collection_unavailable = true;
                    return false;
                }
            };
            originals
                .iter()
                .filter_map(OriginalRecord::bound)
                .any(|held| {
                    Arc::ptr_eq(&held.retained, original)
                        && held.operation == OriginalOperationDisposition::Open
                        && held.source_custody == Some(SourceCustody::Admitted)
                })
        });
        if collection_unavailable {
            return Err(OriginalWriterError::Unavailable);
        }
        match admitted {
            Ok((true, _)) => {
                return Ok(Some(PendingToolkitOriginal {
                    owner: self.clone(),
                    original: original.clone(),
                    pending: false,
                }));
            }
            Ok((false, _)) => {}
            Err(PublicationError::Changed) => {
                // Preserve the old slow-path Admitted recheck. A changing
                // observation is not proof that this is a first input.
                let _publication = self
                    .publication
                    .begin_owner_change()
                    .map_err(|_| OriginalWriterError::Unavailable)?;
                let originals = self
                    .originals
                    .lock()
                    .map_err(|_| OriginalWriterError::Unavailable)?;
                let held = originals
                    .iter()
                    .filter_map(OriginalRecord::bound)
                    .find(|held| Arc::ptr_eq(&held.retained, original))
                    .ok_or(OriginalWriterError::WrongOwner)?;
                match held.operation {
                    OriginalOperationDisposition::Open => {}
                    OriginalOperationDisposition::Cancelled { .. } => {
                        return Err(OriginalWriterError::OperationCancelled);
                    }
                    _ => return Err(OriginalWriterError::Unavailable),
                }
                if held.source_custody == Some(SourceCustody::Admitted) {
                    return Ok(Some(PendingToolkitOriginal {
                        owner: self.clone(),
                        original: original.clone(),
                        pending: false,
                    }));
                }
            }
            Err(PublicationError::Unavailable) => return Err(OriginalWriterError::Unavailable),
        }
        Ok(None)
    }

    fn record_candidate_with_controller(
        self: &Arc<Self>,
        original: &Arc<RetainedToolkitOriginal>,
        controller: Option<ControllerModelClient>,
    ) -> Result<PendingToolkitOriginal, OriginalWriterError> {
        if let Some(pending) = self.admitted_guard(original)? {
            return Ok(pending);
        }
        let _publication = self
            .publication
            .begin_owner_change()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let mut originals = self
            .originals
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let index = originals
            .iter()
            .position(|entry| {
                entry
                    .bound()
                    .is_some_and(|held| Arc::ptr_eq(&held.retained, original))
            })
            .ok_or(OriginalWriterError::WrongOwner)?;
        let held = originals[index]
            .bound()
            .ok_or(OriginalWriterError::WrongOwner)?;
        match &held.operation {
            OriginalOperationDisposition::CancellationPending => {
                return Err(OriginalWriterError::Unavailable);
            }
            OriginalOperationDisposition::Cancelled { .. } => {
                return Err(OriginalWriterError::OperationCancelled);
            }
            OriginalOperationDisposition::Open => {}
        }
        match held.source_custody {
            Some(SourceCustody::Admitted) => {
                return Ok(PendingToolkitOriginal {
                    owner: self.clone(),
                    original: original.clone(),
                    pending: false,
                });
            }
            Some(SourceCustody::Candidate) => return Err(OriginalWriterError::Conflict),
            Some(SourceCustody::RestorationUnknown) => {
                return Err(OriginalWriterError::Unavailable);
            }
            None => {}
        }
        let claims = original.association.candidate();
        let namespace = &claims.ingress_namespace;
        // The native source seam also queries contributors with this incoming
        // occurrence. A newly staged candidate must never name itself or any
        // other unadmitted candidate as a contributor. Native checks continue
        // resolving these contributors and their current grants afterwards.
        if claims.contributing_work.iter().any(|contributor| {
            !originals
                .iter()
                .filter_map(OriginalRecord::bound)
                .any(|entry| {
                    entry.source_custody == Some(SourceCustody::Admitted)
                        && entry.names(namespace, contributor)
                        && entry.association.candidate().requester == claims.requester
                })
        }) {
            return Err(OriginalWriterError::Conflict);
        }
        // Scoped original lookup passes the incoming occurrence for contributor
        // checks too. At most one candidate may authenticate that occurrence;
        // a candidate can therefore never stand in for another contributor.
        if originals
            .iter()
            .filter_map(OriginalRecord::bound)
            .any(|entry| {
                entry.source_custody == Some(SourceCustody::Candidate)
                    && entry.association.candidate().ingress_namespace == *namespace
            })
        {
            return Err(OriginalWriterError::Conflict);
        }
        let held = originals[index]
            .bound_mut()
            .ok_or(OriginalWriterError::WrongOwner)?;
        held.source_custody = Some(SourceCustody::Candidate);
        held.custody_settlement = None;
        held.controller = controller;
        held.service_set_admission = None;
        Ok(PendingToolkitOriginal {
            owner: self.clone(),
            original: original.clone(),
            pending: true,
        })
    }

    /// Release only current admitted source custody under the shared publication.
    /// Durable evidence and native grants remain intact. Reacquiring source
    /// custody requires exact native admission observation or submission
    /// settlement. Neither observation nor custody grants replay permission.
    pub fn release_original(
        &self,
        original: &Arc<RetainedToolkitOriginal>,
    ) -> Result<(), OriginalWriterError> {
        let _publication = self
            .publication
            .begin_owner_change()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let mut originals = self
            .originals
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = originals
            .iter_mut()
            .filter_map(OriginalRecord::bound_mut)
            .find(|entry| Arc::ptr_eq(&entry.retained, original))
            .ok_or(OriginalWriterError::WrongOwner)?;
        if entry.source_custody == Some(SourceCustody::Candidate) {
            return Err(OriginalWriterError::Conflict);
        }
        entry.source_custody = None;
        entry.controller = None;
        Ok(())
    }

    /// Revoke the exact original-specific child, never the base/account grant.
    /// This primitive does not publish the durable operation-cancelled
    /// disposition; use `ToolkitGatewayHost::cancel_original_operation` for that.
    /// Retaining evidence after revocation does not retain permission: native
    /// ingress/operation checks resolve current grants. No cancellation flag is
    /// written. Actual attached controller custody remains mandatory. Successful
    /// revocation is not run/descendant cancellation or settlement; the native
    /// run/turn owner supplies those separate outcomes.
    pub fn cancel_original(
        &self,
        original: &Arc<RetainedToolkitOriginal>,
        authenticated_caller: &PrincipalRef,
        custody: &mut impl ControllerGrantMutationCustody,
    ) -> Result<(), OriginalWriterError> {
        self.require_retained(original)?;
        if !matches!(&original.grant_binding, OriginalGrantBinding::Task { issuer: Some(issuer) } if issuer == authenticated_caller)
        {
            // Standing permission and a decoded historical association are not
            // an original-specific revocation mandate. Durable original
            // cancellation remains a separate source-owner operation.
            return Err(OriginalWriterError::UnsupportedMandate);
        }
        let WorkAuthorityBasis::GrantLineage { lineage } =
            &original.association.candidate().authority_basis
        else {
            return Err(OriginalWriterError::UnsupportedMandate);
        };
        if lineage.len() < 2 {
            return Err(OriginalWriterError::UnsupportedMandate);
        }
        let child = lineage
            .last()
            .ok_or(OriginalWriterError::UnsupportedMandate)?;
        self.grants
            .revoke(authenticated_caller, child, custody)
            .map_err(OriginalWriterError::Grant)
    }

    fn require_retained(
        &self,
        original: &Arc<RetainedToolkitOriginal>,
    ) -> Result<(), OriginalWriterError> {
        if self.try_require_retained(original)? {
            Ok(())
        } else {
            Err(OriginalWriterError::Unavailable)
        }
    }

    /// [`Self::require_retained`] where the caller must not wait: `Ok(false)`
    /// when the source collection is held by another owner's short section
    /// and nothing was checked.
    fn try_require_retained(
        &self,
        original: &Arc<RetainedToolkitOriginal>,
    ) -> Result<bool, OriginalWriterError> {
        let originals = match self.originals.try_lock() {
            Ok(originals) => originals,
            Err(std::sync::TryLockError::WouldBlock) => return Ok(false),
            Err(std::sync::TryLockError::Poisoned(_)) => {
                return Err(OriginalWriterError::Unavailable);
            }
        };
        if originals
            .iter()
            .filter_map(OriginalRecord::bound)
            .any(|current| Arc::ptr_eq(&current.retained, original))
        {
            Ok(true)
        } else {
            Err(OriginalWriterError::WrongOwner)
        }
    }
}

// Source identity is realm + ingress domain + original, with its immutable
// occurrence retained in the record. Only admitted custody can answer for an
// earlier occurrence as a contributor. Candidates authenticate their own
// occurrence only and are never retained contributor custody.
impl AuthenticatedOriginalSource for ToolkitOriginalOwner {
    fn enrolled_service_original(
        &self,
        namespace: &QualifiedIngressNamespace,
        original: &OriginalWorkRef,
    ) -> Result<
        Option<meerkat_runtime::meerkat_machine::EnrolledServiceOriginal>,
        ControllerReadinessFailure,
    > {
        self.current_collector_original(namespace, original)
            .map(|record| {
                record.map(meerkat_runtime::meerkat_machine::EnrolledServiceOriginal::new)
            })
    }

    /// Currentness for the canonical original, independent of admission
    /// promotion. Authentication, native membership and grants remain separate
    /// native checks. This reads only the existing source-owner collection;
    /// storage is observed by the serialized writer before publication.
    fn original_disposition(
        &self,
        namespace: &QualifiedIngressNamespace,
        original: &OriginalWorkRef,
    ) -> Result<OriginalDisposition, ControllerReadinessFailure> {
        if namespace.ingress != self.ingress_domain {
            return Err(ControllerReadinessFailure::PolicyUnavailable);
        }
        if let Some(disposition) = self.collector_disposition(namespace, original)? {
            return Ok(disposition);
        }
        let originals = self
            .originals
            .lock()
            .map_err(|_| ControllerReadinessFailure::PolicyUnavailable)?;
        let record = originals
            .iter()
            .find(|entry| entry.names(&namespace.realm, original))
            .ok_or(ControllerReadinessFailure::PolicyUnavailable)?;
        let operation = match record {
            OriginalRecord::Bound(held) => &held.operation,
            OriginalRecord::UnboundCancellation { operation, .. } => operation,
        };
        match operation {
            OriginalOperationDisposition::Open
                if matches!(
                    record,
                    OriginalRecord::Bound(_)
                        | OriginalRecord::UnboundCancellation {
                            withdrawn_association: Some(_),
                            ..
                        }
                ) =>
            {
                Ok(OriginalDisposition::Active)
            }
            OriginalOperationDisposition::Cancelled { record } => {
                let evidence =
                    crate::private_originals::cancellation_evidence(record, namespace, original)
                        .map_err(|_| ControllerReadinessFailure::PolicyUnavailable)?;
                Ok(OriginalDisposition::OperationCancelled { evidence })
            }
            _ => Err(ControllerReadinessFailure::PolicyUnavailable),
        }
    }

    fn authenticated_original(
        &self,
        ingress: &NativeIngressContext,
        namespace: &QualifiedIngressNamespace,
        original: &OriginalWorkRef,
    ) -> Result<Option<AuthenticatedOriginal>, ControllerReadinessFailure> {
        let selected = {
            let originals = self.wait_for_source_collection()?;
            originals
                .iter()
                .filter_map(OriginalRecord::bound)
                .find(|retained| {
                    let claims = retained.association.candidate();
                    retained.names(namespace, original)
                        && &claims.requester == ingress.requester()
                        && &claims.ingress_namespace.realm == ingress.realm()
                        && (retained.source_custody == Some(SourceCustody::Admitted)
                            || (retained.source_custody == Some(SourceCustody::Candidate)
                                && claims.ingress_namespace.occurrence_scope
                                    == namespace.occurrence_scope))
                })
                .map(|held| {
                    (
                        held.retained.clone(),
                        held.service_set_admission.clone(),
                        held.source_custody,
                    )
                })
        };
        let Some((retained, guard, custody)) = selected else {
            return Ok(None);
        };
        // Native alone supplies this verdict at its decisive admission point,
        // with driver/credential custody and no await before input acceptance.
        // The earlier host fence does not close the queued-admission window.
        // The same incoming context also reaches admitted contributors. Only
        // this own Candidate requires the first-set guard; an admitted source
        // never inherits a new input's FirstAuthentication obligation.
        // Later operation authentication and verified retained replay deliberately
        // preserve independent target revocation rather than gating the set.
        if custody == Some(SourceCustody::Candidate)
            && matches!(
                retained.grant_binding,
                OriginalGrantBinding::ServiceSet { .. }
            )
            && matches!(
                ingress.replay_lookup(),
                Some(meerkat_runtime::input_authority::NativeReplayLookup::FirstAuthentication)
            )
        {
            let guard = guard.ok_or(ControllerReadinessFailure::PolicyUnavailable)?;
            if !self
                .clock
                .now()
                .is_ok_and(|now| now.unix_ms < guard.expires_at_ms)
            {
                return Err(ControllerReadinessFailure::PolicyUnavailable);
            }
            guard
                .publication
                .check_current()
                .map_err(|_| ControllerReadinessFailure::PolicyUnavailable)?;
            let claims = retained.association.candidate();
            for lineage in claims.authority_basis.grant_lineages() {
                self.grants
                    .resolve_lineage(
                        lineage,
                        &claims.logical_executor,
                        claims.represented_subject.as_ref(),
                    )
                    .map_err(|_| ControllerReadinessFailure::PolicyUnavailable)?
                    .check_current()
                    .map_err(|_| ControllerReadinessFailure::PolicyUnavailable)?;
            }
            guard
                .publication
                .check_current()
                .map_err(|_| ControllerReadinessFailure::PolicyUnavailable)?;
        }
        Ok(Some(retained.custody.clone()))
    }

    fn retained_custody(
        &self,
        namespace: &QualifiedIngressNamespace,
        original: &OriginalWorkRef,
    ) -> Result<RetainedCustody, ControllerReadinessFailure> {
        let originals = self.wait_for_source_collection()?;
        let Some(held) = originals
            .iter()
            .filter_map(OriginalRecord::bound)
            .find(|held| held.names(namespace, original))
        else {
            return Ok(RetainedCustody::NotRetained);
        };
        match held.source_custody {
            Some(SourceCustody::RestorationUnknown) => {
                Err(ControllerReadinessFailure::PolicyUnavailable)
            }
            Some(SourceCustody::Admitted) => Ok(RetainedCustody::Retained(
                held.custody.authority_basis.clone(),
            )),
            Some(SourceCustody::Candidate) => match &held.custody_settlement {
                Some(settlement) => {
                    let settlement = settlement
                        .upgrade()
                        .ok_or(ControllerReadinessFailure::PolicyUnavailable)?;
                    // Read without the recovery control or invocation lock:
                    // their holders take this source collection after them.
                    // No native lock, storage read or recovery action belongs
                    // in this callback.
                    settlement.pending_readiness()?;
                    Ok(RetainedCustody::AdmissionPending)
                }
                None => Ok(RetainedCustody::NotRetained),
            },
            None => Ok(RetainedCustody::NotRetained),
        }
    }
}

/// Take `lock` for a synchronous native reader, waiting out a holder until
/// `deadline`, with `pause` between attempts. The first attempt is immediate;
/// every later attempt checks the deadline first, so a reader descheduled
/// across its deadline never reads after it, even when the holder released
/// meanwhile. Contention past the deadline and poison answer unavailable.
fn wait_for_lock<T>(
    lock: &std::sync::Mutex<T>,
    deadline: std::time::Instant,
    mut pause: impl FnMut(std::time::Duration),
) -> Result<std::sync::MutexGuard<'_, T>, ControllerReadinessFailure> {
    let mut step = std::time::Duration::from_micros(20);
    loop {
        match lock.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(std::sync::TryLockError::Poisoned(_)) => {
                return Err(ControllerReadinessFailure::PolicyUnavailable);
            }
            Err(std::sync::TryLockError::WouldBlock) => {}
        }
        let now = std::time::Instant::now();
        if now >= deadline {
            return Err(ControllerReadinessFailure::PolicyUnavailable);
        }
        pause(step.min(deadline - now));
        step = (step * 2).min(std::time::Duration::from_millis(1));
        // Expiry before retry, as Core's owner-change wait checks its
        // deadline before every read.
        if std::time::Instant::now() >= deadline {
            return Err(ControllerReadinessFailure::PolicyUnavailable);
        }
    }
}

/// How an owner step takes the source collection. `Wait` waits out a holder
/// within Core's owner-change bound, and is used only on a path that holds no
/// owner change or reservation of the shared publication. `Try` refuses a held
/// collection at once: inside an open change, inside a native fence and
/// wherever the caller already pauses between attempts itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CollectionAccess {
    Wait,
    Try,
}

impl ToolkitOriginalOwner {
    /// The source collection for the synchronous native readers that
    /// authenticate an original and read its retained custody. Its holders
    /// are this owner's own short synchronous sections: they never await,
    /// call back into native or wait for a lock that a native caller holds,
    /// and the sibling disposition read already takes this lock blocking. So
    /// a held collection is contention, never a policy answer, and the reader
    /// waits for the holder. The wait is bounded by Core's owner-change bound
    /// and then answers unavailable, as before, so no unforeseen holder can
    /// turn a refusal into a hang. A poisoned collection answers unavailable
    /// at once.
    fn wait_for_source_collection(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, Vec<OriginalRecord>>, ControllerReadinessFailure> {
        wait_for_lock(
            &self.originals,
            std::time::Instant::now() + meerkat_core::authorization::OWNER_CHANGE_WAIT_BOUND,
            std::thread::sleep,
        )
    }

    /// The source collection for this owner's own writers and checks: the
    /// same bounded wait as the native readers, for the same reason (its
    /// holders are short synchronous sections that never wait for a lock a
    /// waiter here holds; the publication writer, the source writer and the
    /// recovery locks are always taken before it, never under it). A holder
    /// that outlasts the bound, or a poisoned collection, is unavailable.
    /// Only for a path that holds no owner change of the shared publication:
    /// inside one, use [`Self::change_with_collection`].
    fn wait_for_collection(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, Vec<OriginalRecord>>, OriginalWriterError> {
        self.wait_for_source_collection()
            .map_err(|_| OriginalWriterError::Unavailable)
    }

    fn source_collection(
        &self,
        access: CollectionAccess,
    ) -> Result<std::sync::MutexGuard<'_, Vec<OriginalRecord>>, OriginalWriterError> {
        match access {
            CollectionAccess::Wait => self.wait_for_collection(),
            CollectionAccess::Try => self
                .originals
                .try_lock()
                .map_err(|_| OriginalWriterError::Unavailable),
        }
    }

    /// Run `apply` under a new owner change of the shared publication and the
    /// source collection, taken in that order. Nothing waits while the change
    /// is open: the collection is only tried inside it. When another owner
    /// holds the collection, the change ends with no fact changed (Core's
    /// rule for a contended owner: dropping the guard only invalidates
    /// observations), the collection is waited for outside any change,
    /// within Core's owner-change bound, and the change begins again. `apply`
    /// must not wait or await, and runs at most once.
    fn change_with_collection<T>(
        &self,
        apply: impl FnMut(&mut Vec<OriginalRecord>) -> Result<T, OriginalWriterError>,
    ) -> Result<T, OriginalWriterError> {
        self.change_with_collection_within(
            std::time::Instant::now() + meerkat_core::authorization::OWNER_CHANGE_WAIT_BOUND,
            std::thread::sleep,
            || {},
            apply,
        )
    }

    /// [`Self::change_with_collection`] with its one deadline, the pause of
    /// its outside wait, and `before_round`, run before every round after the
    /// first with no change open: a test's seam for a caller descheduled
    /// there.
    fn change_with_collection_within<T>(
        &self,
        deadline: std::time::Instant,
        mut pause: impl FnMut(std::time::Duration),
        mut before_round: impl FnMut(),
        mut apply: impl FnMut(&mut Vec<OriginalRecord>) -> Result<T, OriginalWriterError>,
    ) -> Result<T, OriginalWriterError> {
        let mut first = true;
        loop {
            if !first {
                before_round();
                // Expiry before every round after the first: one deadline
                // bounds all rounds, so a caller past it begins no change and
                // applies nothing, and repeated takes of the collection
                // between rounds never extend it.
                if std::time::Instant::now() >= deadline {
                    return Err(OriginalWriterError::Unavailable);
                }
            }
            first = false;
            {
                let _publication = self
                    .publication
                    .begin_owner_change()
                    .map_err(|_| OriginalWriterError::Unavailable)?;
                match self.originals.try_lock() {
                    Ok(mut originals) => return apply(&mut originals),
                    Err(std::sync::TryLockError::Poisoned(_)) => {
                        return Err(OriginalWriterError::Unavailable);
                    }
                    Err(std::sync::TryLockError::WouldBlock) => {}
                }
            }
            drop(
                wait_for_lock(&self.originals, deadline, &mut pause)
                    .map_err(|_| OriginalWriterError::Unavailable)?,
            );
        }
    }

    /// The source writer for an async writer that takes it before any other
    /// guard: a cancellation, or a Find's helper admission. Another writer
    /// holds it across its own durable write, so the caller waits for that
    /// writer, within Core's owner-change bound, and then reads every fact
    /// afresh under it. A holder that outlasts the bound is unavailable, as
    /// before, and nothing was changed.
    async fn bounded_source_writer(
        &self,
    ) -> Result<tokio::sync::MutexGuard<'_, ()>, OriginalWriterError> {
        tokio::time::timeout(
            meerkat_core::authorization::OWNER_CHANGE_WAIT_BOUND,
            self.source_writer(),
        )
        .await
        .map_err(|_| OriginalWriterError::Unavailable)
    }

    pub(crate) fn admitted_original_basis(
        &self,
        namespace: &QualifiedIngressNamespace,
        original: &OriginalWorkRef,
    ) -> Result<Option<WorkAuthorityBasis>, ControllerReadinessFailure> {
        let originals = self
            .originals
            .try_lock()
            .map_err(|_| ControllerReadinessFailure::PolicyUnavailable)?;
        Ok(admitted_basis_in(&originals, namespace, original))
    }

    /// [`Self::admitted_original_basis`] for a reader that holds no owner
    /// change, reservation or other guard: a source collection held by
    /// another owner's short section is contention, waited for until
    /// `deadline` (the reader's step's one deadline), as the native readers
    /// wait. Past the deadline, or poisoned, it is unavailable.
    pub(crate) fn admitted_original_basis_until(
        &self,
        namespace: &QualifiedIngressNamespace,
        original: &OriginalWorkRef,
        deadline: std::time::Instant,
    ) -> Result<Option<WorkAuthorityBasis>, ControllerReadinessFailure> {
        let originals = wait_for_lock(&self.originals, deadline, std::thread::sleep)?;
        Ok(admitted_basis_in(&originals, namespace, original))
    }
}

/// The admitted custody's retained basis of `original`, when it has one.
fn admitted_basis_in(
    originals: &[OriginalRecord],
    namespace: &QualifiedIngressNamespace,
    original: &OriginalWorkRef,
) -> Option<WorkAuthorityBasis> {
    originals
        .iter()
        .filter_map(OriginalRecord::bound)
        .find(|retained| {
            retained.source_custody == Some(SourceCustody::Admitted)
                && retained.names(namespace, original)
        })
        .map(|retained| retained.custody.authority_basis.clone())
}

#[cfg(test)]
#[path = "original_ingress_tests.rs"]
pub(crate) mod tests;
