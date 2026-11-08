//! Dormant pre-spawn workflow obligations in the existing SetupStore.
//!
//! One record CAS owns membership, sealing and existing outbox effects. It
//! does not admit a native lead, copy context, grant permission, cancel an
//! acquisition or acknowledge a result. The future installed source owner must
//! supply current host grouping/disclosure authority and the native entry fence.
use super::*;
use meerkat_core::{MobMemberBinding, RealmId, auth::PrincipalRef};
use meerkat_workgraph::{WorkItemRef, WorkOwnerKey};
use std::collections::BTreeMap;
use toolkit_package::Revision;
use toolkit_store::{ConditionalAdmission, ConditionalTransition, EffectKey, RecordCondition};

const SCHEMA: &str = "meerkat_toolkit.setup_workflow";
const MAX_WAITERS: usize = 128;
const CAS_ATTEMPTS: usize = 16;

/// Immutable host-selected facts, not a permission or a model-selected group.
/// `key` names one logical workflow in its SetupScope. Changed terms at that
/// same address conflict instead of silently creating another workflow.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupWorkflowTerms {
    pub key: Id,
    pub backend: SetupWorkBackendPin,
    pub parent: WorkItemRef,
    pub parent_owner: WorkOwnerKey,
    pub cycle_label: String,
    pub requester: PrincipalRef,
    pub realm: RealmId,
    pub authority_revision: Revision,
    pub planning_source: StoreKey,
    pub lead: MobMemberBinding,
    /// Digest of the host's frozen eligible destination/operation envelope.
    /// Equality binds terms only; the actual owner must authorize that envelope.
    pub envelope_digest: ContentDigest,
    pub deadline_ms: u64,
    pub max_waiters: u16,
}

