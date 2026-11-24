//! Changes of agents that a person decides.
//!
//! A change whose outcome under the host's rules is `require_approval` is
//! not committed when it is asked: it is HELD as a durable decision, and
//! committed once a person approved it. [`IdentityDecisions`] is the holder:
//!
//! - **Hold** ([`IdentityDecisions::hold`]). The normalized request becomes
//!   one decision of kind `identity.<action>` (`identity.create`,
//!   `identity.update`, `identity.archive`, `identity.unarchive`,
//!   `identity.remove`, `identity.adopt`) under the subject key
//!   `{mob id}/{operation id}` (for a mob a host serves beside its launch
//!   mob, `{mob id}@{incarnation}/{operation id}`: a held change of an
//!   earlier mob of an id is none of a later one's). Its body is the
//!   normalized request, its
//!   request digest the request's own ([`IdentityRequest::request_digest`]),
//!   and its rendering the canonical text of the request
//!   ([`render_held_v1`]): the host's consequence lines and what the mob
//!   declares for every profile an agent of the request runs on (its model,
//!   auth binding, skills and tool configuration, under one digest of the
//!   whole declaration), ahead of anything the requester wrote. A profile
//!   bound to a realm profile is read from the realm through the family's
//!   implementation, and refused when it cannot be. Nothing else is written: no agent is
//!   created, built or woken, and no other agent's record is read. The
//!   answer is `pending_decision`; the same operation asked again answers
//!   the decision as it stands, and any other request under its id, of the
//!   same action or another, is refused `operation_reused`: one operation
//!   id names one change. One decision covers the whole request: a create
//!   of N agents is one decision.
//! - **Decide.** A person decides through the decisions surface that exists
//!   (`meta/decisions/decide`), with attested evidence: the stdio operator,
//!   or a person an application attests from its own channel. A person
//!   never decides a request the decision records as theirs: a request a
//!   signed-in person asked, and a request a host surface asked that names
//!   the person as its `proposer` by their principal key. A request a host
//!   asks with no such proposer is the host's, whoever the application
//!   carried it for, and any approver may decide it.
//! - **Carry out.** One [`DecisionExecutor`] per kind runs in Core's
//!   [`DecisionExecutorLoop`], which alone authorizes, settles and releases.
//!   The executor renders the request again from what stands now and
//!   compares the rendering's digest with the one the person was shown: a
//!   profile whose declaration changed in any part (model, auth binding,
//!   skills, wiring rules, tools), an agent that moved to another profile,
//!   or host lines that differ, refuse the change `subject_widened` and
//!   close the decision refused. Otherwise it commits through the executor seam
//!   ([`IdentityChangeExecutor`]) under the decision's permit, as the
//!   recorded requester. An agent that changed since the request was made
//!   is Core's to refuse: the request names the spec digest its requester
//!   read, and Core's commit answers `stale_spec`, which closes the
//!   decision too. Whether a commit landed is read from Core's receipt
//!   of the operation ([`committed_outcome`]), so a process that ends
//!   between the commit and the settlement settles it executed at its next
//!   start, and commits nothing twice.
//! - **Tell.** A decision that ended (carried out, rejected, expired,
//!   withdrawn, refused when carried out) owes its requester one notice with
//!   the outcome ([`IdentityDecisionNotices`]), when the requester can be
//!   told. An agent is told by a keyed send to its own name
//!   ([`KeyedSendNotices`]).
//! - **Withdraw** ([`IdentityDecisions::withdraw`]). The requester or the
//!   host withdraws a change that is still held.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use meerkat_core::approval::ApprovalConsentDeliveryState;
use meerkat_core::approval::decision::{
    ApproverId, ConsentProvenance, DECISION_PAGE_MAX, DecisionHoldRequest, DecisionId,
    DecisionKind, DecisionListFilter, DecisionOperationId, DecisionPhase, DecisionPrincipalKey,
    DecisionReference, DecisionRenderer, DecisionRendering, DecisionRequestBody,
    DecisionSubjectKey, DecisionTier, StoredDecision,
};
use meerkat_core::{PrincipalKind, PrincipalRef, TrustDomainId};
use meerkat_mob::{MobHandle, ProfileName};
use meerkat_runtime::decision::{
    DecisionApproverCurrentness, DecisionClock, DecisionError, DecisionExecution,
    DecisionExecutionPermit, DecisionExecutor, DecisionExecutorLoop, DecisionExecutorLoopError,
    DecisionExecutorRecordUnavailable, DecisionPassOutcome, DecisionRefusal, DecisionWithdrawer,
    SystemDecisionClock,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::durable_decisions::{DecisionExecutorDriver, RuntimeDecisions};
use crate::identity_first::PublicIdentityRuntime;

use super::approval::PlannedOutcome;
use super::consequences::{ConsequenceLine, HookRefusal, HostConsequences};
use super::executor::{
    ChangeCommit, ChangeConsent, ChangeRequester, IdentityChangeExecutor, committed_as,
    committed_outcome,
};
use super::family::{WithdrawRequest, Withdrawn};
use super::operation::OperationId;
use super::outcome::IdentityOutcome;
use super::refusal::{IdentityRefusal, IdentityRefusalKind, RequestProblem};
use super::render::{
    AgentTools, ProfileTools, RENDERER, Rendering, ToolConfigurations, render_held_v1,
};
use super::request::{
    IdentityEntryFacts, IdentityRequest, NormalizedRequest, RequestAction, Requester,
};

/// The decision kinds of a held change of agents, one per action.
pub const IDENTITY_DECISION_KINDS: [&str; 6] = [
    "identity.create",
    "identity.update",
    "identity.archive",
    "identity.unarchive",
    "identity.remove",
    "identity.adopt",
];

/// The format of the body a held change's decision holds.
pub const HELD_CHANGE_FORMAT: &str = "meerkat.identity.held_change.v1";

/// The prefix of the trust domain an agent or a host surface that asks for
/// a change is named in as a decision's requester: the prefix, then the
/// mob's id. A person is named by their own principal key.
pub const REQUESTER_TRUST_DOMAIN_PREFIX: &str = "meta-mob-";

/// The execution refusal of a decision whose rendering, made again when the
/// change is carried out, is no longer the one the person was shown.
pub const SUBJECT_WIDENED_REFUSAL: &str = "subject_widened";

/// The body of a held change's decision: the mob it changes and the
/// normalized request, and, for a mob a host serves beside its launch mob,
/// which mob of that id it is. A body member this version does not know
/// makes the body unreadable here, and nothing carries it out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeldIdentityChange {
    /// [`HELD_CHANGE_FORMAT`].
    pub format: String,
    /// The mob the change is of.
    pub mob_id: String,
    /// The incarnation of the mob the change is of, as the holder of a mob
    /// a host serves beside its launch mob writes it: the one minted when
    /// that mob was created, the same over an unload, a load and a restart,
    /// and another one for a mob created later under the id. The record
    /// itself so says which mob of an id a change was held for. The launch
    /// mob's holder writes none, and its body is as it always was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mob_incarnation: Option<String>,
    /// The normalized request.
    pub request: IdentityRequest,
}

impl HeldIdentityChange {
    /// Whether this body says the change is of the incarnation
    /// `incarnation` of its mob. A body that names no incarnation is of
    /// none.
    pub fn is_of_incarnation(&self, incarnation: &str) -> bool {
        self.mob_incarnation.as_deref() == Some(incarnation)
    }

    /// The change `decision` holds, when it is one this version reads: a
    /// decision of an `identity.*` kind, of this body format, whose request
    /// is the one its descriptor binds.
    pub fn of(decision: &StoredDecision) -> Result<Self, String> {
        let descriptor = &decision.descriptor;
        if !IDENTITY_DECISION_KINDS.contains(&descriptor.kind.as_str()) {
            return Err(format!(
                "decision {} is of kind {}",
                decision.decision_id, descriptor.kind
            ));
        }
        // The reason names the decision and never quotes its body: it is
        // logged, and kept as the failure reason of a notice.
        let body: Self = serde_json::from_str(descriptor.request.get()).map_err(|_| {
            format!(
                "decision {} holds a body this version does not read",
                decision.decision_id
            )
        })?;
        if body.format != HELD_CHANGE_FORMAT {
            return Err(format!(
                "decision {} holds the body {}",
                decision.decision_id, body.format
            ));
        }
        if body.request.decision_kind() != descriptor.kind.as_str() {
            return Err(format!(
                "decision {} of kind {} holds a request of kind {}",
                decision.decision_id,
                descriptor.kind,
                body.request.decision_kind()
            ));
        }
        let digest = body.request.request_digest().map_err(|_| {
            format!(
                "decision {} holds a request that has no digest",
                decision.decision_id
            )
        })?;
        if digest.as_str() != descriptor.request_digest.as_str() {
            return Err(format!(
                "decision {} holds a request other than the one it binds",
                decision.decision_id
            ));
        }
        Ok(body)
    }
}

