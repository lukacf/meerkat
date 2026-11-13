//! Questions a member asks a person (`ask_user`).
//!
//! Core owns the tool and its vocabulary (`meerkat_core::ask_user`) and the
//! decision kind that holds a pending question set
//! (`meerkat_runtime::ask_user`, kind `ask_user.questions`): the decision's
//! machine owns the whole lifecycle of a set, and this module keeps no state
//! of its own. Meta is the host:
//!
//! - **Mount.** Every member build passes the base session layer's hook
//!   ([`AskUserSlot::after_hook`]), which composes the tool over the build's
//!   external tools ([`crate::tool_compose::ComposedExternalTools`]) with a
//!   [`MemberAskUserHost`] for that member. A build whose tools already name
//!   `ask_user` (a host's own tool of that name) keeps its own, and Meta
//!   mounts none: the questions that tool asks are that tool's, so no
//!   decision is held for them, the read lists none of them and no notice
//!   is sent for them. Nothing is opened, read or started for the mount.
//! - **Surface.** The slot is late-bound: the runtime fills it
//!   ([`AskUserSurface`]) once it keeps durable decisions and serves its
//!   long-term agents. A call answers `no_human_surface` at once, holding
//!   nothing, while the slot is empty, when the runtime keeps no durable
//!   decisions, on a candidate launch (which acts nowhere), when the runtime
//!   serves no Console ([`crate::console_schedules::ConsoleServed`]), and for
//!   an asker no answer can be routed back to (a member of another mob than
//!   the runtime's own, or a name that is no agent name). Otherwise the set
//!   is held and the call answers `asked`.
//! - **Read.** [`open_question_sets`] lists the open sets, soonest deadline
//!   first, for `meta/gating/questions`.
//! - **Answer.** [`answer_question_set`] is the one hand-off of a person's
//!   answer to the decision owner, for `meta/gating/decide` on the Console
//!   and on stdio. Its match on the decision's kind is closed: a decision of
//!   any other kind is [`QuestionSetRefusal::NotAQuestionSet`], which each
//!   route answers as it answers an unknown pending id.
//! - **Notice.** The kind's executor is driven for the runtime from its first
//!   set on (an executor claim creates files, so none is made before)
//!   ([`spawn_ask_user_driver`]); the end of a set reaches its asker as a
//!   keyed send ([`MemberKeyedNotices`], key [`ask_user_notice_key`]) whose
//!   body is the typed result ([`ask_user_result_input`]).

