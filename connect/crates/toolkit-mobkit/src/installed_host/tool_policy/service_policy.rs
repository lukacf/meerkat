//! The existing application service decision's ordinary member-policy scope.
//! Candidate names are proposals. The native call target remains authoritative.
use super::*;
use meerkat_core::{CompiledMemberToolAction, CompiledMemberToolGrant, CompiledToolConsequence,
    PolicyDigest, PolicyId, PolicyProviderId, PolicyRevision};
use serde::{Deserialize, Serialize};
use toolkit_package::canonical_json;

/// Proposed exact names only. The actual original member, provider slot and
/// native catalog session are derived by the retained service-task owner.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceToolPolicySelection {
    pub tools: Vec<ServiceToolInvocation>,
    /// Select both catalog controls with this explicit application consequence.
    /// Native catalog authority remains separate from ordinary Invoke grants.
    pub catalog: Option<CompiledToolConsequence>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceToolInvocation {
    pub name: String,
    pub consequence: CompiledToolConsequence,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberPolicyLifetime {
    /// Current authority lives in the application owner's canonical slot. Task
    /// expiry is not revocation. An externally revocable approval cannot be
    /// represented by this value without the owner's current recovery check.
    UntilSuperseded,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceToolPolicyVersion {
    pub revision: PolicyRevision,
    pub digest: PolicyDigest,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceToolPolicyProposal {
    pub provider_id: PolicyProviderId,
    pub policy_id: PolicyId,
    pub member: String,
    pub selection: ServiceToolPolicySelection,
    pub catalog_session: Option<String>,
    pub prior: ServiceToolPolicyVersion,
    pub candidate: ServiceToolPolicyVersion,
    /// Exact complete successor, including preserved members/grants. Approval
    /// covers this whole candidate, not just selection.
    pub candidate_canonical_json: String,
    pub lifetime: MemberPolicyLifetime,
}
impl ServiceToolPolicyProposal {
    pub fn sha256(&self) -> Result<String, HostLifetimeError> {
        use sha2::{Digest, Sha256};
        canonical_json(self).map(|bytes| format!("{:x}", Sha256::digest(bytes)))
            .map_err(|_| HostLifetimeError::InvalidSelection)
    }
}

/// An affirmative acknowledgement from the trusted application decision, not
/// an input accepted from an agent. The current reservation still authenticates
/// the producer. Copying this value or historical evidence grants nothing.
#[derive(Clone)]
pub struct ServiceToolPolicyApproval {
    proposal_sha256: String,
    human_review: Option<meerkat_core::ApprovalRecord>,
}
impl ServiceToolPolicyApproval {
    pub fn until_superseded(proposal: &ServiceToolPolicyProposal) -> Result<Self, HostLifetimeError> {
        Ok(Self { proposal_sha256: proposal.sha256()?, human_review: None })
    }
    pub fn proposal_sha256(&self) -> &str { &self.proposal_sha256 }
    pub(crate) fn reviewed(proposal: &ServiceToolPolicyProposal, record: meerkat_core::ApprovalRecord)
        -> Result<Self, HostLifetimeError> {
        Ok(Self { proposal_sha256: proposal.sha256()?, human_review: Some(record) })
    }
    pub(crate) fn check(&self, proposal: &ServiceToolPolicyProposal) -> Result<(), HostLifetimeError> {
        if self.proposal_sha256 != proposal.sha256()? { return Err(HostLifetimeError::Denied); }
        Ok(())
    }
}

impl ServiceToolPolicySelection {
    pub(crate) fn grants(&self) -> Result<Vec<CompiledMemberToolGrant>, HostLifetimeError> {
        if self.tools.len() > 128 || (self.tools.is_empty() && self.catalog.is_none()) {
            return Err(HostLifetimeError::InvalidSelection);
        }
        let mut grants = self.tools.iter().map(|tool| CompiledMemberToolGrant {
            tool_name: tool.name.clone(), action: CompiledMemberToolAction::Invoke,
            consequence: tool.consequence.clone(),
        }).collect::<Vec<_>>();
        if let Some(consequence) = &self.catalog {
            for name in ["tool_catalog_search", "tool_catalog_load"] {
                grants.push(CompiledMemberToolGrant { tool_name: name.into(),
                    action: CompiledMemberToolAction::Invoke, consequence: consequence.clone() });
            }
        }
        grants.sort_by(|a, b| a.tool_name.cmp(&b.tool_name));
        if grants.iter().any(|grant| grant.tool_name.is_empty() || grant.tool_name.len() > 512
            || grant.tool_name.trim() != grant.tool_name || grant.tool_name.chars().any(char::is_control))
            || grants.windows(2).any(|pair| pair[0].tool_name == pair[1].tool_name)
        { return Err(HostLifetimeError::InvalidSelection); }
        Ok(grants)
    }
}

/// Current canonical authority retained by the same service-decision owner.
/// Values received from elsewhere cannot instantiate an accepted slot decision.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActiveMemberToolPolicyDecision {
    pub approval_id: String,
    pub owner: meerkat_core::PrincipalRef,
    pub realm: String,
    pub deployment: String,
    pub original: crate::private_originals::PrivateOriginalRef,
    pub acquisition: toolkit_store::StoreKey,
    pub proposal: ServiceToolPolicyProposal,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub human_review: Option<meerkat_core::ApprovalRecord>,
}

/// Supplied by the actual canonical owner before native configuration leaves
/// preparation. No final host lifetime or expired task request is fabricated.
pub struct ApplicationToolPolicyRecoveryRequest<'a> {
    pub decision: &'a ActiveMemberToolPolicyDecision,
    pub canonical_json: &'a [u8],
}
pub enum ApplicationToolPolicyRecoveryDecision {
    RequiresHuman,
    Permit(Box<dyn crate::private_originals::ApplicationServicePolicyReservation>),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceToolPolicyNativeOutcome { Accepted, Refused }
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceToolPolicyObservation {
    pub proposal_sha256: String,
    pub candidate: ServiceToolPolicyVersion,
    pub native: Option<ServiceToolPolicyNativeOutcome>,
    /// Observed durable promotion, not a promise that no successor follows.
    pub retained_as_current: bool,
}

use crate::private_originals::{ApplicationServicePolicy, ApplicationServicePolicyReservation,
    PrivateOriginalRef};
use meerkat_core::{CompiledPolicySourceProvenance, PrincipalRef};
use std::sync::Mutex;
use toolkit_store::{AdmissionRequest, EventWrite, RecordCondition, RecordData, StoreKey,
    StoreScope, StoredRecord, ToolkitStore, Transition};

const SCHEMA: &str = "toolkit.application-service-tool-policy";
const VERSION: u32 = 1;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Slot {
    bootstrap: String,
    current: String,
    decision: Option<ActiveMemberToolPolicyDecision>,
    pending: Option<Pending>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    decision: ActiveMemberToolPolicyDecision,
    original: PrivateOriginalRef,
    acquisition: StoreKey,
    entered: bool,
    observation: ServiceToolPolicyObservation,
}

/// This is the ordinary-policy part of the existing application service owner.
/// The native source owner retains the physical store lease. All participants
/// share this one provider and exclusion; Store leases cannot replace either.
pub(crate) struct ServicePolicyOwner {
    store: Arc<dyn ToolkitStore>,
    key: StoreKey,
    application: PrincipalRef,
    provider: Arc<CompiledPolicyProvider>,
    bootstrap: String,
    participant: Option<Arc<dyn ApplicationServicePolicy>>,
    exclusion: Arc<tokio::sync::Mutex<()>>,
    observed_current: Mutex<Option<Arc<ObservedPolicy>>>,
    // Actual return retained until its terminal Store write settles. This is
    // not a second journal; a process loss before persistence stays unknown.
    known_refusal: Mutex<Option<String>>,
}
struct ObservedPolicy {
    canonical: String,
    // This reservation is for the durable complete policy decision. It is
    // never the task/login reservation that approved the initial service use.
    reservation: Option<Arc<dyn ApplicationServicePolicyReservation>>,
}

pub(crate) struct PreparedServiceToolPolicy {
    owner: Arc<ServicePolicyOwner>,
    prior: StoredRecord,
    pub(crate) proposal: ServiceToolPolicyProposal,
    original: PrivateOriginalRef,
    acquisition: StoreKey,
    already_current: bool,
}

/// Retained inside the existing service operation, never a borrowed result
/// waiter. Dropping the guard does not erase an entered/pending durable intent.
pub(crate) struct ServiceToolPolicyTransaction {
    owner: Arc<ServicePolicyOwner>,
    stored: StoredRecord,
    slot: Slot,
    _exclusion: tokio::sync::OwnedMutexGuard<()>,
    observation: ServiceToolPolicyObservation,
    member_reservation: Arc<dyn ApplicationServicePolicyReservation>,
    prior_observed: Option<Arc<ObservedPolicy>>,
}

fn invalid() -> HostLifetimeError { HostLifetimeError::InvalidSelection }
fn unavailable() -> HostLifetimeError { HostLifetimeError::Unavailable }
fn sha(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}
fn version(bytes: &[u8]) -> Result<ServiceToolPolicyVersion, HostLifetimeError> {
    let policy = CompiledApplicationToolPolicy::parse_canonical_json(bytes).map_err(|_| invalid())?;
    Ok(ServiceToolPolicyVersion { revision: policy.revision,
        digest: PolicyDigest::from_canonical_bytes(bytes) })
}
fn encode(slot: &Slot) -> Result<RecordData, HostLifetimeError> {
    let data = RecordData { schema: SCHEMA.into(), version: VERSION,
        body: serde_json::to_value(slot).map_err(|_| invalid())? };
    data.validate().map_err(|_| invalid())?;
    Ok(data)
}
fn event(kind: &str, slot: &Slot) -> Result<EventWrite, HostLifetimeError> {
    Ok(EventWrite { kind: kind.into(), data: encode(slot)? })
}

impl ServicePolicyOwner {
    /// Read this same policy owner's retained outcome. Grant cleanup never
    /// alters a pending candidate, current policy, or later successor.
    pub(crate) async fn recovery_observation(&self, proposal: &ServiceToolPolicyProposal,
        original: &PrivateOriginalRef, acquisition: &StoreKey) -> Result<&'static str, HostLifetimeError>
    {
        let (_, slot) = self.read().await?;
        let matches = |decision: &ActiveMemberToolPolicyDecision| decision.proposal == *proposal
            && decision.original == *original && decision.acquisition == *acquisition;
        if slot.pending.as_ref().is_some_and(|pending| matches(&pending.decision)) { return Ok("pending"); }
        if slot.decision.as_ref().is_some_and(matches) { return Ok("retained_current"); }
        Ok("superseded_or_unobserved")
    }

    pub(crate) async fn open(store: Arc<dyn ToolkitStore>, scope: StoreScope,
        application: PrincipalRef, provider: Arc<CompiledPolicyProvider>, bootstrap: String,
        participant: Option<Arc<dyn ApplicationServicePolicy>>,
    ) -> Result<Arc<Self>, HostLifetimeError> {
        let policy = CompiledApplicationToolPolicy::parse_canonical_json(bootstrap.as_bytes()).map_err(|_| invalid())?;
        let identity = canonical_json(&(SCHEMA, &policy.provider_id, &policy.policy_id)).map_err(|_| invalid())?;
        let key = StoreKey { realm: scope.realm, owner: scope.owner, id: format!("service-tool-policy-{}", sha(&identity)) };
        let initial = Slot { bootstrap: bootstrap.clone(), current: bootstrap.clone(), decision: None, pending: None };
        let admitted = store.admit(AdmissionRequest { key: key.clone(), admission_key: key.id.clone(),
            data: encode(&initial)?, events: vec![event("bootstrap", &initial)?], effects: vec![] })
            .await.map_err(|_| unavailable())?;
        let owner = Arc::new(Self { store, key, application, provider, bootstrap, participant,
            exclusion: Arc::new(tokio::sync::Mutex::new(())), observed_current: Mutex::new(None),
            known_refusal: Mutex::new(None) });
        owner.decode(admitted.record())?;
        owner.recover().await?;
        Ok(owner)
    }

    pub(crate) fn same_participant(&self, participant: &Arc<dyn ApplicationServicePolicy>) -> bool {
        self.participant.as_ref().is_some_and(|selected| Arc::ptr_eq(selected, participant))
    }
    pub(crate) fn same_binding(&self, store: &Arc<dyn ToolkitStore>, scope: &StoreScope,
        application: &PrincipalRef, participant: &Option<Arc<dyn ApplicationServicePolicy>>,
    ) -> bool {
        Arc::ptr_eq(&self.store, store) && self.key.scope() == *scope && &self.application == application
            && match (&self.participant, participant) {
                (None, None) => true,
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                _ => false,
            }
    }
    pub(crate) fn is_observed_current(&self, bytes: &[u8]) -> Result<bool, HostLifetimeError> {
        self.check_current()?;
        Ok(self.observed_current.lock().map_err(|_| unavailable())?.as_ref()
            .is_some_and(|current| current.canonical.as_bytes() == bytes))
    }
    pub(crate) fn check_current(&self) -> Result<(), HostLifetimeError> {
        let selected = self.observed_current.lock().map_err(|_| unavailable())?.clone().ok_or_else(unavailable)?;
        if let Some(reservation) = &selected.reservation {
            if let Err(error) = reservation.check_current() {
                let mut observed = self.observed_current.lock().map_err(|_| unavailable())?;
                if observed.as_ref().is_some_and(|current| Arc::ptr_eq(current, &selected)) { *observed = None; }
                return Err(error);
            }
        }
        if !self.observed_current.lock().map_err(|_| unavailable())?.as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, &selected)) { return Err(unavailable()); }
        Ok(())
    }
    fn acquisition_key_matches(&self, acquisition: &StoreKey) -> bool {
        let (Ok(realm), Ok(deployment), Ok(operation)) = (
            toolkit_package::Id::new(self.key.realm.clone()),
            toolkit_package::Id::new(self.key.owner.clone()),
            toolkit_package::Id::new(acquisition.id.clone()),
        ) else { return false; };
        meerkat_toolkit::setup::SetupScope::new(realm, deployment).key(&operation) == *acquisition
    }
    fn decision(&self, decision: &ActiveMemberToolPolicyDecision, bytes: &[u8]) -> Result<(), HostLifetimeError> {
        let proposal = &decision.proposal;
        let policy = CompiledApplicationToolPolicy::parse_canonical_json(bytes).map_err(|_| invalid())?;
        if decision.owner != self.application || decision.realm != self.key.realm
            || decision.deployment != self.key.owner || !self.acquisition_key_matches(&decision.acquisition)
            || decision.approval_id != proposal.sha256()?
            || proposal.candidate != version(bytes)? || proposal.candidate_canonical_json.as_bytes() != bytes
            || proposal.provider_id != policy.provider_id || proposal.policy_id != policy.policy_id
            || !policy.members.iter().any(|member| member.member_identity == proposal.member)
            || proposal.selection.catalog.is_some() != proposal.catalog_session.is_some()
        { return Err(invalid()); }
        proposal.selection.grants()?;
        Ok(())
    }
    fn decode(&self, stored: &StoredRecord) -> Result<Slot, HostLifetimeError> {
        if stored.key != self.key || stored.data.schema != SCHEMA || stored.data.version != VERSION {
            return Err(invalid());
        }
        let slot: Slot = serde_json::from_value(stored.data.body.clone()).map_err(|_| invalid())?;
        if slot.bootstrap != self.bootstrap { return Err(invalid()); }
        let bootstrap = CompiledApplicationToolPolicy::parse_canonical_json(self.bootstrap.as_bytes()).map_err(|_| invalid())?;
        let current = CompiledApplicationToolPolicy::parse_canonical_json(slot.current.as_bytes()).map_err(|_| invalid())?;
        if current.provider_id != bootstrap.provider_id || current.policy_id != bootstrap.policy_id {
            return Err(invalid());
        }
        match &slot.decision {
            Some(decision) => self.decision(decision, slot.current.as_bytes())?,
            None if slot.current != slot.bootstrap => return Err(invalid()),
            None => {},
        }
        if let Some(pending) = &slot.pending {
            self.decision(&pending.decision, pending.decision.proposal.candidate_canonical_json.as_bytes())?;
            let proposal = &pending.decision.proposal;
            if proposal.provider_id != current.provider_id || proposal.policy_id != current.policy_id
                || proposal.candidate_canonical_json.as_bytes() != self.candidate(&slot.current, &proposal.member,
                    &proposal.selection, &pending.original, &pending.acquisition)?.as_slice()
                || proposal.prior != version(slot.current.as_bytes())?
                || pending.observation.proposal_sha256 != pending.decision.approval_id
                || pending.observation.candidate != pending.decision.proposal.candidate
                || pending.observation.retained_as_current
                || pending.observation.native == Some(ServiceToolPolicyNativeOutcome::Refused)
                || !self.acquisition_key_matches(&pending.acquisition)
                || pending.original != pending.decision.original || pending.acquisition != pending.decision.acquisition
            { return Err(invalid()); }
        }
        Ok(slot)
    }
    async fn read(&self) -> Result<(StoredRecord, Slot), HostLifetimeError> {
        let stored = self.store.get(&self.key).await.map_err(|_| unavailable())?.ok_or_else(unavailable)?;
        let slot = self.decode(&stored)?;
        Ok((stored, slot))
    }
    async fn change(&self, stored: &StoredRecord, slot: &Slot, kind: &str) -> Result<StoredRecord, HostLifetimeError> {
        self.store.compare_and_swap(Transition { key: self.key.clone(), expected_revision: stored.revision,
            expected_digest: Some(stored.content_digest.clone()), claim: None,
            data: encode(slot)?, events: vec![event(kind, slot)?], effects: vec![] })
            .await.map_err(|_| unavailable())
    }
    fn recovery_reservation(&self, decision: &ActiveMemberToolPolicyDecision, bytes: &[u8])
        -> Result<Box<dyn ApplicationServicePolicyReservation>, HostLifetimeError> {
        let participant = self.participant.as_ref().ok_or_else(unavailable)?;
        let reserved = match participant.reserve_tool_policy_recovery(&ApplicationToolPolicyRecoveryRequest {
            decision, canonical_json: bytes,
        })? {
            ApplicationToolPolicyRecoveryDecision::RequiresHuman => return Err(unavailable()),
            ApplicationToolPolicyRecoveryDecision::Permit(reserved) => reserved,
        };
        if reserved.owner() != &self.application { return Err(HostLifetimeError::WrongOwner); }
        reserved.check_current()?;
        Ok(reserved)
    }

    async fn settle_refusal(&self, stored: &StoredRecord, slot: &mut Slot)
        -> Result<StoredRecord, HostLifetimeError> {
        let pending = slot.pending.as_mut().ok_or_else(invalid)?;
        let id = pending.decision.approval_id.clone();
        pending.observation.native = Some(ServiceToolPolicyNativeOutcome::Refused);
        *self.known_refusal.lock().map_err(|_| unavailable())? = Some(id.clone());
        let refused = slot.clone();
        let mut next = slot.clone();
        next.pending = None;
        let saved = self.store.compare_and_swap(Transition {
            key: self.key.clone(), expected_revision: stored.revision,
            expected_digest: Some(stored.content_digest.clone()), claim: None,
            data: encode(&next)?, events: vec![event("native-refused", &refused)?], effects: vec![],
        }).await.map_err(|_| unavailable())?;
        *slot = next;
        let mut known = self.known_refusal.lock().map_err(|_| unavailable())?;
        if known.as_deref() == Some(id.as_str()) { *known = None; }
        Ok(saved)
    }
    fn activate_current(&self, slot: &Slot) -> Result<(), HostLifetimeError> {
        let reservation = slot.decision.as_ref().map(|decision| self.recovery_reservation(decision,
            slot.current.as_bytes()).map(Arc::<dyn ApplicationServicePolicyReservation>::from)).transpose()?;
        if let Some(reservation) = &reservation { reservation.check_current()?; }
        self.provider.accept_canonical_json(slot.current.as_bytes()).map_err(|_| unavailable())?;
        if let Some(reservation) = &reservation { reservation.check_current()?; }
        *self.observed_current.lock().map_err(|_| unavailable())? = Some(Arc::new(ObservedPolicy {
            canonical: slot.current.clone(), reservation,
        }));
        Ok(())
    }

    /// Reconcile the exact active whole-candidate decision. Pending takes
    /// precedence over prior, unless this same surviving owner knows the native
    /// refusal and must finish retaining it without re-entering that candidate.
    pub(crate) async fn recover(&self) -> Result<(), HostLifetimeError> {
        let _exclusion = self.exclusion.lock().await;
        *self.observed_current.lock().map_err(|_| unavailable())? = None;
        let (stored, mut slot) = self.read().await?;
        let Some(pending) = &slot.pending else {
            *self.known_refusal.lock().map_err(|_| unavailable())? = None;
            return self.activate_current(&slot);
        };
        let known = self.known_refusal.lock().map_err(|_| unavailable())?.clone();
        if known.as_ref().is_some_and(|id| id != &pending.decision.approval_id) { return Err(unavailable()); }
        if known.is_some() {
            let settled = self.settle_refusal(&stored, &mut slot).await;
            // Permission for prior is revalidated independently. Neither the
            // failed task nor the refused candidate can revive that authority.
            self.activate_current(&slot)?;
            settled?;
            return Ok(());
        }
        let bytes = pending.decision.proposal.candidate_canonical_json.clone();
        let reservation: Arc<dyn ApplicationServicePolicyReservation> = Arc::from(
            self.recovery_reservation(&pending.decision, bytes.as_bytes())?);
        reservation.check_current()?;
        if self.provider.accept_canonical_json(bytes.as_bytes()).is_err() {
            let settled = self.settle_refusal(&stored, &mut slot).await;
            self.activate_current(&slot)?;
            settled?;
            return Ok(());
        }
        let pending = slot.pending.take().ok_or_else(invalid)?;
        slot.current = bytes.clone();
        slot.decision = Some(pending.decision);
        self.change(&stored, &slot, "recovered-current").await?;
        reservation.check_current()?;
        *self.observed_current.lock().map_err(|_| unavailable())? = Some(Arc::new(ObservedPolicy {
            canonical: bytes, reservation: Some(reservation),
        }));
        Ok(())
    }

    fn candidate(&self, current_bytes: &str, member: &str, selection: &ServiceToolPolicySelection,
        original: &PrivateOriginalRef, acquisition: &StoreKey,
    ) -> Result<Vec<u8>, HostLifetimeError> {
        let current = CompiledApplicationToolPolicy::parse_canonical_json(current_bytes.as_bytes()).map_err(|_| invalid())?;
        let mut members = current.members.clone();
        let selected_member = members.iter_mut().find(|row| row.member_identity == member).ok_or_else(invalid)?;
        for grant in selection.grants()? {
            if let Some(existing) = selected_member.grants.iter().find(|row| row.tool_name == grant.tool_name) {
                if existing != &grant { return Err(HostLifetimeError::Denied); }
            } else { selected_member.grants.push(grant); }
        }
        selected_member.grants.sort_by(|a,b| a.tool_name.cmp(&b.tool_name));
        let source = canonical_json(&(&self.key, original, acquisition, member, selection, current_bytes)).map_err(|_| invalid())?;
        CompiledApplicationToolPolicy::new(current.provider_id.clone(), current.policy_id.clone(),
            PolicyRevision(current.revision.0.checked_add(1).ok_or_else(invalid)?),
            CompiledPolicySourceProvenance { source_id: "toolkit-application-service-decision".into(),
                source_digest: PolicyDigest::from_canonical_bytes(&source) }, members).map_err(|_| invalid())?
            .canonical_json().map_err(|_| invalid())
    }

    pub(crate) async fn prepare(self: &Arc<Self>, member: &str, selection: ServiceToolPolicySelection,
        catalog_session: Option<String>, original: PrivateOriginalRef, acquisition: StoreKey,
    ) -> Result<PreparedServiceToolPolicy, HostLifetimeError> {
        // Any previous operation must have released the same exclusion. A
        // pending durable decision can only settle under its current owner.
        self.recover().await?;
        let _exclusion = self.exclusion.lock().await;
        let (prior, slot) = self.read().await?;
        if slot.pending.is_some() || selection.catalog.is_some() != catalog_session.is_some() {
            return Err(unavailable());
        }
        if let Some(decision) = &slot.decision {
            if decision.original == original && decision.acquisition == acquisition {
                if decision.proposal.member != member || decision.proposal.selection != selection
                    || decision.proposal.catalog_session != catalog_session { return Err(HostLifetimeError::Denied); }
                return Ok(PreparedServiceToolPolicy { owner: self.clone(), prior,
                    proposal: decision.proposal.clone(), original, acquisition, already_current: true });
            }
        }
        let current = CompiledApplicationToolPolicy::parse_canonical_json(slot.current.as_bytes()).map_err(|_| invalid())?;
        let candidate = self.candidate(&slot.current, member, &selection, &original, &acquisition)?;
        let proposal = ServiceToolPolicyProposal { provider_id: current.provider_id, policy_id: current.policy_id,
            member: member.into(), selection, catalog_session, prior: version(slot.current.as_bytes())?,
            candidate: version(&candidate)?, candidate_canonical_json: String::from_utf8(candidate).map_err(|_| invalid())?,
            lifetime: MemberPolicyLifetime::UntilSuperseded };
        Ok(PreparedServiceToolPolicy { owner: self.clone(), prior, proposal, original, acquisition, already_current: false })
    }
}