/// The agent a held change names, as it stands when it is asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct HeldTarget {
    /// What normalization is given of the agent: its spec digest, its
    /// profile and its authority labels, read now.
    pub entry: IdentityEntryFacts,
    /// Whether its host configures it
    /// ([`ChangeCommit::of_configured_target`]).
    pub configured: bool,
}

impl HeldTarget {
    /// The agent `entry`, which its host configures when `configured`.
    pub fn new(entry: IdentityEntryFacts, configured: bool) -> Self {
        Self { entry, configured }
    }
}

/// What the holder of decisions asks the family's implementation: the mob a
/// held change names, and the agent it changes as it stands now. The same
/// reads the implementation makes to normalize a request.
#[async_trait]
pub trait HeldChangeHost: Send + Sync {
    /// The mob `mob_id`, when this host runs it now. `None` leaves a held
    /// change for a later pass: it is neither carried out nor refused.
    fn mob(&self, mob_id: &str) -> Option<MobHandle>;

    /// The agent `identity` of `mob` as it stands now; `Ok(None)` when no
    /// agent has the name. It wakes nothing.
    async fn target(
        &self,
        mob: &MobHandle,
        identity: &str,
    ) -> Result<Option<HeldTarget>, IdentityRefusal>;

    /// The realm profile `name`, which a profile of `mob` is bound to by
    /// reference, as the realm holds it now. `Ok(None)` when this host
    /// cannot read it: a change whose agent runs on such a profile is
    /// refused `unsupported`, and is neither held nor carried out for a
    /// profile nobody can show. The default reads none.
    async fn realm_profile(
        &self,
        mob: &MobHandle,
        name: &str,
    ) -> Result<Option<meerkat_mob::Profile>, IdentityRefusal> {
        let _ = (mob, name);
        Ok(None)
    }
}

/// The holder of a mob's held changes, as the one driver of a host finds
/// it when it carries a decision out or tells its end.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum HeldFamily {
    /// The host runs the mob now and its family holds changes: the whole
    /// holder of that mob (its rules, its reads, its executor, its notice
    /// route).
    Here(Box<IdentityDecisions>),
    /// The host has the mob and does not run it now, or runs it with no
    /// family that holds changes, or has no record of a mob of the id at
    /// all (a store it does not see, a mob an earlier process destroyed):
    /// a held change of it is left for a later pass, and nothing of it is
    /// told meanwhile.
    Away,
    /// The host destroyed the mob, by its mob set's own record of that
    /// destroy, and hosts no mob under the id since: nothing of it is
    /// carried out, and a notice it owes has no one to reach. Absence
    /// alone is never this.
    Gone,
}

/// The holders of the held changes of every mob one host serves. A host
/// has one driver of the `identity.*` decision kinds; a held change names
/// its mob, and the driver carries it out through that mob's own holder.
/// Nothing is loaded for the question and nothing is kept of the answer.
#[async_trait]
pub trait HeldChangeFamilies: Send + Sync {
    /// The holder of the held changes of the mob `mob_id`, as the host
    /// stands now.
    async fn of(&self, mob_id: &str) -> HeldFamily;
}

/// The notice a decision that ended owes its requester.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct IdentityDecisionNotice {
    /// The decision.
    pub decision_id: String,
    /// The mob the change is of.
    pub mob_id: String,
    /// The operation the change was asked as.
    pub operation_id: OperationId,
    /// Who asked.
    pub requester: Requester,
    /// The change's action: `create`, `update`, ...
    pub action: &'static str,
    /// The agents the change names.
    pub agents: Vec<String>,
    /// What became of the change: the committed outcome of a change that
    /// was carried out, a refusal otherwise.
    pub outcome: IdentityOutcome,
}

impl IdentityDecisionNotice {
    /// The notice as JSON: what a host event or a requester's message
    /// carries.
    pub fn to_json(&self) -> Value {
        json!({
            "decision_id": self.decision_id,
            "mob_id": self.mob_id,
            "operation_id": self.operation_id.as_str(),
            "action": self.action,
            "agents": self.agents,
            "result": self.outcome.to_json(),
        })
    }

    /// The notice as a requesting agent reads it: one sentence that says
    /// what was decided, then the typed outcome. It names the decision and
    /// no operation id: an operation id is the host's handle on a change,
    /// and appears in nothing an agent reads.
    pub fn text(&self) -> String {
        let agents = self
            .agents
            .iter()
            .map(|agent| format!("\"{agent}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let summary = match &self.outcome {
            IdentityOutcome::Done { .. } | IdentityOutcome::PendingPreparation { .. } => {
                "was approved and carried out".to_owned()
            }
            IdentityOutcome::Refused(refusal) => format!(
                "was not carried out ({}). {}",
                refusal.kind_token(),
                refusal.detail()
            ),
            IdentityOutcome::PendingDecision { .. } => "is still held".to_owned(),
        };
        let mut typed = self.to_json();
        if let Some(members) = typed.as_object_mut() {
            members.remove("operation_id");
            // The outcome of a change that was carried out names it too.
            if let Some(result) = members.get_mut("result").and_then(Value::as_object_mut) {
                result.remove("operation_id");
            }
        }
        format!(
            "The change you asked for ({} of {agents}) {summary}\n\
             Do not ask again for the same change unless this says it was not carried out.\n{typed}",
            self.action,
        )
    }
}

/// Where the notices of ended decisions go.
#[async_trait]
pub trait IdentityDecisionNotices: Send + Sync {
    /// Whether `requester` can be told the end of a decision: one that
    /// cannot is owed no notice, and reads the operation's state instead.
    fn routable(&self, requester: &Requester) -> bool;

    /// Deliver `notice` to its requester: the receipt (admitted, or failed
    /// terminally), or `None` when it cannot be delivered now, so a later
    /// pass asks again. A delivery whose receipt was not recorded is asked
    /// again too: deliver idempotently per decision.
    async fn deliver(
        &self,
        notice: &IdentityDecisionNotice,
    ) -> Option<ApprovalConsentDeliveryState>;
}

/// No requester can be told: every decision is held with no notice owed.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoRequesterNotices;

#[async_trait]
impl IdentityDecisionNotices for NoRequesterNotices {
    fn routable(&self, _requester: &Requester) -> bool {
        false
    }

    async fn deliver(
        &self,
        _notice: &IdentityDecisionNotice,
    ) -> Option<ApprovalConsentDeliveryState> {
        Some(ApprovalConsentDeliveryState::Failed {
            reason: "the requester has no notice route".to_owned(),
        })
    }
}

/// What a notice route asks for the identity runtime of the mob a notice
/// names: `None` while the host does not run that mob.
type NoticeRuntime = dyn Fn(&str) -> Option<PublicIdentityRuntime> + Send + Sync;

/// Notices to requesting agents, as a keyed send to the agent's own name:
/// the key is the decision, so a notice asked for twice is delivered once.
/// The send starts a turn of an idle agent: it asked, and is told. A host
/// surface and a person have no route here and are owed no notice.
pub struct KeyedSendNotices {
    runtime: Box<NoticeRuntime>,
}

impl std::fmt::Debug for KeyedSendNotices {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("KeyedSendNotices").finish()
    }
}

impl KeyedSendNotices {
    /// Notices sent through the identity runtime `runtime` answers when a
    /// notice is delivered (`UnifiedRuntime::identity_runtime`). It is asked
    /// per delivery and never kept, so the notices keep no mob loaded. The
    /// route is one mob's: a notice of a change of another mob than the
    /// answered runtime's is delivered to nobody.
    pub fn new(
        runtime: impl Fn() -> Option<PublicIdentityRuntime> + Send + Sync + 'static,
    ) -> Self {
        Self {
            runtime: Box::new(move |_mob_id| runtime()),
        }
    }

