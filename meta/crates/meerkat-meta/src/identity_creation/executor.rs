//! The executor seam: commit this normalized request under this consent, as
//! this requester, in this mob.
//!
//! [`IdentityChangeExecutor::commit`] is the one place an identity change
//! reaches Core's change path. It takes the normalized request
//! ([`IdentityRequest`]), the operation it commits as ([`OperationId`]), who
//! asks as the transport attributed it ([`ChangeRequester`]), the consent it
//! commits under ([`ChangeConsent`]: the host's rule, a decision's execution
//! permit, or imported consent) and the mob (Core's handle to it), and
//! answers the typed outcome. It decides no policy: which consent a request
//! gets is its caller's, from the host's rules, and a held decision is its
//! holder's.
//!
//! Before Core is called the seam refuses, for every requester:
//! - a requester that is not the one the request records
//!   (`invalid_request`);
//! - a member private to a person (`private_requester`);
//! - a change, other than an adoption, of an agent its host configures
//!   (`owner_read_only`).
//!
//! The first implementation, [`CoreChangeExecutor`], commits a create, an
//! archive, an unarchive and a removal through Core's handle. An update and
//! an adoption answer `unsupported` until their bodies are filled: see the
//! functions marked `BODY TO FILL`.
//!
//! After a commit the seam calls the instruction step it was given
//! ([`InstructionStep`]), twice: after an update that changed an agent's
//! instructions text, whose answer is the update's instruction outcome, and
//! after a removal, so what the step holds for the agent ends with it. An
//! archive calls nothing: an archived agent keeps what the step holds, so
//! its unarchive resumes on its current instructions.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::SessionId;
use meerkat_mob::{
    IdentityChangeEntryOutcome, IdentityChangeOutcome, IdentityChangeReceipt,
    IdentityChangeRefusal, IdentityNameHolder, IdentityOperation, IdentityPolicyOutcome, MobError,
    MobHandle,
};

use super::approval::{ApprovalPlan, PlannedOutcome};
use super::digest::Sha256Digest;
use super::operation::OperationId;
use super::outcome::{
    AgentChangeState, AgentOutcome, AgentSeat, IdentityOutcome, InstructionOutcome,
    InstructionsChanged,
};
use super::policy::PreparationRule;
use super::refusal::{
    IdentityRefusal, IdentityRefusalKind, NameTakenBy, NotAdoptableReason, RequestProblem,
};
use super::request::{
    IdentityRequest, RequestAction, Requester, UpdatedSpec, core_member_name,
    identity_name_of_member,
};

mod core_commit;

/// The consent a change commits under, as Core's commit takes it: the
/// host's rule committed it without a decision (`Policy`, `allow` or
/// `notify`), a decision's execution permit (`Decided`: Core refuses a
/// permit that binds another operation or request, or is past its deadline),
/// or consent decided outside Meerkat, which only a host operator's import
/// carries (`Imported`).
pub use meerkat_mob::IdentityChangeConsentInput as ChangeConsent;

/// The consent the approval plan `plan` gives without a decision: `allow`
/// and `notify` commit at once. `None` for a plan that holds a decision: its
/// consent is the decision's execution permit.
pub fn consent_of_plan(plan: &ApprovalPlan) -> Option<ChangeConsent<'static>> {
    match plan.outcome {
        PlannedOutcome::Allow => Some(ChangeConsent::Policy(IdentityPolicyOutcome::Allow)),
        PlannedOutcome::Notify { .. } => Some(ChangeConsent::Policy(IdentityPolicyOutcome::Notify)),
        PlannedOutcome::Review { .. } | PlannedOutcome::RequireApproval { .. } => None,
    }
}

/// How an agent requester's change is admitted by its ordinary spawn grant.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AgentAdmission {
    /// By the live grant of the session its tool call ran in, as the
    /// dispatch that carries the call names it, never a value from the
    /// call's arguments.
    Session(SessionId),
    /// From the recorded requester identity: a change that waited for its
    /// decision, or is carried out after a restart. It reads no session and
    /// wakes nothing.
    Recorded,
}

