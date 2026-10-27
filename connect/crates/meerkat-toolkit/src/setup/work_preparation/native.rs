//! Native format of the same immutable prepared-work owner. These are storage
//! and integrity mechanics for the application's publication operation. They
//! are not a policy adapter, canonical root locator, or execution permit.
use super::*;
use crate::authority::{Ceiling, SetupRequestEvidence, SetupUseContext};
use meerkat_authorization_contracts::work_association::{
    InputAuthorityAssociation, NativeWorkTarget, OriginalWorkRef,
};
use meerkat_core::connection::RealmId;
use meerkat_workgraph::{WorkAdmission, WorkAdmissionKey, WorkItemId};
use toolkit_store::{ConditionalAdmission, EffectLease, RecordCondition};

mod effects;
pub use effects::{
    NativeSetupWorkOwner, NativeWorkCompletion, NativeWorkObservation, NativeWorkStep,
};
mod metadata;
use metadata::PreparedMetadata;
pub use metadata::{SetupWorkBackend, SetupWorkBackendPin};

const VERSION: u32 = 4;

// Failure-only static diagnostics stay active when this crate is a dependency.
// No request, identity, permission, stored value or error payload is emitted.
// Diagnostic I/O must never change the publication result or use protocol stdout.
#[cold]
fn publication_failure(branch: &'static str) {
    use std::io::Write as _;
    let _ = writeln!(std::io::stderr().lock(), "toolkit.setup.publication.refused:{branch}");
}

fn publication_mismatch(branch: &'static str, mismatch: bool) -> bool {
    if mismatch {
        publication_failure(branch);
    }
    mismatch
}

fn publication_error<E>(branch: &'static str) -> impl FnOnce(E) -> E {
    move |error| {
        publication_failure(branch);
        error
    }
}

// Frozen input and final publication share a record owner, but are distinct
// local admissions. Bind the stable destination, not mutable publication data;
// WorkGraph admission and effect identities remain unchanged.
fn publication_admission_key(
    key: &StoreKey,
    native_admission: &str,
) -> Result<String, SetupError> {
    let identity = canonical_json(&(
        "meerkat-toolkit-native-publication-v1",
        key,
        native_admission,
    ))?;
    Ok(format!(
        "native-publication/{}",
        ContentDigest::of_bytes(&identity)
    ))
}

