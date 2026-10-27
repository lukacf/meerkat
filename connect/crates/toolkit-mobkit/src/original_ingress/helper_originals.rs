//! Internal helper originals authenticated by the actual WorkGraph admission.
//!
//! This is part of the existing source owner. A prepared-work row, an item
//! copied from another backend, or a frontend-shaped event cannot enter here.
//! The native admission fence remains supplied by its real authority owner.
//! Work admission and durable source publication confer no input custody.

use super::*;
use meerkat::governed_realm::helper_admission::{
    GovernedHelperAdmissionFence, HelperAdmissionRefusal,
};
use meerkat_authorization::work::RetainedOriginal;
use meerkat_authorization_contracts::{
    evidence::EvidenceDigest,
    grant::GrantLineageRef,
    protocol::ContractRequirements,
    resource::ResourceRef,
    work_association::{InputAuthorityAssociationCandidate, NativeWorkTarget},
};
use meerkat_core::{ToolDispatchContext, types::ToolCallView};
use meerkat_workgraph::{
    CreateWorkItemRequest, ExternalWorkRef, WorkAdmission, WorkAdmissionKey, WorkAdmissionOutcome,
    WorkGraphAdmissionFence, WorkGraphError, WorkGraphService, WorkItemId, WorkNamespace,
};
use serde::{Deserialize, Serialize};
use toolkit_package::{ContentDigest, canonical_json};
use toolkit_store::{
    AdmissionRequest, EffectKey, RecordCondition, RecordData, StoreKey, StoreScope, StoredRecord,
    ToolkitStore,
};

const SCHEMA: &str = "meerkat_toolkit.helper_original";
const SOURCE_VERSION: u32 = 3;
const CUSTODY_SOURCE_VERSION: u32 = 4;
const RETAINED_INPUT_SOURCE_VERSION: u32 = 5;

mod controller;
mod settlement;
pub(crate) use controller::HelperControllerIntent;
const CORRELATION: &str = "meerkat_toolkit.helper_source.v1";

/// The original acquisition/effect owner selects these exact coordinates.
/// This value carries historical identity, not a native operation permit.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HelperOperation {
    pub acquisition: StoreKey,
    pub effect: String,
    pub input_digest: ContentDigest,
}

/// Exact source-owner/realm-qualified request selected by the source owner.
/// The source owner supplies this historical coordinate; it is not a grant.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HelperSetupOrigin {
    pub realm: meerkat_core::RealmId,
    pub source_owner: ResourceDomain,
    /// Exact source authority of the designated original, independent of ingress.
    pub original_authority: PrincipalRef,
    pub original: crate::private_originals::PrivateOriginalRef,
}
impl HelperSetupOrigin {
    fn matches(&self, association: &InputAuthorityAssociation) -> bool {
        let claims = association.candidate();
        self.realm == claims.ingress_namespace.realm
            && self.source_owner == claims.ingress_namespace.ingress
            && self.original_authority == claims.original_work.authority
            && self.original.original == claims.original_work.work
            && self.original.event_sha256.len() == 64
            && self.original.event_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
    }

    /// Historical membership only. These immutable rows do not answer whether
    /// any operation is currently permitted. The live source/native owners do.
    fn qualifies(&self, contributors: &[InputAuthorityAssociation]) -> bool {
        let mut matching = contributors.iter().filter(|source| self.matches(source));
        let Some(origin) = matching.next() else { return false; };
        if matching.next().is_some() { return false; }
        let requester = &origin.candidate().requester;
        contributors.iter().enumerate().all(|(index, source)| {
            let claims = source.candidate();
            claims.ingress_namespace.realm == self.realm
                && claims.ingress_namespace.ingress == self.source_owner
                && &claims.requester == requester
                && !contributors[..index].iter().any(|prior|
                    prior.candidate().original_work == claims.original_work)
                && claims.contributing_work.iter().all(|ancestor| contributors.iter().any(|item|
                    &item.candidate().original_work == ancestor))
        })
    }
}

/// Values selected by the installed helper/native owners, independently of
/// the contributor's grants. It contains no source event placeholders. The
/// actual native binder supplies target and controller selection; native
/// ingress verifies that selection against its real controller client.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HelperNativeBinding {
    pub ingress_actor: PrincipalRef,
    pub logical_executor: PrincipalRef,
    pub represented_subject: Option<PrincipalRef>,
    pub target: NativeWorkTarget,
    pub authority_basis: WorkAuthorityBasis,
    pub controller_grant_lineage: Vec<GrantLineageRef>,
    pub controller_model: meerkat_core::ControllerModelSelection,
    pub controller_ceiling: ExecutionRestrictions,
    pub admitted_ceiling: ExecutionRestrictions,
}

/// Exact trusted installed selection, pinned before any new worker effects.
/// The final runtime target and selected controller are not guessed here.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HelperAdmissionSelection {
    pub helper: crate::installed_host::acquisition::InstalledSetupSelection,
    pub member: meerkat_toolkit::host_lifetime::policy::CompiledNativeMember,
    pub grant_caller: PrincipalRef,
}

/// Returned only by the real fenced WorkGraph call. This retained observation
/// carries no native input custody or reusable permission to start a worker.
pub(crate) struct AdmittedHelperWork {
    owner: Arc<ToolkitOriginalOwner>,
    source_scope: StoreScope,
    facts: HelperAdmissionFacts,
    contributor: Arc<RetainedToolkitOriginal>,
    contributors: Vec<Arc<RetainedToolkitOriginal>>,
}

impl AdmittedHelperWork {
    pub(crate) fn item_id(&self) -> &WorkItemId {
        &self.facts.item
    }
    pub(crate) fn admission(&self) -> &WorkAdmission {
        &self.facts.admission
    }
    pub(crate) fn selection(&self) -> &HelperAdmissionSelection {
        &self.facts.selection
    }
    pub(crate) fn operation(&self) -> &HelperOperation {
        &self.facts.operation
    }
    pub(crate) fn origin(&self) -> &HelperSetupOrigin { &self.facts.origin }
    pub(crate) fn realm(&self) -> &str {
        &self.facts.realm
    }
    pub(crate) fn namespace(&self) -> &WorkNamespace {
        &self.facts.namespace
    }
    pub(crate) fn admitted_request(&self) -> Result<CreateWorkItemRequest, OriginalWriterError> {
        serde_json::from_value(self.facts.admitted_request.clone())
            .map_err(|_| OriginalWriterError::Conflict)
    }
    pub(crate) fn contributors(&self) -> &[Arc<RetainedToolkitOriginal>] {
        &self.contributors
    }
    pub(crate) fn qualified_contributors(&self) -> Arc<[RetainedOriginal]> {
        qualified_helper_originals(self.contributors.iter().map(|source| source.association()))
    }
    fn source_key(&self) -> Result<StoreKey, OriginalWriterError> {
        Ok(StoreKey {
            realm: self.source_scope.realm.clone(),
            owner: format!("toolkit/helper-originals/{}", self.source_scope.owner),
            id: helper_identity(&self.facts.namespace, &self.facts.item)?
                .as_str()
                .to_owned(),
        })
    }
    pub(crate) fn original_work(&self) -> Result<OriginalWorkRef, OriginalWriterError> {
        Ok(OriginalWorkRef {
            authority: self.owner.ingress_domain.authority.clone(),
            work: helper_identity(&self.facts.namespace, &self.facts.item)?,
        })
    }
}

/// Immutable facts authenticated by this call into the selected WorkGraph
/// service. Keep only its admission identity, not mutable lifecycle/revision.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HelperAdmissionFacts {
    realm: String,
    namespace: WorkNamespace,
    item: WorkItemId,
    admission: WorkAdmission,
    operation: HelperOperation,
    origin: HelperSetupOrigin,
    // Exact final request frozen by the preparation owner before admission.
    // Both fields deliberately name those same native outbound bytes.
    preparation_request: serde_json::Value,
    admitted_request: serde_json::Value,
    contributors: Vec<InputAuthorityAssociation>,
    // The installed selection is known before any helper worker is created.
    selection: HelperAdmissionSelection,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HelperSourceRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    custody_attempt: Option<super::custody_attempt::CustodyAttempt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bound_input: Option<crate::private_originals::RetainedBoundInput>,
    facts: HelperAdmissionFacts,
    controller: HelperControllerIntent,
    state: HelperSourceState,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case", deny_unknown_fields)]
enum HelperSourceState {
    ControllerPending,
    Bound {
        binding: HelperNativeBinding,
        association: InputAuthorityAssociation,
    },
}

impl HelperSourceRecord {
    fn bound(&self) -> Result<(&HelperNativeBinding, &InputAuthorityAssociation), OriginalWriterError> {
        match &self.state {
            HelperSourceState::Bound { binding, association } => Ok((binding, association)),
            HelperSourceState::ControllerPending => Err(OriginalWriterError::Unavailable),
        }
    }
    fn validate_custody(&self) -> Result<(), OriginalWriterError> {
        if let Some(attempt) = &self.custody_attempt {
            let (_, association) = self.bound()?;
            attempt.validate_binding(association)?;
        }
        if let Some(retained) = &self.bound_input {
            let attempt = self.custody_attempt.as_ref().ok_or(OriginalWriterError::Conflict)?;
            let input = retained.decode().map_err(|_| OriginalWriterError::Conflict)?;
            let event_key = IdempotencyKey::new(
                helper_identity(&self.facts.namespace, &self.facts.item)?.as_str().to_owned());
            if input.header().authority_association.as_ref() != Some(self.bound()?.1)
                || input.header().idempotency_key.as_ref() != Some(&event_key)
                || !attempt.matches_input(&input)?
            { return Err(OriginalWriterError::Conflict); }
        }
        Ok(())
    }
    fn encode(&self) -> Result<RecordData, OriginalWriterError> {
        self.validate_custody()?;
        Ok(RecordData { schema: SCHEMA.into(),
            version: if self.bound_input.is_some() { RETAINED_INPUT_SOURCE_VERSION }
                else if self.custody_attempt.is_some() { CUSTODY_SOURCE_VERSION } else { SOURCE_VERSION },
            body: serde_json::to_value(self).map_err(|_| OriginalWriterError::Conflict)? })
    }
    fn same_bound_facts(&self, other: &Self) -> bool {
        matches!(&self.state, HelperSourceState::Bound { .. })
            && self.facts == other.facts && self.controller == other.controller && self.state == other.state
            && match (&self.bound_input, &other.bound_input) {
                (Some(left), Some(right)) => left == right,
                _ => true, // A retained v3 snapshot predates first enrollment.
            }
    }
    fn decode(saved: &StoredRecord) -> Result<Self, OriginalWriterError> {
        if saved.data.schema != SCHEMA || !matches!(saved.data.version,
            SOURCE_VERSION | CUSTODY_SOURCE_VERSION | RETAINED_INPUT_SOURCE_VERSION)
            || saved.content_digest != saved.data.digest().map_err(|_| OriginalWriterError::Conflict)?
        { return Err(OriginalWriterError::Conflict); }
        let record: Self = serde_json::from_value(saved.data.body.clone())
            .map_err(|_| OriginalWriterError::Conflict)?;
        record.validate_custody()?;
        let revision_matches = match (saved.data.version, &record.state, &record.custody_attempt, &record.bound_input) {
            (SOURCE_VERSION, HelperSourceState::ControllerPending, None, None) => saved.revision == 1,
            (SOURCE_VERSION, HelperSourceState::Bound { .. }, None, None) => saved.revision == 2,
            (CUSTODY_SOURCE_VERSION, HelperSourceState::Bound { .. }, Some(_), None) => saved.revision >= 3,
            (RETAINED_INPUT_SOURCE_VERSION, HelperSourceState::Bound { .. }, Some(_), Some(_)) => saved.revision >= 3,
            _ => false,
        };
        if !revision_matches
            || canonical_json(&record).map_err(|_| OriginalWriterError::Conflict)?
                != canonical_json(&saved.data.body).map_err(|_| OriginalWriterError::Conflict)?
        { return Err(OriginalWriterError::Conflict); }
        Ok(record)
    }
}

/// A first-binding retry retains its original entered-write witness. A reread
/// may be used only while the same exact helper facts/controller remain pending.
pub(super) fn same_unbound_source(left: &StoredRecord, right: &StoredRecord) -> bool {
    match (HelperSourceRecord::decode(left), HelperSourceRecord::decode(right)) {
        (Ok(left), Ok(right)) => matches!(&left.state, HelperSourceState::ControllerPending)
            && left == right,
        _ => false,
    }
}

/// Only the actual fenced WorkGraph call constructs this value. It is not
/// deserializable and does not mark the helper admitted into the native runtime.
pub(crate) struct RetainedHelperSource {
    pub original: Arc<RetainedToolkitOriginal>,
    pub source_record: Arc<StoredRecord>,
    pub event_key: IdempotencyKey,
    pub authentication: HistoricalEvidenceRef,
}

/// Historical source projection for the same installed policy owner. It
/// contains no current grant answer and cannot be decoded into source custody.
pub(crate) struct HelperPolicySource {
    pub operation: HelperOperation,
    pub origin: HelperSetupOrigin,
    pub selection: HelperAdmissionSelection,
    pub contributors: Vec<InputAuthorityAssociation>,
}