/// Who asks for a change, as the transport attributed it: what the seam
/// commits "as". The request records the same requester
/// ([`Self::recorded`]); what is here besides is what only the transport
/// knows.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ChangeRequester {
    /// A member agent of the mob.
    Agent {
        /// Its identity.
        identity: String,
        /// How its grant is read.
        admission: AgentAdmission,
        /// Whether it is a member private to a person. Such a member asks
        /// for no change: the seam refuses it `private_requester`.
        private: bool,
    },
    /// The host, on one of its surfaces.
    Host {
        /// The surface (`console` for the console).
        surface: String,
        /// Whether the caller is the host's operator: only an operator's
        /// change commits under imported consent. The console never is.
        operator: bool,
    },
    /// An authenticated human, through a host surface.
    Human {
        /// The human's principal key.
        principal: String,
    },
}

impl ChangeRequester {
    /// The member agent `identity`, by the session its tool call ran in.
    pub fn agent_session(identity: impl Into<String>, session: SessionId) -> Self {
        Self::Agent {
            identity: identity.into(),
            admission: AgentAdmission::Session(session),
            private: false,
        }
    }

    /// The member agent `identity`, by its recorded identity.
    pub fn agent_recorded(identity: impl Into<String>) -> Self {
        Self::Agent {
            identity: identity.into(),
            admission: AgentAdmission::Recorded,
            private: false,
        }
    }

    /// The host on `surface`, not its operator.
    pub fn host(surface: impl Into<String>) -> Self {
        Self::Host {
            surface: surface.into(),
            operator: false,
        }
    }

    /// The host's operator on `surface`.
    pub fn host_operator(surface: impl Into<String>) -> Self {
        Self::Host {
            surface: surface.into(),
            operator: true,
        }
    }

    /// The authenticated human `principal`.
    pub fn human(principal: impl Into<String>) -> Self {
        Self::Human {
            principal: principal.into(),
        }
    }

    /// This requester, marked a member private to a person when `private`
    /// (an agent requester only; the host and a human are no members).
    #[must_use]
    pub fn private_to_a_person(mut self, private: bool) -> Self {
        if let Self::Agent {
            private: recorded, ..
        } = &mut self
        {
            *recorded = private;
        }
        self
    }

    /// The requester as a request records it.
    pub fn recorded(&self) -> Requester {
        match self {
            Self::Agent { identity, .. } => Requester::Agent {
                identity: identity.clone(),
            },
            Self::Host { surface, .. } => Requester::Host {
                surface: surface.clone(),
            },
            Self::Human { principal } => Requester::Human {
                principal: principal.clone(),
            },
        }
    }

    /// Whether the requester is a member private to a person.
    pub fn is_private_member(&self) -> bool {
        matches!(self, Self::Agent { private: true, .. })
    }
}

/// One commit through the seam: the normalized request, the operation it
/// commits as, who asks, the consent, and the mob.
#[derive(Clone, Copy)]
#[non_exhaustive]
pub struct ChangeCommit<'a> {
    /// The mob: Core's handle to it.
    pub mob: &'a MobHandle,
    /// The operation the change commits as ([`OperationId::for_request`]).
    pub operation_id: &'a OperationId,
    /// Who asks.
    pub requester: &'a ChangeRequester,
    /// The consent the change commits under.
    pub consent: ChangeConsent<'a>,
    /// The normalized request. Its digest is the operation's request
    /// digest, which a decision's permit binds.
    pub request: &'a IdentityRequest,
    /// Whether the agent the request changes is one its host configures: a
    /// run-time change of it, other than its adoption, is refused
    /// `owner_read_only` before Core is called. Never set for a create.
    pub configured_target: bool,
    /// How the host prepares an agent of each profile before its first use,
    /// by profile, from its policy (`IdentityCreationPolicy::preparations`).
    /// A create commits each agent with its profile's mode: an agent of a
    /// profile the host prepares is committed pending its preparation, has
    /// no member and is not usable until the host settles it, and the
    /// create answers `pending_preparation`. `None`, and a profile the map
    /// lacks, prepare nothing.
    pub preparations: Option<&'a BTreeMap<String, PreparationRule>>,
}