use std::sync::{Arc, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use meerkat_core::approval::ApprovalConsentDeliveryState;
use meerkat_core::approval::consent::{AuthenticatedApprover, ConsentEligibility};
use meerkat_core::approval::decision::{
    AttestedDecision, DECISION_PAGE_MAX, DecisionId, DecisionIngress, DecisionListFilter,
    DecisionPhase, DecisionPrincipalKey, DecisionReference, PresentationAudience,
    PresentationRecord, Sha256Digest, StoredDecision,
};
use meerkat_core::ask_user::{
    ASK_USER_RESULT_INPUT, ASK_USER_TOOL, AskUserAnswers, AskUserDispatcher, AskUserHost,
    AskUserQuestionAnswer, AskUserQuestionSet, AskUserRefusal, AskUserRequest, AskUserResult,
};
use meerkat_core::service::CreateSessionRequest;
use meerkat_core::{PrincipalKind, PrincipalRef, TrustDomainId};
use meerkat_runtime::ask_user::{
    ASK_USER_QUESTIONS_KIND, AskUserApproverCurrent, AskUserBody, AskUserError, AskUserExecutor,
    AskUserHold, AskUserNotice, AskUserNotices, ask_user_questions_kind, ask_user_result_of,
    attest_ask_user_answers, hold_ask_user,
};
use meerkat_runtime::decision::{
    DecisionClock, DecisionError, DecisionExecutor, DecisionRefusal, SystemDecisionClock,
};
use serde::Serialize;
use serde_json::{Value, json};

use crate::console_schedules::ConsoleServed;
use crate::durable_decisions::{DecisionExecutorDriver, RuntimeDecisions};
use crate::mob_handle_runtime::{MobRuntime, PreBuildHook};

/// The `kind` of a decided question set in a `meta/gating/decide` result.
pub const QUESTION_SET_RESULT_KIND: &str = "question_set";

/// The ingress and channel of an answer given on the Console.
pub(crate) const CONSOLE_CHANNEL: &str = "console";

/// The ingress and channel of an answer the stdio host carries.
pub(crate) const HOST_CHANNEL: &str = "host";

/// What a decide of a set that ended is told.
pub(crate) const NO_LONGER_OPEN: &str = "the question set is no longer open";

/// What a decide on another digest than the set's is told.
const CHANGED_SINCE_SHOWN: &str = "the question set changed since it was shown";

/// The key the notice of the decision `decision_id` is sent under: a notice
/// asked for twice is delivered once.
pub fn ask_user_notice_key(decision_id: &str) -> String {
    format!("ask-user:{decision_id}")
}

/// The input that carries `result` to the asking member: the label
/// [`ASK_USER_RESULT_INPUT`] on a line of its own, then the typed result as
/// one JSON document. The one encoding of a result as an input.
///
/// # Errors
/// The encoder's, which a typed result does not raise.
pub fn ask_user_result_input(result: &AskUserResult) -> Result<String, serde_json::Error> {
    Ok(format!(
        "{ASK_USER_RESULT_INPUT}\n{}",
        serde_json::to_string(result)?
    ))
}

/// What a runtime serves of the questions its members ask: its durable
/// decisions, whether it serves a Console, and the mob whose members it can
/// route an answer back to.
pub(crate) struct AskUserSurface {
    decisions: RuntimeDecisions,
    console: ConsoleServed,
    mob_id: String,
    clock: Arc<dyn DecisionClock>,
    /// Signalled by each set the runtime holds: what starts the executor's
    /// driver at the first one ([`AskUserDriver`]).
    held: Arc<tokio::sync::Notify>,
}

impl std::fmt::Debug for AskUserSurface {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AskUserSurface")
            .field("decisions", &self.decisions)
            .field("console_served", &self.console.is_served())
            .field("mob_id", &self.mob_id)
            .finish_non_exhaustive()
    }
}

impl AskUserSurface {
    /// The surface of the runtime of the mob `mob_id` over `decisions`,
    /// which serves a Console while `console` says so.
    pub(crate) fn new(
        decisions: RuntimeDecisions,
        console: ConsoleServed,
        mob_id: impl Into<String>,
    ) -> Self {
        Self {
            decisions,
            console,
            mob_id: mob_id.into(),
            clock: Arc::new(SystemDecisionClock),
            held: Arc::default(),
        }
    }

    /// The signal each set the runtime holds raises.
    pub(crate) fn held_signal(&self) -> Arc<tokio::sync::Notify> {
        Arc::clone(&self.held)
    }

    /// This surface reading `clock` when it holds a set.
    #[cfg(test)]
    pub(crate) fn with_clock(mut self, clock: Arc<dyn DecisionClock>) -> Self {
        self.clock = clock;
        self
    }

    /// Whether a person can be shown a question now: the runtime keeps
    /// durable decisions, it speaks for its composition, and it serves a
    /// Console.
    fn shows_questions(&self) -> bool {
        if !self.decisions.is_available() {
            return false;
        }
        if self.decisions.is_candidate() {
            return false;
        }
        self.console.is_served()
    }

    /// Whether the answer to a question of `asker`, a member of the mob
    /// `mob_id`, can be routed back to it: it is a member of the runtime's
    /// own mob, under an agent name.
    fn routes_answers_to(&self, mob_id: &str, asker: &str) -> bool {
        if mob_id != self.mob_id {
            return false;
        }
        crate::identity_first::AgentIdentity::parse(asker).is_ok()
    }
}

/// The late-bound surface every member's `ask_user` reads at each call;
/// empty until the runtime fills it, and again once it shut down.
#[derive(Clone, Default)]
pub struct AskUserSlot {
    surface: Arc<RwLock<Option<Arc<AskUserSurface>>>>,
}

impl std::fmt::Debug for AskUserSlot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AskUserSlot")
            .field("filled", &self.surface().is_some())
            .finish()
    }
}

