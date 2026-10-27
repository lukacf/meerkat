//! Reports from the actual selected helper, recorded by the existing acquisition
//! owner. A report can establish failure/unavailability, never role readiness.
//! The borrowed authorization remains the helper's actual native invocation.

use super::*;
use meerkat_authorization_contracts::work_association::InputAuthorityAssociation;
use toolkit_package::Revision;
use toolkit_store::RecordCondition;

pub const SETUP_RESULTS_BUNDLE: &str = "meerkat_toolkit.setup_results";
pub const SETUP_REPORT: &str = "setup_report";

/// Model content only. These values do not accept operation, requester, source,
/// grant, runtime or routing identities. Success is deliberately not a variant.
/// Detail is a bounded human-facing explanation, never raw tool logs or auth
/// payloads. Validation enforces size and shape, not automatic secret removal.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SetupHelperFailure {
    Failed { detail: String },
    Unavailable { detail: String },
}
impl SetupHelperFailure {
    fn detail(&self) -> &str {
        match self {
            Self::Failed { detail } | Self::Unavailable { detail } => detail,
        }
    }
    fn phase(&self) -> AcquisitionPhase {
        match self {
            Self::Failed { .. } => AcquisitionPhase::Failed,
            Self::Unavailable { .. } => AcquisitionPhase::Unavailable,
        }
    }
    fn validate(&self) -> Result<(), SetupError> {
        if self.detail().trim().is_empty() || self.detail().len() > 4096 {
            return Err(SetupError::InvalidRecord);
        }
        Ok(())
    }
}

/// Historical facts re-opened by the installed source owner for this actual
/// helper invocation. These are not permission tokens. Native operation
/// admission must check the full current helper/original lineage separately.
/// The publication/source/backend fields are never taken from tool arguments.
pub struct SetupHelperResultAccess<'a> {
    pub helper: &'a InputAuthorityAssociation,
    /// Current canonical condition after the installed source owner has checked
    /// immutable helper facts and exact native input correlation. Custody-only
    /// writes may advance it beyond the historical publication condition.
    pub source: &'a RecordCondition,
    pub contributors: &'a [InputAuthorityAssociation],
    /// Exact current source rows paired by the installed source owner with
    /// every canonical contributor, in the same order as `contributors`.
    pub contributor_sources: &'a [(InputAuthorityAssociation, RecordCondition)],
    /// Current approved-task ancestors captured by the same source owner.
    pub ancestor_sources: &'a [RecordCondition],
    pub effect: &'a toolkit_store::EffectKey,
    pub input_digest: &'a ContentDigest,
    pub context: &'a SetupUseContext<'a>,
}

/// Content retained inside the same canonical acquisition record. This is not
/// another result ledger and does not mirror a native receipt or readiness.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HelperDetailRecord {
    publication: StoreKey,
    publication_digest: ContentDigest,
    helper: meerkat_authorization_contracts::work_association::OriginalWorkRef,
    report: SetupHelperFailure,
}
impl HelperDetailRecord {
    pub(super) fn explanation(&self) -> &str {
        self.report.detail()
    }
    fn reference(&self, acquisition: &StoreKey) -> Result<ContextReference, SetupError> {
        Ok(ContextReference {
            owner: Id::new(acquisition.owner.clone()).map_err(|_| SetupError::InvalidRecord)?,
            id: Id::new(format!("{}/helper-detail", acquisition.id))
                .map_err(|_| SetupError::InvalidRecord)?,
            revision: Revision::new("1").map_err(|_| SetupError::InvalidRecord)?,
            content_digest: digest(self)?,
        })
    }
}

pub(super) fn validate_helper_detail(record: &AcquisitionRecord) -> Result<(), SetupError> {
    let Some(detail) = &record.helper_detail else {
        return Ok(());
    };
    detail.report.validate()?;
    detail.publication.validate()?;
    let request = record.request.recover()?;
    let key = SetupScope::new(
        request.constraints().tenant.clone(),
        request.constraints().deployment.clone(),
    )
    .key(&operation_id(&request)?);
    if record.state.lifecycle_phase != detail.report.phase()
        || record.detail.as_ref() != Some(&detail.reference(&key)?)
        || detail.helper == *request.original_work()
    {
        return Err(SetupError::InvalidRecord);
    }
    Ok(())
}