    /// Notices of the changes of every mob a host serves: `runtime` is
    /// asked, per delivery, for the identity runtime of the mob the notice
    /// names, and answers `None` while the host does not run that mob (the
    /// notice is asked again at a later pass). Nothing is kept of the
    /// answer, and no mob is loaded for it.
    pub fn per_mob(
        runtime: impl Fn(&str) -> Option<PublicIdentityRuntime> + Send + Sync + 'static,
    ) -> Self {
        Self {
            runtime: Box::new(runtime),
        }
    }

    /// The key a decision's notice is sent under.
    pub fn key_of(decision_id: &str) -> String {
        format!("identity-decision:{decision_id}")
    }
}

#[async_trait]
impl IdentityDecisionNotices for KeyedSendNotices {
    fn routable(&self, requester: &Requester) -> bool {
        matches!(requester, Requester::Agent { .. })
    }

    async fn deliver(
        &self,
        notice: &IdentityDecisionNotice,
    ) -> Option<ApprovalConsentDeliveryState> {
        let failed = |reason: &str| {
            Some(ApprovalConsentDeliveryState::Failed {
                reason: reason.to_owned(),
            })
        };
        let Requester::Agent { identity } = &notice.requester else {
            return failed("the requester has no notice route");
        };
        let Ok(identity) = crate::identity_first::AgentIdentity::parse(identity) else {
            return failed("the requester is not an agent name");
        };
        // No identity runtime now: asked again at a later pass.
        let runtime = (self.runtime)(&notice.mob_id)?;
        // The route answers one mob's runtime: an agent of another mob is
        // never told of a change it did not ask for.
        if runtime.owner().handle().mob_id().as_str() != notice.mob_id {
            return failed("the notice is of a change of another mob than its route's");
        }
        let content = meerkat_core::ContentInput::Text(notice.text());
        let key = Self::key_of(&notice.decision_id);
        match runtime.send_keyed(&identity, None, &content, &key).await {
            Ok(receipt) => match receipt.status {
                meerkat_contracts::WireContinuationStatus::Admitted {
                    session_id,
                    input_id,
                } => match meerkat_core::SessionId::parse(&session_id) {
                    Ok(session_id) => Some(ApprovalConsentDeliveryState::Admitted {
                        session_id,
                        input_id,
                    }),
                    Err(_) => failed("the notice was admitted to a session with no readable id"),
                },
                meerkat_contracts::WireContinuationStatus::Pending => None,
                meerkat_contracts::WireContinuationStatus::Stranded => {
                    failed("the requester was replaced before the notice was admitted")
                }
                meerkat_contracts::WireContinuationStatus::Refused { reason } => failed(&reason),
            },
            Err(crate::identity_first::IdentityRuntimeError::UnknownIdentity(_)) => {
                failed("the requester is no longer an agent of the mob")
            }
            Err(error) => {
                tracing::debug!(%error, "a decision notice could not be sent now");
                None
            }
        }
    }
}

/// The holder of the decisions of changes of agents: see the module
/// documentation. Cloning shares it.
#[derive(Clone)]
pub struct IdentityDecisions {
    decisions: RuntimeDecisions,
    executor: Arc<dyn IdentityChangeExecutor>,
    host: Arc<dyn HeldChangeHost>,
    consequences: Option<Arc<dyn HostConsequences>>,
    notices: Arc<dyn IdentityDecisionNotices>,
    clock: Arc<dyn DecisionClock>,
    /// Whether this holder files a held change under its mob's
    /// incarnation: the holder of a mob a host serves beside its launch
    /// mob. See [`Self::subject_key`].
    filed_by_incarnation: bool,
    /// The holders of the host's mobs, on the holder whose executors the
    /// host's one driver runs: a held change is carried out through the
    /// holder of its own mob. Without them this holder carries out the
    /// changes of its own mob alone.
    families: Option<Arc<dyn HeldChangeFamilies>>,
}

impl std::fmt::Debug for IdentityDecisions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IdentityDecisions")
            .field("decisions", &self.decisions)
            .field("consequences", &self.consequences.is_some())
            .finish_non_exhaustive()
    }
}

/// The binding of one held change: the decision it is, the operation and
/// the request it binds.
struct Binding {
    kind: DecisionKind,
    subject_key: DecisionSubjectKey,
    decision_id: DecisionId,
    operation_id: DecisionOperationId,
    request_digest: meerkat_core::approval::decision::Sha256Digest,
}

fn malformed(field: &str) -> IdentityRefusal {
    IdentityRefusal::new(IdentityRefusalKind::InvalidRequest {
        problem: RequestProblem::Malformed,
    })
    .at(field)
}

/// A decision that could not be read or written now: nothing was written.
fn unavailable(error: &DecisionError) -> IdentityRefusal {
    tracing::warn!(%error, "a decision of a change of agents could not be read or written");
    IdentityRefusal::new(IdentityRefusalKind::ApprovalUnavailable).with_detail(
        "The decision could not be read or recorded now and nothing was written: ask the same \
         change again later.",
    )
}

/// The mob's held changes could not be listed now: what is open for its
/// agents is not known, and a read says so instead of answering "nothing".
fn unlisted(error: &DecisionError) -> IdentityRefusal {
    tracing::warn!(%error, "the held changes of a mob's agents could not be listed");
    IdentityRefusal::new(IdentityRefusalKind::ApprovalUnavailable).with_detail(
        "The mob's held changes could not be read now, so what is open for its agents is not \
         known: read again later.",
    )
}

/// The reference a decision ends refused with when it was filed for an
/// earlier mob of its mob's id: the mob the host runs under the id now is
/// not the one the change was asked and approved for.
pub const MOB_REPLACED_REFUSAL: &str = "mob_replaced";

impl Binding {
    fn of(
        subject_key: DecisionSubjectKey,
        operation_id: &OperationId,
        request: &IdentityRequest,
    ) -> Result<Self, IdentityRefusal> {
        let kind = DecisionKind::new(request.decision_kind()).map_err(|_| malformed("request"))?;
        let decision_id = DecisionId::derive(&kind, &subject_key);
        let digest = request.request_digest().map_err(|_| malformed("request"))?;
        Ok(Self {
            kind,
            subject_key,
            decision_id,
            operation_id: DecisionOperationId::new(operation_id.as_str())
                .map_err(|_| malformed("request_id"))?,
            request_digest: meerkat_core::approval::decision::Sha256Digest::new(digest.as_str())
                .map_err(|_| malformed("request"))?,
        })
    }
}

/// The person `text` names, when it is a person's principal key
/// (`principal:["human", trust domain, id]`); `None` for any other text,
/// and for the key of a principal that is no person.
fn person_key(text: &str) -> Option<DecisionPrincipalKey> {
    let key = DecisionPrincipalKey::new(text).ok()?;
    text.starts_with("principal:[\"human\",").then_some(key)
}

/// The principal the decision of `request` in `mob_id` records as its
/// requester: who a decide is compared with. A request a host surface asks
/// and names a person as the proposer of, by that person's principal key,
/// is that person's: they cannot approve it. A proposer that is any other
/// text is recorded and compared with nobody, and a request with no
/// proposer is the host's.
fn decision_requester(
    mob_id: &str,
    request: &IdentityRequest,
) -> Result<DecisionPrincipalKey, IdentityRefusal> {
    let proposed_by = request.proposer.as_deref().and_then(person_key);
    match (&request.requester, proposed_by) {
        (Requester::Host { .. }, Some(person)) => Ok(person),
        (requester, _) => requester_key(mob_id, requester),
    }
}