impl std::fmt::Debug for ChangeCommit<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ChangeCommit")
            .field("mob", &self.mob.mob_id())
            .field("operation_id", &self.operation_id)
            .field("requester", &self.requester)
            .field("consent", &self.consent)
            .field("request", &self.request)
            .field("configured_target", &self.configured_target)
            .field("preparations", &self.preparations)
            .finish()
    }
}

impl<'a> ChangeCommit<'a> {
    /// The commit of `request` as `operation_id`, by `requester`, under
    /// `consent`, in `mob`, of an agent its host does not configure.
    pub fn new(
        mob: &'a MobHandle,
        operation_id: &'a OperationId,
        requester: &'a ChangeRequester,
        consent: ChangeConsent<'a>,
        request: &'a IdentityRequest,
    ) -> Self {
        Self {
            mob,
            operation_id,
            requester,
            consent,
            request,
            configured_target: false,
            preparations: None,
        }
    }

    /// This commit, with how the host prepares an agent of each profile
    /// before its first use (`IdentityCreationPolicy::preparations`).
    #[must_use]
    pub fn with_preparations(
        mut self,
        preparations: &'a BTreeMap<String, PreparationRule>,
    ) -> Self {
        self.preparations = Some(preparations);
        self
    }

    /// This commit, of an agent its host configures when `configured`.
    #[must_use]
    pub fn of_configured_target(mut self, configured: bool) -> Self {
        self.configured_target = configured;
        self
    }
}

/// What the seam answers: the typed outcome, and whether the operation
/// had committed before (a replay writes nothing again).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ChangeCommitted {
    /// The typed outcome.
    pub outcome: IdentityOutcome,
    /// Whether the operation committed before with the same request: Core
    /// answered its receipt and wrote nothing.
    pub replayed: bool,
}

impl ChangeCommitted {
    /// A change that committed now, or before when `replayed`.
    pub fn new(outcome: IdentityOutcome, replayed: bool) -> Self {
        Self { outcome, replayed }
    }

    /// A refused change: nothing was written.
    pub fn refused(refusal: IdentityRefusal) -> Self {
        Self::new(IdentityOutcome::Refused(refusal), false)
    }
}

/// A commit that ended with no typed outcome: the change may or may not
/// have committed. The same operation asked again replays it or commits it
/// once, so a caller that holds a decision's permit does not settle it on
/// this error.
#[derive(Debug)]
#[non_exhaustive]
pub enum ExecutorError {
    /// Core failed in a way that is no refusal.
    Core(Box<MobError>),
    /// The normalized request has no digest: it cannot be written as JSON.
    Request(serde_json::Error),
    /// Core's receipt names an entry outcome the family's changes do not
    /// have.
    UnexpectedOutcome(String),
}

impl std::fmt::Display for ExecutorError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Core(error) => {
                write!(
                    formatter,
                    "Core could not complete the identity change: {error}"
                )
            }
            Self::Request(error) => {
                write!(
                    formatter,
                    "the identity request cannot be digested: {error}"
                )
            }
            Self::UnexpectedOutcome(outcome) => write!(
                formatter,
                "Core answered an identity change with an outcome it cannot have: {outcome}"
            ),
        }
    }
}

impl std::error::Error for ExecutorError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Core(error) => Some(error.as_ref()),
            Self::Request(error) => Some(error),
            Self::UnexpectedOutcome(_) => None,
        }
    }
}