impl SetupStore<'_> {
    /// Record an actual helper's known failure using the original acquisition
    /// owner. Exact replay retains the first canonical detail and terminal row.
    /// The existing record claim/CAS owns cancellation races and bounded cleanup.
    /// No parent WorkAuthorizationContext is constructed or reused here.
    pub async fn resolve_helper_failure(
        &self,
        operation: &Id,
        report: SetupHelperFailure,
        access: SetupHelperResultAccess<'_>,
        backend: &SetupWorkBackend,
        worker: &str,
    ) -> Result<SetupControlReceipt, SetupError> {
        report.validate()?;
        let (snapshot, published) = self
            .helper_result_snapshot(operation, &access, backend)
            .await?;
        let detail = HelperDetailRecord {
            publication: published.key().clone(),
            publication_digest: published.digest().clone(),
            helper: published.child_reference().clone(),
            report,
        };
        if let Some(prior) = &snapshot.record.helper_detail {
            if canonical_json(prior)? != canonical_json(&detail)? {
                return Err(StoreError::AdmissionConflict.into());
            }
            return control_receipt(&snapshot);
        }
        let lease = self
            .store
            .claim_record(snapshot.key(), snapshot.stored.revision, worker, 30_000)
            .await?;
        let result = async {
            // Re-open the same canonical acquisition/publication after claiming.
            // A cancellation or another result wins through the existing CAS.
            let (current, exact) = self
                .helper_result_snapshot(operation, &access, backend)
                .await?;
            if current.stored.revision != snapshot.stored.revision
                || exact.key() != published.key()
                || exact.digest() != published.digest()
            {
                return Err(StoreError::Conflict.into());
            }
            let now =
                u64::try_from(self.store.now_ms().await?).map_err(|_| SetupError::InvalidRecord)?;
            check_helper_context(exact.child(), access.context, now)?;
            let reference = detail.reference(current.key())?;
            let state = current.state();
            let input = match &detail.report {
                SetupHelperFailure::Failed { .. } => AcquisitionInput::ReportFailure {
                    operation_id: state.operation_id.clone(),
                    authority_digest: state.authority_digest.clone(),
                    expected_revision: state.revision,
                    detail_ref: digest(&reference)?.to_string(),
                    now,
                },
                SetupHelperFailure::Unavailable { .. } => AcquisitionInput::ReportUnavailable {
                    operation_id: state.operation_id.clone(),
                    authority_digest: state.authority_digest.clone(),
                    expected_revision: state.revision,
                    detail_ref: digest(&reference)?.to_string(),
                    now,
                },
            };
            let (mut record, effects) = apply(&current, input)?;
            record.detail = Some(reference);
            record.helper_detail = Some(detail);
            // Fence cancellation at the same write boundary as the terminal
            // acquisition and continuation outbox. A later source reread cannot
            // undo a canonical result already committed for a cancelled caller.
            let deadline = current.request().host().limits.deadline_ms
                .min(current.request().constraints().expires_at_ms)
                .min(exact.child().constraints().expires_at_ms);
            let mut direct = vec![access.source.clone()];
            direct.extend(access.contributor_sources.iter().map(|(_, source)| source.clone()));
            let conditions = result_conditions(direct, access.ancestor_sources)?;
            validate_record(&record, current.request())?;
            let (events, effects) = handoffs(&record, effects)?;
            let before_ms = i64::try_from(deadline).map_err(|_| SetupError::InvalidRecord)?;
            let stored = self.conditional_store
                .ok_or(SetupError::NativeExecutionUnavailable)?
                .compare_and_swap_conditionally(toolkit_store::ConditionalTransition {
                    transition: Transition {
                        key: current.key().clone(), expected_revision: current.stored.revision,
                        expected_digest: Some(current.stored.content_digest.clone()),
                        claim: Some(lease.clone()), data: encode_record(&record)?, events, effects,
                    },
                    conditions, before_ms,
                }).await?;
            let saved = self.recover(stored)?;
            control_receipt(&saved)
        }
        .await;
        // A dropped callback retains only this bounded store claim. There is no
        // detached cleanup task or second helper/result lifecycle.
        let released = self.store.release_record_claim(&lease).await;
        match result {
            Ok(value) => {
                released?;
                Ok(value)
            }
            Err(error) => Err(error),
        }
    }

    /// Reopen the exact installed helper publication and parent task. The
    /// returned snapshot is historical content, not an operation permission.
    pub async fn helper_result_snapshot(
        &self,
        operation: &Id,
        access: &SetupHelperResultAccess<'_>,
        backend: &SetupWorkBackend,
    ) -> Result<(SetupSnapshot, PublishedSetupWork), SetupError> {
        let stored = self
            .store
            .get(&self.scope.key(operation))
            .await?
            .ok_or(StoreError::NotFound)?;
        let snapshot = self.recover(stored)?;
        // The actual source owner supplies the complete canonical original set.
        // The acquisition's historical copy is only compared with that set.
        let parent = snapshot.request.evidence();
        if !access
            .contributors
            .iter()
            .any(|source| source == parent.association())
        {
            return Err(SetupError::Denied);
        }
        let published = self
            .load_native_preparation(operation, parent, None)
            .await?;
        let source = published.helper_source().ok_or(SetupError::Denied)?;
        if &source.association != access.helper
            || source.source.key != access.source.key
            || access.source.revision < source.source.revision
            || (access.source.revision == source.source.revision
                && access.source.content_digest != source.source.content_digest)
            || access.source.claim.is_some()
            || access.source.dispatch_claim.is_some()
            || source.contributors != access.contributors
            || source.effect != *access.effect
            || &published.input_reference().content_digest != access.input_digest
            || source.effect.record != *snapshot.key()
            || source.effect != self.original_work_effect(&snapshot).await?.key
            || published.backend_pin() != backend.pin()
        {
            return Err(SetupError::Denied);
        }
        if access.contributor_sources.len() != access.contributors.len() {
            return Err(SetupError::Denied);
        }
        for (index, (association, expected)) in access.contributor_sources.iter().enumerate() {
            if association != &access.contributors[index]
                || expected.key == access.source.key
                || access.contributor_sources[..index].iter().any(|(_, prior)| prior.key == expected.key)
                || expected.claim.is_some() || expected.dispatch_claim.is_some()
            {
                return Err(SetupError::Denied);
            }
        }
        // Publication remains historical. Reopen every captured source and
        // approved-task ancestor; the final transaction checks these same rows.
        let mut direct = vec![access.source.clone()];
        direct.extend(access.contributor_sources.iter().map(|(_, source)| source.clone()));
        for expected in result_conditions(direct, access.ancestor_sources)? {
            let actual = self.store.get(&expected.key).await?.ok_or(StoreError::NotFound)?;
            if actual.revision != expected.revision
                || actual.content_digest != expected.content_digest
                || actual.content_digest != actual.data.digest()?
            {
                return Err(SetupError::InvalidRecord);
            }
        }
        let now =
            u64::try_from(self.store.now_ms().await?).map_err(|_| SetupError::InvalidRecord)?;
        check_helper_context(published.child(), access.context, now)?;
        Ok((snapshot, published))
    }
}