/// The principal a decision names for the requester `requester` of a change
/// in `mob_id`. An agent and a host surface are named in the mob's own
/// domain; a person is named by the principal key the transport carries for
/// them, which is the key a decide by the same person carries, so that a
/// person never decides their own request.
fn requester_key(
    mob_id: &str,
    requester: &Requester,
) -> Result<DecisionPrincipalKey, IdentityRefusal> {
    let in_mob = |kind: PrincipalKind, id: &str| {
        let domain = TrustDomainId::new(format!("{REQUESTER_TRUST_DOMAIN_PREFIX}{mob_id}"))
            .map_err(|_| malformed("requester"))?;
        let principal =
            PrincipalRef::in_domain(kind, id, domain).map_err(|_| malformed("requester"))?;
        DecisionPrincipalKey::of(&principal).map_err(|_| malformed("requester"))
    };
    match requester {
        Requester::Agent { identity } => in_mob(PrincipalKind::SharedAgent, identity),
        Requester::Host { surface } => in_mob(PrincipalKind::RuntimeHost, surface),
        Requester::Human { principal } => person_key(principal).ok_or_else(|| {
            malformed("requester").with_detail(
                "A person who asks for a change is named by their principal key: \
                 `principal:[\"human\", trust domain, id]`.",
            )
        }),
    }
}

/// The requester a held request records, as the seam commits "as": an
/// agent by its recorded identity (it reads no session and wakes nothing),
/// a host surface, or a person.
fn recorded_requester(requester: &Requester) -> ChangeRequester {
    match requester {
        Requester::Agent { identity } => ChangeRequester::agent_recorded(identity.as_str()),
        Requester::Host { surface } => ChangeRequester::host(surface.as_str()),
        Requester::Human { principal } => ChangeRequester::human(principal.as_str()),
    }
}

/// The refusal kind an execution refusal `token` names, for the kinds that
/// carry nothing but their token; `None` for any other.
fn execution_refusal_kind(token: &str) -> Option<IdentityRefusalKind> {
    [
        IdentityRefusalKind::SubjectWidened,
        IdentityRefusalKind::GrantRevoked,
        IdentityRefusalKind::IdentityUnknown,
        IdentityRefusalKind::OwnerReadOnly,
        IdentityRefusalKind::NotArchived,
        IdentityRefusalKind::PrivateRequester,
        IdentityRefusalKind::OperationReused,
    ]
    .into_iter()
    .find(|kind| kind.token() == token)
}

/// The refusal a decision that ended without its change answers: the kind
/// it was refused with when it was carried out, where the kind carries
/// nothing but its token, and `decision_closed` with the decision's state
/// otherwise.
fn closed_refusal(row: &StoredDecision) -> IdentityRefusal {
    let state = row.state.phase.as_str();
    let closed = || IdentityRefusalKind::DecisionClosed {
        state: state.to_owned(),
    };
    match row.state.phase {
        DecisionPhase::ExecutionRefused => {
            let token = row
                .execution_refusal
                .as_ref()
                .map_or("refused", DecisionReference::as_str);
            match execution_refusal_kind(token) {
                Some(kind) => IdentityRefusal::new(kind),
                // A decision ends this way only for a refusal the same
                // request cannot outlive (a refusal it can is released and
                // tried again), so the detail names the refusal and does
                // not say to ask again: the refusal is not retryable.
                None => IdentityRefusal::new(closed()).with_detail(format!(
                    "The approved change was refused when it was carried out ({token})."
                )),
            }
        }
        DecisionPhase::Rejected => IdentityRefusal::new(closed())
            .with_detail("The change was rejected by the person who decides it."),
        DecisionPhase::Expired => IdentityRefusal::new(closed())
            .with_detail("Nobody decided the change before its deadline: ask again."),
        DecisionPhase::Withdrawn => {
            IdentityRefusal::new(closed()).with_detail("The change was withdrawn: ask again.")
        }
        DecisionPhase::Approved => IdentityRefusal::new(closed()).with_detail(
            "The change is approved and is being carried out: it can no longer be withdrawn.",
        ),
        _ => IdentityRefusal::new(closed()),
    }
}

impl IdentityDecisions {
    /// The holder over the runtime's decisions `decisions`, committing
    /// through the seam `executor` and reading mobs and targets through
    /// `host`. It asks no host for consequence lines and tells no requester
    /// until it is given a source and a route.
    pub fn new(
        decisions: &RuntimeDecisions,
        executor: Arc<dyn IdentityChangeExecutor>,
        host: Arc<dyn HeldChangeHost>,
    ) -> Self {
        Self {
            decisions: decisions.clone(),
            executor,
            host,
            consequences: None,
            notices: Arc::new(NoRequesterNotices),
            clock: Arc::new(SystemDecisionClock),
            filed_by_incarnation: false,
            families: None,
        }
    }

    /// This holder as the holder of a mob a host serves beside its launch
    /// mob: it files a held change under the mob's incarnation
    /// ([`Self::subject_key`]).
    #[must_use]
    pub(crate) fn filed_by_incarnation(mut self) -> Self {
        self.filed_by_incarnation = true;
        self
    }

    /// This holder as the one whose executors a host's one driver runs,
    /// over the holders of every mob the host serves: a held change is
    /// carried out, and its end told, through the holder of its own mob
    /// ([`HeldChangeFamilies`]).
    #[must_use]
    pub fn with_families(mut self, families: Arc<dyn HeldChangeFamilies>) -> Self {
        self.families = Some(families);
        self
    }

    /// The subject key this holder files the decision of `operation_id` in
    /// `mob` under. The launch mob's holder files it under
    /// `{mob id}/{operation id}`. The holder of a mob a host serves beside
    /// its launch mob files it under
    /// `{mob id}@{incarnation}/{operation id}`: the incarnation is minted
    /// when a mob is created and kept by its store, so it is the same over
    /// an unload, a load and a restart, and another one for a mob created
    /// later under the same id. A held change of an earlier mob of an id is
    /// therefore no decision of a later one: it is not found as its
    /// operation, and is not carried out in it.
    fn subject_key(
        &self,
        mob: &MobHandle,
        operation_id: &OperationId,
    ) -> Result<DecisionSubjectKey, IdentityRefusal> {
        let mob_id = mob.mob_id();
        let key = if self.filed_by_incarnation {
            format!("{mob_id}@{}/{operation_id}", mob.incarnation())
        } else {
            format!("{mob_id}/{operation_id}")
        };
        DecisionSubjectKey::new(key).map_err(|_| malformed("request_id"))
    }

    /// Whether `row` is a decision this holder filed for `mob` as it runs
    /// now: its id is the one this holder derives for its kind, its
    /// operation and `mob`, and, for the holder of a mob a host serves
    /// beside its launch mob, its body names the incarnation of `mob`. A
    /// decision filed for an earlier mob of the id is not, and neither is
    /// one whose body names no incarnation (a body written before the body
    /// carried it): the record does not say it is of this mob, so it is of
    /// none this holder answers for.
    fn filed_for(&self, mob: &MobHandle, row: &StoredDecision) -> bool {
        let Some(operation_id) = OperationId::from_recorded(row.descriptor.operation_id.as_str())
        else {
            return false;
        };
        let filed_under_its_key = self
            .subject_key(mob, &operation_id)
            .is_ok_and(|key| DecisionId::derive(&row.descriptor.kind, &key) == row.decision_id);
        filed_under_its_key
            && (!self.filed_by_incarnation
                || HeldIdentityChange::of(row)
                    .is_ok_and(|held| held.is_of_incarnation(&mob.incarnation().to_string())))
    }

    /// This holder, asking `consequences` for the host's lines when a change
    /// is held and again when it is carried out.
    #[must_use]
    pub fn with_consequences(mut self, consequences: Arc<dyn HostConsequences>) -> Self {
        self.consequences = Some(consequences);
        self
    }

    /// This holder, telling requesters through `notices`.
    #[must_use]
    pub fn with_notices(mut self, notices: Arc<dyn IdentityDecisionNotices>) -> Self {
        self.notices = notices;
        self
    }

    /// This holder, reading deadlines on `clock`: the clock of the decision
    /// owner, when that is not the host's wall clock.
    #[must_use]
    pub fn with_clock(mut self, clock: Arc<dyn DecisionClock>) -> Self {
        self.clock = clock;
        self
    }

    /// The decision that holds `decision_id`, as it stands now.
    fn read(&self, decision_id: &DecisionId) -> Result<Option<StoredDecision>, IdentityRefusal> {
        match self.decisions.owner().get(decision_id) {
            Ok(row) => Ok(Some(row)),
            Err(DecisionError::NotFound(_)) => Ok(None),
            Err(error) => Err(unavailable(&error)),
        }
    }