/// A scoped address and exact immutable terms digest. Decoding this value
/// restores no live source custody or permission to launch or disclose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupWorkflowRef {
    key: StoreKey,
    terms_digest: ContentDigest,
}
impl SetupWorkflowRef {
    pub fn key(&self) -> &StoreKey {
        &self.key
    }
    pub fn terms_digest(&self) -> &ContentDigest {
        &self.terms_digest
    }
    pub fn launch_effect(&self) -> EffectKey {
        effect_key(self, "launch".into())
    }
    pub fn close_effect(&self) -> EffectKey {
        effect_key(self, "close".into())
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WaiterIdentity {
    acquisition: StoreKey,
    input: ContextReference,
    input_digest: ContentDigest,
    authority_digest: ContentDigest,
    source: StoreKey,
    work: SetupWorkContext,
    deadline_ms: u64,
}

/// Exact identity of one membership, retained before the join await so its
/// existing owner can recover a lost response. This is not disclosure custody.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupWorkflowWaiter {
    workflow: SetupWorkflowRef,
    identity: WaiterIdentity,
}
impl SetupWorkflowWaiter {
    pub fn workflow(&self) -> &SetupWorkflowRef {
        &self.workflow
    }
    pub fn acquisition(&self) -> &StoreKey {
        &self.identity.acquisition
    }
    pub fn input(&self) -> &ContextReference {
        &self.identity.input
    }
    pub fn source(&self) -> &StoreKey {
        &self.identity.source
    }
    pub fn deadline_ms(&self) -> u64 {
        self.identity.deadline_ms
    }
    pub fn delivery_effect(&self) -> Result<EffectKey, SetupError> {
        Ok(effect_key(
            &self.workflow,
            format!("deliver-{}", digest(&self.identity.acquisition)?),
        ))
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Waiter {
    identity: WaiterIdentity,
    departed: bool,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Workflow {
    terms: SetupWorkflowTerms,
    waiters: BTreeMap<String, Waiter>,
    sealed: bool,
}

/// Historical orchestration snapshot. Neither `sealed` nor a succeeded outbox
/// effect establishes native cleanup, approval or an acquisition outcome.
pub struct SetupWorkflowSnapshot {
    reference: SetupWorkflowRef,
    stored: StoredRecord,
    record: Workflow,
}
impl SetupWorkflowSnapshot {
    /// Exact historical row condition for the existing source publication CAS.
    /// This is correlation only; native source custody remains independently checked.
    pub fn condition(&self) -> toolkit_store::RecordCondition {
        toolkit_store::RecordCondition {
            key: self.stored.key.clone(),
            revision: self.stored.revision,
            content_digest: self.stored.content_digest.clone(),
            claim: None,
            dispatch_claim: None,
        }
    }
    pub fn reference(&self) -> &SetupWorkflowRef {
        &self.reference
    }
    pub fn revision(&self) -> u64 {
        self.stored.revision
    }
    pub fn terms(&self) -> &SetupWorkflowTerms {
        &self.record.terms
    }
    pub fn sealed(&self) -> bool {
        self.record.sealed
    }
    pub fn waiter_count(&self) -> usize {
        self.record.waiters.len()
    }
    pub fn active_waiters(&self) -> usize {
        self.record
            .waiters
            .values()
            .filter(|waiter| !waiter.departed)
            .count()
    }
    /// Canonical bounded membership census for the actual source owner after
    /// cancellation. Includes departed rows for idempotent cleanup. These
    /// identities remain historical facts, never live disclosure or authority.
    pub fn waiters_for_source(&self, source: &StoreKey) -> Vec<SetupWorkflowWaiter> {
        self.record
            .waiters
            .values()
            .filter(|waiter| &waiter.identity.source == source)
            .map(|waiter| SetupWorkflowWaiter {
                workflow: self.reference.clone(),
                identity: waiter.identity.clone(),
            })
            .collect()
    }

    pub fn departed(&self, waiter: &SetupWorkflowWaiter) -> Result<bool, SetupError> {
        Ok(exact_waiter(self, waiter)?.departed)
    }
}

pub struct SetupWorkflowJoin {
    /// False is an exact historical replay, never a fresh dispatch permit.
    pub joined: bool,
    pub snapshot: SetupWorkflowSnapshot,
}

impl SetupStore<'_> {
    pub fn workflow_ref(&self, terms: &SetupWorkflowTerms) -> Result<SetupWorkflowRef, SetupError> {
        self.check_workflow_terms(terms)?;
        Ok(SetupWorkflowRef {
            key: StoreKey {
                realm: self.scope.tenant.to_string(),
                owner: format!("toolkit/acquisition/{}/workflow", self.scope.deployment),
                id: terms.key.to_string(),
            },
            terms_digest: digest(terms)?,
        })
    }

    /// Freeze the exact canonical input locator and independently retained
    /// source/work identity. The caller's real source owner still authorizes
    /// grouping; equality checks here are not native authentication.
    pub fn workflow_waiter(
        &self,
        terms: &SetupWorkflowTerms,
        snapshot: &SetupSnapshot,
        source: StoreKey,
    ) -> Result<SetupWorkflowWaiter, SetupError> {
        self.check_snapshot_scope(snapshot)?;
        source.validate()?;
        let request = snapshot.request();
        let claims = request.evidence().association().candidate();
        let work = request
            .host()
            .work_context
            .as_ref()
            .ok_or(SetupError::Denied)?;
        work.check_structure()?;
        if !same_group(terms, work)
            || claims.requester != terms.requester
            || claims.ingress_namespace.realm != terms.realm
            || request.constraints().policy_revision != terms.authority_revision
            || source.realm != self.scope.tenant.as_str()
            || request.host().limits.deadline_ms > terms.deadline_ms
        {
            return Err(SetupError::WrongScope);
        }
        Ok(SetupWorkflowWaiter {
            workflow: self.workflow_ref(terms)?,
            identity: WaiterIdentity {
                acquisition: snapshot.key().clone(),
                input: canonical_setup_input_reference(request)?,
                input_digest: request.input_digest().clone(),
                authority_digest: request.authority_digest()?,
                source,
                work: work.clone(),
                deadline_ms: request.host().limits.deadline_ms,
            },
        })
    }

    /// Metadata-only recovery read for an already selected waiter. This does
    /// not expose the saved request or replace its live Find authorization.
    /// The caller must obtain the expected association from its native source
    /// owner and separately serialize current interest with cancellation.
    pub async fn workflow_waiter_recovery_snapshot(
        &self,
        waiter: &SetupWorkflowWaiter,
        expected: &meerkat_authorization_contracts::work_association::InputAuthorityAssociation,
    ) -> Result<(SetupWorkflowSnapshot, Id), SetupError> {
        let (workflow, setup) = self
            .workflow_waiter_recovery_setup(waiter, expected)
            .await?;
        Ok((workflow, setup.request().host().correlation.clone()))
    }

    /// The exact canonical acquisition retained by an eligible recovery row.
    /// This supplies historical request bytes, not permission to disclose or
    /// dispatch them. The source owner must fence live membership and entry.
    pub async fn workflow_waiter_recovery_setup(
        &self,
        waiter: &SetupWorkflowWaiter,
        expected: &meerkat_authorization_contracts::work_association::InputAuthorityAssociation,
    ) -> Result<(SetupWorkflowSnapshot, SetupSnapshot), SetupError> {
        let workflow = self.load_setup_workflow(waiter.workflow()).await?;
        // Validate membership before following its exact acquisition locator.
        exact_waiter(&workflow, waiter)?;
        let stored = self
            .store
            .get(waiter.acquisition())
            .await?
            .ok_or(StoreError::NotFound)?;
        let setup = self.recover(stored)?;
        if setup.request().evidence().association() != expected
            || self.workflow_waiter(workflow.terms(), &setup, waiter.source().clone())? != *waiter
        {
            return Err(SetupError::Denied);
        }
        if !matches!(
            setup.state().lifecycle_phase,
            AcquisitionPhase::Investigating | AcquisitionPhase::Preparing
        ) || workflow.departed(waiter)?
        {
            return Err(SetupError::Denied);
        }
        Ok((workflow, setup))
    }

    /// Reopen the exact current waiter for the installed service-task owner.
    /// This read creates no authorization, helper publication or result effect.
    pub async fn workflow_service_task_snapshot(
        &self,
        waiter: &SetupWorkflowWaiter,
        expected: &meerkat_authorization_contracts::work_association::InputAuthorityAssociation,
        backend: &SetupWorkBackend,
    ) -> Result<SetupSnapshot, SetupError> {
        let workflow = self.load_setup_workflow(waiter.workflow()).await?;
        if &workflow.terms().backend != backend.pin() {
            return Err(SetupError::Denied);
        }
        exact_waiter(&workflow, waiter)?;
        let stored = self
            .store
            .get(waiter.acquisition())
            .await?
            .ok_or(StoreError::NotFound)?;
        let snapshot = self.recover(stored)?;
        if snapshot.request().evidence().association() != expected
            || self.workflow_waiter(workflow.terms(), &snapshot, waiter.source().clone())?
                != *waiter
            || workflow.departed(waiter)?
            || !matches!(
                snapshot.state().lifecycle_phase,
                AcquisitionPhase::Investigating | AcquisitionPhase::Preparing
            )
        {
            return Err(SetupError::Denied);
        }
        Ok(snapshot)
    }

    /// Compare a private committed inline envelope with this owner's exact
    /// canonical waiter and acquisition. This returns no request or permission;
    /// callers separately own recipient access and current consumption checks.
    /// Donor departure, expiry or terminal cancellation does not recall an
    /// already completed disclosure.
    pub async fn validate_committed_workflow_context(
        &self,
        waiter: &SetupWorkflowWaiter,
        expected: &meerkat_authorization_contracts::work_association::InputAuthorityAssociation,
        correlation: &Id,
        input_digest: &ContentDigest,
        request: &FindAndInstallRequest,
        deadline_ms: u64,
        output_audiences: &BTreeSet<Id>,
    ) -> Result<SetupWorkflowSnapshot, SetupError> {
        let workflow = self.load_setup_workflow(waiter.workflow()).await?;
        exact_waiter(&workflow, waiter)?;
        let stored = self
            .store
            .get(waiter.acquisition())
            .await?
            .ok_or(StoreError::NotFound)?;
        let setup = self.recover(stored)?;
        let recorded = setup.request();
        if recorded.evidence().association() != expected
            || self.workflow_waiter(workflow.terms(), &setup, waiter.source().clone())? != *waiter
            || &recorded.host().correlation != correlation
            || recorded.input_digest() != input_digest
            || recorded.request() != request
            || recorded.host().limits.deadline_ms != deadline_ms
            || output_audiences.is_empty()
            || !output_audiences.is_subset(&recorded.constraints().ceiling.output_audiences)
        {
            return Err(SetupError::Denied);
        }
        Ok(workflow)
    }

    /// Commit membership and its delivery obligation together. The first
    /// admission also commits exactly one immutable launch intent. Native
    /// launch/delivery are deliberately not called by this store operation.
    ///
    /// Durable reads and WorkGraph observation are async and occur outside any
    /// native synchronous publication callback. The future source owner must
    /// serialize its own grouping/cancellation custody and fence actual entry;
    /// acquisition and workflow commits are not a cross-record transaction.
    pub async fn join_setup_workflow(
        &self,
        terms: &SetupWorkflowTerms,
        waiter: &SetupWorkflowWaiter,
        access: SetupAccess<'_>,
        backend: &SetupWorkBackend,
    ) -> Result<SetupWorkflowJoin, SetupError> {
        self.join_workflow(terms, waiter, access, backend, None)
            .await
    }

    /// Installed source-owned join. Both first admission and later membership
    /// CAS check the exact source and acquisition rows plus the waiter deadline
    /// in the same commit. Requires the explicitly supplied atomic adapter;
    /// ordinary ToolkitStore capability flags are not sufficient.
    ///
    /// The source owner must supply the current authenticated A row and fence
    /// its local cancellation gate. A committed historical replay creates no
    /// new obligation and does not reauthorize launch or context delivery.
    pub async fn join_setup_workflow_conditionally(
        &self,
        terms: &SetupWorkflowTerms,
        waiter: &SetupWorkflowWaiter,
        access: SetupAccess<'_>,
        backend: &SetupWorkBackend,
        source: RecordCondition,
    ) -> Result<SetupWorkflowJoin, SetupError> {
        self.conditional_store
            .ok_or(SetupError::NativeExecutionUnavailable)?;
        if source.key != *waiter.source()
            || source.claim.is_some()
            || source.dispatch_claim.is_some()
        {
            return Err(SetupError::WrongScope);
        }
        self.join_workflow(terms, waiter, access, backend, Some(source))
            .await
    }

    async fn join_workflow(
        &self,
        terms: &SetupWorkflowTerms,
        waiter: &SetupWorkflowWaiter,
        access: SetupAccess<'_>,
        backend: &SetupWorkBackend,
        source: Option<RecordCondition>,
    ) -> Result<SetupWorkflowJoin, SetupError> {
        if self.workflow_ref(terms)? != waiter.workflow {
            return Err(SetupError::WrongScope);
        }
        let stored = self
            .store
            .get(waiter.acquisition())
            .await?
            .ok_or(StoreError::NotFound)?;
        let acquisition = self.recover(stored)?;
        self.check_snapshot(&acquisition, &access, true)?;
        if self.workflow_waiter(terms, &acquisition, waiter.identity.source.clone())? != *waiter {
            return Err(StoreError::AdmissionConflict.into());
        }
        if !matches!(
            acquisition.state().lifecycle_phase,
            AcquisitionPhase::Investigating | AcquisitionPhase::Preparing
        ) || access.context.now_ms >= terms.deadline_ms
        {
            return Err(SetupError::Rejected);
        }
        waiter.identity.work.check_current(backend).await?;
        let conditions = source.map(|source| vec![source, acquisition.condition()]);
        let before_ms = i64::try_from(waiter.deadline_ms().min(terms.deadline_ms))
            .map_err(|_| SetupError::InvalidSelection)?;
        for _ in 0..CAS_ATTEMPTS {
            match self.store.get(waiter.workflow.key()).await? {
                Some(stored) => {
                    let prior = self.decode_workflow(stored, &waiter.workflow)?;
                    if prior.record.terms != *terms {
                        return Err(StoreError::AdmissionConflict.into());
                    }
                    if let Some(existing) =
                        prior.record.waiters.get(&waiter.identity.acquisition.id)
                    {
                        if existing.identity != waiter.identity {
                            return Err(StoreError::AdmissionConflict.into());
                        }
                        // Historical replay cannot reopen a departed or sealed
                        // record, create an effect or confer permission.
                        return Ok(SetupWorkflowJoin {
                            joined: false,
                            snapshot: prior,
                        });
                    }
                    if prior.record.sealed
                        || prior.record.waiters.len() >= usize::from(terms.max_waiters)
                    {
                        return Err(SetupError::Rejected);
                    }
                    let mut record = prior.record.clone();
                    record.waiters.insert(
                        waiter.identity.acquisition.id.clone(),
                        Waiter {
                            identity: waiter.identity.clone(),
                            departed: false,
                        },
                    );
                    match self
                        .workflow_commit_with_conditions(
                            &prior,
                            record,
                            "waiter_joined",
                            vec![delivery_intent(waiter)?],
                            conditions.as_deref(),
                            before_ms,
                        )
                        .await
                    {
                        Ok(snapshot) => {
                            return Ok(SetupWorkflowJoin {
                                joined: true,
                                snapshot,
                            });
                        }
                        Err(SetupError::Store(StoreError::Conflict)) => continue,
                        Err(error) => return Err(error),
                    }
                }
                None => {
                    let record = Workflow {
                        terms: terms.clone(),
                        waiters: [(
                            waiter.identity.acquisition.id.clone(),
                            Waiter {
                                identity: waiter.identity.clone(),
                                departed: false,
                            },
                        )]
                        .into(),
                        sealed: false,
                    };
                    let data = encode(&record)?;
                    let admission = AdmissionRequest {
                        key: waiter.workflow.key.clone(),
                        admission_key: waiter.workflow.key.id.clone(),
                        data: data.clone(),
                        events: vec![EventWrite {
                            kind: "workflow_admitted".into(),
                            data,
                        }],
                        effects: vec![
                            intent(&waiter.workflow, "launch".into(), "launch", &record.terms)?,
                            delivery_intent(waiter)?,
                        ],
                    };
                    let outcome = match &conditions {
                        Some(conditions) => {
                            self.conditional_store
                                .ok_or(SetupError::NativeExecutionUnavailable)?
                                .admit_conditionally(ConditionalAdmission {
                                    admission,
                                    conditions: conditions.clone(),
                                    before_ms,
                                })
                                .await
                        }
                        None => self.store.admit(admission).await,
                    };
                    match outcome {
                        Ok(AdmissionOutcome::Created(stored)) => {
                            return Ok(SetupWorkflowJoin {
                                joined: true,
                                snapshot: self.decode_workflow(stored, &waiter.workflow)?,
                            });
                        }
                        Ok(AdmissionOutcome::Existing(_))
                        | Err(StoreError::AdmissionConflict | StoreError::AlreadyExists) => {
                            continue;
                        }
                        Err(error) => return Err(error.into()),
                    }
                }
            }
        }
        Err(StoreError::Conflict.into())
    }

    /// Observe only orchestration facts. Native receipts stay in the existing
    /// effect rows and must be authenticated by their eventual delivery owner.
    pub async fn load_setup_workflow(
        &self,
        reference: &SetupWorkflowRef,
    ) -> Result<SetupWorkflowSnapshot, SetupError> {
        let stored = self
            .store
            .get(reference.key())
            .await?
            .ok_or(StoreError::NotFound)?;
        self.decode_workflow(stored, reference)
    }

    /// Called by the existing source owner after it fences this exact waiter.
    /// This records departure only; it does not cancel that source or infer a
    /// native outcome. Last departure seals and adds one cleanup obligation in
    /// the same CAS. The actual cleanup owner must later settle that effect.
    /// Pending launch/delivery intents remain evidence; departure grants no
    /// permission to execute them. The future entry owner must fence dispatch.
    pub async fn depart_setup_workflow(
        &self,
        waiter: &SetupWorkflowWaiter,
    ) -> Result<SetupWorkflowSnapshot, SetupError> {
        for _ in 0..CAS_ATTEMPTS {
            let prior = self.load_setup_workflow(&waiter.workflow).await?;
            if exact_waiter(&prior, waiter)?.departed {
                return Ok(prior);
            }
            let mut record = prior.record.clone();
            record
                .waiters
                .get_mut(&waiter.identity.acquisition.id)
                .ok_or(SetupError::InvalidRecord)?
                .departed = true;
            let effects = if record.waiters.values().all(|waiter| waiter.departed) {
                record.sealed = true;
                vec![intent(
                    &waiter.workflow,
                    "close".into(),
                    "close",
                    &waiter.workflow,
                )?]
            } else {
                vec![]
            };
            match self
                .workflow_commit(&prior, record, "waiter_departed", effects)
                .await
            {
                Ok(snapshot) => return Ok(snapshot),
                Err(SetupError::Store(StoreError::Conflict)) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(StoreError::Conflict.into())
    }

    /// Stop accepting new waiters without treating active members as departed.
    /// Sealing is monotonic and does not trigger early cleanup of those members.
    pub async fn seal_setup_workflow(
        &self,
        reference: &SetupWorkflowRef,
    ) -> Result<SetupWorkflowSnapshot, SetupError> {
        for _ in 0..CAS_ATTEMPTS {
            let prior = self.load_setup_workflow(reference).await?;
            if prior.record.sealed {
                return Ok(prior);
            }
            let mut record = prior.record.clone();
            record.sealed = true;
            match self
                .workflow_commit(&prior, record, "workflow_sealed", vec![])
                .await
            {
                Ok(snapshot) => return Ok(snapshot),
                Err(SetupError::Store(StoreError::Conflict)) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(StoreError::Conflict.into())
    }

    fn check_workflow_terms(&self, terms: &SetupWorkflowTerms) -> Result<(), SetupError> {
        terms.planning_source.validate()?;
        let text = |value: &str| {
            !value.is_empty()
                && value.len() <= 128
                && value.trim() == value
                && !value.chars().any(char::is_control)
        };
        if terms.planning_source.realm != self.scope.tenant.as_str()
            || terms.parent.realm_id != terms.backend.realm().as_str()
            || &terms.parent.namespace != terms.backend.namespace()
            || WorkOwnerKey::new(terms.parent_owner.kind, terms.parent_owner.id.clone()).is_err()
            || !text(&terms.cycle_label)
            || !text(&terms.lead.mob_id)
            || !text(&terms.lead.role)
            || !text(&terms.lead.member)
            || terms.deadline_ms == 0
            || terms.max_waiters == 0
            || usize::from(terms.max_waiters) > MAX_WAITERS
        {
            return Err(SetupError::InvalidSelection);
        }
        Ok(())
    }

    fn decode_workflow(
        &self,
        stored: StoredRecord,
        reference: &SetupWorkflowRef,
    ) -> Result<SetupWorkflowSnapshot, SetupError> {
        if stored.key != reference.key
            || stored.data.schema != SCHEMA
            || stored.data.version != 1
            || stored.content_digest != stored.data.digest()?
        {
            return Err(SetupError::InvalidRecord);
        }
        let record: Workflow = serde_json::from_value(stored.data.body.clone())?;
        if self.workflow_ref(&record.terms)? != *reference {
            return Err(StoreError::AdmissionConflict.into());
        }
        if record.waiters.is_empty()
            || record.waiters.len() > usize::from(record.terms.max_waiters)
            || !record.sealed && record.waiters.values().all(|waiter| waiter.departed)
        {
            return Err(SetupError::InvalidRecord);
        }
        for (id, waiter) in &record.waiters {
            let identity = &waiter.identity;
            identity.source.validate()?;
            identity.work.check_structure()?;
            if &identity.acquisition.id != id
                || identity.acquisition
                    != self
                        .scope
                        .key(&Id::new(id.clone()).map_err(|_| SetupError::InvalidRecord)?)
                || identity.input.owner.as_str() != identity.acquisition.owner
                || identity.input.id.as_str() != id
                || identity.input.revision.as_str() != format!("input/{}", identity.input_digest)
                || identity.source.realm != self.scope.tenant.as_str()
                || !same_group(&record.terms, &identity.work)
                || identity.deadline_ms == 0
                || identity.deadline_ms > record.terms.deadline_ms
            {
                return Err(SetupError::InvalidRecord);
            }
        }
        Ok(SetupWorkflowSnapshot {
            reference: reference.clone(),
            stored,
            record,
        })
    }

    async fn workflow_commit(
        &self,
        prior: &SetupWorkflowSnapshot,
        record: Workflow,
        kind: &str,
        effects: Vec<EffectIntent>,
    ) -> Result<SetupWorkflowSnapshot, SetupError> {
        self.workflow_commit_with_conditions(prior, record, kind, effects, None, 0)
            .await
    }

    async fn workflow_commit_with_conditions(
        &self,
        prior: &SetupWorkflowSnapshot,
        record: Workflow,
        kind: &str,
        effects: Vec<EffectIntent>,
        conditions: Option<&[RecordCondition]>,
        before_ms: i64,
    ) -> Result<SetupWorkflowSnapshot, SetupError> {
        let data = encode(&record)?;
        let transition = Transition {
            key: prior.reference.key.clone(),
            expected_revision: prior.stored.revision,
            expected_digest: Some(prior.stored.content_digest.clone()),
            claim: None,
            data: data.clone(),
            events: vec![EventWrite {
                kind: kind.into(),
                data,
            }],
            effects,
        };
        let stored = match conditions {
            Some(conditions) => {
                self.conditional_store
                    .ok_or(SetupError::NativeExecutionUnavailable)?
                    .compare_and_swap_conditionally(ConditionalTransition {
                        transition,
                        conditions: conditions.to_vec(),
                        before_ms,
                    })
                    .await?
            }
            None => self.store.compare_and_swap(transition).await?,
        };
        self.decode_workflow(stored, &prior.reference)
    }
}

fn same_group(terms: &SetupWorkflowTerms, work: &SetupWorkContext) -> bool {
    terms.backend == *work.backend()
        && terms.parent == *work.parent()
        && &terms.parent_owner == work.parent_owner()
        && terms.cycle_label == work.cycle_label()
}
fn exact_waiter<'a>(
    snapshot: &'a SetupWorkflowSnapshot,
    waiter: &SetupWorkflowWaiter,
) -> Result<&'a Waiter, SetupError> {
    if snapshot.reference != waiter.workflow {
        return Err(SetupError::WrongScope);
    }
    let actual = snapshot
        .record
        .waiters
        .get(&waiter.identity.acquisition.id)
        .ok_or(SetupError::Denied)?;
    if actual.identity != waiter.identity {
        return Err(SetupError::WrongScope);
    }
    Ok(actual)
}
fn encode(record: &Workflow) -> Result<RecordData, SetupError> {
    Ok(RecordData {
        schema: SCHEMA.into(),
        version: 1,
        body: serde_json::to_value(record)?,
    })
}
fn effect_key(reference: &SetupWorkflowRef, id: String) -> EffectKey {
    EffectKey {
        record: reference.key.clone(),
        id,
    }
}
fn intent(
    reference: &SetupWorkflowRef,
    id: String,
    kind: &str,
    payload: &impl Serialize,
) -> Result<EffectIntent, SetupError> {
    let request = RecordData {
        schema: format!("{SCHEMA}.{kind}"),
        version: 1,
        body: serde_json::json!({"workflow": reference, "payload": payload}),
    };
    Ok(EffectIntent {
        id,
        kind: format!("setup_workflow_{kind}"),
        idempotency_key: digest(&request)?.to_string(),
        request,
        replay_safety: ReplaySafety::ReconcileBeforeRetry,
    })
}
fn delivery_intent(waiter: &SetupWorkflowWaiter) -> Result<EffectIntent, SetupError> {
    intent(
        &waiter.workflow,
        waiter.delivery_effect()?.id,
        "delivery",
        waiter,
    )
}