impl AskUserSlot {
    /// Fill the slot; it replaces the surface before.
    pub(crate) fn fill(&self, surface: AskUserSurface) {
        *self
            .surface
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(surface));
    }

    /// Empty the slot: every later call answers `no_human_surface`.
    pub(crate) fn clear(&self) {
        *self
            .surface
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    fn surface(&self) -> Option<Arc<AskUserSurface>> {
        self.surface
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The signal each set held through the slot's surface raises, when the
    /// slot is filled.
    #[cfg(test)]
    pub(crate) fn held_signal(&self) -> Option<Arc<tokio::sync::Notify>> {
        self.surface().map(|surface| surface.held_signal())
    }

    /// The durable decisions the question sets are held in, when the slot is
    /// filled.
    pub(crate) fn decisions(&self) -> Option<RuntimeDecisions> {
        self.surface().map(|surface| surface.decisions.clone())
    }

    /// `hook`, then [`Self::mount`]: the pre-build hook of the base session
    /// layer, which every member create and rebuild passes last, after the
    /// hooks of every layer around it.
    pub(crate) fn after_hook(&self, hook: PreBuildHook) -> PreBuildHook {
        let slot = self.clone();
        Arc::new(move |req: &mut CreateSessionRequest| {
            let hook = Arc::clone(&hook);
            let slot = slot.clone();
            Box::pin(async move {
                hook(&mut *req).await?;
                slot.mount(req);
                Ok(())
            })
        })
    }

    /// Compose the `ask_user` tool over the external tools of a member
    /// build, with a host for that member. Nothing changes for a request
    /// that is not a member build, and for a build whose tools already name
    /// `ask_user` (the host's own tool of that name stays reachable).
    pub(crate) fn mount(&self, req: &mut CreateSessionRequest) {
        let Some(build) = req.build.as_mut() else {
            return;
        };
        let Some(binding) = build.mob_member_binding.as_ref() else {
            return;
        };
        if build.external_tools.as_ref().is_some_and(|tools| {
            tools
                .tools()
                .iter()
                .any(|tool| tool.name.as_ref() == ASK_USER_TOOL)
        }) {
            tracing::debug!(
                member = %binding.member,
                "the member's build already has a tool named ask_user: Meta mounts none"
            );
            return;
        }
        let host = MemberAskUserHost::new(
            self.clone(),
            binding.mob_id.clone(),
            crate::member_comms_id::logical_memory_identity(&binding.member),
        );
        let tool: Arc<dyn meerkat_core::AgentToolDispatcher> =
            Arc::new(AskUserDispatcher::new(Arc::new(host)));
        build.external_tools = Some(crate::tool_compose::ComposedExternalTools::over(
            tool,
            build.external_tools.take(),
        ));
    }
}

/// The principal a question set names as its requester: the asking agent,
/// in its mob's own domain, as a change of an agent names an agent
/// requester.
fn asker_requester(mob_id: &str, asker: &str) -> Option<DecisionPrincipalKey> {
    let domain = TrustDomainId::new(format!(
        "{}{mob_id}",
        crate::identity_creation::REQUESTER_TRUST_DOMAIN_PREFIX
    ))
    .ok()?;
    let principal = PrincipalRef::in_domain(PrincipalKind::SharedAgent, asker, domain).ok()?;
    DecisionPrincipalKey::of(&principal).ok()
}

/// Where one member's `ask_user` calls go: see the module documentation.
pub struct MemberAskUserHost {
    slot: AskUserSlot,
    /// The mob the member is a member of.
    mob_id: String,
    /// The member's identity: what a gating origin's `identity` carries, and
    /// the name its notice is sent to.
    asker: String,
}

impl std::fmt::Debug for MemberAskUserHost {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MemberAskUserHost")
            .field("mob_id", &self.mob_id)
            .field("asker", &self.asker)
            .finish_non_exhaustive()
    }
}

impl MemberAskUserHost {
    /// The host of the member `asker` of the mob `mob_id`, reading `slot`
    /// at each call.
    pub(crate) fn new(slot: AskUserSlot, mob_id: String, asker: String) -> Self {
        Self {
            slot,
            mob_id,
            asker,
        }
    }
}

/// The refusal of a hold the decision owner or the kind's code refused.
fn hold_refusal(error: AskUserError) -> AskUserRefusal {
    match error {
        AskUserError::Invalid(reason) => AskUserRefusal::Invalid(reason),
        AskUserError::Decision(DecisionError::Refused {
            refusal: DecisionRefusal::HoldConflict,
            ..
        }) => AskUserRefusal::Invalid(
            "this call asked another set of questions before: ask a new set in a new call"
                .to_owned(),
        ),
        other => AskUserRefusal::Failed(other.to_string()),
    }
}