/// What follows a committed change for an agent's instructions: the step
/// the seam calls after its commit. The store commit stands whatever the
/// step answers. The executor is given one
/// ([`CoreChangeExecutor::with_instruction_step`]); without one, an
/// update's outcome says nothing of its instructions change.
#[async_trait]
pub trait InstructionStep: Send + Sync {
    /// After the commit of an update that changed an agent's instructions
    /// text, in `mob`: bring the change to the agent, and answer what
    /// became of it. `change` names the agent, the text before and the
    /// text now.
    async fn after_update(
        &self,
        mob: &MobHandle,
        change: &InstructionsChanged,
    ) -> InstructionOutcome;

    /// After the commit of a removal of `identity`, in `mob`: end what the
    /// step holds for the agent. Called again for a replay of the removal,
    /// and never for an archive.
    async fn after_removal(&self, mob: &MobHandle, identity: &str);
}

/// The executor seam.
#[async_trait]
pub trait IdentityChangeExecutor: Send + Sync {
    /// Commit `commit.request` under `commit.consent`, as
    /// `commit.requester`, in `commit.mob`. A typed refusal is an outcome,
    /// with nothing written; an error is a commit that ended with no
    /// outcome ([`ExecutorError`]).
    async fn commit(&self, commit: ChangeCommit<'_>) -> Result<ChangeCommitted, ExecutorError>;
}

/// The executor over Core's change path: each change is one call of the
/// mob's handle (`create_store_owned_identities`, `archive_identity`,
/// `unarchive_identity`, `remove_identity`), which commits the agents'
/// records, their seats and the receipt in one store transaction.
#[derive(Clone, Default)]
pub struct CoreChangeExecutor {
    instructions: Option<Arc<dyn InstructionStep>>,
}

impl CoreChangeExecutor {
    /// The executor, with no instruction step.
    pub fn new() -> Self {
        Self::default()
    }

    /// This executor, calling `step` after an update that changed an
    /// agent's instructions text and after a removal.
    #[must_use]
    pub fn with_instruction_step(mut self, step: Arc<dyn InstructionStep>) -> Self {
        self.instructions = Some(step);
        self
    }

    /// The instruction outcome of a committed update in `mob` that changed
    /// an agent's instructions text: the step's answer, or `None` when the
    /// executor has no step.
    #[allow(
        dead_code,
        reason = "called by the update's body, which is not written yet"
    )]
    pub(super) async fn after_update(
        &self,
        mob: &MobHandle,
        change: &InstructionsChanged,
    ) -> Option<InstructionOutcome> {
        match &self.instructions {
            Some(step) => Some(step.after_update(mob, change).await),
            None => None,
        }
    }

    /// Tell the step that `identity` in `mob` was removed.
    pub(super) async fn after_removal(&self, mob: &MobHandle, identity: &str) {
        if let Some(step) = &self.instructions {
            step.after_removal(mob, identity).await;
        }
    }
}

impl std::fmt::Debug for CoreChangeExecutor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CoreChangeExecutor")
            .field("instruction_step", &self.instructions.is_some())
            .finish()
    }
}

#[async_trait]
impl IdentityChangeExecutor for CoreChangeExecutor {
    async fn commit(&self, commit: ChangeCommit<'_>) -> Result<ChangeCommitted, ExecutorError> {
        if let Some(refusal) = refused_before_core(&commit) {
            return Ok(ChangeCommitted::refused(refusal));
        }
        let digest = commit
            .request
            .request_digest()
            .map_err(ExecutorError::Request)?;
        let operation = IdentityOperation::new(commit.operation_id.as_str(), digest.as_str());
        match &commit.request.action {
            RequestAction::Create { identities } => {
                core_commit::create(self, &commit, operation, identities).await
            }
            RequestAction::Archive {
                identity,
                expected_spec_digest,
            } => {
                core_commit::entry_change(
                    self,
                    &commit,
                    operation,
                    core_commit::EntryChange::Archive,
                    identity,
                    expected_spec_digest,
                )
                .await
            }
            RequestAction::Unarchive {
                identity,
                expected_spec_digest,
            } => {
                core_commit::entry_change(
                    self,
                    &commit,
                    operation,
                    core_commit::EntryChange::Unarchive,
                    identity,
                    expected_spec_digest,
                )
                .await
            }
            RequestAction::Remove {
                identity,
                expected_spec_digest,
            } => {
                core_commit::entry_change(
                    self,
                    &commit,
                    operation,
                    core_commit::EntryChange::Remove,
                    identity,
                    expected_spec_digest,
                )
                .await
            }
            RequestAction::Update(update) => commit_update(self, &commit, operation, update).await,
            RequestAction::Adopt {
                identity,
                expected_spec_digest,
            } => commit_adopt(&commit, operation, identity, expected_spec_digest).await,
        }
    }
}