    /// The decision that holds `operation_id` in `mob`, of whichever kind.
    fn find(
        &self,
        mob: &MobHandle,
        operation_id: &OperationId,
    ) -> Result<Option<StoredDecision>, IdentityRefusal> {
        if !self.decisions.is_available() {
            return Ok(None);
        }
        let subject_key = self.subject_key(mob, operation_id)?;
        for kind in IDENTITY_DECISION_KINDS {
            let kind = DecisionKind::new(kind).map_err(|_| malformed("request"))?;
            if let Some(row) = self.read(&DecisionId::derive(&kind, &subject_key))? {
                return Ok(Some(row));
            }
        }
        Ok(None)
    }

    /// The outcome a decision answers as it stands: `pending_decision`
    /// while it is held or approved, the committed outcome once it was
    /// carried out, a refusal once it ended otherwise.
    async fn state_of(
        &self,
        mob: &MobHandle,
        operation_id: &OperationId,
        row: &StoredDecision,
    ) -> IdentityOutcome {
        match row.state.phase {
            DecisionPhase::Open | DecisionPhase::Approved => IdentityOutcome::PendingDecision {
                operation_id: operation_id.clone(),
                decision_id: row.decision_id.to_string(),
                deadline_ms: row.descriptor.deadline_at_ms,
            },
            DecisionPhase::Executed => match committed_outcome(mob, operation_id).await {
                Ok(Some(outcome)) => outcome,
                // The decision says the change was carried out and its
                // receipt cannot be read now: the same read later answers.
                Ok(None) | Err(_) => {
                    IdentityOutcome::refused(IdentityRefusalKind::AdmissionUnsettled {
                        correlation: operation_id.to_string(),
                    })
                }
            },
            _ => IdentityOutcome::Refused(closed_refusal(row)),
        }
    }

    /// What holds the operation `operation_id` in `mob`: the outcome of its
    /// decision as it stands, or `None` when no decision holds it. The
    /// family's implementation asks this before it normalizes a request
    /// again, and for the `operation` read.
    pub async fn held(
        &self,
        mob: &MobHandle,
        operation_id: &OperationId,
    ) -> Result<Option<IdentityOutcome>, IdentityRefusal> {
        match self.find(mob, operation_id)? {
            Some(row) => Ok(Some(self.state_of(mob, operation_id, &row).await)),
            None => Ok(None),
        }
    }

    /// The operations the decisions of `mob`'s changes keep open, by the
    /// agent each names: a change held for a person's decision, and one a
    /// person approved that is not carried out yet (the states an operation
    /// reads `pending_decision` in). It is read from the decision rows at
    /// each call, one listing of the open and one of the approved decisions:
    /// nothing is kept of it, nothing is written (a decision past its
    /// deadline is left out, and its expiry is not recorded here), and no
    /// agent is read, loaded or woken. A decision of another mob, of another
    /// kind, or whose body this version does not read keeps nothing open
    /// here. A launch whose decisions are unavailable holds no change, so
    /// nothing is open; a listing that fails is refused, never read as
    /// "nothing is open".
    pub fn open_operations(
        &self,
        mob: &MobHandle,
    ) -> Result<BTreeMap<String, Vec<OperationId>>, IdentityRefusal> {
        let mut open: BTreeMap<String, Vec<OperationId>> = BTreeMap::new();
        if !self.decisions.is_available() {
            return Ok(open);
        }
        let mob_id = mob.mob_id();
        for phase in [DecisionPhase::Open, DecisionPhase::Approved] {
            let filter = DecisionListFilter::default().with_phase(phase);
            let mut after: Option<String> = None;
            loop {
                let page = self
                    .decisions
                    .owner()
                    .list(&filter, after.as_deref(), DECISION_PAGE_MAX)
                    .map_err(|error| unlisted(&error))?;
                for row in &page.decisions {
                    // The listing reads each row at its clock: one past its
                    // deadline is no longer held.
                    if !matches!(
                        row.state.phase,
                        DecisionPhase::Open | DecisionPhase::Approved
                    ) {
                        continue;
                    }
                    let Ok(held) = HeldIdentityChange::of(row) else {
                        continue;
                    };
                    if held.mob_id != mob_id.as_str() {
                        continue;
                    }
                    // A decision filed for an earlier mob of the id keeps
                    // nothing open in this one.
                    if !self.filed_for(mob, row) {
                        continue;
                    }
                    let Some(operation_id) =
                        OperationId::from_recorded(row.descriptor.operation_id.as_str())
                    else {
                        continue;
                    };
                    for identity in held.request.action.identities() {
                        open.entry(identity.to_owned())
                            .or_default()
                            .push(operation_id.clone());
                    }
                }
                match page.next_after {
                    // A store that names the same row again has no more.
                    Some(next) if after.as_ref() != Some(&next) => after = Some(next),
                    _ => break,
                }
            }
        }
        Ok(open)
    }

    /// The host's lines for `request`, asked now.
    async fn host_lines(
        &self,
        request: &IdentityRequest,
    ) -> Result<Vec<ConsequenceLine>, IdentityRefusal> {
        let Some(consequences) = &self.consequences else {
            return Ok(Vec::new());
        };
        consequences.lines(request).await.map_err(|refusal| {
            tracing::warn!(%refusal, "the host's identity consequence lines are unavailable");
            IdentityRefusal::new(IdentityRefusalKind::ConsequencesUnavailable)
        })
    }

    /// The agent `identity` of `mob` as it stands now.
    async fn target(&self, mob: &MobHandle, identity: &str) -> Result<HeldTarget, IdentityRefusal> {
        self.host
            .target(mob, identity)
            .await?
            .ok_or_else(|| IdentityRefusal::new(IdentityRefusalKind::IdentityUnknown).at("name"))
    }

    /// The tool configuration of `request` in `mob` as it stands now, and
    /// whether the agent it changes is one its host configures: each agent
    /// the request names with the profile it runs on after the change, and
    /// what the mob's definition declares for each of those profiles.
    /// Nothing is built and no record of a build is read.
    async fn tool_configurations(
        &self,
        mob: &MobHandle,
        request: &IdentityRequest,
    ) -> Result<(ToolConfigurations, bool), IdentityRefusal> {
        let mut configured = false;
        let agents = match &request.action {
            RequestAction::Create { identities } => identities
                .iter()
                .map(|created| {
                    AgentTools::new(
                        created.spec.identity.as_str(),
                        created.spec.profile.as_str(),
                    )
                })
                .collect(),
            RequestAction::Update(update) => {
                let target = self.target(mob, &update.identity).await?;
                configured = target.configured;
                let profile = update.profile.as_ref().map_or_else(
                    || target.entry.profile.clone(),
                    |change| change.to.as_str().to_owned(),
                );
                vec![AgentTools::new(update.identity.as_str(), profile)]
            }
            RequestAction::Remove { identity, .. }
            | RequestAction::Archive { identity, .. }
            | RequestAction::Unarchive { identity, .. }
            | RequestAction::Adopt { identity, .. } => {
                let target = self.target(mob, identity).await?;
                configured = target.configured;
                vec![AgentTools::new(identity.as_str(), target.entry.profile)]
            }
        };
        let definition = mob.definition();
        let mut profiles = BTreeMap::new();
        for agent in &agents {
            if profiles.contains_key(agent.profile()) {
                continue;
            }
            let Some(binding) = definition.profiles.get(&ProfileName::from(agent.profile())) else {
                return Err(IdentityRefusal::new(IdentityRefusalKind::ProfileUnknown {
                    profile: agent.profile().to_owned(),
                }));
            };
            let realm = match binding {
                meerkat_mob::ProfileBinding::RealmRef { realm_profile } => {
                    self.host.realm_profile(mob, realm_profile).await?
                }
                meerkat_mob::ProfileBinding::Inline(_) => None,
            };
            let declared =
                ProfileTools::declared(agent.profile(), binding, definition, realm.as_ref())?;
            profiles.insert(agent.profile().to_owned(), declared);
        }
        let tools = ToolConfigurations::of_request(request, agents, profiles)?;
        Ok((tools, configured))
    }