#[async_trait]
impl AskUserHost for MemberAskUserHost {
    async fn ask(&self, request: AskUserRequest<'_>) -> Result<AskUserResult, AskUserRefusal> {
        let Some(surface) = self.slot.surface() else {
            return Ok(AskUserResult::no_human_surface());
        };
        if !surface.shows_questions() {
            return Ok(AskUserResult::no_human_surface());
        }
        if !surface.routes_answers_to(&self.mob_id, &self.asker) {
            return Ok(AskUserResult::no_human_surface());
        }
        let Some(requester) = asker_requester(&self.mob_id, &self.asker) else {
            return Err(AskUserRefusal::Failed(
                "the asking agent cannot be named as a requester".to_owned(),
            ));
        };
        let now_ms = surface
            .clock
            .now_ms()
            .map_err(|error| AskUserRefusal::Failed(error.to_string()))?;
        let held = hold_ask_user(
            surface.decisions.owner(),
            now_ms,
            AskUserHold {
                asker: self.asker.clone(),
                requester,
                questions: request.questions.clone(),
                timeout: request.timeout,
                run_id: request.run_id.clone(),
                call_id: request.call_id.to_owned(),
            },
        )
        .map_err(hold_refusal)?;
        surface.held.notify_one();
        surface.decisions.notify_work();
        ask_user_result_of(&held).map_err(|error| AskUserRefusal::Failed(error.to_string()))
    }
}

/// Notices of ended question sets, as a keyed send to the asking member's
/// own name through the mob's identity runtime: the key is the decision, so
/// a notice asked for twice is delivered once, and the send starts a turn
/// of an idle agent. A member that is no longer an agent of the mob is a
/// terminal failed receipt; one that cannot take the input now is asked
/// again at a later pass.
pub(crate) struct MemberKeyedNotices {
    mob: MobRuntime,
}

impl MemberKeyedNotices {
    /// Notices sent through the identity runtime `mob` has installed when a
    /// notice is delivered. `mob` is the composition's own handle of its
    /// mob, never its public owner.
    pub(crate) fn new(mob: MobRuntime) -> Self {
        Self { mob }
    }
}

#[async_trait]
impl AskUserNotices for MemberKeyedNotices {
    async fn deliver(&self, notice: &AskUserNotice) -> Option<ApprovalConsentDeliveryState> {
        let failed = |reason: &str| {
            Some(ApprovalConsentDeliveryState::Failed {
                reason: reason.to_owned(),
            })
        };
        let Ok(identity) = crate::identity_first::AgentIdentity::parse(&notice.asker) else {
            return failed("the asker is not an agent name");
        };
        let Ok(text) = ask_user_result_input(&notice.result) else {
            return failed("the result could not be encoded");
        };
        // No identity runtime now: asked again at a later pass.
        let runtime = self.mob.installed_identity_runtime()?;
        let content = meerkat_core::ContentInput::Text(text);
        let key = ask_user_notice_key(notice.decision_id.as_str());
        match runtime
            .send_keyed_tracked(
                &self.mob,
                &identity,
                None,
                &content,
                meerkat_core::types::HandlingMode::Queue,
                &key,
            )
            .await
        {
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
                    failed("the asker was replaced before the notice was admitted")
                }
                meerkat_contracts::WireContinuationStatus::Refused { reason } => failed(&reason),
            },
            Err(crate::identity_first::IdentityRuntimeError::UnknownIdentity(_)) => {
                failed("the asker is no longer an agent of the mob")
            }
            Err(error) => {
                tracing::debug!(%error, "the notice of a question set could not be sent now");
                None
            }
        }
    }
}

/// The driver of the executor of a runtime's `ask_user.questions`
/// decisions, started when it has work: at once when the decision store
/// already holds a set (an earlier process's, which its first pass
/// settles), otherwise at the first set the runtime holds. Until then it
/// claims nothing, creates no file and makes no pass. Dropping it stops the
/// passes.
pub(crate) struct AskUserDriver {
    starter: tokio::task::JoinHandle<()>,
    started: Arc<std::sync::Mutex<Option<DecisionExecutorDriver>>>,
}

impl std::fmt::Debug for AskUserDriver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AskUserDriver")
            .field("passes", &self.passes())
            .finish()
    }
}

impl AskUserDriver {
    /// How many passes the executor's driver has finished; `None` while it
    /// has not started (no set was ever held).
    pub(crate) fn passes(&self) -> Option<u64> {
        self.started
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(DecisionExecutorDriver::passes)
    }