impl PreparedServiceToolPolicy {
    pub(crate) async fn begin(self, approval: &ServiceToolPolicyApproval,
        current: &(impl Fn() -> Result<(), HostLifetimeError> + Sync),
    ) -> Result<ServiceToolPolicyTransaction, HostLifetimeError> {
        approval.check(&self.proposal)?;
        let exclusion = self.owner.exclusion.clone().lock_owned().await;
        current()?;
        let (stored, mut slot) = self.owner.read().await?;
        if stored.revision != self.prior.revision || stored.content_digest != self.prior.content_digest
            || slot.pending.is_some() { return Err(HostLifetimeError::Denied); }
        if self.already_current {
            let decision = slot.decision.as_ref().ok_or_else(invalid)?;
            if decision.proposal != self.proposal || decision.original != self.original
                || decision.acquisition != self.acquisition { return Err(HostLifetimeError::Denied); }
            let member_reservation = Arc::from(self.owner.recovery_reservation(decision, slot.current.as_bytes())?);
            let observation = ServiceToolPolicyObservation { proposal_sha256: self.proposal.sha256()?,
                candidate: self.proposal.candidate.clone(), native: Some(ServiceToolPolicyNativeOutcome::Accepted),
                retained_as_current: true };
            let prior_observed = self.owner.observed_current.lock().map_err(|_| unavailable())?.clone();
            return Ok(ServiceToolPolicyTransaction { owner: self.owner, stored, slot, _exclusion: exclusion,
                observation, member_reservation, prior_observed });
        }
        if version(slot.current.as_bytes())? != self.proposal.prior { return Err(HostLifetimeError::Denied); }
        let observation = ServiceToolPolicyObservation { proposal_sha256: self.proposal.sha256()?,
            candidate: self.proposal.candidate.clone(), native: None, retained_as_current: false };
        let decision = ActiveMemberToolPolicyDecision {
            approval_id: observation.proposal_sha256.clone(), owner: self.owner.application.clone(),
            realm: self.owner.key.realm.clone(), deployment: self.owner.key.owner.clone(), proposal: self.proposal,
            human_review: approval.human_review.clone(), original: self.original.clone(), acquisition: self.acquisition.clone(),
        };
        let member_reservation: Arc<dyn ApplicationServicePolicyReservation> = Arc::from(self.owner
            .recovery_reservation(&decision, decision.proposal.candidate_canonical_json.as_bytes())?);
        slot.pending = Some(Pending { decision, original: self.original, acquisition: self.acquisition,
            entered: false, observation: observation.clone() });
        current()?;
        let stored = self.owner.change(&stored, &slot, "approved-pending").await?;
        let prior_observed = self.owner.observed_current.lock().map_err(|_| unavailable())?.clone();
        Ok(ServiceToolPolicyTransaction { owner: self.owner, stored, slot, _exclusion: exclusion,
            observation, member_reservation, prior_observed })
    }
}
impl ServiceToolPolicyTransaction {
    pub(crate) fn observation(&self) -> ServiceToolPolicyObservation { self.observation.clone() }
    pub(crate) async fn publish(&mut self, current: &(impl Fn() -> Result<(), HostLifetimeError> + Sync))
        -> Result<RecordCondition, HostLifetimeError> {
        current()?;
        self.member_reservation.check_current()?;
        if self.slot.pending.is_none() {
            if !self.observation.retained_as_current || !self.owner.is_observed_current(self.slot.current.as_bytes())? {
                return Err(unavailable());
            }
            return Ok(RecordCondition { key: self.stored.key.clone(), revision: self.stored.revision,
                content_digest: self.stored.content_digest.clone(), claim: None, dispatch_claim: None });
        }
        let pending = self.slot.pending.as_mut().ok_or_else(invalid)?;
        pending.entered = true;
        self.stored = self.owner.change(&self.stored, &self.slot, "entered").await?;
        current()?;
        let bytes = self.slot.pending.as_ref().ok_or_else(invalid)?.decision.proposal.candidate_canonical_json.clone();
        self.member_reservation.check_current()?;
        *self.owner.observed_current.lock().map_err(|_| unavailable())? = None;
        match self.owner.provider.accept_canonical_json(bytes.as_bytes()) {
            Ok(()) => { self.observation.native = Some(ServiceToolPolicyNativeOutcome::Accepted); }
            Err(_) => {
                self.observation.native = Some(ServiceToolPolicyNativeOutcome::Refused);
                let retained = self.owner.settle_refusal(&self.stored, &mut self.slot).await;
                // This call definitely left prior unchanged. Restore only
                // its still-current durable owner reservation, independently
                // of whether the refusal write acknowledgment was available.
                if let Some(prior) = &self.prior_observed {
                    if prior.canonical != self.slot.current { return Err(unavailable()); }
                    if let Some(reservation) = &prior.reservation { reservation.check_current()?; }
                    *self.owner.observed_current.lock().map_err(|_| unavailable())? = Some(prior.clone());
                }
                self.stored = retained?;
                return Err(HostLifetimeError::Denied);
            }
        }
        // Even cancellation after acceptance cannot skip retaining its effect.
        let pending = self.slot.pending.take().ok_or_else(invalid)?;
        self.slot.current = bytes.clone();
        self.slot.decision = Some(pending.decision);
        self.stored = self.owner.change(&self.stored, &self.slot, "accepted-current").await?;
        self.observation.retained_as_current = true;
        current()?;
        self.member_reservation.check_current()?;
        *self.owner.observed_current.lock().map_err(|_| unavailable())? = Some(Arc::new(ObservedPolicy {
            canonical: bytes, reservation: Some(self.member_reservation.clone()),
        }));
        Ok(RecordCondition { key: self.stored.key.clone(), revision: self.stored.revision,
            content_digest: self.stored.content_digest.clone(), claim: None, dispatch_claim: None })
    }
}

#[cfg(test)]
mod tests;