    /// The rendering of `request` in `mob` as it stands now, and whether the
    /// agent it changes is one its host configures.
    async fn rendered(
        &self,
        mob: &MobHandle,
        request: &IdentityRequest,
    ) -> Result<(Rendering, bool), IdentityRefusal> {
        let lines = self.host_lines(request).await?;
        let (tools, configured) = self.tool_configurations(mob, request).await?;
        let hook = move |_: &IdentityRequest| -> Result<Vec<ConsequenceLine>, HookRefusal> {
            Ok(lines.clone())
        };
        let rendering = render_held_v1(request, Some(&hook), &tools)?;
        Ok((rendering, configured))
    }

    /// The rendering a decision of `request` in `mob` holds: the canonical
    /// text of the request with the host's lines and the tool configuration,
    /// as they stand now. What a plan shows is this, so that the digest it
    /// names is the one a decision binds.
    pub async fn render(
        &self,
        mob: &MobHandle,
        request: &IdentityRequest,
    ) -> Result<Rendering, IdentityRefusal> {
        Ok(self.rendered(mob, request).await?.0)
    }

    /// Hold `normalized`, a change of `mob` asked as `operation_id` whose
    /// outcome under the host's rules is `require_approval`: answer
    /// `pending_decision`, with nothing written but the decision. The same
    /// operation and request asked again answer the decision as it stands
    /// (still held, carried out, or ended); another request under the
    /// operation is refused `operation_reused`.
    pub async fn hold(
        &self,
        mob: &MobHandle,
        operation_id: &OperationId,
        normalized: &NormalizedRequest,
    ) -> IdentityOutcome {
        match self.hold_or_refuse(mob, operation_id, normalized).await {
            Ok(outcome) => outcome,
            Err(refusal) => IdentityOutcome::Refused(refusal),
        }
    }

    async fn hold_or_refuse(
        &self,
        mob: &MobHandle,
        operation_id: &OperationId,
        normalized: &NormalizedRequest,
    ) -> Result<IdentityOutcome, IdentityRefusal> {
        let PlannedOutcome::RequireApproval { approver_id } = &normalized.plan.outcome else {
            return Err(IdentityRefusal::new(IdentityRefusalKind::Unsupported {
                feature: "a held decision for a change no person decides".to_owned(),
            }));
        };
        if self.decisions.is_candidate() {
            return Err(IdentityRefusal::new(IdentityRefusalKind::CandidateLaunch));
        }
        if !self.decisions.is_available() {
            return Err(
                IdentityRefusal::new(IdentityRefusalKind::ApprovalUnavailable).with_detail(
                    "The change needs a decision and this host keeps no durable decisions: \
                     nothing was written.",
                ),
            );
        }
        let request = &normalized.request;
        let binding = Binding::of(self.subject_key(mob, operation_id)?, operation_id, request)?;
        // One operation id names one change: a decision that holds it, of
        // whichever kind, answers it, and another request under it is
        // refused.
        if let Some(row) = self.find(mob, operation_id)? {
            return self.replayed(mob, operation_id, &binding, &row).await;
        }
        let rendering = self.render(mob, request).await?;
        let mob_id = mob.mob_id().as_str().to_owned();
        let requester = decision_requester(&mob_id, request)?;
        let body = serde_json::to_value(HeldIdentityChange {
            format: HELD_CHANGE_FORMAT.to_owned(),
            mob_id,
            mob_incarnation: self
                .filed_by_incarnation
                .then(|| mob.incarnation().to_string()),
            request: request.clone(),
        })
        .map_err(|_| malformed("request"))?;
        let body = DecisionRequestBody::from_value(&body).map_err(|_| {
            IdentityRefusal::new(IdentityRefusalKind::TextTooLarge {
                limit: super::request::REQUEST_BODY_MAX_BYTES as u64,
            })
            .at("request")
        })?;
        let approver_id = ApproverId::new(approver_id.clone())
            .map_err(|_| IdentityRefusal::new(IdentityRefusalKind::ApprovalUnavailable))?;
        let subject_digest = meerkat_core::approval::decision::Sha256Digest::new(
            rendering.subject_digest().as_str(),
        )
        .map_err(|_| malformed("rendering"))?;
        let rendering = DecisionRendering::new(rendering.into_string()).map_err(|_| {
            IdentityRefusal::new(IdentityRefusalKind::TextTooLarge {
                limit: super::render::RENDERING_MAX_BYTES as u64,
            })
            .at("rendering")
        })?;
        let renderer = DecisionRenderer::new(RENDERER).map_err(|_| malformed("rendering"))?;
        let now_ms = self.clock.now_ms().map_err(|_| {
            IdentityRefusal::new(IdentityRefusalKind::ApprovalUnavailable)
                .with_detail("The host's clock gave no time: nothing was written.")
        })?;
        let hold = DecisionHoldRequest::new(
            binding.kind.clone(),
            binding.subject_key.clone(),
            binding.operation_id.clone(),
            binding.request_digest.clone(),
            body,
            requester,
            approver_id,
            now_ms.saturating_add(normalized.plan.timeout_ms),
            rendering,
            renderer,
        )
        .with_expected_subject_digest(subject_digest);
        let routable = self.notices.routable(&request.requester);
        match self.decisions.owner().hold(hold, routable) {
            Ok(row) => {
                self.decisions.notify_work();
                Ok(self.state_of(mob, operation_id, &row).await)
            }
            // Another hold of the same operation was written first: this
            // one is its replay, or another request under its id.
            Err(DecisionError::Refused {
                refusal: DecisionRefusal::HoldConflict,
                ..
            }) => match self.read(&binding.decision_id)? {
                Some(row) => self.replayed(mob, operation_id, &binding, &row).await,
                None => Err(IdentityRefusal::new(IdentityRefusalKind::OperationReused)),
            },
            Err(DecisionError::CandidateLaunch) => {
                Err(IdentityRefusal::new(IdentityRefusalKind::CandidateLaunch))
            }
            Err(error) => Err(unavailable(&error)),
        }
    }

    /// The answer of a hold whose decision exists: the decision as it
    /// stands when it binds this operation and request, `operation_reused`
    /// otherwise.
    async fn replayed(
        &self,
        mob: &MobHandle,
        operation_id: &OperationId,
        binding: &Binding,
        row: &StoredDecision,
    ) -> Result<IdentityOutcome, IdentityRefusal> {
        let same = row.descriptor.operation_id == binding.operation_id
            && row.descriptor.request_digest == binding.request_digest;
        if !same {
            return Err(IdentityRefusal::new(IdentityRefusalKind::OperationReused));
        }
        Ok(self.state_of(mob, operation_id, row).await)
    }

    /// Withdraw the change `request` names, whose decision is still held:
    /// by its requester, or by the host. Refused `decision_not_found` when
    /// no decision holds the operation (or the requester is not the
    /// decision's), and `decision_closed` with the decision's state once it
    /// is no longer open.
    pub fn withdraw(
        &self,
        mob: &MobHandle,
        requester: &ChangeRequester,
        request: &WithdrawRequest,
    ) -> Result<Withdrawn, IdentityRefusal> {
        let not_found = || IdentityRefusal::new(IdentityRefusalKind::DecisionNotFound);
        let Some(row) = self.find(mob, &request.operation_id)? else {
            return Err(not_found());
        };
        let withdrawer = match requester {
            ChangeRequester::Host { .. } => DecisionWithdrawer::Host,
            other => DecisionWithdrawer::Requester(requester_key(
                mob.mob_id().as_str(),
                &other.recorded(),
            )?),
        };
        match self
            .decisions
            .owner()
            .withdraw(&row.decision_id, &withdrawer)
        {
            Ok(row) => {
                self.decisions.notify_work();
                Ok(Withdrawn::new(
                    request.operation_id.clone(),
                    row.decision_id.to_string(),
                ))
            }
            Err(DecisionError::Refused {
                refusal: DecisionRefusal::NotRequester,
                ..
            }) => Err(not_found()),
            Err(DecisionError::Refused {
                refusal: DecisionRefusal::NotOpen | DecisionRefusal::Expired,
                ..
            }) => match self.read(&row.decision_id)? {
                Some(row) => Err(closed_refusal(&row)),
                None => Err(not_found()),
            },
            Err(DecisionError::CandidateLaunch) => {
                Err(IdentityRefusal::new(IdentityRefusalKind::CandidateLaunch))
            }
            Err(error) => Err(unavailable(&error)),
        }
    }