    /// Stop the passes and wait until the driver's task has ended.
    #[cfg(test)]
    pub(crate) async fn shutdown(self) {
        self.starter.abort();
        let driver = self
            .started
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(driver) = driver {
            driver.shutdown().await;
        }
    }
}

impl Drop for AskUserDriver {
    fn drop(&mut self) {
        self.starter.abort();
        // The started driver stops its passes when it is dropped.
        self.started
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }
}

/// Whether `decisions` hold a decision of the kind, open or ended. A count
/// the store cannot give counts as one that does.
fn holds_a_set(decisions: &RuntimeDecisions) -> bool {
    let Ok(kind) = ask_user_questions_kind() else {
        return true;
    };
    decisions
        .owner()
        .count(&DecisionListFilter::default().with_kind(kind))
        .map_or(true, |count| count > 0)
}

/// Drive the executor of `ask_user.questions` decisions for a runtime, from
/// the moment it has work ([`AskUserDriver`]): `held` is signalled by each
/// set the runtime holds. Once started: a pass at start (which settles
/// what an earlier process left), after each change of a decision, and per
/// `tick` while a set waits for its deadline. `None`, starting nothing,
/// for a runtime that keeps no durable decisions, for a candidate launch,
/// and outside an async runtime.
pub(crate) fn spawn_ask_user_driver(
    decisions: &RuntimeDecisions,
    notices: Arc<dyn AskUserNotices>,
    tick: Duration,
    held: Arc<tokio::sync::Notify>,
) -> Option<AskUserDriver> {
    if !decisions.is_available() {
        return None;
    }
    if decisions.is_candidate() {
        return None;
    }
    if tokio::runtime::Handle::try_current().is_err() {
        tracing::warn!(
            "ended question sets are not delivered: their driver was asked for outside an \
             async runtime"
        );
        return None;
    }
    let executor: Arc<dyn DecisionExecutor> = match AskUserExecutor::new(notices) {
        Ok(executor) => Arc::new(executor),
        Err(error) => {
            tracing::warn!(%error, "ended question sets are not delivered: no executor");
            return None;
        }
    };
    let started = Arc::new(std::sync::Mutex::new(None));
    let starter = tokio::spawn({
        let started = Arc::clone(&started);
        let decisions = decisions.clone();
        async move {
            // The executor's claim creates the store's lock files: it waits
            // for the first set, unless an earlier process left one.
            if !holds_a_set(&decisions) {
                held.notified().await;
            }
            match DecisionExecutorDriver::spawn(
                &decisions,
                vec![executor],
                Arc::new(AskUserApproverCurrent),
                tick,
            ) {
                Ok(driver) => {
                    *started
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(driver);
                }
                Err(error) => {
                    tracing::warn!(
                        error = ?error,
                        "ended question sets are not delivered: their driver did not start"
                    );
                }
            }
        }
    });
    Some(AskUserDriver { starter, started })
}

/// Whether `pending_id` has the shape of a decision id: `decision:` and 64
/// lowercase hex digits. A gating pending id never has it.
pub(crate) fn is_decision_id(pending_id: &str) -> bool {
    pending_id.strip_prefix("decision:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

/// One held question set: its decision and the body it holds.
#[derive(Debug, Clone)]
pub(crate) struct HeldQuestionSet {
    row: StoredDecision,
    body: AskUserBody,
}

/// A question set as `meta/gating/questions` lists it.
#[derive(Serialize)]
struct WireQuestionSet<'a> {
    pending_id: &'a str,
    origin: WireOrigin<'a>,
    #[serde(flatten)]
    questions: &'a AskUserQuestionSet,
    created_at_ms: u64,
    deadline_at_ms: u64,
    subject_digest: String,
}

#[derive(Serialize)]
struct WireOrigin<'a> {
    identity: &'a str,
}

impl HeldQuestionSet {
    /// The question set `row` holds. The match on the decision's kind is
    /// closed: only `ask_user.questions` is one, and a body of that kind
    /// this version cannot read is none either.
    fn of(row: StoredDecision) -> Result<Self, QuestionSetRefusal> {
        match row.descriptor.kind.as_str() {
            ASK_USER_QUESTIONS_KIND => {}
            _ => return Err(QuestionSetRefusal::NotAQuestionSet),
        }
        let body = AskUserBody::of(&row).map_err(|_| QuestionSetRefusal::NotAQuestionSet)?;
        Ok(Self { row, body })
    }