impl RetainedHelperSource {
    pub(crate) fn same_bound_source(&self, other: &Self) -> Result<bool, OriginalWriterError> {
        Ok(Arc::ptr_eq(&self.original, &other.original)
            && self.source_record.key == other.source_record.key
            && self.event_key == other.event_key && self.authentication == other.authentication
            && HelperSourceRecord::decode(&self.source_record)?
                .same_bound_facts(&HelperSourceRecord::decode(&other.source_record)?))
    }

    /// A live helper's result may observe its own first-input accounting CAS.
    /// Keep immutable source facts and the actual retained input exact; this
    /// validates data progress only and supplies no native operation authority.
    pub(crate) fn check_result_record(
        &self, saved: &StoredRecord,
        first: Option<(&crate::private_originals::RetainedBoundInput,
            &meerkat_core::SessionId)>,
    ) -> Result<(), OriginalWriterError> {
        let retained = HelperSourceRecord::decode(&self.source_record)?;
        let current = HelperSourceRecord::decode(saved)?;
        if saved.key != self.source_record.key || !current.same_bound_facts(&retained)
            || saved.revision < self.source_record.revision
            || (saved.revision == self.source_record.revision
                && saved.content_digest != self.source_record.content_digest)
        { return Err(OriginalWriterError::Conflict); }
        let Some((input, session)) = first else {
            return if saved.revision == self.source_record.revision
                && saved.content_digest == self.source_record.content_digest {
                Ok(())
            } else { Err(OriginalWriterError::Conflict) };
        };
        let last = current.custody_attempt.as_ref().ok_or(OriginalWriterError::Conflict)?;
        if current.bound_input.as_ref() != Some(input) || last.session() != session
            || last.decision() == super::custody_attempt::CustodyDecision::Withdrawn
        { return Err(OriginalWriterError::Conflict); }
        // The custody owner validates its writes. This reader checks exact
        // correlation and returns the current condition, never reconstructing
        // an intervening transition history or authorizing native execution.
        Ok(())
    }

    pub(crate) fn policy_source(&self) -> Result<HelperPolicySource, OriginalWriterError> {
        let saved = self.source_record.as_ref();
        let record = HelperSourceRecord::decode(saved)?;
        let (_, association) = record.bound()?;
        if association != self.original.association()
            || association.candidate().original_authentication != self.authentication
        { return Err(OriginalWriterError::Conflict); }
        checked_helper_policy_source(saved, record)
    }

    pub(crate) fn matches_admission(&self, admitted: &AdmittedHelperWork) -> bool {
        let Ok(record) = HelperSourceRecord::decode(&self.source_record) else { return false; };
        let Ok((_, association)) = record.bound() else { return false; };
        record.facts == admitted.facts && association == self.original.association()
            && admitted.original_work().as_ref().ok()
                == Some(&self.original.association().candidate().original_work)
    }
}

fn checked_helper_policy_source(saved: &StoredRecord, record: HelperSourceRecord)
    -> Result<HelperPolicySource, OriginalWriterError>
{
        let (binding, association) = record.bound()?;
        record.controller.check_record_binding(&record.facts, binding)?;
        let claims = association.candidate();
        let helper = &record.facts.selection.helper;
        let member = &record.facts.selection.member;
        if canonical_json(&record).map_err(|_| OriginalWriterError::Conflict)?
                != canonical_json(&saved.data.body).map_err(|_| OriginalWriterError::Conflict)?
            || saved.key.realm != record.facts.realm
            || saved.key.id != helper_identity(&record.facts.namespace, &record.facts.item)?.as_str()
            || claims.original_work.work.as_str() != saved.key.id
            || claims.ingress_namespace.occurrence_scope != claims.original_work.work
            || claims.ingress_namespace.realm.as_str() != record.facts.realm
            || claims.root_event != claims.original_authentication
            || claims.original_authentication != admission_evidence(&claims.ingress_namespace.ingress, &record.facts)?
            || claims.ingress_actor != helper.ingress_actor || claims.logical_executor != helper.logical_executor
            || claims.represented_subject != helper.represented_subject
            || claims.authority_basis != binding.authority_basis
            || claims.target != binding.target
            || claims.target.logical_owner != member.logical_owner || claims.target.audience != member.audience
            || claims.target.context_generation != member.context_generation
            || helper.member != member.member
            || claims.controller_grant_lineage != binding.controller_grant_lineage
            || claims.controller_grant_lineage.len() != helper.controller_parent.len() + 1
            || !claims.controller_grant_lineage.starts_with(&helper.controller_parent)
            || claims.controller_model.as_ref() != Some(&binding.controller_model)
            || claims.controller_ceiling != binding.controller_ceiling
            || claims.admitted_ceiling != binding.admitted_ceiling
            || record.facts.origin.realm.as_str() != record.facts.realm
            || !record.facts.origin.qualifies(&record.facts.contributors)
            || claims.contributing_work != record.facts.contributors.iter()
                .map(|source| source.candidate().original_work.clone()).collect::<Vec<_>>()
        { return Err(OriginalWriterError::Conflict); }
        Ok(HelperPolicySource {
            operation: record.facts.operation,
            origin: record.facts.origin,
            selection: record.facts.selection,
            contributors: record.facts.contributors,
        })

}

// Data-only decoding for the same intact installed pair's retained recovery
// task. It creates no RetainedHelperSource, grant, actor or native custody.
fn cold_helper_record(scope: &StoreScope, saved: &StoredRecord)
    -> Result<HelperSourceRecord, OriginalWriterError>
{
    let record = HelperSourceRecord::decode(saved)?;
    let key = StoreKey { realm: scope.realm.clone(),
        owner: format!("toolkit/helper-originals/{}", scope.owner),
        id: helper_identity(&record.facts.namespace, &record.facts.item)?.as_str().to_owned() };
    if saved.key != key { return Err(OriginalWriterError::WrongOwner); }
    checked_helper_policy_source(saved, record.clone())?;
    Ok(record)
}

pub(crate) fn cold_custody_attempt(scope: &StoreScope, saved: &StoredRecord)
    -> Result<Option<super::custody_attempt::CustodyAttempt>, OriginalWriterError>
{
    if saved.data.schema != SCHEMA { return Ok(None); }
    // An unbound/legacy row is not an enrollment request or non-entry proof.
    // It must still decode; unsupported selected versions cannot disappear.
    let decoded = HelperSourceRecord::decode(saved)?;
    if saved.data.version == SOURCE_VERSION { return Ok(None); }
    if decoded.custody_attempt.is_none() { return Err(OriginalWriterError::Conflict); }
    Ok(cold_helper_record(scope, saved)?.custody_attempt)
}

pub(crate) fn cold_association(scope: &StoreScope, saved: &StoredRecord)
    -> Result<InputAuthorityAssociation, OriginalWriterError>
{
    Ok(cold_helper_record(scope, saved)?.bound()?.1.clone())
}

pub(crate) fn cold_bound_attempt(scope: &StoreScope, saved: &StoredRecord,
    association: &InputAuthorityAssociation,
) -> Result<Option<super::custody_attempt::CustodyAttempt>, OriginalWriterError> {
    let record = cold_helper_record(scope, saved)?;
    if record.bound()?.1 != association {
        return Err(OriginalWriterError::Conflict);
    }
    Ok(record.custody_attempt)
}

pub(crate) fn cold_bound_input(scope: &StoreScope, saved: &StoredRecord)
    -> Result<Option<crate::private_originals::RetainedBoundInput>, OriginalWriterError>
{
    // Legacy v3/v4 rows remain decodable but gain no missing input evidence.
    Ok(cold_helper_record(scope, saved)?.bound_input)
}

pub(crate) fn cold_custody_transition(scope: &StoreScope, baseline: &StoredRecord,
    saved: &StoredRecord, next: super::custody_attempt::CustodyAttempt,
) -> Result<toolkit_store::Transition, OriginalWriterError> {
    let mut current = cold_helper_record(scope, saved)?;
    if saved.key != baseline.key || !current.same_bound_facts(&cold_helper_record(scope, baseline)?) {
        return Err(OriginalWriterError::Conflict);
    }
    if current.bound_input != cold_helper_record(scope, baseline)?.bound_input {
        return Err(OriginalWriterError::Conflict);
    }
    super::custody_attempt::CustodyAttempt::validate_transition(current.custody_attempt.as_ref(), &next)?;
    current.custody_attempt = Some(next);
    Ok(toolkit_store::Transition { key: saved.key.clone(), expected_revision: saved.revision,
        expected_digest: Some(saved.content_digest.clone()), data: current.encode()?,
        claim: None, events: vec![], effects: vec![] })
}

pub(crate) fn historical_custody_attempt(saved: &StoredRecord)
    -> Result<Option<super::custody_attempt::CustodyAttempt>, OriginalWriterError>
{
    if saved.data.schema != SCHEMA { return Ok(None); }
    Ok(HelperSourceRecord::decode(saved)?.custody_attempt)
}

#[cfg(test)]
impl ToolkitOriginalOwner {
    /// Component-fixture binding of a real prior helper row after owner death.
    /// This retains no Candidate or native admission. The test must separately
    /// invoke the production native-backed Promoted restoration callback.
    pub(crate) fn bind_cold_helper_for_recovery_test(self: &Arc<Self>, store: &dyn ToolkitStore,
        scope: &StoreScope, saved: &StoredRecord,
    ) -> Result<RetainedHelperSource, OriginalWriterError> {
        self.check_canonical_source(store, scope)?;
        let record = cold_helper_record(scope, saved)?;
        let (_, association) = record.bound()?;
        let contributors = {
            let originals = self.originals.lock().map_err(|_| OriginalWriterError::Unavailable)?;
            record.facts.contributors.iter().map(|association| originals.iter()
                .filter_map(OriginalRecord::bound).find(|held| held.association() == association)
                .map(|held| held.retained.clone()).ok_or(OriginalWriterError::Unavailable))
                .collect::<Result<Vec<_>, _>>()?
        };
        let contributor = contributors.iter().find(|source| record.facts.origin.matches(source.association()))
            .ok_or(OriginalWriterError::Conflict)?;
        self.check_helper_contributors(contributor, &contributors)?;
        let c = association.candidate();
        let custody = AuthenticatedOriginal {
            original_work: c.original_work.clone(), occurrence_scope: c.ingress_namespace.occurrence_scope.clone(),
            original_authentication: c.original_authentication.clone(), root_event: c.root_event.clone(),
            contributing_work: c.contributing_work.clone(), target: c.target.clone(), authority_basis: c.authority_basis.clone(),
        };
        let original = self.publish_binding(association.clone(), custody, OriginalGrantBinding::Helper, None)?;
        Ok(RetainedHelperSource {
            original, source_record: Arc::new(saved.clone()),
            event_key: IdempotencyKey::new(helper_identity(&record.facts.namespace, &record.facts.item)?.as_str().to_owned()),
            authentication: c.original_authentication.clone(),
        })
    }
}

#[derive(Clone, Copy)]
enum HelperAdmissionUse {
    Create,
    Observe,
}