/// What the seam refuses before Core is called, for every requester.
fn refused_before_core(commit: &ChangeCommit<'_>) -> Option<IdentityRefusal> {
    if commit.request.requester != commit.requester.recorded() {
        return Some(
            IdentityRefusal::new(IdentityRefusalKind::InvalidRequest {
                problem: RequestProblem::Conflicting,
            })
            .at("requester"),
        );
    }
    if commit.requester.is_private_member() {
        return Some(IdentityRefusal::new(IdentityRefusalKind::PrivateRequester));
    }
    let adopts_or_creates = matches!(
        commit.request.action,
        RequestAction::Adopt { .. } | RequestAction::Create { .. }
    );
    if commit.configured_target && !adopts_or_creates {
        return Some(IdentityRefusal::new(IdentityRefusalKind::OwnerReadOnly).at("name"));
    }
    None
}

/// The feature an update through the seam answers until its body is filled.
pub const UPDATE_NOT_BUILT: &str = "an update of an agent";
/// The feature an adoption through the seam answers until its body is
/// filled.
pub const ADOPT_NOT_BUILT: &str = "an adoption of an agent";

/// BODY TO FILL (the executor and host methods part): commit an update.
///
/// What the body does: read the agent's record from Core
/// (`MobHandle::identity_record`), apply `update` to its spec whole, refuse
/// placement, binding and backend `unsupported`, admit an agent requester's
/// grant (`core_commit::requester_input` with
/// `IdentityGrantRequest::for_update`), call
/// `MobHandle::update_store_owned_identity` with the peers added and
/// removed, and answer [`core_commit::committed`] of its change. When the
/// update changed the agent's instructions text
/// (`InstructionsChanged::between` of the record's text before the update
/// and `update.additional_instructions`), call
/// `executor.after_update(commit.mob, &change)` after the commit and record
/// its answer on the agent ([`IdentityOutcome::with_instruction_outcome`]);
/// otherwise record `InstructionOutcome::Unchanged`.
/// A refusal of Core's goes through [`refusal_of_error`], which answers the
/// retryable `admission_unsettled` for a member step in flight.
async fn commit_update(
    _executor: &CoreChangeExecutor,
    _commit: &ChangeCommit<'_>,
    _operation: IdentityOperation,
    _update: &UpdatedSpec,
) -> Result<ChangeCommitted, ExecutorError> {
    Ok(ChangeCommitted::refused(IdentityRefusal::new(
        IdentityRefusalKind::Unsupported {
            feature: UPDATE_NOT_BUILT.to_owned(),
        },
    )))
}

/// BODY TO FILL (the executor and host methods part): commit an adoption.
///
/// An adoption releases an agent from configuration: from it on the agent
/// is changed at run time and configuration no longer declares it. What the
/// body does: read the spec the agent's host configures now (the spec whose
/// digest is `expected_spec_digest`), refuse `stale_spec` when it moved,
/// and call `MobHandle::adopt_identity` with `AdoptionSource::roster` of
/// that spec, then answer [`core_commit::committed`] of its change.
async fn commit_adopt(
    _commit: &ChangeCommit<'_>,
    _operation: IdentityOperation,
    _identity: &str,
    _expected_spec_digest: &Sha256Digest,
) -> Result<ChangeCommitted, ExecutorError> {
    Ok(ChangeCommitted::refused(IdentityRefusal::new(
        IdentityRefusalKind::Unsupported {
            feature: ADOPT_NOT_BUILT.to_owned(),
        },
    )))
}