    /// The executors of the `identity.*` decision kinds, one per kind: what
    /// a [`DecisionExecutorLoop`] over the runtime's decisions registers.
    pub fn executors(&self) -> Vec<Arc<dyn DecisionExecutor>> {
        IDENTITY_DECISION_KINDS
            .into_iter()
            .filter_map(|kind| DecisionKind::new(kind).ok())
            .map(|kind| {
                Arc::new(IdentityDecisionExecutor {
                    kind,
                    decisions: self.clone(),
                }) as Arc<dyn DecisionExecutor>
            })
            .collect()
    }

    /// Core's executor loop over the runtime's decisions for the
    /// `identity.*` kinds: a host that drives passes itself makes them on
    /// it. `approver` answers whether the party that decided may still do
    /// so (`UnifiedRuntime::decision_approver_currentness`).
    ///
    /// # Errors
    /// What [`DecisionExecutorLoop::start`] refuses.
    pub fn executor_loop(
        &self,
        approver: Arc<dyn DecisionApproverCurrentness>,
    ) -> Result<DecisionExecutorLoop, DecisionExecutorLoopError> {
        DecisionExecutorLoop::start(self.decisions.owner().clone(), self.executors(), approver)
    }

    /// Drive the executors of the `identity.*` kinds for the runtime: a
    /// pass at start, after each change of a decision, and per `tick` while
    /// a decision waits. `Ok(None)` for a candidate launch, which carries
    /// nothing out.
    ///
    /// # Errors
    /// What [`DecisionExecutorLoop::start`] refuses, but a candidate launch.
    pub fn spawn_driver(
        &self,
        approver: Arc<dyn DecisionApproverCurrentness>,
        tick: Duration,
    ) -> Result<Option<DecisionExecutorDriver>, DecisionExecutorLoopError> {
        match DecisionExecutorDriver::spawn(&self.decisions, self.executors(), approver, tick) {
            Ok(driver) => Ok(Some(driver)),
            Err(DecisionExecutorLoopError::Decision(DecisionError::CandidateLaunch)) => Ok(None),
            Err(error) => Err(error),
        }
    }
}

/// The executor of one `identity.*` decision kind: see the module
/// documentation.
struct IdentityDecisionExecutor {
    kind: DecisionKind,
    decisions: IdentityDecisions,
}

impl IdentityDecisionExecutor {
    fn refused(reason: &str) -> DecisionExecution {
        match DecisionReference::new(reason) {
            Ok(reference) => DecisionExecution::Refused(reference),
            // Every reason this executor names is a valid reference.
            Err(_) => DecisionExecution::Released,
        }
    }

    /// What a refusal ends an execution with: an attempt that may be made
    /// again for a refusal the same request may outlive, the decision's end
    /// otherwise.
    fn refusal(refusal: &IdentityRefusal) -> DecisionExecution {
        // A permit past its deadline commits nothing: its release records
        // the expiry.
        let lapsed = matches!(refusal.kind, IdentityRefusalKind::DecisionClosed { .. });
        if refusal.is_retryable() || lapsed {
            DecisionExecution::Released
        } else {
            Self::refused(refusal.kind_token())
        }
    }

    /// The held change of `decision`, its operation, and where its mob
    /// stands on this host now.
    async fn held(
        &self,
        decision: &StoredDecision,
    ) -> Result<(HeldIdentityChange, OperationId, HeldMob), String> {
        let body = HeldIdentityChange::of(decision)?;
        let operation_id = OperationId::from_recorded(decision.descriptor.operation_id.as_str())
            .ok_or_else(|| {
                format!(
                    "decision {} names no operation of a change of agents",
                    decision.decision_id
                )
            })?;
        let mob = self.mob_of(&body.mob_id).await;
        Ok((body, operation_id, mob))
    }

    /// Where the mob `mob_id` a held change names stands on this host now:
    /// with the holder of that mob's held changes and the mob's handle when
    /// the host runs it. The mob is the one the host's family wrote into
    /// the held body when it held the change; nothing a requester or a
    /// decider wrote chooses it.
    async fn mob_of(&self, mob_id: &str) -> HeldMob {
        let holder = match &self.decisions.families {
            // The holder of one mob: it answers for its own mob alone.
            None => self.decisions.clone(),
            Some(families) => match families.of(mob_id).await {
                HeldFamily::Here(holder) => *holder,
                HeldFamily::Away => return HeldMob::Away,
                HeldFamily::Gone => return HeldMob::Gone,
            },
        };
        match holder.host.mob(mob_id) {
            Some(mob) => HeldMob::Here(Box::new((holder, mob))),
            None => HeldMob::Away,
        }
    }
}

/// Where the mob of a held change stands when its decision is carried out
/// or its end is told.
enum HeldMob {
    /// The host runs the mob: the holder of its held changes, and its
    /// handle.
    Here(Box<(IdentityDecisions, MobHandle)>),
    /// The host does not run the mob now: asked again at a later pass.
    Away,
    /// The host has no mob of the id any more.
    Gone,
}

#[async_trait]
impl DecisionExecutor for IdentityDecisionExecutor {
    fn kind(&self) -> &DecisionKind {
        &self.kind
    }

    async fn execute(
        &self,
        decision: &StoredDecision,
        permit: &DecisionExecutionPermit,
    ) -> DecisionExecution {
        let descriptor = &decision.descriptor;
        if !permit.binds(&descriptor.operation_id, &descriptor.request_digest) {
            return DecisionExecution::Released;
        }
        // Decided by people only.
        if !matches!(
            permit.consent(),
            ConsentProvenance::Decided {
                tier: DecisionTier::Human,
                ..
            }
        ) {
            return Self::refused("not_a_human_decision");
        }
        let (body, operation_id, mob) = match self.held(decision).await {
            Ok(held) => held,
            Err(reason) => {
                tracing::warn!(%reason, "a held change of agents cannot be read");
                return Self::refused("unreadable_request");
            }
        };
        // A mob the host does not run now, and a mob that is gone: nothing
        // is carried out, and the decision ends at its deadline.
        let HeldMob::Here(here) = mob else {
            return DecisionExecution::Released;
        };
        let (holder, mob) = *here;
        // The mob the host runs under the id now is the one the change was
        // held for, or the change is carried out in none.
        if !holder.filed_for(&mob, decision) {
            return Self::refused(MOB_REPLACED_REFUSAL);
        }
        // What the person was shown, made again from what stands now: any
        // difference ends the decision.
        let configured = match holder.rendered(&mob, &body.request).await {
            Ok((rendering, configured)) => {
                if rendering.subject_digest().as_str() != descriptor.subject_digest.as_str() {
                    return Self::refused(SUBJECT_WIDENED_REFUSAL);
                }
                configured
            }
            Err(refusal) => return Self::refusal(&refusal),
        };
        let requester = recorded_requester(&body.request.requester);
        let commit = ChangeCommit::new(
            &mob,
            &operation_id,
            &requester,
            ChangeConsent::Decided(permit),
            &body.request,
        )
        .of_configured_target(configured);
        match holder.executor.commit(commit).await {
            Ok(committed) => match &committed.outcome {
                IdentityOutcome::Done { .. } | IdentityOutcome::PendingPreparation { .. } => {
                    DecisionExecution::Committed
                }
                IdentityOutcome::Refused(refusal) => Self::refusal(refusal),
                // The seam holds nothing: no commit was made.
                IdentityOutcome::PendingDecision { .. } => DecisionExecution::Released,
            },
            // The commit ended with no outcome: Core's receipt says
            // whether it landed. With no receipt the permit is released,
            // and the next pass asks the same operation again, which Core
            // commits at most once. A known limit: Core's loop has no
            // answer that leaves a permit outstanding, so a commit that
            // landed while its receipt could not be read, at the decision's
            // deadline, ends the decision expired though the change stands.
            Err(error) => {
                tracing::warn!(%error, "a held change of agents ended with no outcome");
                match committed_as(&mob, &operation_id, &body.request).await {
                    Ok(true) => DecisionExecution::Committed,
                    Ok(false) | Err(_) => DecisionExecution::Released,
                }
            }
        }
    }