impl ToolkitOriginalOwner {
    /// Commit/recover one helper's WorkGraph admission before any new worker.
    /// The exact installed selection is known here; actual runtime/controller
    /// binding follows later through the same original owner. No requester or
    /// native mandate is inherited by copying the contributor's permission.
    ///
    /// The native owner constructs the fence for this exact WorkAdmission
    /// identity and actual live dispatch. Its guard gates creation and replay
    /// disclosure with each retained original under its own ingress namespace.
    /// The Toolkit writer mutex is not the native admission fence.
    ///
    /// If WorkGraph commits and the source write fails or the future is lost,
    /// retry the identical operation/key/request. WorkGraph recovers that item;
    /// this method never allocates another key or overwrites a source record.
    /// Native input submission and its source settlement remain separate.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn admit_fenced_helper(
        self: &Arc<Self>,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        store: &dyn ToolkitStore,
        source_scope: &StoreScope,
        service: &WorkGraphService,
        admission_key: WorkAdmissionKey,
        request: CreateWorkItemRequest,
        operation: HelperOperation,
        origin: HelperSetupOrigin,
        contributor: &Arc<RetainedToolkitOriginal>,
        contributors: &[Arc<RetainedToolkitOriginal>],
        own_originals: &[Arc<RetainedToolkitOriginal>],
        selection: HelperAdmissionSelection,
    ) -> Result<AdmittedHelperWork, OriginalWriterError> {
        self.access_fenced_helper(
            call,
            context,
            store,
            source_scope,
            service,
            admission_key,
            request,
            operation,
            origin,
            contributor,
            contributors,
            own_originals,
            selection,
            HelperAdmissionUse::Create,
        )
        .await?
        .ok_or(OriginalWriterError::Unavailable)
    }

    /// Read an existing exact admission for a live M1 invocation without
    /// creating anything. Current complete originating operation bounds still
    /// apply. This is NOT independent host-private recovery/settlement policy.
    /// `None` is an absence observation, never a no-effects or replay permit.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn observe_fenced_helper(
        self: &Arc<Self>,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        store: &dyn ToolkitStore,
        source_scope: &StoreScope,
        service: &WorkGraphService,
        admission_key: WorkAdmissionKey,
        request: CreateWorkItemRequest,
        operation: HelperOperation,
        origin: HelperSetupOrigin,
        contributor: &Arc<RetainedToolkitOriginal>,
        contributors: &[Arc<RetainedToolkitOriginal>],
        own_originals: &[Arc<RetainedToolkitOriginal>],
        selection: HelperAdmissionSelection,
    ) -> Result<Option<AdmittedHelperWork>, OriginalWriterError> {
        self.access_fenced_helper(
            call,
            context,
            store,
            source_scope,
            service,
            admission_key,
            request,
            operation,
            origin,
            contributor,
            contributors,
            own_originals,
            selection,
            HelperAdmissionUse::Observe,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn access_fenced_helper(
        self: &Arc<Self>,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        store: &dyn ToolkitStore,
        source_scope: &StoreScope,
        service: &WorkGraphService,
        admission_key: WorkAdmissionKey,
        request: CreateWorkItemRequest,
        operation: HelperOperation,
        origin: HelperSetupOrigin,
        contributor: &Arc<RetainedToolkitOriginal>,
        contributors: &[Arc<RetainedToolkitOriginal>],
        own_originals: &[Arc<RetainedToolkitOriginal>],
        selection: HelperAdmissionSelection,
        usage: HelperAdmissionUse,
    ) -> Result<Option<AdmittedHelperWork>, OriginalWriterError> {
        self.check_canonical_source(store, source_scope)?;
        store
            .capabilities()
            .require_durable_contract()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let _mutation = self
            .mutation
            .try_lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        self.check_helper_sources(contributor, contributors, own_originals)?;
        let parent = contributor.association.candidate();
        if !origin.matches(contributor.association())
            || parent.ingress_namespace.ingress != self.ingress_domain
            || source_scope.realm != parent.ingress_namespace.realm.to_string()
            || operation.acquisition.realm != source_scope.realm
            || operation.effect.is_empty()
            || operation.effect.len() > 512
            || operation.effect.chars().any(char::is_control)
        {
            return Err(OriginalWriterError::WrongOwner);
        }
        operation
            .acquisition
            .validate()
            .map_err(|_| OriginalWriterError::Conflict)?;
        let grant = service.namespace_grant();
        if grant.realm_id != source_scope.realm
            || request.realm_id.as_deref() != Some(grant.realm_id.as_str())
            || request.namespace.as_ref().map(WorkNamespace::as_str)
                != Some(grant.namespace.as_str())
        {
            return Err(OriginalWriterError::WrongOwner);
        }
        // Preparation froze the FINAL native request before any native commit.
        // Current contributors must reproduce its exact owner-generated binding.
        // A different invocation cannot silently replace the frozen ceiling set.
        validate_bound_work_request(&request, &operation, &origin, contributors, &selection)?;
        let preparation_request =
            serde_json::to_value(&request).map_err(|_| OriginalWriterError::Conflict)?;
        let admitted_request = preparation_request.clone();
        let admission = service
            .admission_identity(admission_key.clone(), request.clone())
            .map_err(|_| OriginalWriterError::Conflict)?;
        // Every handle came from the exact native-selected invocation and
        // canonical source owner. Preserve the full set for native currentness;
        // the source's retained basis alone is not recursive custody.
        let complete_contributors = qualified_helper_originals(
            contributors.iter().map(|original| original.association()),
        );
        // Native extracts its own exact operation and policy from this live
        // borrowed call/context. Only its owned fence enters the blocking task;
        // neither a reconstructed call nor a caller-supplied policy is used.
        let fence = GovernedHelperAdmissionFence::from_dispatch(
            call,
            context,
            self.clone(),
            complete_contributors,
            grant.realm_id.clone(),
            WorkNamespace::new(grant.namespace.clone())
                .map_err(|_| OriginalWriterError::Conflict)?,
            admission.clone(),
        )
        .map_err(helper_admission_refusal)?;
        let service = service.clone();
        let runtime = tokio::runtime::Handle::current();
        let exact_admission = admission.clone();
        let realm = grant.realm_id.clone();
        let namespace = WorkNamespace::new(grant.namespace.clone())
            .map_err(|_| OriginalWriterError::Conflict)?;
        // WorkGraph may execute synchronous SQLite work. Native owns a late
        // commit after this join is dropped; exact keyed recovery handles it.
        // The read path calls no create API. Its async read finishes BEFORE
        // publication custody. The synchronous guard releases only that same
        // fixed observation, with no I/O or block_on inside the callback.
        // This fresh fence belongs only to this one operation. Retain its
        // report handle locally; never share/reuse it for another attempt.
        let report = fence.clone();
        let result = tokio::task::spawn_blocking(move || match usage {
            HelperAdmissionUse::Create => runtime
                .block_on(service.create_idempotent_with_fence(admission_key, request, fence))
                .map(Some),
            HelperAdmissionUse::Observe => {
                // The entire read result stays private until current native
                // disclosure, including absence, conflict and backend errors.
                let mut pending =
                    Some(runtime.block_on(service.observe_admission(admission_key, request)));
                let mut observed = None;
                fence.execute_if_current(
                    &realm,
                    &namespace,
                    &exact_admission,
                    Box::new(|| {
                        observed = pending.take();
                        Ok(())
                    }),
                )?;
                observed.ok_or_else(|| {
                    WorkGraphError::Conflict("native helper observation was not executed".into())
                })?
            }
        })
        .await
        .map_err(|_| OriginalWriterError::Unavailable)?;
        let outcome = helper_admission_result(result, report.refusal())?;
        let Some(outcome) = outcome else {
            self.check_helper_sources(contributor, contributors, own_originals)?;
            return Ok(None);
        };
        let item = match outcome {
            WorkAdmissionOutcome::Created(item) | WorkAdmissionOutcome::Replayed(item) => item,
            WorkAdmissionOutcome::Conflict { .. } => return Err(OriginalWriterError::Conflict),
        };
        if item.realm_id != grant.realm_id || item.namespace.as_str() != grant.namespace {
            return Err(OriginalWriterError::Conflict);
        }
        let facts = HelperAdmissionFacts {
            realm: item.realm_id,
            namespace: item.namespace,
            item: item.id,
            admission,
            operation,
            origin,
            preparation_request,
            admitted_request,
            contributors: contributors
                .iter()
                .map(|value| value.association.clone())
                .collect(),
            selection,
        };
        self.check_helper_sources(contributor, contributors, own_originals)?;
        Ok(Some(AdmittedHelperWork {
            owner: self.clone(),
            source_scope: source_scope.clone(),
            facts,
            contributor: contributor.clone(),
            contributors: contributors.to_vec(),
        }))
    }

    /// Read an existing immutable binding after exact native admission replay.
    /// Returning its history grants no source custody or permission to rebind
    /// another runtime. The later binder reconstructs and compares the full row.
    pub(crate) async fn observe_helper_binding(
        self: &Arc<Self>,
        store: &dyn ToolkitStore,
        admitted: &AdmittedHelperWork,
    ) -> Result<Option<HelperNativeBinding>, OriginalWriterError> {
        self.check_canonical_source(store, &admitted.source_scope)?;
        self.check_admitted_helper(admitted)?;
        let key = admitted.source_key()?;
        let Some(saved) = store
            .get(&key)
            .await
            .map_err(|_| OriginalWriterError::Unavailable)?
        else {
            return Ok(None);
        };
        let record = HelperSourceRecord::decode(&saved)?;
        if saved.key != key || record.facts != admitted.facts {
            return Err(OriginalWriterError::Conflict);
        }
        self.check_admitted_helper(admitted)?;
        match record.state {
            HelperSourceState::ControllerPending => Ok(None),
            HelperSourceState::Bound { binding, association } => {
                record.controller.check_record_binding(&record.facts, &binding)?;
                if association.candidate().original_work != admitted.original_work()? {
                    return Err(OriginalWriterError::Conflict);
                }
                Ok(Some(binding))
            }
        }
    }

    /// Bind only after the actual native identity owner selected the helper's
    /// session/controller. WorkGraph admission has already happened, so retries
    /// cannot create another helper work item after a lost source response.
    pub(crate) async fn bind_admitted_helper(
        self: &Arc<Self>,
        store: &dyn ToolkitStore,
        admitted: &AdmittedHelperWork,
        binding: HelperNativeBinding,
    ) -> Result<RetainedHelperSource, OriginalWriterError> {
        if !Arc::ptr_eq(self, &admitted.owner)
            || !admitted.facts.origin.matches(admitted.contributor.association())
        {
            return Err(OriginalWriterError::WrongOwner);
        }
        self.check_canonical_source(store, &admitted.source_scope)?;
        store
            .capabilities()
            .require_durable_contract()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let _mutation = self.source_writer().await;
        let contributor = &admitted.contributor;
        let contributors = &admitted.contributors;
        self.check_helper_contributors(contributor, contributors)?;
        let facts = &admitted.facts;
        let selected = &facts.selection.helper;
        if binding.ingress_actor != selected.ingress_actor
            || binding.logical_executor != selected.logical_executor
            || binding.represented_subject != selected.represented_subject
            || binding.target.logical_owner != facts.selection.member.logical_owner
            || binding.target.audience != facts.selection.member.audience
            || binding.target.context_generation != facts.selection.member.context_generation
            || binding.controller_grant_lineage.len() != selected.controller_parent.len() + 1
            || !binding
                .controller_grant_lineage
                .starts_with(&selected.controller_parent)
        {
            return Err(OriginalWriterError::Conflict);
        }
        let parent = contributor.association.candidate();
        let authentication = admission_evidence(&self.ingress_domain, &facts)?;
        // These identities come from the actual admitted WorkGraph item. They
        // are not hashes of an operation, model argument or frontend event.
        let helper_id = helper_identity(&facts.namespace, &facts.item)?;
        let candidate = InputAuthorityAssociationCandidate {
            requester: parent.requester.clone(),
            ingress_actor: binding.ingress_actor.clone(),
            represented_subject: binding.represented_subject.clone(),
            logical_executor: binding.logical_executor.clone(),
            target: binding.target.clone(),
            original_work: OriginalWorkRef {
                authority: self.ingress_domain.authority.clone(),
                work: helper_id.clone(),
            },
            original_authentication: authentication.clone(),
            root_event: authentication.clone(),
            contributing_work: facts
                .contributors
                .iter()
                .map(|value| value.candidate().original_work.clone())
                .collect(),
            authority_basis: binding.authority_basis.clone(),
            controller_grant_lineage: binding.controller_grant_lineage.clone(),
            controller_model: Some(binding.controller_model.clone()),
            controller_ceiling: binding.controller_ceiling.clone(),
            admitted_ceiling: binding.admitted_ceiling.clone(),
            source_observations: vec![],
            ingress_namespace: QualifiedIngressNamespace {
                realm: parent.ingress_namespace.realm.clone(),
                ingress: self.ingress_domain.clone(),
                occurrence_scope: helper_id,
            },
            contract: ContractRequirements::local_governed_v1(),
        };
        let association =
            InputAuthorityAssociation::new(candidate).map_err(|_| OriginalWriterError::Conflict)?;
        // Read the same settled intent before publishing a source. An absent
        // or uncertain intent write cannot be replaced by a binding append.
        let key = admitted.source_key()?;
        let prior = store.get(&key).await.map_err(|_| OriginalWriterError::Unavailable)?
            .ok_or(OriginalWriterError::Unavailable)?;
        let mut record = HelperSourceRecord::decode(&prior)?;
        if prior.key != key || record.facts != *facts {
            return Err(OriginalWriterError::Conflict);
        }
        let state = HelperSourceState::Bound { binding: binding.clone(), association: association.clone() };
        let saved = match &record.state {
            HelperSourceState::ControllerPending => {
                record.controller.check_binding(self, admitted, &binding)?;
                self.retain_first_binding_write(store, &admitted.source_scope, &key,
                    Arc::new(prior.clone()), &association)?;
                record.state = state;
                store.compare_and_swap(toolkit_store::Transition {
                    key: key.clone(), expected_revision: prior.revision,
                    expected_digest: Some(prior.content_digest.clone()),
                    data: record.encode()?,
                    claim: None, events: vec![], effects: vec![],
                }).await.map_err(controller::store_error)?
            }
            existing if existing == &state => {
                // Exact historical replay preserves the full native intent and
                // binding. Each operation resolves only its own current grant.
                record.controller.check_retained_binding(admitted, &binding)?;
                prior
            },
            _ => return Err(OriginalWriterError::Conflict),
        };
        let settled = HelperSourceRecord::decode(&saved)?;
        if saved.key != key || settled != record {
            return Err(OriginalWriterError::Conflict);
        }
        // Cancellation after WorkGraph commit cannot confer source custody.
        // Retain the immutable row for exact reconciliation, but refuse binding.
        self.check_helper_contributors(contributor, contributors)?;
        let c = association.candidate();
        let custody = AuthenticatedOriginal {
            original_work: c.original_work.clone(),
            occurrence_scope: c.ingress_namespace.occurrence_scope.clone(),
            original_authentication: c.original_authentication.clone(),
            root_event: c.root_event.clone(),
            contributing_work: c.contributing_work.clone(),
            target: c.target.clone(),
            authority_basis: c.authority_basis.clone(),
        };
        let original =
            self.publish_binding(association, custody, OriginalGrantBinding::Helper, None)?;
        Ok(RetainedHelperSource {
            original,
            source_record: Arc::new(saved),
            event_key: IdempotencyKey::new(
                helper_identity(&facts.namespace, &facts.item)?
                    .as_str()
                    .to_owned(),
            ),
            authentication,
        })
    }

    /// First enrollment requires the same canonical Bound helper and actual
    /// prepared native input. No source read, write or custody is published here.
    pub(crate) fn prepare_helper_custody_enrollment(
        self: &Arc<Self>, store: &dyn ToolkitStore, admitted: &AdmittedHelperWork,
        source: &RetainedHelperSource, saved: &StoredRecord,
        session: &meerkat_core::SessionId, input: &Input,
    ) -> Result<toolkit_store::Transition, OriginalWriterError> {
        self.check_canonical_source(store, &admitted.source_scope)?;
        self.check_admitted_helper(admitted)?;
        self.check_bound_source(&source.original)?;
        if input.header().idempotency_key.as_ref() != Some(&source.event_key) {
            return Err(OriginalWriterError::Conflict);
        }
        let mut next = super::custody_attempt::CustodyAttempt::for_prepared(
            session, input, source.original.association())?;
        if let Some(existing) = HelperSourceRecord::decode(saved)?.custody_attempt {
            if existing.decision() != super::custody_attempt::CustodyDecision::Pending
                || existing.session() != session || existing.observation_entered() || !existing.matches_input(input)?
            { return Err(OriginalWriterError::Conflict); }
            next = existing;
        }
        let mut transition = self.helper_custody_transition(store, admitted, source, saved, next.clone(), true)?;
        let mut record = HelperSourceRecord::decode(saved)?;
        let retained = crate::private_originals::RetainedBoundInput::from_bound(input)
            .map_err(|_| OriginalWriterError::Conflict)?;
        if let Some(existing) = &record.bound_input {
            if existing != &retained { return Err(OriginalWriterError::Conflict); }
        } else if record.custody_attempt.is_some() {
            // A legacy attempt is not a fresh first-CAS enrollment. Do not
            // backfill evidence into an already staged historical invocation.
            return Err(OriginalWriterError::Unavailable);
        }
        record.custody_attempt = Some(next);
        record.bound_input = Some(retained);
        transition.data = record.encode()?;
        Ok(transition)
    }

    /// Persist under the actual tracked helper binder before Candidate. Native
    /// retains the entered write; this method does not own a second task.
    pub(crate) async fn persist_helper_custody_attempt(
        self: &Arc<Self>, store: &dyn ToolkitStore, admitted: &AdmittedHelperWork,
        source: &RetainedHelperSource, session: &meerkat_core::SessionId, input: &Input,
    ) -> Result<(), OriginalWriterError> {
        self.check_canonical_source(store, &admitted.source_scope)?;
        let _writer = self.source_writer().await;
        let stored = store.get(&source.source_record.key).await
            .map_err(|_| OriginalWriterError::Unavailable)?.ok_or(OriginalWriterError::Unavailable)?;
        let record = HelperSourceRecord::decode(&stored)?;
        if !source.original.first_binding_for(&stored.key) {
            return Err(OriginalWriterError::Unavailable);
        }
        let transition = self.prepare_helper_custody_enrollment(
            store, admitted, source, &stored, session, input)?;
        if transition.data != stored.data {
            store.compare_and_swap(transition).await.map_err(controller::store_error)?;
        }
        self.check_admitted_helper(admitted)?;
        self.check_bound_source(&source.original)
    }

    pub(crate) fn helper_custody_attempt(
        self: &Arc<Self>, store: &dyn ToolkitStore, admitted: &AdmittedHelperWork,
        source: &RetainedHelperSource, saved: &StoredRecord,
    ) -> Result<super::custody_attempt::CustodyAttempt, OriginalWriterError> {
        let current = HelperSourceRecord::decode(saved)?.custody_attempt
            .ok_or(OriginalWriterError::Unavailable)?;
        // Reuse the exact store/key/Bound-facts validator, without enrolling
        // or checking a new grant after native settlement.
        self.helper_custody_transition(store, admitted, source, saved, current.clone(), false)?;
        Ok(current)
    }

    /// Prepare data under the same selected helper store. This owns no write
    /// or native settlement; enrollment/terminal callers retain those owners.
    pub(crate) fn prepare_helper_custody_transition(
        self: &Arc<Self>, store: &dyn ToolkitStore, admitted: &AdmittedHelperWork,
        source: &RetainedHelperSource, saved: &StoredRecord,
        next: super::custody_attempt::CustodyAttempt,
    ) -> Result<toolkit_store::Transition, OriginalWriterError> {
        self.helper_custody_transition(store, admitted, source, saved, next, false)
    }

    fn helper_custody_transition(
        self: &Arc<Self>, store: &dyn ToolkitStore, admitted: &AdmittedHelperWork,
        source: &RetainedHelperSource, saved: &StoredRecord,
        next: super::custody_attempt::CustodyAttempt, enrollment: bool,
    ) -> Result<toolkit_store::Transition, OriginalWriterError> {
        self.check_canonical_source(store, &admitted.source_scope)?;
        if !Arc::ptr_eq(self, &admitted.owner) { return Err(OriginalWriterError::WrongOwner); }
        if !source.matches_admission(admitted) || saved.key != admitted.source_key()? {
            return Err(OriginalWriterError::Conflict);
        }
        let mut current = HelperSourceRecord::decode(saved)?;
        let retained = HelperSourceRecord::decode(&source.source_record)?;
        if !current.same_bound_facts(&retained)
            || (!enrollment && current.custody_attempt.is_none())
            || (enrollment && current.custody_attempt.as_ref().is_some_and(|existing| existing != &next))
        { return Err(OriginalWriterError::Conflict); }
        super::custody_attempt::CustodyAttempt::validate_transition(current.custody_attempt.as_ref(), &next)?;
        current.custody_attempt = Some(next);
        Ok(toolkit_store::Transition { key: saved.key.clone(), expected_revision: saved.revision,
            expected_digest: Some(saved.content_digest.clone()), claim: None,
            data: current.encode()?, events: vec![], effects: vec![] })
    }

    /// Historical source facts for the existing prepared-work owner. The
    /// exact immutable row participates in that owner's atomic publication;
    /// these facts do not attest native input custody or authorize an effect.
    pub(crate) async fn helper_publication_source(
        self: &Arc<Self>,
        store: &dyn ToolkitStore,
        service: &WorkGraphService,
        admitted: &AdmittedHelperWork,
        source: &RetainedHelperSource,
    ) -> Result<meerkat_toolkit::setup::NativeHelperSource, OriginalWriterError> {
        self.check_canonical_source(store, &admitted.source_scope)?;
        self.check_admitted_helper(admitted)?;
        if !source.matches_admission(admitted) {
            return Err(OriginalWriterError::Conflict);
        }
        let saved = store
            .get(&source.source_record.key)
            .await
            .map_err(|_| OriginalWriterError::Unavailable)?
            .ok_or(OriginalWriterError::Unavailable)?;
        let current = HelperSourceRecord::decode(&saved)?;
        let retained = HelperSourceRecord::decode(&source.source_record)?;
        if saved.key != source.source_record.key || !current.same_bound_facts(&retained) {
            return Err(OriginalWriterError::Conflict);
        }
        // Custody-only CAS may advance the row. The immutable helper facts
        // remain exact; the returned condition below uses this current row.
        let facts = &admitted.facts;
        let grant = service.namespace_grant();
        if grant.realm_id != facts.realm || grant.namespace != facts.namespace.as_str() {
            return Err(OriginalWriterError::WrongOwner);
        }
        let preparation_request: CreateWorkItemRequest =
            serde_json::from_value(facts.preparation_request.clone())
                .map_err(|_| OriginalWriterError::Conflict)?;
        let admitted_request: CreateWorkItemRequest =
            serde_json::from_value(facts.admitted_request.clone())
                .map_err(|_| OriginalWriterError::Conflict)?;
        validate_bound_work_request(
            &preparation_request,
            &facts.operation,
            &facts.origin,
            &admitted.contributors,
            &facts.selection,
        )?;
        if canonical_json(&preparation_request).map_err(|_| OriginalWriterError::Conflict)?
            != canonical_json(&admitted_request).map_err(|_| OriginalWriterError::Conflict)?
            || service
                .admission_identity(facts.admission.key.clone(), admitted_request.clone())
                .map_err(|_| OriginalWriterError::Conflict)?
                != facts.admission
        {
            return Err(OriginalWriterError::Conflict);
        }
        self.check_admitted_helper(admitted)?;
        // A retained row cannot conceal source cancellation. This remains an
        // observation; native effect/input owners perform their current check.
        let originals = self
            .originals
            .try_lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let held = originals
            .iter()
            .filter_map(OriginalRecord::bound)
            .find(|held| Arc::ptr_eq(&held.retained, &source.original))
            .ok_or(OriginalWriterError::WrongOwner)?;
        match &held.operation {
            OriginalOperationDisposition::Open => {}
            OriginalOperationDisposition::CancellationPending => {
                return Err(OriginalWriterError::Unavailable);
            }
            OriginalOperationDisposition::Cancelled { .. } => {
                return Err(OriginalWriterError::OperationCancelled);
            }
        }
        Ok(meerkat_toolkit::setup::NativeHelperSource {
            source: RecordCondition {
                key: saved.key,
                revision: saved.revision,
                content_digest: saved.content_digest,
                claim: None,
                dispatch_claim: None,
            },
            realm: facts.realm.clone(),
            namespace: facts.namespace.clone(),
            item: facts.item.clone(),
            admission: facts.admission.clone(),
            effect: EffectKey {
                record: facts.operation.acquisition.clone(),
                id: facts.operation.effect.clone(),
            },
            preparation_request,
            admitted_request,
            association: source.original.association().clone(),
            contributors: facts.contributors.clone(),
        })
    }

    /// Source observation only. Native effect admission still checks current
    /// complete contributor authority under its own publication boundary.
    pub(crate) fn check_admitted_helper(
        self: &Arc<Self>,
        admitted: &AdmittedHelperWork,
    ) -> Result<(), OriginalWriterError> {
        if !Arc::ptr_eq(self, &admitted.owner)
            || !admitted.facts.origin.matches(admitted.contributor.association())
            || admitted.facts.origin.realm.as_str() != admitted.facts.realm
            || !admitted.facts.origin.qualifies(&admitted.facts.contributors)
            || !admitted.contributors.iter().map(|source| source.association())
                .eq(admitted.facts.contributors.iter())
        {
            return Err(OriginalWriterError::WrongOwner);
        }
        self.check_helper_contributors(&admitted.contributor, &admitted.contributors)
    }

    fn check_helper_contributors(
        &self,
        contributor: &Arc<RetainedToolkitOriginal>,
        contributors: &[Arc<RetainedToolkitOriginal>],
    ) -> Result<(), OriginalWriterError> {
        // Beyond the originating live dispatch all these originals are named
        // contributors to the child. No own-original exemption survives here.
        self.check_helper_sources(contributor, contributors, &[])
    }

    /// Source integrity only. Live callers supply the exact staged subset from
    /// canonical lookup and still enter native from_dispatch with the unchanged
    /// borrowed call/context. Native checks all accepted associations, ceilings
    /// and current permission; this source check never substitutes for that.
    fn check_helper_sources(
        &self,
        contributor: &Arc<RetainedToolkitOriginal>,
        contributors: &[Arc<RetainedToolkitOriginal>],
        own_originals: &[Arc<RetainedToolkitOriginal>],
    ) -> Result<(), OriginalWriterError> {
        match self.observe_helper_sources(contributor, contributors, own_originals)? {
            HelperSourceSettlement::Admitted => Ok(()),
            HelperSourceSettlement::Candidate => {
                #[cfg(test)]
                eprintln!("AA1 failure: stage=source.helper_non_own_custody variant=SourceCustody::Candidate");
                Err(OriginalWriterError::Unavailable)
            }
            HelperSourceSettlement::Contended | HelperSourceSettlement::CancellationPending => {
                Err(OriginalWriterError::Unavailable)
            }
        }
    }

    // This is the same strict source check, with transient observations kept
    // distinct for the bounded preparation wait. No observation grants custody.
    fn observe_helper_sources(
        &self,
        contributor: &Arc<RetainedToolkitOriginal>,
        contributors: &[Arc<RetainedToolkitOriginal>],
        own_originals: &[Arc<RetainedToolkitOriginal>],
    ) -> Result<HelperSourceSettlement, OriginalWriterError> {
        if own_originals.iter().any(|own| !contributors.iter().any(|item| Arc::ptr_eq(own, item)))
            || contributors.is_empty()
            || contributors.len()
                > meerkat_authorization_contracts::work_association::MAX_ASSOCIATION_REFERENCES
            || !contributors
                .iter()
                .any(|value| Arc::ptr_eq(value, contributor))
        {
            return Err(OriginalWriterError::WrongOwner);
        }
        let originals = match self.originals.try_lock() {
            Ok(originals) => originals,
            Err(std::sync::TryLockError::WouldBlock) => return Ok(HelperSourceSettlement::Contended),
            Err(std::sync::TryLockError::Poisoned(_)) => return Err(OriginalWriterError::Unavailable),
        };
        let mut candidate = false;
        let mut cancellation_pending = false;
        let parent = contributor.association.candidate();
        for (index, value) in contributors.iter().enumerate() {
            let claims = value.association.candidate();
            if claims.requester != parent.requester
                || claims.ingress_namespace.realm != parent.ingress_namespace.realm
                || claims.ingress_namespace.ingress != self.ingress_domain
                || contributors[..index].iter().any(|prior|
                    prior.association.candidate().original_work == claims.original_work)
                // Validate completeness of the supplied canonical set. Never
                // expand it from names or treat one retained basis as ancestry.
                || claims.contributing_work.iter().any(|ancestor| !contributors.iter().any(|item|
                    &item.association.candidate().original_work == ancestor))
            {
                return Err(OriginalWriterError::WrongOwner);
            }
            let held = originals
                .iter()
                .filter_map(OriginalRecord::bound)
                .find(|held| Arc::ptr_eq(&held.retained, value))
                .ok_or(OriginalWriterError::WrongOwner)?;
            match &held.operation {
                OriginalOperationDisposition::CancellationPending => {
                    cancellation_pending = true;
                }
                OriginalOperationDisposition::Cancelled { .. } => {
                    return Err(OriginalWriterError::OperationCancelled);
                }
                OriginalOperationDisposition::Open => {}
            }
            if !own_originals.iter().any(|own| Arc::ptr_eq(own, value)) {
                match held.source_custody {
                    Some(SourceCustody::Admitted) => {}
                    // A real held source can converge when the native
                    // acceptance callback arrives. Keep existing reconciliation
                    // available; this is not a foreign owner or permission.
                    Some(SourceCustody::Candidate) => {
                        candidate = true;
                    }
                    None => return Err(OriginalWriterError::WrongOwner),
                }
            }
        }
        Ok(if cancellation_pending {
            HelperSourceSettlement::CancellationPending
        } else if candidate {
            HelperSourceSettlement::Candidate
        } else {
            HelperSourceSettlement::Admitted
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HelperSourceSettlement {
    Admitted,
    Candidate,
    Contended,
    CancellationPending,
}

/// Preserve source-owned historical identity verbatim. Do not collapse equal
/// work references from different ingresses, or copy the anchor's namespace
/// onto other contributors. Native owns any qualified deduplication and checks.
fn qualified_helper_originals<'a>(
    contributors: impl IntoIterator<Item = &'a InputAuthorityAssociation>,
) -> Arc<[RetainedOriginal]> {
    contributors
        .into_iter()
        .map(|association| {
            let claims = association.candidate();
            RetainedOriginal {
                namespace: claims.ingress_namespace.clone(),
                original: claims.original_work.clone(),
            }
        })
        .collect::<Vec<_>>()
        .into()
}

fn helper_admission_refusal(refusal: HelperAdmissionRefusal) -> OriginalWriterError {
    match refusal {
        HelperAdmissionRefusal::Denied => OriginalWriterError::OperationDenied,
        HelperAdmissionRefusal::Unavailable => OriginalWriterError::Unavailable,
    }
}

/// Interpret only the result and reason from this attempt's fresh native
/// fence. A native denial is not contention, and an unexplained store failure
/// is never absence or permission to dispatch again.
fn helper_admission_result<T>(
    result: Result<T, WorkGraphError>,
    refusal: Option<HelperAdmissionRefusal>,
) -> Result<T, OriginalWriterError> {
    match (result, refusal) {
        (Err(_), Some(HelperAdmissionRefusal::Denied)) => Err(OriginalWriterError::OperationDenied),
        (Err(_), Some(HelperAdmissionRefusal::Unavailable) | None)
        | (Ok(_), Some(_)) => Err(OriginalWriterError::Unavailable),
        (Ok(value), None) => Ok(value),
    }
}

/// Qualified owner-issued WorkGraph coordinate. The length prefix preserves
/// exact namespace/item boundaries without treating a digest as a new original.
fn helper_identity(
    namespace: &WorkNamespace,
    item: &WorkItemId,
) -> Result<EvidenceId, OriginalWriterError> {
    EvidenceId::new(format!(
        "setup-helper/{}:{}/{}",
        namespace.as_str().len(),
        namespace,
        item
    ))
    .map_err(|_| OriginalWriterError::Conflict)
}

/// WorkGraph keys are scoped by namespace, whereas Toolkit admissions are
/// scoped by source owner. Preserve the native qualified identity when moving
/// between these stores; a bare key would alias independent namespaces.
fn source_admission_key(
    realm: &str,
    namespace: &WorkNamespace,
    key: &WorkAdmissionKey,
) -> Result<String, OriginalWriterError> {
    let identity = canonical_json(&(SCHEMA, realm, namespace, key))
        .map_err(|_| OriginalWriterError::Conflict)?;
    Ok(format!(
        "helper-source/{}",
        ContentDigest::of_bytes(&identity)
    ))
}

fn admission_evidence(
    domain: &ResourceDomain,
    facts: &HelperAdmissionFacts,
) -> Result<HistoricalEvidenceRef, OriginalWriterError> {
    let bytes = canonical_json(facts).map_err(|_| OriginalWriterError::Conflict)?;
    Ok(HistoricalEvidenceRef {
        resource: ResourceRef {
            domain: domain.clone(),
            resource_id: helper_identity(&facts.namespace, &facts.item)?
                .as_str()
                .to_owned(),
        },
        revision: EvidenceId::new("1").map_err(|_| OriginalWriterError::Conflict)?,
        digest: EvidenceDigest::of_bytes(&bytes),
    })
}

pub(crate) fn bind_work_request(
    request: &mut CreateWorkItemRequest,
    operation: &HelperOperation,
    origin: &HelperSetupOrigin,
    contributors: &[Arc<RetainedToolkitOriginal>],
    selection: &HelperAdmissionSelection,
) -> Result<(), OriginalWriterError> {
    if request
        .external_refs
        .iter()
        .any(|reference| reference.kind == CORRELATION)
    {
        return Err(OriginalWriterError::Conflict);
    }
    // This is exact payload binding only. A changed requester/contributor,
    // operation or helper placement conflicts under the SAME admission key.
    let correlation = canonical_json(&(
        CORRELATION,
        operation,
        origin,
        contributors
            .iter()
            .map(|value| value.association())
            .collect::<Vec<_>>(),
        selection,
    ))
    .map_err(|_| OriginalWriterError::Conflict)?;
    request.external_refs.push(ExternalWorkRef {
        kind: CORRELATION.into(),
        id: ContentDigest::of_bytes(&correlation).to_string(),
        url: None,
    });
    Ok(())
}

/// Verify this source owner's one generated trailing reference. This does not
/// reinterpret arbitrary provider references or derive authority from a digest.
/// Its purpose is to prove that current accepted sources reproduce the exact
/// final request frozen before native admission, without replacing that input.
fn validate_bound_work_request(
    request: &CreateWorkItemRequest,
    operation: &HelperOperation,
    origin: &HelperSetupOrigin,
    contributors: &[Arc<RetainedToolkitOriginal>],
    selection: &HelperAdmissionSelection,
) -> Result<(), OriginalWriterError> {
    let mut reconstructed = request.clone();
    let generated = reconstructed
        .external_refs
        .pop()
        .ok_or(OriginalWriterError::Conflict)?;
    if generated.kind != CORRELATION || generated.url.is_some() {
        return Err(OriginalWriterError::Conflict);
    }
    bind_work_request(&mut reconstructed, operation, origin, contributors, selection)?;
    if canonical_json(&reconstructed).map_err(|_| OriginalWriterError::Conflict)?
        != canonical_json(request).map_err(|_| OriginalWriterError::Conflict)?
    {
        return Err(OriginalWriterError::Conflict);
    }
    Ok(())
}

#[cfg(test)]
fn fixture_origin(primary: &RetainedToolkitOriginal) -> HelperSetupOrigin {
    let claims = primary.association().candidate();
    HelperSetupOrigin {
        realm: claims.ingress_namespace.realm.clone(),
        source_owner: claims.ingress_namespace.ingress.clone(),
        original_authority: claims.original_work.authority.clone(),
        original: crate::private_originals::PrivateOriginalRef {
            route: toolkit_package::Id::new("source-fixture").unwrap(),
            original: claims.original_work.work.clone(),
            event_sha256: "0".repeat(64),
        },
    }
}

/// Policy-component fixture: establish real keyed WorkGraph/source history
/// without claiming native helper admission. All selection/binding inputs come
/// from the installed fixture and its actual separately issued native grants.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn retained_policy_fixture(
    owner: &Arc<ToolkitOriginalOwner>,
    store: &dyn ToolkitStore,
    source_scope: StoreScope,
    service: &WorkGraphService,
    label: &str,
    primary: Arc<RetainedToolkitOriginal>,
    contributors: Vec<Arc<RetainedToolkitOriginal>>,
    selection: HelperAdmissionSelection,
    binding: HelperNativeBinding,
) -> RetainedHelperSource {
    // Explicit source-fixture custody promotion for already retained exact
    // rows. This does not exercise native runtime input admission.
    for source in &contributors {
        owner.record_candidate(source).unwrap().admit().unwrap();
    }
    let origin = fixture_origin(&primary);
    let operation = HelperOperation {
        acquisition: StoreKey { realm: source_scope.realm.clone(), owner: "policy-fixture".into(), id: label.into() },
        effect: "work-admission".into(),
        input_digest: ContentDigest::of_bytes(b"source policy fixture"),
    };
    let mut request = CreateWorkItemRequest {
        title: "Source policy fixture".into(), ..Default::default()
    };
    let key = WorkAdmissionKey::new(label).unwrap();
    bind_work_request(&mut request, &operation, &origin, &contributors, &selection).unwrap();
    let admission = service.admission_identity(key.clone(), request.clone()).unwrap();
    // Deliberately isolate persistence/policy. This is not a fake allow fence:
    // no native helper admission or effect permission is exercised here.
    let outcome = service.create_idempotent(key, request.clone()).await.unwrap();
    let item = outcome.item().unwrap();
    let admitted = AdmittedHelperWork {
        owner: owner.clone(), source_scope,
        facts: HelperAdmissionFacts {
            realm: item.realm_id.clone(), namespace: item.namespace.clone(), item: item.id.clone(),
            admission, operation, origin,
            preparation_request: serde_json::to_value(&request).unwrap(),
            admitted_request: serde_json::to_value(request).unwrap(),
            contributors: contributors.iter().map(|original| original.association().clone()).collect(),
            selection,
        },
        contributor: primary, contributors,
    };
    let binding = controller::stage_fixture_controller(owner, store, &admitted, binding).await;
    owner.bind_admitted_helper(store, &admitted, binding).await.unwrap()
}

#[cfg(test)]
mod tests {
    // These fixtures promote source custody explicitly. They exercise source
    // and keyed-storage integrity, not native input/model/operation admission.
    use super::*;
    use crate::original_ingress::tests::{claims, initialized, principal, source};
    use meerkat::governed_realm::helper_admission::helper_contributing_work;
    use meerkat_core::service::WorkGraphNamespaceGrant;
    use meerkat_workgraph::{SqliteWorkGraphStore, WorkGraphStore};
    use std::os::unix::fs::PermissionsExt;

    pub(super) fn retained(
        owner: &Arc<ToolkitOriginalOwner>,
        base: &GrantLineageRef,
        label: &str,
        requester: &str,
        ancestors: &[Arc<RetainedToolkitOriginal>],
    ) -> Arc<RetainedToolkitOriginal> {
        let (original, pending) = pending_source(owner, base, label, requester, ancestors);
        pending.admit().unwrap();
        original
    }

    pub(super) fn pending_source(
        owner: &Arc<ToolkitOriginalOwner>,
        base: &GrantLineageRef,
        label: &str,
        requester: &str,
        ancestors: &[Arc<RetainedToolkitOriginal>],
    ) -> (Arc<RetainedToolkitOriginal>, PendingToolkitOriginal) {
        let mut candidate = claims(label, base).candidate().clone();
        candidate.requester = principal(requester);
        candidate.contributing_work = ancestors
            .iter()
            .map(|value| value.association.candidate().original_work.clone())
            .collect();
        let association = InputAuthorityAssociation::new(candidate).unwrap();
        let original = owner
            .publish_binding(
                association.clone(),
                source(&association),
                OriginalGrantBinding::Helper,
                None,
            )
            .unwrap();
        let pending = owner.record_candidate(&original).unwrap();
        (original, pending)
    }

    pub(super) fn fixture() -> (
        tempfile::TempDir,
        Arc<ToolkitOriginalOwner>,
        GrantLineageRef,
    ) {
        let directory = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let owner = initialized(&directory.path().join("grants"));
        let base = owner
            .grants()
            .issue_root(
                &principal("admin"),
                EvidenceId::new("helper-source-base").unwrap(),
                principal("agent"),
                None,
                ExecutionRestrictions::unrestricted(),
            )
            .unwrap();
        (directory, owner, base)
    }

    fn binding(original: &RetainedToolkitOriginal) -> HelperAdmissionSelection {
        let c = original.association.candidate();
        let member = meerkat_core::MobMemberBinding {
            mob_id: "helper-fixture".into(),
            role: "setup".into(),
            member: "setup".into(),
        };
        HelperAdmissionSelection {
            helper: crate::installed_host::acquisition::InstalledSetupSelection {
                catalog_capability: None,
                report_capability: None,
                member: member.clone(),
                ingress_actor: principal("setup-owner"),
                logical_executor: principal("setup-agent"),
                represented_subject: None,
                controller_parent: c.controller_grant_lineage.clone(),
                operation_basis: c.authority_basis.clone(),
            },
            member: meerkat_toolkit::host_lifetime::policy::CompiledNativeMember {
                setup_capability: None,
                member,
                logical_owner: c.target.logical_owner.clone(),
                context_generation: c.target.context_generation,
                audience: c.target.audience.clone(),
                initial_surface:
                    meerkat_toolkit::host_lifetime::policy::CompiledArtifactReference {
                        name: "helper-surface.json".into(),
                        sha256: "0".repeat(64),
                    },
            },
            grant_caller: principal("admin"),
        }
    }

    #[test]
    fn final_frozen_request_refuses_changed_contributors_selection_and_reserved_refs() {
        let (_directory, owner, base) = fixture();
        let one = retained(&owner, &base, "one", "user", &[]);
        let another = retained(&owner, &base, "another", "user", &[]);
        let origin = fixture_origin(&one);
        let operation = HelperOperation {
            acquisition: StoreKey {
                realm: "test-realm".into(),
                owner: "acquisitions/app".into(),
                id: "frozen".into(),
            },
            effect: "work-admission".into(),
            input_digest: ContentDigest::of_bytes(b"read Gmail"),
        };
        let selection = binding(&one);
        let mut final_request = CreateWorkItemRequest {
            realm_id: Some("test-realm".into()),
            namespace: Some(WorkNamespace::new("setup").unwrap()),
            title: "exact native request".into(),
            external_refs: vec![ExternalWorkRef {
                kind: "provider-reference".into(),
                id: "untouched".into(),
                url: None,
            }],
            ..Default::default()
        };
        bind_work_request(&mut final_request, &operation, &origin, &[one.clone()], &selection).unwrap();
        let frozen = canonical_json(&final_request).unwrap();
        validate_bound_work_request(&final_request, &operation, &origin, &[one.clone()], &selection)
            .unwrap();
        assert_eq!(canonical_json(&final_request).unwrap(), frozen);
        assert!(
            validate_bound_work_request(
                &final_request,
                &operation,
                &origin,
                &[one.clone(), another],
                &selection
            )
            .is_err()
        );
        let mut changed_selection = selection.clone();
        changed_selection.helper.logical_executor = principal("changed-setup-agent");
        assert!(
            validate_bound_work_request(
                &final_request,
                &operation,
                &origin,
                &[one.clone()],
                &changed_selection
            )
            .is_err()
        );
        let generated = final_request.external_refs.last().unwrap().clone();
        final_request.external_refs.insert(0, generated);
        assert!(
            validate_bound_work_request(&final_request, &operation, &origin, &[one], &selection).is_err()
        );
    }

    #[test]
    fn own_pending_source_never_becomes_named_custody_without_acceptance() {
        let (_directory, owner, base) = fixture();
        let (one, pending) = pending_source(&owner, &base, "pending", "user", &[]);
        let sources = [one.clone()];
        owner.check_helper_sources(&one, &sources, &sources).unwrap();
        assert!(matches!(owner.check_helper_contributors(&one, &sources),
            Err(OriginalWriterError::Unavailable)));
        let c = one.association().candidate();
        assert!(owner.admitted_original_basis(&c.ingress_namespace, &c.original_work).unwrap().is_none());
        // Only the existing source guard settles this fixture's acknowledgement.
        // No live-dispatch membership is stored in AdmittedHelperWork.
        pending.admit().unwrap();
        owner.check_helper_contributors(&one, &sources).unwrap();
        owner.release_original(&one).unwrap();
        assert!(matches!(owner.check_helper_contributors(&one, &sources),
            Err(OriginalWriterError::WrongOwner)));
    }

    #[test]
    fn live_own_membership_does_not_exempt_a_named_ancestor_or_foreign_handle() {
        let (_directory, owner, base) = fixture();
        let ancestor = retained(&owner, &base, "ancestor", "user", &[]);
        let (one, pending) = pending_source(&owner, &base, "pending", "user", &[ancestor.clone()]);
        let all = [one.clone(), ancestor.clone()];
        owner.check_helper_sources(&one, &all, &[one.clone()]).unwrap();
        owner.release_original(&ancestor).unwrap();
        assert!(matches!(owner.check_helper_sources(&one, &all, &[one.clone()]),
            Err(OriginalWriterError::WrongOwner)));
        let foreign = retained(&owner, &base, "foreign", "other-user", &[]);
        assert!(matches!(owner.check_helper_sources(&one, &all, &[one.clone(), foreign]),
            Err(OriginalWriterError::WrongOwner)));
        drop(pending);
    }

    #[test]
    fn live_own_membership_preserves_cancellation_pending() {
        let (_directory, owner, base) = fixture();
        let (one, _pending) = pending_source(&owner, &base, "pending", "user", &[]);
        owner.originals.lock().unwrap().iter_mut()
            .filter_map(OriginalRecord::bound_mut)
            .find(|held| Arc::ptr_eq(&held.retained, &one)).unwrap()
            .operation = OriginalOperationDisposition::CancellationPending;
        assert!(matches!(owner.check_helper_sources(&one, &[one.clone()], &[one.clone()]),
            Err(OriginalWriterError::Unavailable)));
    }

    #[test]
    fn helper_source_requires_complete_canonical_ancestors() {
        let (_directory, owner, base) = fixture();
        let grandparent = retained(&owner, &base, "grandparent", "user", &[]);
        let parent = retained(&owner, &base, "parent", "user", &[grandparent.clone()]);
        let child = retained(&owner, &base, "child", "user", &[parent.clone()]);
        assert!(
            owner
                .check_helper_contributors(&child, &[child.clone(), parent.clone()])
                .is_err()
        );
        assert!(
            owner
                .check_helper_contributors(
                    &child,
                    &[child.clone(), parent.clone(), grandparent.clone()]
                )
                .is_ok()
        );
        // A new snapshot cannot hide an uncertain ancestor cancellation behind
        // the child's retained basis, even when the child remains admitted.
        owner
            .originals
            .lock()
            .unwrap()
            .iter_mut()
            .filter_map(OriginalRecord::bound_mut)
            .find(|held| Arc::ptr_eq(&held.retained, &grandparent))
            .unwrap()
            .operation = OriginalOperationDisposition::CancellationPending;
        assert!(matches!(
            owner.check_helper_contributors(&child, &[child.clone(), parent, grandparent]),
            Err(OriginalWriterError::Unavailable)
        ));
    }

    #[test]
    fn native_helper_set_preserves_batched_and_transitive_originals() {
        let (_directory, owner, base) = fixture();
        let ancestor = retained(&owner, &base, "ancestor", "user", &[]);
        let parent = retained(&owner, &base, "parent", "user", &[ancestor.clone()]);
        let child = retained(&owner, &base, "child", "user", &[parent.clone()]);
        let batched = retained(&owner, &base, "batched", "user", &[]);
        let exact = [child.clone(), batched.clone(), parent.clone(), ancestor.clone()];
        owner.check_helper_contributors(&child, &exact).unwrap();
        let retained = qualified_helper_originals(
            exact.iter().map(|original| original.association()),
        );
        // The actual native helper function preserves the whole accepted set,
        // even though only the direct parent is named on the selected child.
        let native = helper_contributing_work(child.association(), &retained);
        assert_eq!(native.len(), retained.len());
        assert!(retained.iter().all(|original| native.contains(&original.original)));
        assert!(retained.iter().zip(&exact).all(|(retained, original)| {
            retained.namespace == original.association().candidate().ingress_namespace
        }));
        assert!(owner.check_helper_contributors(
            &child, &[child.clone(), batched, parent]
        ).is_err());
    }

    #[test]
    fn qualified_helper_projection_preserves_each_ingress_and_occurrence() {
        let (_directory, _owner, base) = fixture();
        let first = claims("qualified", &base);
        let mut other = first.candidate().clone();
        other.ingress_namespace.ingress.namespace = "other-ingress".into();
        let other = InputAuthorityAssociation::new(other).unwrap();
        let mut occurrence = first.candidate().clone();
        occurrence.ingress_namespace.occurrence_scope = EvidenceId::new("another-occurrence").unwrap();
        let occurrence = InputAuthorityAssociation::new(occurrence).unwrap();
        // These are only association projection fixtures, not admitted foreign
        // source handles. The same-owner custody checks remain mandatory.
        let qualified = qualified_helper_originals([&first, &other, &occurrence]);
        assert_eq!(qualified.len(), 3, "the caller does not deduplicate");
        for (retained, association) in qualified.iter().zip([&first, &other, &occurrence]) {
            assert_eq!(retained.namespace, association.candidate().ingress_namespace);
            assert_eq!(retained.original, association.candidate().original_work);
        }
        assert!(!qualified[0].same_original(&qualified[1]));
        assert!(qualified[0].same_original(&qualified[2]));
        assert_ne!(qualified[0].namespace, qualified[2].namespace);
        // Native encodes only same-ingress unqualified contributor references.
        // The entire qualified set is still supplied to the admission fence.
        let mut foreign_only = other.candidate().clone();
        foreign_only.original_work.work = EvidenceId::new("foreign-only").unwrap();
        let foreign_only = InputAuthorityAssociation::new(foreign_only).unwrap();
        let full = qualified_helper_originals([&first, &foreign_only]);
        assert_eq!(full.len(), 2);
        assert_eq!(helper_contributing_work(&first, &full), vec![first.candidate().original_work.clone()]);
    }

    #[test]
    fn helper_attempt_refusal_does_not_become_absence_or_retry_authority() {
        assert_eq!(helper_admission_refusal(HelperAdmissionRefusal::Denied),
            OriginalWriterError::OperationDenied);
        assert_eq!(helper_admission_refusal(HelperAdmissionRefusal::Unavailable),
            OriginalWriterError::Unavailable);
        let refused = || Err::<Option<()>, _>(WorkGraphError::Conflict("native refusal".into()));
        assert!(matches!(helper_admission_result(refused(), Some(HelperAdmissionRefusal::Denied)),
            Err(OriginalWriterError::OperationDenied)));
        assert!(matches!(helper_admission_result(refused(), Some(HelperAdmissionRefusal::Unavailable)),
            Err(OriginalWriterError::Unavailable)));
        assert!(matches!(helper_admission_result(refused(), None),
            Err(OriginalWriterError::Unavailable)));
        // An inconsistent success plus refusal must never disclose its value.
        assert!(matches!(helper_admission_result(Ok(Some(())), Some(HelperAdmissionRefusal::Denied)),
            Err(OriginalWriterError::Unavailable)));
        assert_eq!(helper_admission_result(Ok(None::<()>), None).unwrap(), None);
    }

    #[test]
    fn helper_source_refuses_foreign_requester_duplicate_or_withdrawn_handle() {
        let (_directory, owner, base) = fixture();
        let one = retained(&owner, &base, "one", "user", &[]);
        let foreign = retained(&owner, &base, "foreign", "other-user", &[]);
        assert!(
            owner
                .check_helper_contributors(&one, &[one.clone(), foreign])
                .is_err()
        );
        assert!(
            owner
                .check_helper_contributors(&one, &[one.clone(), one.clone()])
                .is_err()
        );
        owner.release_original(&one).unwrap();
        assert!(
            owner
                .check_helper_contributors(&one, &[one.clone()])
                .is_err()
        );
    }

    #[tokio::test]
    async fn native_workgraph_replay_conflicts_on_requester_ancestor_and_operation_changes() {
        let (directory, owner, base) = fixture();
        let one = retained(&owner, &base, "one", "user", &[]);
        let two = retained(&owner, &base, "two", "other-user", &[]);
        let origin = fixture_origin(&one);
        let actual: Arc<dyn WorkGraphStore> =
            Arc::new(SqliteWorkGraphStore::open(directory.path().join("work.sqlite3")).unwrap());
        let service = WorkGraphService::with_namespace_grant(
            actual,
            WorkGraphNamespaceGrant::new("test-realm", "setup").unwrap(),
        )
        .unwrap();
        let operation = HelperOperation {
            acquisition: StoreKey {
                realm: "test-realm".into(),
                owner: "acquisitions/app".into(),
                id: "one".into(),
            },
            effect: "work-admission".into(),
            input_digest: ContentDigest::of_bytes(b"read my Gmail"),
        };
        let target = binding(&one);
        let request = CreateWorkItemRequest {
            title: "Setup helper".into(),
            ..Default::default()
        };
        let mut exact = request.clone();
        bind_work_request(&mut exact, &operation, &origin, &[one.clone()], &target).unwrap();
        let key = WorkAdmissionKey::new("exact-original-effect").unwrap();
        // This test intentionally uses the keyed storage owner directly to
        // isolate payload replay. It is not a production admission fence.
        let first = service
            .create_idempotent(key.clone(), exact.clone())
            .await
            .unwrap();
        let replay = service.create_idempotent(key.clone(), exact).await.unwrap();
        assert!(matches!(&replay, WorkAdmissionOutcome::Replayed(_)));
        assert_eq!(first.item().unwrap().id, replay.item().unwrap().id);
        for contributors in [vec![two], vec![one.clone(), one.clone()]] {
            let mut changed = request.clone();
            bind_work_request(&mut changed, &operation, &origin, &contributors, &target).unwrap();
            assert!(matches!(
                service
                    .create_idempotent(key.clone(), changed)
                    .await
                    .unwrap(),
                WorkAdmissionOutcome::Conflict { .. }
            ));
        }
        let mut changed_operation = operation;
        changed_operation.effect = "other-effect".into();
        let mut changed = request;
        bind_work_request(&mut changed, &changed_operation, &origin, &[one], &target).unwrap();
        assert!(matches!(
            service.create_idempotent(key, changed).await.unwrap(),
            WorkAdmissionOutcome::Conflict { .. }
        ));
    }

    // Source-only test fixture: the actual WorkGraph store establishes keyed
    // identity, but this bypasses the production native fence deliberately. It
    // proves source replay/settlement integrity, not permission or admission.
    pub(super) async fn admitted_source_fixture(
        owner: &Arc<ToolkitOriginalOwner>,
        service: &WorkGraphService,
        primary: &Arc<RetainedToolkitOriginal>,
        contributors: Vec<Arc<RetainedToolkitOriginal>>,
        base: &GrantLineageRef,
    ) -> (AdmittedHelperWork, HelperNativeBinding) {
        admitted_source_fixture_with_report(owner, service, primary, contributors, base, None).await
    }

    pub(super) async fn admitted_source_fixture_with_report(
        owner: &Arc<ToolkitOriginalOwner>, service: &WorkGraphService,
        primary: &Arc<RetainedToolkitOriginal>, contributors: Vec<Arc<RetainedToolkitOriginal>>,
        base: &GrantLineageRef,
        report: Option<crate::installed_host::acquisition::ConfiguredHelperReportCapability>,
    ) -> (AdmittedHelperWork, HelperNativeBinding) {
        admitted_source_fixture_with_report_basis(owner, service, primary, contributors,
            base, report, None, None).await
    }

    pub(super) async fn admitted_source_fixture_with_report_basis(
        owner: &Arc<ToolkitOriginalOwner>, service: &WorkGraphService,
        primary: &Arc<RetainedToolkitOriginal>, contributors: Vec<Arc<RetainedToolkitOriginal>>,
        base: &GrantLineageRef,
        report: Option<crate::installed_host::acquisition::ConfiguredHelperReportCapability>,
        operation_basis: Option<WorkAuthorityBasis>,
        catalog: Option<crate::installed_host::acquisition::ConfiguredHelperCatalogCapability>,
    ) -> (AdmittedHelperWork, HelperNativeBinding) {
        let origin = fixture_origin(primary);
        let mut selection = binding(primary);
        selection.helper.controller_parent = vec![base.clone()];
        selection.helper.report_capability = report;
        selection.helper.catalog_capability = catalog;
        if selection.helper.report_capability.is_some() {
            selection.helper.operation_basis = WorkAuthorityBasis::TargetedGrantLineages { base: None, targets: vec![] };
        }
        if let Some(basis) = operation_basis {
            selection.helper.operation_basis = basis;
        }
        // The source fixture's parent belongs to this actual issuer.
        selection.grant_caller = principal("agent");
        let operation = HelperOperation {
            acquisition: StoreKey {
                realm: "test-realm".into(),
                owner: "acquisitions/app".into(),
                id: "split".into(),
            },
            effect: "work-admission".into(),
            input_digest: ContentDigest::of_bytes(b"read my Gmail"),
        };
        let request = CreateWorkItemRequest {
            title: "Setup helper".into(),
            ..Default::default()
        };
        let mut admitted_request = request.clone();
        admitted_request.realm_id = Some("test-realm".into());
        admitted_request.namespace = Some(WorkNamespace::new("setup").unwrap());
        bind_work_request(&mut admitted_request, &operation, &origin, &contributors, &selection).unwrap();
        let key = WorkAdmissionKey::new("exact-split-source").unwrap();
        let admission = service
            .admission_identity(key.clone(), admitted_request.clone())
            .unwrap();
        let actual = service
            .create_idempotent(key, admitted_request.clone())
            .await
            .unwrap();
        let item = actual.item().unwrap();
        let mut target = primary.association.candidate().target.clone();
        if selection.helper.report_capability.is_some() {
            let session = meerkat_core::SessionId::new();
            target.context = EvidenceId::new(session.to_string()).unwrap();
            target.logical_runtime = EvidenceId::new(meerkat_runtime::LogicalRuntimeId::for_session(&session).to_string()).unwrap();
        }
        let native_binding = HelperNativeBinding {
            ingress_actor: selection.helper.ingress_actor.clone(),
            logical_executor: selection.helper.logical_executor.clone(),
            represented_subject: selection.helper.represented_subject.clone(),
            target,
            authority_basis: selection.helper.operation_basis.clone(),
            controller_grant_lineage: vec![base.clone()],
            controller_model: meerkat_core::ControllerModelSelection::new(
                meerkat_core::SessionLlmIdentity {
                    model: "helper-source-fixture".into(),
                    provider: meerkat_core::Provider::Anthropic,
                    self_hosted_server_id: None,
                    provider_params: None,
                    auth_binding: None,
                },
                serde_json::from_value(serde_json::json!({
                    "realm": "test-realm", "account": "helper-source-fixture"
                }))
                .unwrap(),
                "helper-source-fixture".into(),
                "helper-source-fixture".into(),
            ),
            controller_ceiling: ExecutionRestrictions::unrestricted(),
            admitted_ceiling: ExecutionRestrictions::unrestricted(),
        };
        (
            AdmittedHelperWork {
                owner: owner.clone(),
                source_scope: StoreScope {
                    realm: "test-realm".into(),
                    owner: "app".into(),
                },
                facts: HelperAdmissionFacts {
                    realm: item.realm_id.clone(),
                    namespace: item.namespace.clone(),
                    item: item.id.clone(),
                    admission,
                    operation,
                    origin,
                    preparation_request: serde_json::to_value(&admitted_request).unwrap(),
                    admitted_request: serde_json::to_value(admitted_request).unwrap(),
                    contributors: contributors
                        .iter()
                        .map(|source| source.association.clone())
                        .collect(),
                    selection,
                },
                contributor: primary.clone(),
                contributors,
            },
            native_binding,
        )
    }

    #[tokio::test]
    async fn helper_custody_cas_preserves_bound_replay_and_current_publication_condition() {
        use crate::original_ingress::custody_attempt::CustodyChange;
        use meerkat_core::{AgentError, AgentLlmClient, ControllerModelClient,
            ControllerModelSelection, LlmStreamResult, Message, Provider,
            ProviderParamsOverride, SessionId, ToolDef};
        use meerkat_runtime::{Input, LogicalRuntimeId, PromptInput};

        struct UnusedClient(ControllerModelSelection);
        #[async_trait::async_trait]
        impl AgentLlmClient for UnusedClient {
            fn controller_model_selection(&self) -> Option<ControllerModelSelection> {
                Some(self.0.clone())
            }
            async fn stream_response(&self, _: &[Message], _: &[Arc<ToolDef>], _: u32,
                _: Option<f32>, _: Option<&ProviderParamsOverride>)
                -> Result<LlmStreamResult, AgentError> {
                panic!("helper schema control must not execute a model")
            }
            fn provider(&self) -> Provider { Provider::Anthropic }
            fn model(&self) -> &str { "helper-source-fixture" }
        }

        let (directory, owner, base) = fixture();
        let parent = retained(&owner, &base, "custody-parent", "user", &[]);
        let service = WorkGraphService::with_namespace_grant(
            Arc::new(SqliteWorkGraphStore::open(directory.path().join("work.sqlite3")).unwrap()),
            WorkGraphNamespaceGrant::new("test-realm", "setup").unwrap(),
        ).unwrap();
        let store: Arc<dyn ToolkitStore> = Arc::new(
            toolkit_store::SqliteStore::open(directory.path().join("source.sqlite3")).unwrap());
        let (admitted, mut binding) = admitted_source_fixture(
            &owner, &service, &parent, vec![parent.clone()], &base).await;
        let _adapter = crate::private_originals::PrivateOriginals::new(
            admitted.source_scope.clone(), store.clone(), owner.clone(),
            vec![crate::original_ingress::tests::private_route("route")],
        ).await.unwrap();
        // Select concrete native coordinates before source/controller binding.
        // The existing keyed WorkGraph fixture bypasses the native dispatch
        // fence; this test does not create a runtime session or admit an input.
        let session = SessionId::new();
        binding.target.context = EvidenceId::new(session.to_string()).unwrap();
        binding.target.logical_runtime = EvidenceId::new(
            LogicalRuntimeId::for_session(&session).to_string()).unwrap();
        let binding = controller::stage_fixture_controller(
            &owner, store.as_ref(), &admitted, binding).await;
        let source = owner.bind_admitted_helper(
            store.as_ref(), &admitted, binding.clone()).await.unwrap();
        let before = source.source_record.as_ref().clone();
        assert_eq!(before.revision, 2);
        assert_eq!(before.data.version, SOURCE_VERSION);
        assert!(before.data.body.get("custody_attempt").is_none());
        let prepared = owner.prepare_authenticated_input(
            &source.original, &session,
            Input::Prompt(PromptInput::new("one exact helper schema input", None)),
            source.event_key.clone(), source.authentication.clone(),
            ControllerModelClient::new(binding.controller_model.clone(),
                Arc::new(UnusedClient(binding.controller_model.clone()))),
        ).unwrap();
        let input = prepared.input().clone();
        drop(prepared); // Never stage a Candidate or claim native admission.
        let enrollment = owner.prepare_helper_custody_enrollment(
            store.as_ref(), &admitted, &source, &before, &session, &input).unwrap();
        assert_eq!(enrollment.expected_revision, before.revision);
        assert_eq!(enrollment.expected_digest.as_ref(), Some(&before.content_digest));
        assert!(enrollment.events.is_empty() && enrollment.effects.is_empty());
        let enrolled = store.compare_and_swap(enrollment.clone()).await.unwrap();
        assert!(store.compare_and_swap(enrollment).await.is_err());
        assert_eq!(enrolled.revision, before.revision + 1);
        assert_eq!(enrolled.data.version, RETAINED_INPUT_SOURCE_VERSION);
        let mut unchanged = enrolled.data.body.clone();
        unchanged.as_object_mut().unwrap().remove("custody_attempt");
        let exact_input = unchanged.as_object_mut().unwrap().remove("bound_input").unwrap();
        assert_eq!(exact_input, serde_json::to_value(&input).unwrap());
        assert_eq!(unchanged, before.data.body);
        let attempt = HelperSourceRecord::decode(&enrolled).unwrap().custody_attempt.unwrap();
        assert!(attempt.matches_input(&input).unwrap());
        assert_eq!(serde_json::to_value(cold_bound_input(&admitted.source_scope, &enrolled)
            .unwrap().unwrap().decode().unwrap()).unwrap(), serde_json::to_value(&input).unwrap());
        // Version and exact bytes are checked independently of the store's
        // valid digest. A legacy row stays legacy and cannot be backfilled by
        // replaying the enrollment API after its first attempt already exists.
        let mut legacy = enrolled.clone();
        legacy.data.version = CUSTODY_SOURCE_VERSION;
        legacy.data.body.as_object_mut().unwrap().remove("bound_input");
        legacy.content_digest = legacy.data.digest().unwrap();
        assert!(HelperSourceRecord::decode(&legacy).is_ok());
        assert!(cold_bound_input(&admitted.source_scope, &legacy).unwrap().is_none());
        assert!(matches!(owner.prepare_helper_custody_enrollment(
            store.as_ref(), &admitted, &source, &legacy, &session, &input),
            Err(OriginalWriterError::Unavailable)));
        let mut missing = legacy.clone();
        missing.data.version = RETAINED_INPUT_SOURCE_VERSION;
        missing.content_digest = missing.data.digest().unwrap();
        assert!(HelperSourceRecord::decode(&missing).is_err());
        let mut wrong_version = enrolled.clone();
        wrong_version.data.version = CUSTODY_SOURCE_VERSION;
        wrong_version.content_digest = wrong_version.data.digest().unwrap();
        assert!(HelperSourceRecord::decode(&wrong_version).is_err());
        let mut changed_input = input.clone();
        let Input::Prompt(prompt) = &mut changed_input else { unreachable!() };
        prompt.content = meerkat_core::ContentInput::Text("different helper payload".into());
        let mut tampered = enrolled.clone();
        tampered.data.body["bound_input"] = serde_json::to_value(changed_input).unwrap();
        tampered.content_digest = tampered.data.digest().unwrap();
        assert!(matches!(HelperSourceRecord::decode(&tampered), Err(OriginalWriterError::Conflict)));
        assert_eq!(store.get(&enrolled.key).await.unwrap().as_ref(), Some(&enrolled));
        let entered = attempt.changed(CustodyChange::ChargeResolution).unwrap();
        let charged = store.compare_and_swap(owner.prepare_helper_custody_transition(
            store.as_ref(), &admitted, &source, &enrolled, entered.clone()).unwrap()).await.unwrap();
        assert_eq!(charged.revision, enrolled.revision + 1);
        assert_eq!(charged.data.version, RETAINED_INPUT_SOURCE_VERSION);

        // A retained v3 snapshot still selects these exact immutable facts;
        // publication must return the current v5 condition, not the old one.
        assert_eq!(source.source_record.as_ref(), &before);
        let publication = owner.helper_publication_source(
            store.as_ref(), &service, &admitted, &source).await.unwrap();
        assert_eq!(publication.source.key, charged.key);
        assert_eq!(publication.source.revision, charged.revision);
        assert_eq!(publication.source.content_digest, charged.content_digest);
        assert!(publication.source.claim.is_none() && publication.source.dispatch_claim.is_none());
        let replay = owner.bind_admitted_helper(
            store.as_ref(), &admitted, binding).await.unwrap();
        assert!(Arc::ptr_eq(&source.original, &replay.original));
        assert_eq!(replay.source_record.as_ref(), &charged);
        assert!(HelperSourceRecord::decode(&replay.source_record).unwrap()
            .custody_attempt.as_ref() == Some(&entered));
        assert_eq!(store.get(&charged.key).await.unwrap().as_ref(), Some(&charged));
        let held = owner.originals.lock().unwrap();
        let helper = held.iter().filter_map(OriginalRecord::bound)
            .find(|entry| Arc::ptr_eq(&entry.retained, &source.original)).unwrap();
        assert!(helper.source_custody.is_none());
        drop(held);

        // A current row with a valid checksum but different immutable helper
        // facts must still be refused. Custody-only tolerance is not rebinding.
        let mut changed = HelperSourceRecord::decode(&charged).unwrap();
        changed.facts.selection.helper.logical_executor = principal("foreign-helper");
        let changed = store.compare_and_swap(toolkit_store::Transition {
            key: charged.key.clone(), expected_revision: charged.revision,
            expected_digest: Some(charged.content_digest.clone()), claim: None,
            data: changed.encode().unwrap(), events: vec![], effects: vec![],
        }).await.unwrap();
        assert!(HelperSourceRecord::decode(&changed).is_ok());
        assert!(matches!(owner.helper_publication_source(
            store.as_ref(), &service, &admitted, &source).await,
            Err(OriginalWriterError::Conflict)));
        assert_eq!(store.get(&changed.key).await.unwrap().as_ref(), Some(&changed));
    }

    #[tokio::test]
    async fn helper_store_entrypoints_refuse_another_store_after_adapter_selection() {
        let (directory, owner, base) = fixture();
        let parent = retained(&owner, &base, "selected-parent", "user", &[]);
        let service = WorkGraphService::with_namespace_grant(
            Arc::new(SqliteWorkGraphStore::open(directory.path().join("work.sqlite3")).unwrap()),
            WorkGraphNamespaceGrant::new("test-realm", "setup").unwrap(),
        ).unwrap();
        let store: Arc<dyn ToolkitStore> = Arc::new(
            toolkit_store::SqliteStore::open(directory.path().join("source.sqlite3")).unwrap());
        let other = toolkit_store::SqliteStore::open(directory.path().join("other.sqlite3")).unwrap();
        let (admitted, binding) = admitted_source_fixture(
            &owner, &service, &parent, vec![parent.clone()], &base).await;
        let _adapter = crate::private_originals::PrivateOriginals::new(
            admitted.source_scope.clone(), store.clone(), owner.clone(),
            vec![crate::original_ingress::tests::private_route("route")],
        ).await.unwrap();
        let mut ceiling = binding.controller_ceiling.clone();
        ceiling.delegation_depth = meerkat_authorization_contracts::constraints::DelegationDepth::remaining(0);
        let intent = HelperControllerIntent::new(&admitted, binding.target.clone(),
            binding.controller_model.clone(), ceiling).unwrap();
        let grants_before = std::fs::read(directory.path().join("grants")).unwrap();
        assert!(matches!(owner.observe_helper_binding(&other, &admitted).await,
            Err(OriginalWriterError::WrongOwner)));
        assert!(matches!(owner.retain_helper_operation_intents(&other, &admitted, intent).await,
            Err(OriginalWriterError::WrongOwner)));
        assert!(matches!(owner.bind_admitted_helper(&other, &admitted, binding.clone()).await,
            Err(OriginalWriterError::WrongOwner)));
        assert!(other.list_records(&admitted.source_scope, None, 16).await.unwrap().is_empty());
        assert!(store.list_records(&admitted.source_scope, None, 16).await.unwrap().is_empty());
        assert_eq!(std::fs::read(directory.path().join("grants")).unwrap(), grants_before);

        // Use the fixture's real keyed WorkGraph row and native grant owner.
        // This explicitly bypasses the dispatch fence; it proves store/source
        // binding and preserves the existing fixture's non-admission scope.
        let binding = controller::stage_fixture_controller(
            &owner, store.as_ref(), &admitted, binding).await;
        let source = owner.bind_admitted_helper(store.as_ref(), &admitted, binding).await.unwrap();
        let key = admitted.source_key().unwrap();
        let saved = store.get(&key).await.unwrap();
        let mut wrong_key = source.source_record.as_ref().clone();
        wrong_key.key.id.push_str("/other-helper");
        assert!(matches!(owner.retain_first_binding_write(store.as_ref(), &admitted.source_scope,
            &key, Arc::new(wrong_key), source.original.association()),
            Err(OriginalWriterError::WrongOwner)));
        assert_eq!(store.get(&key).await.unwrap(), saved);
        assert!(matches!(owner.helper_publication_source(&other, &service, &admitted, &source).await,
            Err(OriginalWriterError::WrongOwner)));
        assert!(owner.observe_helper_binding(store.as_ref(), &admitted).await.unwrap().is_some());
        assert!(owner.helper_publication_source(store.as_ref(), &service, &admitted, &source).await.is_ok());
        assert_eq!(store.get(&admitted.source_key().unwrap()).await.unwrap(), saved);
        assert!(other.list_records(&admitted.source_scope, None, 16).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn split_helper_recovers_exact_work_before_source_binding_without_promoting_custody() {
        let (directory, owner, base) = fixture();
        let ancestor = retained(&owner, &base, "ancestor", "user", &[]);
        let parent = retained(&owner, &base, "parent", "user", &[ancestor.clone()]);
        let service = WorkGraphService::with_namespace_grant(
            Arc::new(
                SqliteWorkGraphStore::open(directory.path().join("split-work.sqlite3")).unwrap(),
            ),
            WorkGraphNamespaceGrant::new("test-realm", "setup").unwrap(),
        )
        .unwrap();
        let store =
            toolkit_store::SqliteStore::open(&directory.path().join("split-source.sqlite3"))
                .unwrap();
        let (admitted, native_binding) =
            admitted_source_fixture(&owner, &service, &parent, vec![parent.clone(), ancestor.clone()], &base).await;
        // The WorkGraph commit exists while no source row exists, as after a
        // crash between owners. Exact replay returns the same native item.
        let replay = service
            .create_idempotent(
                admitted.facts.admission.key.clone(),
                serde_json::from_value(admitted.facts.admitted_request.clone()).unwrap(),
            )
            .await
            .unwrap();
        assert!(matches!(replay, WorkAdmissionOutcome::Replayed(_)));
        assert_eq!(replay.item().unwrap().id, *admitted.item_id());
        let native_binding = controller::stage_fixture_controller(&owner, &store, &admitted, native_binding).await;
        let source = owner
            .bind_admitted_helper(&store, &admitted, native_binding.clone())
            .await
            .unwrap();
        let again = owner
            .bind_admitted_helper(&store, &admitted, native_binding)
            .await
            .unwrap();
        assert!(Arc::ptr_eq(&source.original, &again.original));
        assert_eq!(source.source_record, again.source_record);
        assert!(source.matches_admission(&admitted));
        let policy_source = source.policy_source().unwrap();
        assert!(policy_source.selection == admitted.facts.selection);
        assert_eq!(policy_source.contributors, admitted.facts.contributors);
        // Even a recomputed row checksum cannot replace the actual retained
        // helper association with different policy-selection facts.
        let mut changed_row = source.source_record.as_ref().clone();
        let mut changed: HelperSourceRecord =
            serde_json::from_value(changed_row.data.body.clone()).unwrap();
        changed.facts.selection.helper.logical_executor = principal("foreign-helper");
        changed_row.data.body = serde_json::to_value(changed).unwrap();
        changed_row.content_digest = changed_row.data.digest().unwrap();
        let changed_source = RetainedHelperSource {
            original: source.original.clone(),
            source_record: Arc::new(changed_row),
            event_key: source.event_key.clone(),
            authentication: source.authentication.clone(),
        };
        assert!(matches!(changed_source.policy_source(), Err(OriginalWriterError::Conflict)));
        assert!(
            source.authentication != parent.association.candidate().original_authentication,
            "helper authentication remains distinct from its parent's authentication"
        );
        let held = owner.originals.lock().unwrap();
        let helper = held
            .iter()
            .filter_map(OriginalRecord::bound)
            .find(|entry| Arc::ptr_eq(&entry.retained, &source.original))
            .unwrap();
        assert!(helper.source_custody.is_none());
        drop(held);
        let facts = owner
            .helper_publication_source(&store, &service, &admitted, &source)
            .await
            .unwrap();
        assert_eq!(facts.admission, admitted.facts.admission);
        assert_eq!(facts.source.key, source.source_record.key);
        assert_eq!(facts.association, *source.original.association());
        assert_eq!(facts.contributors, vec![parent.association.clone(), ancestor.association.clone()]);
    }

    #[tokio::test]
    async fn split_helper_refuses_cancelled_ancestor_after_work_commit_and_keeps_exact_item() {
        let (directory, owner, base) = fixture();
        let ancestor = retained(&owner, &base, "ancestor", "user", &[]);
        let parent = retained(&owner, &base, "parent", "user", &[ancestor.clone()]);
        let service = WorkGraphService::with_namespace_grant(
            Arc::new(
                SqliteWorkGraphStore::open(directory.path().join("cancel-work.sqlite3")).unwrap(),
            ),
            WorkGraphNamespaceGrant::new("test-realm", "setup").unwrap(),
        )
        .unwrap();
        let store =
            toolkit_store::SqliteStore::open(&directory.path().join("cancel-source.sqlite3"))
                .unwrap();
        let (admitted, native_binding) = admitted_source_fixture(
            &owner,
            &service,
            &parent,
            vec![parent.clone(), ancestor.clone()],
            &base,
        )
        .await;
        // Establish every non-cancellation prerequisite before the negative.
        // The actual native child exists, while this source row remains pending.
        let native_binding = controller::stage_fixture_controller(
            &owner, &store, &admitted, native_binding,
        ).await;
        let saved = store.get(&admitted.source_key().unwrap()).await.unwrap().unwrap();
        assert_eq!(saved.revision, 1);
        assert!(HelperSourceRecord::decode(&saved).unwrap().bound().is_err());
        let child = native_binding.controller_grant_lineage.last().unwrap();
        assert_eq!(owner.grants.issued_reference(&child.grant_id).unwrap().as_ref(), Some(child));
        assert!(owner.observe_helper_binding(&store, &admitted).await.unwrap().is_none());
        owner
            .originals
            .lock()
            .unwrap()
            .iter_mut()
            .filter_map(OriginalRecord::bound_mut)
            .find(|entry| Arc::ptr_eq(&entry.retained, &ancestor))
            .unwrap()
            .operation = OriginalOperationDisposition::CancellationPending;
        assert!(matches!(
            owner
                .bind_admitted_helper(&store, &admitted, native_binding)
                .await,
            Err(OriginalWriterError::Unavailable)
        ));
        assert_eq!(store.get(&saved.key).await.unwrap(), Some(saved.clone()));
        assert!(HelperSourceRecord::decode(&saved).unwrap().bound().is_err());
        let original = admitted.original_work().unwrap();
        let mut namespace = parent.association().candidate().ingress_namespace.clone();
        namespace.occurrence_scope = original.work.clone();
        assert!(owner.original_disposition(&namespace, &original).is_err());
        assert!(owner.admitted_original_basis(&namespace, &original).unwrap().is_none());
        let observed = service
            .observe_admission(
                admitted.facts.admission.key.clone(),
                serde_json::from_value(admitted.facts.admitted_request.clone()).unwrap(),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(observed.item().unwrap().id, *admitted.item_id());
    }

    #[tokio::test]
    async fn helper_source_admission_keys_preserve_native_namespace_scope() {
        let directory = tempfile::tempdir().unwrap();
        let store =
            toolkit_store::SqliteStore::open(&directory.path().join("sources.sqlite3")).unwrap();
        let native_key = WorkAdmissionKey::new("same-key").unwrap();
        let mut records = Vec::new();
        for namespace in ["first", "second"] {
            let request = AdmissionRequest {
                key: StoreKey {
                    realm: "test-realm".into(),
                    owner: "toolkit/helper-originals/app".into(),
                    id: format!("helper-{namespace}"),
                },
                admission_key: source_admission_key(
                    "test-realm",
                    &WorkNamespace::new(namespace).unwrap(),
                    &native_key,
                )
                .unwrap(),
                data: RecordData {
                    schema: SCHEMA.into(),
                    version: 1,
                    body: serde_json::json!({"namespace": namespace}),
                },
                events: vec![],
                effects: vec![],
            };
            let first = store.admit(request.clone()).await.unwrap().record().clone();
            let replay = store.admit(request).await.unwrap().record().clone();
            assert_eq!(first, replay);
            records.push(first);
        }
        assert_ne!(records[0].key, records[1].key);
        for record in records {
            assert_eq!(
                store.get(&record.key).await.unwrap().as_ref(),
                Some(&record)
            );
        }
    }
}