    /// The asking member's identity.
    pub(crate) fn asker(&self) -> &str {
        &self.body.asker
    }

    /// The decision that holds the set.
    pub(crate) fn decision_id(&self) -> &DecisionId {
        &self.row.decision_id
    }

    /// Why the set, as this read found it, would not take `verdict` on the
    /// digest `presented`: it is no longer open, it changed since it was
    /// shown, or the answers are not valid for it. A route asks before it
    /// records anything for a request; the hand-off decides again.
    pub(crate) fn refuses(
        &self,
        verdict: &QuestionSetVerdict,
        presented: &Sha256Digest,
    ) -> Option<String> {
        if self.row.state.phase != DecisionPhase::Open {
            return Some(NO_LONGER_OPEN.to_owned());
        }
        if *presented != self.row.descriptor.subject_digest {
            return Some(CHANGED_SINCE_SHOWN.to_owned());
        }
        match verdict {
            QuestionSetVerdict::Answer(answers) => {
                AskUserAnswers::for_set(&self.body.questions, answers.clone())
                    .err()
                    .map(|error| error.to_string())
            }
            QuestionSetVerdict::Dismiss => None,
        }
    }

    /// The set as `meta/gating/questions` lists it.
    pub(crate) fn wire(&self) -> Value {
        serde_json::to_value(WireQuestionSet {
            pending_id: self.row.decision_id.as_str(),
            origin: WireOrigin {
                identity: &self.body.asker,
            },
            questions: &self.body.questions,
            created_at_ms: self.row.descriptor.created_at_ms,
            deadline_at_ms: self.row.descriptor.deadline_at_ms,
            subject_digest: self.row.descriptor.subject_digest.to_string(),
        })
        .unwrap_or(Value::Null)
    }
}

/// The result of `meta/gating/questions` over `sets`.
pub(crate) fn question_sets_result<'a>(
    sets: impl IntoIterator<Item = &'a HeldQuestionSet>,
) -> Value {
    json!({
        "question_sets": sets.into_iter().map(HeldQuestionSet::wire).collect::<Vec<_>>(),
    })
}

/// The open question sets of `decisions`, soonest deadline first. A row
/// this version cannot read is left out, and so is a set whose deadline
/// passed. A runtime that keeps no durable decisions has none.
///
/// # Errors
/// The decision store's read failures.
pub(crate) fn open_question_sets(
    decisions: &RuntimeDecisions,
) -> Result<Vec<HeldQuestionSet>, DecisionError> {
    if !decisions.is_available() {
        return Ok(Vec::new());
    }
    let filter = DecisionListFilter::default()
        .with_kind(ask_user_questions_kind()?)
        .with_phase(DecisionPhase::Open);
    let owner = decisions.owner();
    let mut sets = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let listing = owner.list(&filter, after.as_deref(), DECISION_PAGE_MAX)?;
        for row in listing.decisions {
            // A listing reads each row at its own clock reading: a set past
            // its deadline reads expired, and is no longer open.
            if row.state.phase != DecisionPhase::Open {
                continue;
            }
            if let Ok(set) = HeldQuestionSet::of(row) {
                sets.push(set);
            }
        }
        match listing.next_after {
            Some(next) if after.as_deref() != Some(next.as_str()) => after = Some(next),
            _ => break,
        }
    }
    sets.sort_by(|left, right| {
        left.row
            .descriptor
            .deadline_at_ms
            .cmp(&right.row.descriptor.deadline_at_ms)
            .then_with(|| {
                left.row
                    .decision_id
                    .as_str()
                    .cmp(right.row.decision_id.as_str())
            })
    });
    Ok(sets)
}

/// The question set the pending id `pending_id` names in `decisions`, open
/// or ended; `None` for an id that names none (not a decision id, no such
/// decision, a decision of another kind, a body this version cannot read),
/// which a decide route then answers as any unknown pending id.
pub(crate) fn held_question_set(
    decisions: &RuntimeDecisions,
    pending_id: &str,
) -> Option<HeldQuestionSet> {
    if !is_decision_id(pending_id) || !decisions.is_available() {
        return None;
    }
    let decision_id = DecisionId::new(pending_id.to_owned()).ok()?;
    let row = decisions.owner().get(&decision_id).ok()?;
    HeldQuestionSet::of(row).ok()
}