    async fn committed(
        &self,
        decision: &StoredDecision,
    ) -> Result<bool, DecisionExecutorRecordUnavailable> {
        let (body, operation_id, mob) = self
            .held(decision)
            .await
            .map_err(DecisionExecutorRecordUnavailable)?;
        let HeldMob::Here(here) = mob else {
            return Err(DecisionExecutorRecordUnavailable(
                "this host does not run the mob of the held change now".to_owned(),
            ));
        };
        let (holder, mob) = *here;
        // A change held for an earlier mob of the id was never committed
        // in this one, whatever this mob's receipts say of the operation.
        if !holder.filed_for(&mob, decision) {
            return Ok(false);
        }
        // Only a receipt of the held request is this decision's commit:
        // another request the requester asked under the same request id
        // may have committed, and that is not the change a person approved.
        committed_as(&mob, &operation_id, &body.request)
            .await
            .map_err(|error| DecisionExecutorRecordUnavailable(error.to_string()))
    }

    async fn deliver_notice(
        &self,
        decision: &StoredDecision,
    ) -> Option<ApprovalConsentDeliveryState> {
        if matches!(
            decision.state.phase,
            DecisionPhase::Open | DecisionPhase::Approved
        ) {
            return None;
        }
        let failed = |reason: &str| {
            Some(ApprovalConsentDeliveryState::Failed {
                reason: reason.to_owned(),
            })
        };
        let (body, operation_id, mob) = match self.held(decision).await {
            Ok(held) => held,
            Err(reason) => return Some(ApprovalConsentDeliveryState::Failed { reason }),
        };
        // The requester is an agent of the mob the change was held for, and
        // is told through that mob's own holder alone. A mob that is gone
        // has nobody to tell, and a later mob of the id has no agent that
        // asked: neither is told. A mob the host does not run now is not
        // told now: no notice is made, and no route is asked, until the
        // mob is there and the decision is seen to be filed for it. A mob
        // that never returns is never told; its notice stays owed.
        let (holder, mob) = match mob {
            HeldMob::Gone => return failed("the mob of the change is gone"),
            HeldMob::Away => return None,
            HeldMob::Here(here) => {
                let (holder, mob) = *here;
                if !holder.filed_for(&mob, decision) {
                    return failed("the mob of the change was replaced");
                }
                (holder, mob)
            }
        };
        let outcome = match decision.state.phase {
            // A change that was carried out is told with what Core
            // committed; a receipt that cannot be read now is asked again.
            DecisionPhase::Executed => committed_outcome(&mob, &operation_id).await.ok()??,
            _ => IdentityOutcome::Refused(closed_refusal(decision)),
        };
        let notice = IdentityDecisionNotice {
            decision_id: decision.decision_id.to_string(),
            mob_id: body.mob_id,
            operation_id,
            action: body.request.action.token(),
            agents: body
                .request
                .action
                .identities()
                .into_iter()
                .map(str::to_owned)
                .collect(),
            requester: body.request.requester,
            outcome,
        };
        // Told through the route of the change's own mob.
        holder.notices.deliver(&notice).await
    }

    /// A held change's execution leaves nothing outside the decision store
    /// to undo: what it commits is Core's, under the operation. What the
    /// host's one driver does here is end the changes of this kind that
    /// are still held for a mob the host no longer has: a change whose mob
    /// the host destroyed (its mob set's record says so), and one filed
    /// for an earlier mob of an id the host runs a later mob under, is
    /// withdrawn as the host, since nobody is left to carry it out. A mob
    /// the host has and does not run now keeps its held changes, and so
    /// does a mob the host has no record of: a store this launch does not
    /// see and a mob an earlier process destroyed are not known to be
    /// gone, and their changes end at their deadlines. The holder of one
    /// mob (a host of one mob) ends nothing here. No outcome: the
    /// withdrawals are the decision service's own records, and a pass is
    /// asked for after them.
    async fn sweep(&self) -> Vec<(DecisionId, DecisionPassOutcome)> {
        if self.decisions.families.is_none() || !self.decisions.decisions.is_available() {
            return Vec::new();
        }
        let owner = self.decisions.decisions.owner();
        let filter = DecisionListFilter::default()
            .with_phase(DecisionPhase::Open)
            .with_kind(self.kind.clone());
        let mut open = Vec::new();
        let mut after: Option<String> = None;
        loop {
            let page = match owner.list(&filter, after.as_deref(), DECISION_PAGE_MAX) {
                Ok(page) => page,
                Err(error) => {
                    tracing::warn!(%error, "the held changes of agents could not be listed");
                    return Vec::new();
                }
            };
            open.extend(
                page.decisions
                    .into_iter()
                    .filter(|row| row.state.phase == DecisionPhase::Open),
            );
            match page.next_after {
                // A store that names the same row again has no more.
                Some(next) if after.as_ref() != Some(&next) => after = Some(next),
                _ => break,
            }
        }
        let mut withdrawn = 0_usize;
        for row in open {
            let Ok(body) = HeldIdentityChange::of(&row) else {
                continue;
            };
            let left_behind = match self.mob_of(&body.mob_id).await {
                HeldMob::Gone => true,
                HeldMob::Here(here) => {
                    let (holder, mob) = *here;
                    !holder.filed_for(&mob, &row)
                }
                HeldMob::Away => false,
            };
            if !left_behind {
                continue;
            }
            match owner.withdraw(&row.decision_id, &DecisionWithdrawer::Host) {
                Ok(_) => withdrawn += 1,
                // Decided, expired or withdrawn since the listing.
                Err(DecisionError::Refused {
                    refusal: DecisionRefusal::NotOpen | DecisionRefusal::Expired,
                    ..
                }) => {}
                Err(error) => tracing::warn!(
                    decision_id = %row.decision_id,
                    %error,
                    "a held change of a mob the host no longer has could not be withdrawn"
                ),
            }
        }
        if withdrawn > 0 {
            tracing::info!(
                kind = %self.kind,
                withdrawn,
                "held changes of mobs the host no longer has were withdrawn"
            );
            // The withdrawn decisions owe their notices: a pass delivers
            // them, or records that nobody is left to tell.
            self.decisions.decisions.notify_work();
        }
        Vec::new()
    }
}

/// Withdraw, as the host, the held changes that are still open and whose
/// body names the mob `mob_id`: what a host does before it creates a mob
/// under the id, when whatever is still held under it was asked for an
/// earlier mob. It reads the decision rows alone: no mob is read or
/// loaded. A change a person already approved is not open and is left: it
/// is carried out in no later mob of the id and ends at its deadline. A
/// launch that keeps no decisions, and a candidate launch, withdraw
/// nothing. Answers how many it withdrew.
///
/// # Errors
/// A listing or a withdrawal the decision store could not make, in words
/// for the host's log: some may be left, and asking again withdraws them.
pub(crate) fn withdraw_open_held_changes_of(
    decisions: &RuntimeDecisions,
    mob_id: &str,
) -> Result<usize, String> {
    if !decisions.is_available() || decisions.is_candidate() {
        return Ok(0);
    }
    let owner = decisions.owner();
    let filter = DecisionListFilter::default().with_phase(DecisionPhase::Open);
    let mut open = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let page = owner
            .list(&filter, after.as_deref(), DECISION_PAGE_MAX)
            .map_err(|error| format!("the open decisions could not be listed: {error}"))?;
        for row in &page.decisions {
            if row.state.phase != DecisionPhase::Open {
                continue;
            }
            let Ok(held) = HeldIdentityChange::of(row) else {
                continue;
            };
            if held.mob_id == mob_id {
                open.push(row.decision_id.clone());
            }
        }
        match page.next_after {
            // A store that names the same row again has no more.
            Some(next) if after.as_ref() != Some(&next) => after = Some(next),
            _ => break,
        }
    }
    let mut withdrawn = 0;
    for decision_id in open {
        match owner.withdraw(&decision_id, &DecisionWithdrawer::Host) {
            Ok(_) => withdrawn += 1,
            // Decided, expired or withdrawn since the listing: not open.
            Err(DecisionError::Refused {
                refusal: DecisionRefusal::NotOpen | DecisionRefusal::Expired,
                ..
            }) => {}
            Err(error) => {
                return Err(format!(
                    "the held change {decision_id} could not be withdrawn: {error}"
                ));
            }
        }
    }
    if withdrawn > 0 {
        decisions.notify_work();
    }
    Ok(withdrawn)
}