/// The outcome Core recorded for the operation `operation_id` in `mob`:
/// read from the receipt of the change that committed under it, which a
/// settlement after a crash reads first. `None` when none committed (never
/// asked, refused, or its decision still held): asking the operation again
/// commits it at most once.
pub async fn committed_outcome(
    mob: &MobHandle,
    operation_id: &OperationId,
) -> Result<Option<IdentityOutcome>, ExecutorError> {
    match mob.identity_operation(operation_id.as_str()).await {
        Ok(Some(receipt)) => outcome_of_receipt(mob, operation_id, &receipt).map(Some),
        Ok(None) => Ok(None),
        // A storage composition that commits no change holds no receipt.
        Err(MobError::IdentityChangeRefused {
            refusal: IdentityChangeRefusal::StorageUnsupported,
        }) => Ok(None),
        Err(error) => Err(ExecutorError::Core(Box::new(error))),
    }
}

/// The typed outcome of the change `change` Core committed as
/// `operation_id`, now or before.
pub(super) fn outcome_of_change(
    mob: &MobHandle,
    operation_id: &OperationId,
    change: &IdentityChangeOutcome,
) -> Result<ChangeCommitted, ExecutorError> {
    let replayed = matches!(change, IdentityChangeOutcome::Replayed { .. });
    let outcome = outcome_of_receipt(mob, operation_id, change.receipt())?;
    Ok(ChangeCommitted::new(outcome, replayed))
}

/// The typed outcome a receipt records: each of its agents with what the
/// change did to it, `pending_preparation` when one of them waits for its
/// preparation and `done` otherwise. An agent's session is read from
/// Core's published entry, which wakes nothing.
fn outcome_of_receipt(
    mob: &MobHandle,
    operation_id: &OperationId,
    receipt: &IdentityChangeReceipt,
) -> Result<IdentityOutcome, ExecutorError> {
    let mut agents = Vec::with_capacity(receipt.identities.len());
    for entry in &receipt.identities {
        let state = match entry.outcome {
            IdentityChangeEntryOutcome::Committed => AgentChangeState::Committed,
            IdentityChangeEntryOutcome::PendingPreparation => AgentChangeState::PendingPreparation,
            IdentityChangeEntryOutcome::Archiving => AgentChangeState::Archiving,
            IdentityChangeEntryOutcome::Archived => AgentChangeState::Archived,
            IdentityChangeEntryOutcome::Restored => AgentChangeState::Restored,
            IdentityChangeEntryOutcome::Retiring => AgentChangeState::Retiring,
            IdentityChangeEntryOutcome::Removed => AgentChangeState::Removed,
            other => {
                return Err(ExecutorError::UnexpectedOutcome(format!(
                    "{other:?} for '{}'",
                    entry.identity
                )));
            }
        };
        let mut agent = AgentOutcome::new(identity_name_of_member(&entry.identity), state);
        agent.spec_digest = entry.spec_digest.as_deref().and_then(Sha256Digest::parse);
        agent.spec_revision = entry.revision;
        let standing = entry_standing(mob, &entry.identity);
        if let Some(meerkat_mob::EntryStanding::Seated(seated)) = &standing {
            agent.session_id = seated.session_id.as_ref().map(ToString::to_string);
        }
        if receipt.change == meerkat_mob::IdentityChangeKind::Create
            && state == AgentChangeState::Committed
        {
            agent.seat = match standing {
                Some(meerkat_mob::EntryStanding::Seated(_)) => Some(AgentSeat::Seated),
                Some(meerkat_mob::EntryStanding::Unseated(
                    meerkat_mob::UnseatedEntryCause::SeatOwed,
                )) => Some(AgentSeat::Owed),
                _ => None,
            };
        }
        agents.push(agent);
    }
    let operation_id = operation_id.clone();
    Ok(
        if agents
            .iter()
            .any(|agent| agent.state == AgentChangeState::PendingPreparation)
        {
            IdentityOutcome::PendingPreparation {
                operation_id,
                agents,
            }
        } else {
            IdentityOutcome::Done {
                operation_id,
                agents,
            }
        },
    )
}