/// What a person decided of a question set.
#[derive(Debug, Clone)]
pub(crate) enum QuestionSetVerdict {
    /// The person answered: one entry per question they answered.
    Answer(Vec<AskUserQuestionAnswer>),
    /// The person closed the questions without answering.
    Dismiss,
}

/// What a decide route read of its request's `decision`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuestionSetChoice {
    /// `approve`: the request carries the answers.
    Answer,
    /// `reject` or `deny`: a dismissal.
    Dismiss,
}

/// The verdict and the presented digest a `meta/gating/decide` request
/// carries for a question set.
///
/// # Errors
/// The sentence that says what the request lacks or gets wrong, for an
/// invalid params answer: a missing or malformed `presented_digest`, an
/// answer without `answers`, `answers` that do not parse, and a dismissal
/// that carries answers.
pub(crate) fn question_set_request(
    choice: QuestionSetChoice,
    params: &Value,
) -> Result<(QuestionSetVerdict, Sha256Digest), String> {
    let Some(digest) = params.get("presented_digest").and_then(Value::as_str) else {
        return Err(
            "presented_digest required: the subject_digest of the question set as it was shown"
                .to_owned(),
        );
    };
    let digest =
        Sha256Digest::new(digest).map_err(|error| format!("invalid presented_digest: {error}"))?;
    let answers = params.get("answers").filter(|answers| !answers.is_null());
    let verdict = match (choice, answers) {
        (QuestionSetChoice::Answer, Some(answers)) => QuestionSetVerdict::Answer(
            serde_json::from_value(answers.clone())
                .map_err(|error| format!("invalid answers: {error}"))?,
        ),
        (QuestionSetChoice::Answer, None) => {
            return Err(
                "answers required: an approving decision of a question set carries its answers"
                    .to_owned(),
            );
        }
        (QuestionSetChoice::Dismiss, None) => QuestionSetVerdict::Dismiss,
        (QuestionSetChoice::Dismiss, Some(_)) => {
            return Err("a dismissal carries no answers".to_owned());
        }
    };
    Ok((verdict, digest))
}

/// The authenticated event a person's decision of a question set arrived
/// as: who decided, on which ingress and channel, and when. A route builds
/// it only after it authenticated the caller; nothing of it is read from
/// the request.
#[derive(Debug, Clone)]
pub(crate) struct QuestionSetEvent {
    /// The person who decided, as the route authenticated them.
    pub(crate) approver: AuthenticatedApprover,
    /// The ingress and channel the decision arrived on: [`CONSOLE_CHANNEL`]
    /// or [`HOST_CHANNEL`].
    pub(crate) channel: &'static str,
    /// The reference of this one decide event.
    pub(crate) event_ref: DecisionReference,
    /// When the route observed the event, in UNIX milliseconds.
    pub(crate) observed_at_ms: u64,
    /// What the evidence keeps as its reason, when the route has something
    /// to record there.
    pub(crate) reason: Option<String>,
}

/// One person's decision of one question set, as its route hands it over.
#[derive(Debug, Clone)]
pub(crate) struct QuestionSetDecide {
    pub(crate) decision_id: DecisionId,
    pub(crate) verdict: QuestionSetVerdict,
    /// The digest of the set as the person was shown it.
    pub(crate) presented_digest: Sha256Digest,
    pub(crate) attested: QuestionSetEvent,
}

/// How a decided question set ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuestionSetOutcome {
    Answered,
    Dismissed,
}

impl QuestionSetOutcome {
    /// The outcome's token in a `meta/gating/decide` result.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Answered => "answered",
            Self::Dismissed => "dismissed",
        }
    }
}

/// A question set a person decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QuestionSetDecided {
    pub(crate) pending_id: String,
    pub(crate) outcome: QuestionSetOutcome,
    pub(crate) decided_at_ms: u64,
}

impl QuestionSetDecided {
    /// The `meta/gating/decide` result of a decided question set.
    pub(crate) fn wire(&self) -> Value {
        json!({
            "pending_id": self.pending_id,
            "kind": QUESTION_SET_RESULT_KIND,
            "outcome": self.outcome.as_str(),
            "decided_at_ms": self.decided_at_ms,
        })
    }
}