/// Concrete setup work selected by the application preparer. The native
/// original identity and target come from the actual application/native owner;
/// Toolkit does not derive an authority locator from its own setup record.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupWorkDelegation {
    pub destination: NativeWorkTarget,
    pub realm_id: RealmId,
    pub original_work: OriginalWorkRef,
    pub delegation_resource: Id,
    pub ceiling: Ceiling,
    pub expires_at_ms: u64,
    pub composition_digest: ContentDigest,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NativePreparedRecord {
    work: PreparedRecord,
    metadata: PreparedMetadata,
    delegation: SetupWorkDelegation,
    child: SetupRequestEvidence,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    helper_source: Option<NativeHelperSource>,
    // Exact historical versions used at publication, never current permission.
    basis: Vec<PublicationBasis>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicationBasis {
    key: StoreKey,
    revision: u64,
    content_digest: ContentDigest,
    record_claim: Option<ClaimBasis>,
    dispatch_claim: Option<DispatchBasis>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaimBasis {
    store_id: String,
    worker: String,
    fence: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DispatchBasis {
    claim: ClaimBasis,
    effect: String,
}

/// Pure pre-worker input. It has no helper identity, destination, grant or
/// permission to dispatch. Its request/key belong to the existing acquisition
/// effect; actual helper source binding supplies delegation later.
#[derive(Clone)]
pub struct NativeWorkPreparation {
    work: PreparedRecord,
    metadata: PreparedMetadata,
    parent: OriginalWorkRef,
    key: StoreKey,
    admission_key: WorkAdmissionKey,
    effect: EffectKey,
    input_record: Option<RecordCondition>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeInputRecord {
    work: PreparedRecord,
    metadata: PreparedMetadata,
    parent: OriginalWorkRef,
    admission_key: WorkAdmissionKey,
    effect: EffectKey,
}

impl NativeWorkPreparation {
    pub fn parent(&self) -> &OriginalWorkRef {
        &self.parent
    }
    pub fn key(&self) -> &StoreKey {
        &self.key
    }
    pub fn work_request(&self) -> &CreateWorkItemRequest {
        &self.work.request
    }
    pub fn admission_key(&self) -> &WorkAdmissionKey {
        &self.admission_key
    }
    pub fn effect_key(&self) -> &EffectKey {
        &self.effect
    }
    pub fn effect_id(&self) -> &str {
        &self.effect.id
    }
    pub fn input_reference(&self) -> &ContextReference {
        &self.work.input_reference
    }
    pub fn backend_pin(&self) -> &SetupWorkBackendPin {
        self.metadata.backend()
    }
}

/// Historical facts supplied by the actual helper source owner after its
/// canonical row is retained and re-read. This public data is not a permit,
/// native custody proof, or substitute for current complete contributor checks.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeHelperSource {
    #[serde(with = "source_condition")]
    pub source: RecordCondition,
    pub realm: String,
    pub namespace: WorkNamespace,
    pub item: WorkItemId,
    pub admission: WorkAdmission,
    pub effect: EffectKey,
    pub preparation_request: CreateWorkItemRequest,
    pub admitted_request: CreateWorkItemRequest,
    pub association: InputAuthorityAssociation,
    pub contributors: Vec<InputAuthorityAssociation>,
}

// Persist immutable source identity only. Effect/record claims belong to the
// separate publication conditions and cannot be smuggled into source history.
mod source_condition {
    use super::*;
    pub fn serialize<S: serde::Serializer>(
        source: &RecordCondition,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        if source.claim.is_some() || source.dispatch_claim.is_some() {
            return Err(serde::ser::Error::custom(
                "helper source condition must be immutable",
            ));
        }
        (&source.key, source.revision, &source.content_digest).serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<RecordCondition, D::Error> {
        let (key, revision, content_digest) = Deserialize::deserialize(deserializer)?;
        Ok(RecordCondition {
            key,
            revision,
            content_digest,
            claim: None,
            dispatch_claim: None,
        })
    }
}

/// Pure candidate tied to an exact validated acquisition and its existing work
/// effect. Private fields prevent callers replacing the parent/key after it is
/// fixed. It carries no authority seal, permission verdict or dispatch capability.
#[derive(Clone)]
pub struct NativePreparedWorkProposal {
    work: PreparedRecord,
    metadata: PreparedMetadata,
    delegation: SetupWorkDelegation,
    parent: OriginalWorkRef,
    key: StoreKey,
    admission_key: String,
    input_record: Option<RecordCondition>,
}

impl NativePreparedWorkProposal {
    pub fn destination(&self) -> NativeWorkTarget {
        self.delegation.destination.clone()
    }
    pub fn parent(&self) -> &OriginalWorkRef {
        &self.parent
    }
    pub fn key(&self) -> &StoreKey {
        &self.key
    }
    /// Original host preparation, retained verbatim. The inert native metadata
    /// request is derived separately and does not copy this request's content.
    pub fn work_request(&self) -> &CreateWorkItemRequest {
        &self.work.request
    }
    /// Host-selected backend identity, not a store attestation or use grant.
    pub fn backend_pin(&self) -> &SetupWorkBackendPin {
        self.metadata.backend()
    }
    pub fn input_reference(&self) -> &ContextReference {
        &self.work.input_reference
    }
    pub fn composition_digest(&self) -> &ContentDigest {
        &self.delegation.composition_digest
    }
    pub fn delegation(&self) -> &SetupWorkDelegation {
        &self.delegation
    }
}

/// Exact participating facts selected by the real application owners after
/// operation-specific authorization. This is not a proof that those owners are
/// authoritative or that the set is complete. Copies of remote state do not
/// qualify. No callback or default supplies missing policy/custody permission.
pub struct PreparedWorkPublicationConditions {
    /// Original request, application policy metadata and any other
    /// relevant owners. Participating Toolkit writers use this physical backend; native grants and runtime custody do not move into it.
    pub owners: Vec<RecordCondition>,
    /// Claim of the original acquisition's work-admission effect. The library
    /// adds the actual acquisition revision/digest itself; callers cannot omit
    /// acquisition cancellation or substitute an unrelated effect claim.
    pub dispatch_claim: EffectLease,
    pub before_ms: i64,
}

/// Committed v4 evidence from the existing preparation owner. It is deliberately
/// not SetupWorkAdmission and has no method producing NativeWorkAvailable.
#[derive(Clone)]
pub struct PublishedSetupWork {
    key: StoreKey,
    digest: ContentDigest,
    record: NativePreparedRecord,
}

impl PublishedSetupWork {
    pub fn key(&self) -> &StoreKey {
        &self.key
    }
    pub fn digest(&self) -> &ContentDigest {
        &self.digest
    }
    pub fn child(&self) -> &SetupRequestEvidence {
        &self.record.child
    }
    pub fn child_reference(&self) -> &OriginalWorkRef {
        self.record.child.original_work()
    }
    /// Immutable source correlation already checked at publication and recovery.
    /// It supplies no current permission or custody.
    pub fn helper_source(&self) -> Option<&NativeHelperSource> {
        self.record.helper_source.as_ref()
    }
    /// Original host preparation, not the metadata request sent to WorkGraph.
    pub fn work_request(&self) -> &CreateWorkItemRequest {
        &self.record.work.request
    }
    pub fn backend_pin(&self) -> &SetupWorkBackendPin {
        self.record.metadata.backend()
    }
    /// Inert prepared metadata. This record does not prove WorkGraph insertion
    /// and carries no claim, execution binding or permission to launch helpers.
    pub fn prepared_item(&self) -> &meerkat_workgraph::WorkItem {
        self.record.metadata.item()
    }
    pub fn prepared_created_event(&self) -> &meerkat_workgraph::WorkGraphEvent {
        self.record.metadata.event()
    }
    pub fn input_reference(&self) -> &ContextReference {
        &self.record.work.input_reference
    }
    pub fn composition_digest(&self) -> &ContentDigest {
        &self.record.delegation.composition_digest
    }
}

impl SetupStore<'_> {
    /// Exact identity of the existing stored work-admission effect. This read
    /// grants no dispatch permission and does not create an effect or key.
    /// Host preparation may bind its immutable request before the input record
    /// is frozen, then the ordinary preparation path verifies the same effect.
    pub async fn native_work_effect_key(
        &self,
        snapshot: &SetupSnapshot,
    ) -> Result<EffectKey, SetupError> {
        if snapshot.key() != &self.scope.key(&operation_id(snapshot.request())?) {
            return Err(SetupError::WrongScope);
        }
        Ok(self.original_work_effect(snapshot).await?.key)
    }

    /// Validate a private recovery projection against this exact canonical
    /// setup owner, frozen input and selected backend. This grants no content
    /// access, retry or native work permission, and tolerates terminal setup
    /// state so an independently authorized settlement owner can reconcile it.
    pub async fn validate_native_work_recovery(
        &self,
        preparation: &NativeWorkPreparation,
        backend: &SetupWorkBackend,
    ) -> Result<(), SetupError> {
        preparation.metadata.check_backend(backend)?;
        let snapshot = self.recover(
            self.store
                .get(&preparation.effect.record)
                .await?
                .ok_or(StoreError::NotFound)?,
        )?;
        let operation = operation_id(snapshot.request())?;
        let saved = self
            .store
            .get(&self.native_input_key(&operation))
            .await?
            .ok_or(StoreError::NotFound)?;
        let current = self.recover_native_input(&snapshot, saved).await?;
        let expected = preparation
            .input_record
            .as_ref()
            .ok_or(SetupError::InvalidRecord)?;
        let actual = current
            .input_record
            .as_ref()
            .ok_or(SetupError::InvalidRecord)?;
        if canonical_json(&current.work)? != canonical_json(&preparation.work)?
            || current.metadata.backend() != preparation.metadata.backend()
            || current.parent != preparation.parent
            || current.key != preparation.key
            || current.effect != preparation.effect
            || current.admission_key != preparation.admission_key
            || actual.key != expected.key
            || actual.revision != expected.revision
            || actual.content_digest != expected.content_digest
            || expected.claim.is_some()
            || expected.dispatch_claim.is_some()
        {
            return Err(SetupError::InvalidRecord);
        }
        Ok(())
    }

    /// Prepare before any helper WorkGraph admission or runtime materialization.
    /// The preparer supplies no original identity: only the actual source owner
    /// can bind it after native work admission and controller preparation.
    pub async fn prepare_native_work(
        &self,
        snapshot: &SetupSnapshot,
        access: SetupAccess<'_>,
        preparer: &dyn SetupWorkPreparer,
        backend: &SetupWorkBackend,
    ) -> Result<NativeWorkPreparation, SetupError> {
        self.conditional_store
            .ok_or(SetupError::NativeExecutionUnavailable)?;
        self.check_snapshot(snapshot, &access, true)?;
        let operation = operation_id(snapshot.request())?;
        let input_key = self.native_input_key(&operation);
        if let Some(saved) = self.store.get(&input_key).await? {
            let preparation = self.recover_native_input(snapshot, saved).await?;
            preparation.metadata.check_backend(backend)?;
            crate::setup::installed_input::verify_work_input_reference(
            snapshot.request(), preparation.input_reference(), access.evidence, access.context, access.references,
        )?;
            return Ok(preparation);
        }
        let proposed = preparer.prepare(snapshot.request())?;
        if proposed.delegation.is_some() {
            return Err(SetupError::InvalidRecord);
        }
        let preparation = self
            .prepare_native_input(snapshot, &access, proposed, backend)
            .await?;
        let record = NativeInputRecord {
            work: preparation.work,
            metadata: preparation.metadata,
            parent: preparation.parent,
            admission_key: preparation.admission_key,
            effect: preparation.effect,
        };
        let saved = self
            .store
            .admit(AdmissionRequest {
                key: input_key.clone(),
                admission_key: record.admission_key.as_str().to_owned(),
                data: RecordData {
                    schema: SCHEMA.into(),
                    version: 5,
                    body: serde_json::to_value(&record)?,
                },
                events: vec![],
                effects: vec![],
            })
            .await;
        let saved = match saved {
            Ok(saved) => saved.record().clone(),
            Err(StoreError::AdmissionConflict | StoreError::AlreadyExists) => self
                .store
                .get(&input_key)
                .await?
                .ok_or(StoreError::NotFound)?,
            Err(error) => return Err(error.into()),
        };
        let winner = self.recover_native_input(snapshot, saved).await?;
        // Native metadata contains inert creation randomness. The persisted
        // winner is reused only when every actual outbound selection is exact.
        if canonical_json(&winner.work)? != canonical_json(&record.work)?
            || winner.metadata.backend() != record.metadata.backend()
            || winner.parent != record.parent
            || winner.effect != record.effect
            || winner.admission_key != record.admission_key
        {
            return Err(StoreError::AdmissionConflict.into());
        }
        Ok(winner)
    }

    fn native_input_key(&self, operation: &Id) -> StoreKey {
        let mut key = self.preparation_key(operation);
        key.id = format!("input/{}", key.id);
        key
    }

    async fn recover_native_input(
        &self,
        snapshot: &SetupSnapshot,
        saved: StoredRecord,
    ) -> Result<NativeWorkPreparation, SetupError> {
        let effect = self.original_work_effect(snapshot).await?;
        let operation = operation_id(snapshot.request())?;
        if saved.key != self.native_input_key(&operation)
            || saved.revision != 1
            || saved.data.schema != SCHEMA
            || saved.data.version != 5
            || saved.content_digest != saved.data.digest()?
        {
            return Err(SetupError::InvalidRecord);
        }
        let input: NativeInputRecord = serde_json::from_value(saved.data.body.clone())?;
        if canonical_json(&input)? != canonical_json(&saved.data.body)? {
            return Err(SetupError::InvalidRecord);
        }
        let preparation = NativeWorkPreparation {
            work: input.work,
            metadata: input.metadata,
            parent: input.parent,
            key: self.preparation_key(&operation),
            admission_key: input.admission_key,
            effect: input.effect,
            input_record: Some(RecordCondition {
                key: saved.key,
                revision: saved.revision,
                content_digest: saved.content_digest,
                claim: None,
                dispatch_claim: None,
            }),
        };
        validate_preparation(self, snapshot, &effect, &preparation)?;
        preparation
            .metadata
            .validate(snapshot, &effect, &preparation.work)
            .await?;
        Ok(preparation)
    }

    /// Bind only the actual post-admission original/destination. This remains
    /// integrity preparation, not child authentication or dispatch permission.
    pub async fn bind_native_work(
        &self,
        snapshot: &SetupSnapshot,
        preparation: &NativeWorkPreparation,
        delegation: SetupWorkDelegation,
        access: SetupAccess<'_>,
    ) -> Result<NativePreparedWorkProposal, SetupError> {
        self.conditional_store
            .ok_or(SetupError::NativeExecutionUnavailable)?;
        self.check_snapshot(snapshot, &access, true)?;
        if snapshot.state().terminal_revision != 0 {
            return Err(SetupError::Rejected);
        }
        let effect = self.original_work_effect(snapshot).await?;
        if preparation.effect != effect.key {
            return Err(SetupError::InvalidRecord);
        }
        let proposal = NativePreparedWorkProposal {
            work: preparation.work.clone(),
            metadata: preparation.metadata.clone(),
            delegation,
            parent: preparation.parent.clone(),
            key: preparation.key.clone(),
            admission_key: preparation.admission_key.as_str().to_owned(),
            input_record: preparation.input_record.clone(),
        };
        validate_proposal_binding(self, snapshot, &effect, &proposal)?;
        proposal
            .metadata
            .validate(snapshot, &effect, &proposal.work)
            .await?;
        crate::setup::installed_input::verify_work_input_reference(
            snapshot.request(), &proposal.work.input_reference, access.evidence, access.context, access.references,
        )?;
        Ok(proposal)
    }

    /// Existing already-bound historical preparation path. New helper setup
    /// uses `prepare_native_work`, then `bind_native_work` with its real original.
    pub async fn propose_native_work(
        &self,
        snapshot: &SetupSnapshot,
        access: SetupAccess<'_>,
        preparer: &dyn SetupWorkPreparer,
        backend: &SetupWorkBackend,
    ) -> Result<NativePreparedWorkProposal, SetupError> {
        self.conditional_store
            .ok_or(SetupError::NativeExecutionUnavailable)?;
        self.check_snapshot(snapshot, &access, true)?;
        let mut proposed = preparer.prepare(snapshot.request())?;
        let delegation = proposed
            .delegation
            .take()
            .ok_or(SetupError::InvalidRecord)?;
        let preparation = self
            .prepare_native_input(snapshot, &access, proposed, backend)
            .await?;
        self.bind_native_work(snapshot, &preparation, delegation, access)
            .await
    }

    async fn prepare_native_input(
        &self,
        snapshot: &SetupSnapshot,
        access: &SetupAccess<'_>,
        proposed: SetupWorkPreparation,
        backend: &SetupWorkBackend,
    ) -> Result<NativeWorkPreparation, SetupError> {
        if snapshot.state().terminal_revision != 0 {
            return Err(SetupError::Rejected);
        }
        let effect = self.original_work_effect(snapshot).await?;
        crate::setup::installed_input::verify_work_input_reference(
            snapshot.request(), &proposed.input_reference, access.evidence, access.context, access.references,
        )?;
        proposed
            .scope
            .validate_prepared_request(&proposed.work_request)?;
        backend.check_scope(&proposed.scope)?;
        let operation = operation_id(snapshot.request())?;
        let work = PreparedRecord {
            acquisition: snapshot.key().clone(),
            effect_digest: effect.request_digest.clone(),
            environment_revision: snapshot.request.host().environment_revision.clone(),
            realm: proposed.scope.realm().clone(),
            namespace: proposed.scope.namespace().clone(),
            input_reference: proposed.input_reference,
            request: proposed.work_request,
        };
        if work.input_reference.content_digest
            != ContentDigest::of_bytes(&canonical_json(snapshot.request.request())?)
        {
            return Err(SetupError::InvalidRecord);
        }
        let metadata = PreparedMetadata::prepare(backend, snapshot, &effect, &work).await?;
        Ok(NativeWorkPreparation {
            work,
            metadata,
            parent: snapshot.request.original_work().clone(),
            key: self.preparation_key(&operation),
            admission_key: WorkAdmissionKey::new(effect.intent.idempotency_key)?,
            effect: effect.key,
            input_record: None,
        })
    }

    /// Record one historical native association through the real conditional store. The
    /// calling application owns current authorization and complete participating
    /// facts; this method validates exact native integrity and storage predicates
    /// only. It never issues native work, grants or creates workers.
    // Each parameter names a separate owner fact; none is a generic allow flag.
    #[allow(clippy::too_many_arguments)]
    pub async fn publish_prepared_work(
        &self,
        snapshot: &SetupSnapshot,
        proposal: &NativePreparedWorkProposal,
        child: SetupRequestEvidence,
        access: SetupAccess<'_>,
        conditions: PreparedWorkPublicationConditions,
    ) -> Result<PublishedSetupWork, SetupError> {
        self.publish_native_work(snapshot, proposal, child, None, access, conditions)
            .await
    }

    /// Publish the actual WorkGraph-authenticated helper source. The host must
    /// obtain this history from its canonical source owner, retain all current
    /// contributor authority at native use, and supply the real source row in
    /// the same atomic publication conditions. These checks are correlation only.
    #[allow(clippy::too_many_arguments)]
    pub async fn publish_prepared_helper_work(
        &self,
        snapshot: &SetupSnapshot,
        proposal: &NativePreparedWorkProposal,
        child: SetupRequestEvidence,
        helper_source: NativeHelperSource,
        access: SetupAccess<'_>,
        conditions: PreparedWorkPublicationConditions,
    ) -> Result<PublishedSetupWork, SetupError> {
        self.publish_native_work(
            snapshot,
            proposal,
            child,
            Some(helper_source),
            access,
            conditions,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn publish_native_work(
        &self,
        snapshot: &SetupSnapshot,
        proposal: &NativePreparedWorkProposal,
        child: SetupRequestEvidence,
        helper_source: Option<NativeHelperSource>,
        access: SetupAccess<'_>,
        conditions: PreparedWorkPublicationConditions,
    ) -> Result<PublishedSetupWork, SetupError> {
        let store = self
            .conditional_store
            .ok_or(SetupError::NativeExecutionUnavailable)
            .map_err(publication_error("publish.conditional_store"))?;
        self.check_snapshot(snapshot, &access, false)
            .map_err(publication_error("publish.snapshot_observation"))?;
        let effect = self.original_work_effect(snapshot).await
            .map_err(publication_error("publish.work_effect"))?;
        validate_proposal_binding(self, snapshot, &effect, proposal)
            .map_err(publication_error("publish.proposal"))?;
        proposal
            .metadata
            .validate(snapshot, &effect, &proposal.work)
            .await.map_err(publication_error("publish.metadata"))?;
        if publication_mismatch("publish.dispatch_effect", conditions.dispatch_claim.key != effect.key)
            || publication_mismatch("publish.owner_conditions_empty", conditions.owners.is_empty()) {
            return Err(SetupError::InvalidRecord);
        }
        let mut owners = conditions.owners;
        if let Some(input) = &proposal.input_record {
            owners.push(input.clone());
        }
        owners.push(RecordCondition {
            key: snapshot.key().clone(),
            revision: snapshot.stored.revision,
            content_digest: snapshot.stored.content_digest.clone(),
            claim: None,
            dispatch_claim: Some(conditions.dispatch_claim),
        });
        let basis = publication_basis(&owners, &proposal.key)?;
        let candidate = NativePreparedRecord {
            work: proposal.work.clone(),
            metadata: proposal.metadata.clone(),
            delegation: proposal.delegation.clone(),
            child,
            helper_source,
            basis: basis.clone(),
        };
        validate_child(&candidate, proposal, access.evidence)?;
        if candidate
            .helper_source
            .as_ref()
            .is_some_and(|source| publication_mismatch("publish.helper_effect", source.effect != effect.key))
        {
            return Err(SetupError::InvalidRecord);
        }
        let exact_child = candidate.child.clone();
        let exact_helper = candidate.helper_source.clone();
        // Recovery is observation, including after old publication conditions
        // expire. It must not reissue, overwrite or infer permission to dispatch.
        if let Some(saved) = self.store.get(&proposal.key).await
            .map_err(publication_error("publish.existing_read"))? {
            return recover_native(
                self,
                snapshot,
                &effect,
                saved,
                Some((proposal, &basis, &exact_child, exact_helper.as_ref())),
                access.evidence,
                Some((access.references, access.context)),
            )
            .await;
        }
        self.check_snapshot(snapshot, &access, true)
            .map_err(publication_error("publish.snapshot_entry"))?;
        if snapshot.state().terminal_revision != 0 {
            publication_failure("publish.terminal");
            return Err(SetupError::Rejected);
        }
        let issued = i64::try_from(candidate.child.constraints().issued_at_ms)
            .map_err(|_| { publication_failure("publish.issue_range"); SetupError::InvalidRecord })?;
        if publication_mismatch("publish.issue_store_clock", issued > store.now_ms().await
                .map_err(publication_error("publish.store_clock"))?)
            || publication_mismatch("publish.issue_access_clock", candidate.child.constraints().issued_at_ms > access.context.now_ms)
        {
            return Err(SetupError::InvalidRecord);
        }
        let expiry = i64::try_from(candidate.child.constraints().expires_at_ms)
            .map_err(|_| { publication_failure("publish.expiry_range"); SetupError::InvalidRecord })?;
        if publication_mismatch("publish.deadline_expiry", conditions.before_ms > expiry)
            || publication_mismatch("publish.deadline_positive", conditions.before_ms <= 0) {
            return Err(SetupError::InvalidRecord);
        }
        let data = RecordData {
            schema: SCHEMA.into(),
            version: VERSION,
            body: serde_json::to_value(candidate).map_err(publication_error("publish.encode_record"))?,
        };
        let result = store
            .admit_conditionally(ConditionalAdmission {
                admission: AdmissionRequest {
                    key: proposal.key.clone(),
                    admission_key: publication_admission_key(
                        &proposal.key,
                        &proposal.admission_key,
                    ).map_err(publication_error("publish.admission_key"))?,
                    data,
                    events: vec![],
                    effects: vec![],
                },
                conditions: owners,
                before_ms: conditions.before_ms,
            })
            .await;
        let saved = match result {
            Ok(saved) => saved.record().clone(),
            Err(error @ (StoreError::AdmissionConflict | StoreError::AlreadyExists)) => {
                // Only the same immutable association, proposal and owner basis
                // may converge on the existing record. A conflict elsewhere is
                // still a conflict, not a missing-publication result.
                self.store.get(&proposal.key).await
                    .map_err(publication_error("publish.conflict_read"))?
                    .ok_or(error).map_err(publication_error("publish.conflict_missing"))?
            }
            Err(error) => {
                publication_failure("publish.conditional_admission");
                return Err(error.into());
            }
        };
        recover_native(
            self,
            snapshot,
            &effect,
            saved,
            Some((proposal, &basis, &exact_child, exact_helper.as_ref())),
            access.evidence,
            Some((access.references, access.context)),
        )
        .await
    }

    /// Observe the existing immutable preparation after a lost publication reply.
    /// The operation and complete expected destination are independently known
    /// routing facts; no candidate, claim, observation time or winning digest is
    /// required. Current work-content observation authorization still applies;
    /// the complete backend pin must match the host's independent selection.
    /// This method neither prepares, writes, retries nor enables use.
    pub async fn load_prepared_work_for_operation(
        &self,
        operation: &Id,
        destination: &NativeWorkTarget,
        access: SetupAccess<'_>,
        backend: &SetupWorkBackend,
    ) -> Result<PublishedSetupWork, SetupError> {
        let result = self
            .load_native_preparation(
                operation,
                access.evidence,
                Some((access.references, access.context)),
            )
            .await?;
        if &result.record.delegation.destination != destination {
            return Err(SetupError::WrongScope);
        }
        result.record.metadata.check_backend(backend)?;
        Ok(result)
    }

    /// Owner-internal integrity observation for a lost publication reply. The
    /// configured authority owner supplies its independently loaded canonical
    /// parent and exact expected destination. Only the saved child is returned,
    /// including expired history. This is not work-content access or permission
    /// to publish, retry an effect or dispatch anything. Pair
    /// integrity is checked, but this does not attest a current backend choice.
    pub async fn load_prepared_child_for_operation(
        &self,
        operation: &Id,
        destination: &NativeWorkTarget,
        parent: &SetupRequestEvidence,
    ) -> Result<SetupRequestEvidence, SetupError> {
        let result = self
            .load_native_preparation(operation, parent, None)
            .await?;
        if &result.record.delegation.destination != destination {
            return Err(SetupError::WrongScope);
        }
        Ok(result.record.child)
    }

    /// Historical lookup at an independently supplied operation location. This
    /// returns only the original child evidence and can verify expired history. It
    /// is not content observation or current permission. The configured caller
    /// must independently obtain the exact parent from its canonical owner;
    /// there is deliberately no acquisition-copy parent fallback.
    pub async fn load_prepared_child_exact(
        &self,
        operation: &Id,
        reference: &OriginalWorkRef,
        parent: &SetupRequestEvidence,
    ) -> Result<SetupRequestEvidence, SetupError> {
        let result = self
            .load_native_preparation(operation, parent, None)
            .await?;
        if result.child_reference() != reference {
            return Err(SetupError::InvalidRecord);
        }
        Ok(result.record.child)
    }

    /// Read-only exact child lookup for configured application routing. A
    /// current observation authorization is required outside and inside this
    /// call; the canonical original parent is supplied independently by its
    /// actual owner. Never scan for latest or use the acquisition copy as the canonical source.
    /// The selected backend must match the full persisted pin.
    pub async fn load_prepared_work_exact(
        &self,
        operation: &Id,
        reference: &OriginalWorkRef,
        access: SetupAccess<'_>,
        backend: &SetupWorkBackend,
    ) -> Result<PublishedSetupWork, SetupError> {
        let result = self
            .load_native_preparation(
                operation,
                access.evidence,
                Some((access.references, access.context)),
            )
            .await?;
        if result.child_reference() != reference {
            return Err(SetupError::InvalidRecord);
        }
        result.record.metadata.check_backend(backend)?;
        Ok(result)
    }

    // One owner-local read path serves both addressing forms. Historical basis
    // remains evidence, so neither a current Dispatch claim nor old conditions
    // participate. Content observation separately requires current access.
    pub(in crate::setup) async fn load_native_preparation(
        &self,
        operation: &Id,
        parent: &SetupRequestEvidence,
        observation: Option<(&dyn ContextReferenceOwner, &SetupUseContext<'_>)>,
    ) -> Result<PublishedSetupWork, SetupError> {
        let snapshot = self.recover(
            self.store
                .get(&self.scope.key(operation))
                .await?
                .ok_or(StoreError::NotFound)?,
        )?;
        if snapshot.request.evidence() != parent {
            return Err(SetupError::InvalidRecord);
        }
        if let Some((references, context)) = observation {
            snapshot
                .request
                .revalidate_access(parent, context, references)?;
        }
        let effect = self.original_work_effect(&snapshot).await?;
        let saved = self
            .store
            .get(&self.preparation_key(operation))
            .await?
            .ok_or(StoreError::NotFound)?;
        recover_native(self, &snapshot, &effect, saved, None, parent, observation).await
    }
}

fn publication_basis(
    conditions: &[RecordCondition],
    key: &StoreKey,
) -> Result<Vec<PublicationBasis>, SetupError> {
    let mut basis = Vec::new();
    for condition in conditions {
        basis.push(PublicationBasis {
            key: condition.key.clone(),
            revision: condition.revision,
            content_digest: ContentDigest::parse(condition.content_digest.clone())
                .map_err(|_| { publication_failure("basis.content_digest"); SetupError::InvalidRecord })?,
            record_claim: condition.claim.as_ref().map(|lease| ClaimBasis {
                store_id: lease.store_id.clone(),
                worker: lease.worker.clone(),
                fence: lease.fence,
            }),
            dispatch_claim: condition
                .dispatch_claim
                .as_ref()
                .map(|lease| DispatchBasis {
                    claim: ClaimBasis {
                        store_id: lease.store_id.clone(),
                        worker: lease.worker.clone(),
                        fence: lease.fence,
                    },
                    effect: lease.key.id.clone(),
                }),
        });
    }
    basis.sort_by(|a, b| a.key.cmp(&b.key));
    validate_saved_basis_shape(&basis, key)?;
    Ok(basis)
}

fn validate_saved_basis_shape(
    basis: &[PublicationBasis],
    key: &StoreKey,
) -> Result<(), SetupError> {
    // 64 native contributor rows plus helper source, input and acquisition.
    if publication_mismatch("basis.count", !(2..=67).contains(&basis.len())) {
        return Err(SetupError::InvalidRecord);
    }
    let mut prior = None;
    for item in basis {
        item.key.validate().map_err(publication_error("basis.key"))?;
        if publication_mismatch("basis.realm", item.key.realm != key.realm)
            || publication_mismatch("basis.distinct_key", item.key == *key)
            || publication_mismatch("basis.revision_nonzero", item.revision == 0)
            || publication_mismatch("basis.revision_range", i64::try_from(item.revision).is_err())
            || publication_mismatch("basis.unique_order", prior.is_some_and(|previous: &StoreKey| previous >= &item.key))
        {
            return Err(SetupError::InvalidRecord);
        }
        for claim in item
            .record_claim
            .iter()
            .chain(item.dispatch_claim.iter().map(|d| &d.claim))
        {
            // Historical claim generation is fixed; renewed expiry is not.
            if publication_mismatch("basis.claim_store", claim.store_id.is_empty())
                || publication_mismatch("basis.claim_worker", claim.worker.is_empty())
                || publication_mismatch("basis.claim_fence", claim.fence == 0) {
                return Err(SetupError::InvalidRecord);
            }
        }
        prior = Some(&item.key);
    }
    Ok(())
}

fn validate_saved_basis(
    basis: &[PublicationBasis],
    key: &StoreKey,
    acquisition: &StoreKey,
) -> Result<(), SetupError> {
    validate_saved_basis_shape(basis, key)?;
    if !basis
        .iter()
        .any(|item| &item.key == acquisition && item.dispatch_claim.is_some())
    {
        publication_failure("basis.acquisition_dispatch");
        return Err(SetupError::InvalidRecord);
    }
    Ok(())
}

fn validate_preparation(
    store: &SetupStore<'_>,
    snapshot: &SetupSnapshot,
    effect: &StoredEffect,
    preparation: &NativeWorkPreparation,
) -> Result<(), SetupError> {
    if preparation.key != store.preparation_key(&operation_id(snapshot.request())?)
        || &preparation.parent != snapshot.request.original_work()
        || preparation.effect != effect.key
        || preparation.admission_key.as_str() != effect.intent.idempotency_key
        || preparation.work.acquisition != *snapshot.key()
        || preparation.work.effect_digest != effect.request_digest
        || preparation.work.environment_revision != snapshot.request.host().environment_revision
        || preparation.work.input_reference.content_digest
            != ContentDigest::of_bytes(&canonical_json(snapshot.request.request())?)
    {
        return Err(SetupError::InvalidRecord);
    }
    let scope = HostWorkGraphScope::new(
        preparation.work.realm.clone(),
        preparation.work.namespace.clone(),
    )?;
    scope.validate_prepared_request(&preparation.work.request)?;
    preparation.metadata.check_scope(&scope)
}

fn validate_proposal_binding(
    store: &SetupStore<'_>,
    snapshot: &SetupSnapshot,
    effect: &StoredEffect,
    proposal: &NativePreparedWorkProposal,
) -> Result<(), SetupError> {
    if proposal.key != store.preparation_key(&operation_id(snapshot.request())?)
        || proposal.parent != snapshot.request.original_work().clone()
        || proposal.admission_key != effect.intent.idempotency_key
        || proposal.work.acquisition != *snapshot.key()
        || proposal.work.effect_digest != effect.request_digest
        || proposal.work.environment_revision != snapshot.request.host().environment_revision
        || proposal.work.input_reference.content_digest
            != ContentDigest::of_bytes(&canonical_json(snapshot.request.request())?)
        || proposal.delegation.expires_at_ms > snapshot.request.host().limits.deadline_ms
    {
        return Err(SetupError::InvalidRecord);
    }
    let scope =
        HostWorkGraphScope::new(proposal.work.realm.clone(), proposal.work.namespace.clone())?;
    scope.validate_prepared_request(&proposal.work.request)?;
    proposal.metadata.check_scope(&scope)?;
    Ok(())
}

fn validate_child(
    record: &NativePreparedRecord,
    proposal: &NativePreparedWorkProposal,
    parent: &SetupRequestEvidence,
) -> Result<(), SetupError> {
    // Historical correlation only. Native admission authenticates the actual
    // child association and its grants; these comparisons never replace it.
    let p = parent.association().candidate();
    let c = record.child.association().candidate();
    let pc = parent.constraints();
    let cc = record.child.constraints();
    let d = &record.delegation;
    if publication_mismatch("child.parent", &proposal.parent != parent.original_work())
        || publication_mismatch("child.original_destination", c.original_work != d.original_work)
        || publication_mismatch("child.distinct_original", c.original_work == p.original_work)
        || publication_mismatch("child.requester", c.requester != p.requester)
        || publication_mismatch("child.target", c.target != d.destination)
        || publication_mismatch("child.realm", c.ingress_namespace.realm != d.realm_id)
        || publication_mismatch("child.tenant", cc.tenant != pc.tenant)
        || publication_mismatch("child.deployment", cc.deployment != pc.deployment)
        || publication_mismatch("child.policy_revision", cc.policy_revision != pc.policy_revision)
        || publication_mismatch("child.ceiling", cc.ceiling != d.ceiling)
        || publication_mismatch("child.parent_ceiling", !pc.ceiling.contains(&cc.ceiling))
        || publication_mismatch("child.delegate_resource", !pc
            .ceiling
            .permits(crate::authority::Action::Delegate, &d.delegation_resource))
        || publication_mismatch("child.expiry", cc.expires_at_ms != d.expires_at_ms)
        || publication_mismatch("child.parent_expiry", cc.expires_at_ms > pc.expires_at_ms)
        || publication_mismatch("child.issue_order", cc.issued_at_ms < pc.issued_at_ms)
        || publication_mismatch("child.time_interval", cc.issued_at_ms >= cc.expires_at_ms)
    {
        return Err(SetupError::InvalidRecord);
    }
    if let Some(helper) = &record.helper_source {
        validate_helper_source(record, proposal, parent, helper)?;
    } else if publication_mismatch("child.legacy_ingress_actor", c.ingress_actor != p.ingress_actor)
        || publication_mismatch("child.legacy_subject", c.represented_subject != p.represented_subject)
        || publication_mismatch("child.legacy_authentication", c.original_authentication != p.original_authentication)
        || publication_mismatch("child.legacy_event", c.root_event != p.root_event)
    {
        // Preserve the older historical record contract. A new helper must use
        // the explicit helper-source publication path, never copied auth facts.
        return Err(SetupError::InvalidRecord);
    }
    Ok(())
}

fn validate_helper_source(
    record: &NativePreparedRecord,
    proposal: &NativePreparedWorkProposal,
    parent: &SetupRequestEvidence,
    source: &NativeHelperSource,
) -> Result<(), SetupError> {
    let p = parent.association().candidate();
    let c = record.child.association().candidate();
    let identity = format!(
        "setup-helper/{}:{}/{}",
        source.namespace.as_str().len(),
        source.namespace,
        source.item
    );
    let originals: Vec<_> = source
        .contributors
        .iter()
        .map(|original| original.candidate().original_work.clone())
        .collect();
    if publication_mismatch("helper.association", &source.association != record.child.association())
        || publication_mismatch("helper.effect_record", source.effect.record != proposal.work.acquisition)
        || publication_mismatch("helper.effect_id", source.effect.id.is_empty())
        || publication_mismatch("helper.admission_key", source.admission.key.as_str() != proposal.admission_key)
        || publication_mismatch("helper.realm", source.realm != proposal.work.realm.as_str())
        || publication_mismatch("helper.namespace", source.namespace != proposal.work.namespace)
        || publication_mismatch("helper.preparation_request", canonical_json(&source.preparation_request).map_err(publication_error("helper.encode_preparation"))? != canonical_json(&proposal.work.request).map_err(publication_error("helper.encode_proposal_request"))?)
        || publication_mismatch("helper.admitted_realm", source.admitted_request.realm_id.as_deref() != Some(source.realm.as_str()))
        || publication_mismatch("helper.admitted_namespace", source.admitted_request.namespace.as_ref() != Some(&source.namespace))
        || publication_mismatch("helper.ingress", c.ingress_namespace.ingress != p.ingress_namespace.ingress)
        // The helper is owned by this ingress source. Its parent can name a
        // different application source authority within the same ingress.
        || publication_mismatch("helper.original_authority", c.original_work.authority != c.ingress_namespace.ingress.authority)
        || publication_mismatch("helper.original_identity", c.original_work.work.as_str() != identity)
        || publication_mismatch("helper.authentication_domain", c.original_authentication.resource.domain != p.ingress_namespace.ingress)
        || publication_mismatch("helper.authentication_identity", c.original_authentication.resource.resource_id != identity)
        || publication_mismatch("helper.authentication_revision", c.original_authentication.revision.as_str() != "1")
        || publication_mismatch("helper.root_event", c.root_event != c.original_authentication)
        || publication_mismatch("helper.source_realm", source.source.key.realm != proposal.key.realm)
        || publication_mismatch("helper.source_owner", source.source.key.owner
            != format!(
                "toolkit/helper-originals/{}",
                parent.constraints().deployment
            ))
        || publication_mismatch("helper.source_identity", source.source.key.id != identity)
        // The source owner may retain non-admitting preparation before its
        // final binding. Publication still requires this exact observed record
        // condition and the actual authenticated helper association below.
        || publication_mismatch("helper.source_revision", source.source.revision == 0)
        || publication_mismatch("helper.source_claim", source.source.claim.is_some())
        || publication_mismatch("helper.source_dispatch_claim", source.source.dispatch_claim.is_some())
        || publication_mismatch("helper.contributor_count", !(1..=64).contains(&source.contributors.len()))
        || publication_mismatch("helper.parent_contributor", !source
            .contributors
            .iter()
            .any(|original| original == parent.association()))
        || publication_mismatch("helper.contributor_order", c.contributing_work != originals)
    {
        return Err(SetupError::InvalidRecord);
    }
    source.source.key.validate().map_err(publication_error("helper.source_key"))?;
    for (index, original) in source.contributors.iter().enumerate() {
        let contributor = original.candidate();
        if publication_mismatch("helper.contributor_ingress", contributor.ingress_namespace.ingress != p.ingress_namespace.ingress)
            || publication_mismatch("helper.contributor_realm", contributor.ingress_namespace.realm != p.ingress_namespace.realm)
            || publication_mismatch("helper.contributor_requester", contributor.requester != p.requester)
            || publication_mismatch("helper.contributor_not_child", contributor.original_work == c.original_work)
            || publication_mismatch("helper.contributor_unique", originals[..index].contains(&contributor.original_work))
            || publication_mismatch("helper.contributor_ancestry", contributor
                .contributing_work
                .iter()
                .any(|ancestor| !originals.contains(ancestor)))
        {
            return Err(SetupError::InvalidRecord);
        }
    }
    // A root's self-reference names that root; other cycles are contradictory
    // historical ancestry and must not be accepted as complete contributor data.
    let mut marks = vec![0u8; originals.len()];
    fn visit(
        index: usize,
        sources: &[InputAuthorityAssociation],
        originals: &[OriginalWorkRef],
        marks: &mut [u8],
    ) -> bool {
        if marks[index] == 1 {
            return false;
        }
        if marks[index] == 2 {
            return true;
        }
        marks[index] = 1;
        for ancestor in &sources[index].candidate().contributing_work {
            if ancestor == &originals[index] {
                continue;
            }
            let Some(next) = originals.iter().position(|value| value == ancestor) else {
                return false;
            };
            if !visit(next, sources, originals, marks) {
                return false;
            }
        }
        marks[index] = 2;
        true
    }
    for index in 0..originals.len() {
        if publication_mismatch("helper.ancestry_graph", !visit(index, &source.contributors, &originals, &mut marks)) {
            return Err(SetupError::InvalidRecord);
        }
    }
    proposal
        .metadata
        .validate_admission(&source.admitted_request, &source.admission)
        .map_err(publication_error("helper.native_admission"))?;
    if !record.basis.iter().any(|basis| {
        basis.key == source.source.key
            && basis.revision == source.source.revision
            && basis.content_digest.to_string() == source.source.content_digest
            && basis.record_claim.is_none()
            && basis.dispatch_claim.is_none()
    }) {
        publication_failure("helper.source_basis");
        return Err(SetupError::InvalidRecord);
    }
    Ok(())
}

// Keep independently reopened parent evidence separate from current content access.
#[allow(clippy::too_many_arguments)]
async fn recover_native(
    store: &SetupStore<'_>,
    snapshot: &SetupSnapshot,
    effect: &StoredEffect,
    saved: StoredRecord,
    expected: Option<(
        &NativePreparedWorkProposal,
        &[PublicationBasis],
        &SetupRequestEvidence,
        Option<&NativeHelperSource>,
    )>,
    parent: &SetupRequestEvidence,
    observation: Option<(&dyn ContextReferenceOwner, &SetupUseContext<'_>)>,
) -> Result<PublishedSetupWork, SetupError> {
    if saved.data.version != VERSION {
        publication_failure("recover.version");
        return Err(SetupError::UnsupportedRecordVersion);
    }
    if publication_mismatch("recover.key", saved.key != store.preparation_key(&operation_id(snapshot.request()).map_err(publication_error("recover.operation"))?))
        || publication_mismatch("recover.revision", saved.revision != 1)
        || publication_mismatch("recover.schema", saved.data.schema != SCHEMA)
        || publication_mismatch("recover.digest", saved.content_digest != saved.data.digest().map_err(publication_error("recover.encode_digest"))?)
    {
        return Err(SetupError::InvalidRecord);
    }
    let record: NativePreparedRecord = serde_json::from_value(saved.data.body.clone()).map_err(publication_error("recover.decode"))?;
    if publication_mismatch("recover.canonical_record", canonical_json(&record).map_err(publication_error("recover.encode_record"))? != canonical_json(&saved.data.body).map_err(publication_error("recover.encode_body"))?) {
        return Err(SetupError::InvalidRecord);
    }
    let proposal = NativePreparedWorkProposal {
        work: record.work.clone(),
        metadata: record.metadata.clone(),
        delegation: record.delegation.clone(),
        parent: snapshot.request.original_work().clone(),
        key: saved.key.clone(),
        admission_key: effect.intent.idempotency_key.clone(),
        input_record: None,
    };
    validate_proposal_binding(store, snapshot, effect, &proposal)
        .map_err(publication_error("recover.proposal"))?;
    record
        .metadata
        .validate(snapshot, effect, &record.work)
        .await.map_err(publication_error("recover.metadata"))?;
    validate_child(&record, &proposal, parent)?;
    if record
        .helper_source
        .as_ref()
        .is_some_and(|source| publication_mismatch("recover.helper_effect", source.effect != effect.key))
    {
        return Err(SetupError::InvalidRecord);
    }
    validate_saved_basis(&record.basis, &saved.key, snapshot.key())?;
    if !record.basis.iter().any(|item| {
        &item.key == snapshot.key()
            && item
                .dispatch_claim
                .as_ref()
                .is_some_and(|claim| claim.effect == effect.key.id)
    }) {
        publication_failure("recover.dispatch_effect_basis");
        return Err(SetupError::InvalidRecord);
    }
    if let Some((expected, basis, child, helper)) = expected
        && canonical_json(&(
            &expected.work,
            expected.metadata.backend(),
            &expected.delegation,
            &expected.parent,
            child,
            helper,
            basis,
        )).map_err(publication_error("recover.encode_expected"))? != canonical_json(&(
            &proposal.work,
            proposal.metadata.backend(),
            &proposal.delegation,
            &proposal.parent,
            &record.child,
            &record.helper_source,
            &record.basis,
        )).map_err(publication_error("recover.encode_observed"))?
    {
        publication_failure("recover.exact_publication_conflict");
        return Err(StoreError::AdmissionConflict.into());
    }
    if let Some((references, context)) = observation {
        crate::setup::installed_input::verify_work_input_reference(
            snapshot.request(), &record.work.input_reference, parent, context, references,
        ).map_err(publication_error("recover.input_reference"))?;
    }
    Ok(PublishedSetupWork {
        key: saved.key,
        digest: ContentDigest::of_bytes(&canonical_json(&saved.data).map_err(publication_error("recover.encode_saved"))?),
        record,
    })
}

#[cfg(test)]
mod tests;