/// Where the member of `member` stands, from Core's published entry: no
/// actor command and no wake. `None` when the name is no readable entry.
fn entry_standing(
    mob: &MobHandle,
    member: &meerkat_mob::AgentIdentity,
) -> Option<meerkat_mob::EntryStanding> {
    match mob.identity_entry(member) {
        Ok(meerkat_mob::IdentityEntryRead::Entry(view)) => Some(view.standing),
        _ => None,
    }
}

/// The typed refusal of an error of Core's change path: a refusal of the
/// change under its own kind ([`refusal_of_core`]), and a member step in
/// flight as the retryable `admission_unsettled`. Any other error is no
/// refusal ([`ExecutorError::Core`]).
pub fn refusal_of_error(error: MobError) -> Result<IdentityRefusal, ExecutorError> {
    match error {
        MobError::IdentityChangeRefused { refusal } => Ok(refusal_of_core(&refusal)),
        MobError::LifecycleOperationPending { intent } => Ok(IdentityRefusal::new(
            IdentityRefusalKind::AdmissionUnsettled {
                correlation: intent,
            },
        )),
        other => Err(ExecutorError::Core(Box::new(other))),
    }
}

/// A refusal of Core's change path in the closed vocabulary, under Core's
/// own kind token. Its detail is the kind's own sentence: Core's text may
/// say where an agent's record lives, which no surface does. A malformed
/// request keeps Core's words for what is malformed.
pub fn refusal_of_core(refusal: &IdentityChangeRefusal) -> IdentityRefusal {
    use IdentityChangeRefusal as Core;
    use IdentityRefusalKind as Kind;
    let kind = match refusal {
        Core::InvalidRequest { problem } => {
            return IdentityRefusal::new(Kind::InvalidRequest {
                problem: RequestProblem::Malformed,
            })
            .with_detail(format!("The request is malformed: {problem}."));
        }
        Core::BatchTooLarge { max } => Kind::BatchTooLarge { max: *max as u64 },
        Core::ProfileUnknown { profile } => Kind::ProfileUnknown {
            profile: profile.as_str().to_owned(),
        },
        Core::ProfileNotAllowed { profile } => Kind::ProfileNotAllowed {
            profile: profile.as_str().to_owned(),
        },
        Core::PrivilegedField { field } => Kind::PrivilegedField {
            field: (*field).to_owned(),
        },
        Core::GrantRevoked => Kind::GrantRevoked,
        Core::NameTaken { by, .. } => Kind::NameTaken {
            by: match by {
                IdentityNameHolder::Removed => NameTakenBy::Removed,
                IdentityNameHolder::Worker => NameTakenBy::Worker,
                IdentityNameHolder::Unreadable => NameTakenBy::Unreadable,
                _ => NameTakenBy::Identity,
            },
        },
        Core::OperationReused { .. } => Kind::OperationReused,
        Core::ChangesHeld => Kind::ChangesHeld,
        Core::CandidateLaunch => Kind::CandidateLaunch,
        Core::StorageUnsupported => Kind::StorageUnsupported,
        Core::MobNotRunning => Kind::MobNotRunning,
        Core::SubjectWidened => Kind::SubjectWidened,
        Core::DecisionClosed { state } => Kind::DecisionClosed {
            state: (*state).to_owned(),
        },
        Core::ImportRefused => Kind::ImportRefused,
        Core::Unsupported { feature } => Kind::Unsupported {
            feature: feature.clone(),
        },
        Core::IdentityUnknown { .. } => Kind::IdentityUnknown,
        Core::NotArchived { .. } => Kind::NotArchived,
        Core::OwnerReadOnly { .. } => Kind::OwnerReadOnly,
        Core::StaleSpec {
            current_digest,
            current_revision,
            ..
        } => match Sha256Digest::parse(current_digest) {
            Some(current_digest) => Kind::StaleSpec {
                current_digest,
                current_revision: *current_revision,
            },
            None => Kind::Unsupported {
                feature: "a spec digest this build cannot read".to_owned(),
            },
        },
        Core::PreparationNotPending { .. } => Kind::PreparationNotPending,
        Core::PreparationMismatch { .. } => Kind::PreparationMismatch,
        Core::ConstructionOpen { .. } => Kind::ConstructionOpen,
        Core::StaleAttempt { current, .. } => Kind::StaleAttempt { current: *current },
        Core::NotConstructed { .. } => Kind::NotConstructed,
        Core::ConstructionRequired { .. } => Kind::ConstructionRequired,
        Core::ActivationPending { attempt, .. } => Kind::ActivationPending { attempt: *attempt },
        Core::ConstructionRefused { reason, .. } => Kind::ConstructionRefused {
            kind: reason.clone(),
        },
        Core::HostPreparationPending { .. } => Kind::HostPreparationPending,
        Core::PeerUnknown { .. } => Kind::PeerUnknown,
        Core::NotAdoptable { reason, .. } => Kind::NotAdoptable {
            reason: not_adoptable_reason(*reason),
        },
        Core::PredecessorMismatch { .. } => Kind::PredecessorMismatch,
        Core::AdmissionUnsettled { identity } => Kind::AdmissionUnsettled {
            correlation: identity_name_of_member(identity),
        },
        Core::DeclaredApplyOpen => Kind::DeclaredApplyOpen,
        Core::Purged => Kind::Purged,
        Core::PurgeRefused { reason } => Kind::PurgeRefused {
            reason: reason.as_str().to_owned(),
        },
        // Core and Meta release together, so every kind Core has is named
        // above; a kind a later Core adds is refused under its own token as
        // something this build does not support.
        other => Kind::Unsupported {
            feature: other.kind().to_owned(),
        },
    };
    IdentityRefusal::new(kind)
}