/// Why a decision of a question set was not taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum QuestionSetRefusal {
    /// The id names no question set: a route answers it as it answers an
    /// unknown pending id, and the decision it names, if any, is untouched.
    NotAQuestionSet,
    /// The request is at fault, or the set no longer takes a decision: an
    /// invalid params answer with this sentence.
    Invalid(String),
    /// The decider was not eligible when the decision committed: the route
    /// that built the eligibility says why.
    NotEligible,
    /// The decision owner failed: nothing is known to be decided.
    Failed(String),
}

/// Hand one person's decision of a question set to the decision owner.
///
/// The match on the decision's kind is closed: only a decision of kind
/// `ask_user.questions` proceeds, and every other kind is
/// [`QuestionSetRefusal::NotAQuestionSet`] with the decision untouched. For
/// a question set, the answers are validated against the held set, carried
/// as typed data in the human evidence, and the decision is decided for the
/// set's approver id with a presentation that shows every canonical part of
/// its rendering at the event's observed time. `eligibility` is checked by
/// the owner when the decision commits.
///
/// # Errors
/// [`QuestionSetRefusal`].
pub(crate) fn answer_question_set(
    decisions: &RuntimeDecisions,
    decide: QuestionSetDecide,
    eligibility: &dyn ConsentEligibility,
) -> Result<QuestionSetDecided, QuestionSetRefusal> {
    let invalid = |error: &dyn std::fmt::Display| QuestionSetRefusal::Invalid(error.to_string());
    let owner = decisions.owner();
    let row = match owner.get(&decide.decision_id) {
        Ok(row) => row,
        Err(DecisionError::NotFound(_) | DecisionError::Unreadable { .. }) => {
            return Err(QuestionSetRefusal::NotAQuestionSet);
        }
        Err(error) => return Err(QuestionSetRefusal::Failed(error.to_string())),
    };
    let set = HeldQuestionSet::of(row)?;
    let answers = match decide.verdict {
        QuestionSetVerdict::Answer(answers) => Some(
            AskUserAnswers::for_set(&set.body.questions, answers)
                .map_err(|error| invalid(&error))?,
        ),
        QuestionSetVerdict::Dismiss => None,
    };
    let QuestionSetEvent {
        approver,
        channel,
        event_ref,
        observed_at_ms,
        reason,
    } = decide.attested;
    let descriptor = &set.row.descriptor;
    let presentation = PresentationRecord::complete(
        PresentationAudience::Approver {
            approver_id: descriptor.approver_id.clone(),
        },
        &descriptor.rendering,
        None,
        DecisionReference::new(channel).map_err(|error| invalid(&error))?,
        event_ref.clone(),
        observed_at_ms,
    );
    let mut decision = AttestedDecision::new(
        approver,
        DecisionIngress::new(channel).map_err(|error| invalid(&error))?,
        event_ref,
        observed_at_ms,
        descriptor.approver_id.clone(),
        decide.presented_digest,
        answers.is_some(),
    )
    .map_err(|error| invalid(&error))?
    .with_presentation(presentation);
    if let Some(reason) = reason {
        decision = decision
            .with_reason(reason)
            .map_err(|error| invalid(&error))?;
    }
    if let Some(answers) = &answers {
        decision = attest_ask_user_answers(decision, answers).map_err(|error| invalid(&error))?;
    }
    match owner.decide(&decide.decision_id, &decision, eligibility) {
        Ok(decided) => {
            decisions.notify_work();
            Ok(QuestionSetDecided {
                pending_id: decided.decision_id.to_string(),
                outcome: if answers.is_some() {
                    QuestionSetOutcome::Answered
                } else {
                    QuestionSetOutcome::Dismissed
                },
                decided_at_ms: decided.state.decided_at_ms.unwrap_or(observed_at_ms),
            })
        }
        Err(DecisionError::Refused { refusal, .. }) => Err(match refusal {
            DecisionRefusal::NotEligible => QuestionSetRefusal::NotEligible,
            DecisionRefusal::SubjectChanged => {
                QuestionSetRefusal::Invalid(CHANGED_SINCE_SHOWN.to_owned())
            }
            DecisionRefusal::NotOpen | DecisionRefusal::Expired => {
                QuestionSetRefusal::Invalid(NO_LONGER_OPEN.to_owned())
            }
            _ => QuestionSetRefusal::Invalid("the question set did not take the answer".to_owned()),
        }),
        Err(error) => Err(QuestionSetRefusal::Failed(error.to_string())),
    }
}