// Multiple callers may share an approved-task ancestor. Identical predicates
// collapse; conflicting snapshots of the same row must never select a winner.
fn result_conditions(
    mut direct: Vec<RecordCondition>, ancestors: &[RecordCondition],
) -> Result<Vec<RecordCondition>, SetupError> {
    direct.extend_from_slice(ancestors);
    let mut conditions: Vec<RecordCondition> = Vec::with_capacity(direct.len());
    for condition in direct {
        if condition.claim.is_some() || condition.dispatch_claim.is_some() {
            return Err(SetupError::Denied);
        }
        if let Some(prior) = conditions.iter().find(|prior| prior.key == condition.key) {
            if prior != &condition { return Err(SetupError::Denied); }
        } else {
            conditions.push(condition);
        }
    }
    Ok(conditions)
}

fn check_helper_context(
    child: &SetupRequestEvidence,
    context: &SetupUseContext<'_>,
    now_ms: u64,
) -> Result<(), SetupError> {
    child
        .check_context(&SetupUseContext {
            tenant: context.tenant,
            deployment: context.deployment,
            realm: context.realm,
            target: context.target,
            policy_revision: context.policy_revision,
            now_ms,
            authorization: context.authorization,
        })
        .map_err(RequestError::from)?;
    Ok(())
}
fn control_receipt(snapshot: &SetupSnapshot) -> Result<SetupControlReceipt, SetupError> {
    Ok(SetupControlReceipt {
        operation: Id::new(snapshot.key().id.clone()).map_err(|_| SetupError::InvalidRecord)?,
        revision: snapshot.state().revision,
        phase: snapshot.state().lifecycle_phase,
    })
}