/// Core's reason an adoption takes no agent, without where its record
/// lives.
fn not_adoptable_reason(reason: meerkat_mob::NotAdoptableReason) -> NotAdoptableReason {
    use meerkat_mob::NotAdoptableReason as Core;
    match reason {
        Core::StoreOwned => NotAdoptableReason::NothingToAdopt,
        Core::Placed => NotAdoptableReason::Placed,
        Core::NoSession => NotAdoptableReason::NoSession,
        Core::HostedOnFirstUse => NotAdoptableReason::HostedOnFirstUse,
        Core::AwaitingHeldCreation => NotAdoptableReason::AwaitingHeldCreation,
        Core::AwaitingHostPreparation => NotAdoptableReason::AwaitingHostPreparation,
        Core::ProfileOverride => NotAdoptableReason::ProfileOverride,
        // The name is nothing the adoption named: no agent holds it, or
        // what holds it is not what the adoption took it for.
        _ => NotAdoptableReason::NotAnAgent,
    }
}

/// The names the request's identities have in Core's mob, or the refusal
/// of the first name the encoding does not admit.
pub(super) fn core_name(
    name: &str,
    field: &str,
) -> Result<meerkat_mob::AgentIdentity, IdentityRefusal> {
    core_member_name(name)
        .ok_or_else(|| IdentityRefusal::new(IdentityRefusalKind::InvalidName).at(field))
}
