//! Approval-carried tool grants on this runtime: the executor of the
//! `tool_grant.issue` decisions of the persistence's decision store, where
//! the notices of their requests go, and who a host's request names as its
//! requester.
//!
//! [`SessionRuntime::start_tool_grant_request_executor`] starts one executor
//! loop for the kind on a runtime whose persistence keeps durable decisions;
//! the router starts it when it is constructed. The loop holds the kind's
//! executor claim of the store while it runs (another process's loop of the
//! kind then executes nothing), and it passes over the store once at start,
//! whenever a decision of the kind is recorded here
//! ([`SessionRuntime::nudge_tool_grant_request_executor`]), and every
//! [`TOOL_GRANT_EXECUTOR_INTERVAL`] for the deadlines. A pass issues approved
//! grants (grant rows only: an issued grant wakes nothing) and delivers each
//! ended request's notice to its subject: a session's as an input, like any
//! input, and a member identity's through the mob's member input. The
//! runtime's shutdown stops the loop before its registrations are torn down.
//! While the loop runs, the runtime takes its agents' grant requests
//! (`request_tool_grant`), naming the realm's agent `rkat-agent` as their
//! requester.
//!
//! The party that decided is current while the realm's configuration still
//! lists the decision's approver id for the operator
//! (`[governance.decisions] operator_approver_ids`), as for a decide on a
//! stdio connection: an id dropped from the list ends the approved decision
//! refused, and a configuration that cannot be read decides nothing.

use std::sync::{Arc, Mutex as StdMutex, PoisonError, RwLock as StdRwLock, Weak};
use std::time::Duration;

use meerkat_core::approval::ApprovalConsentDeliveryState;
use meerkat_core::approval::consent::{
    ConsentEligibility, ConsentEligibilityAnswer, ConsentIneligible,
};
use meerkat_core::approval::decision::{DecisionPrincipalKey, StoredDecision};
use meerkat_core::tool_grant::ToolGrantSubject;
use meerkat_core::types::SessionId;
use meerkat_core::{DecisionGovernanceConfig, PrincipalKind, PrincipalRef, TrustDomainId};
use meerkat_runtime::decision::{
    DecisionApproverCurrentness, DecisionError, DecisionExecutor, DecisionExecutorLoop,
    DecisionExecutorLoopError, DecisionPassOutcome,
};
use meerkat_runtime::tool_grant::wire::wire_tool_grant_target;
use meerkat_runtime::tool_grant::{
    TOOL_GRANT_ISSUE_KIND, ToolGrantRequestHost, ToolGrantRequestNotice, ToolGrantRequestNotices,
    ToolGrantRequestOutcome, ToolGrantRequestPolicy, ToolGrantSubjectObserver,
    ToolGrantSubjectPresence,
};
use tokio::sync::{Notify, oneshot};

use super::{ExternalEventRuntimeContext, SessionRuntime};
use crate::error;

/// How long the executor waits between passes when nothing nudges it: a
/// request's deadline is observed within this bound.
pub(crate) const TOOL_GRANT_EXECUTOR_INTERVAL: Duration = Duration::from_secs(30);

/// How long the runtime's shutdown waits for a pass in flight to end. A pass
/// may be dropped at any await: the next one settles what it left.
const TOOL_GRANT_EXECUTOR_STOP_GRACE: Duration = Duration::from_secs(10);

/// The event type of a request notice input.
const TOOL_GRANT_REQUEST_EVENT: &str = "tool_grant_request";

/// The executor of this runtime's `tool_grant.issue` decisions: its nudge,
/// and its task while it runs.
#[derive(Default)]
pub(crate) struct ToolGrantExecutorSlot {
    nudge: Arc<Notify>,
    running: StdMutex<Option<RunningExecutor>>,
}

/// A started executor loop: its stop signal and its task.
struct RunningExecutor {
    stop: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
}

/// The approver ids the realm's configuration lists for the operator, as
/// last read before a pass: `None` when it could not be read.
struct ListedOperatorApprovers(StdRwLock<Option<DecisionGovernanceConfig>>);

/// Evidence that an approver id is still listed, as of the pass's reading.
struct StillListed;

impl ConsentEligibility for StillListed {
    fn check_current(&self) -> Result<(), ConsentIneligible> {
        Ok(())
    }
}

impl DecisionApproverCurrentness for ListedOperatorApprovers {
    fn approver_answer(&self, decision: &StoredDecision) -> ConsentEligibilityAnswer {
        match &*self.0.read().unwrap_or_else(PoisonError::into_inner) {
            Some(listed) if listed.lists(decision.descriptor.approver_id.as_str()) => {
                ConsentEligibilityAnswer::Eligible(Arc::new(StillListed))
            }
            Some(_) => ConsentEligibilityAnswer::Refused,
            None => ConsentEligibilityAnswer::Unsettled,
        }
    }
}

/// The grant subjects this runtime serves: its sessions, and the member
/// identities of the mobs it runs.
struct RuntimeSubjects(Weak<SessionRuntime>);

#[async_trait::async_trait]
impl ToolGrantSubjectObserver for RuntimeSubjects {
    async fn presence(&self, subject: &ToolGrantSubject) -> ToolGrantSubjectPresence {
        match self.0.upgrade() {
            Some(runtime) => runtime.tool_grant_subject_presence(subject).await,
            None => ToolGrantSubjectPresence::Unknown,
        }
    }
}

/// Why a member identity subject's mob is not at hand.
#[cfg(feature = "mob")]
enum MobAbsence {
    /// No mob runs under the id, or another incarnation of it does: the
    /// identity ended.
    Ended,
    /// The mob could not be read now.
    Unknown,
}

/// The request notices of this runtime's sessions.
struct RuntimeNotices(Weak<SessionRuntime>);

#[async_trait::async_trait]
impl ToolGrantRequestNotices for RuntimeNotices {
    async fn deliver(
        &self,
        notice: &ToolGrantRequestNotice,
    ) -> Option<ApprovalConsentDeliveryState> {
        let runtime = self.0.upgrade()?;
        runtime.deliver_tool_grant_request_notice(notice).await
    }
}

/// The key a request's notice is delivered under: one delivery per decision.
fn notice_key(notice: &ToolGrantRequestNotice) -> String {
    format!("{TOOL_GRANT_ISSUE_KIND}:{}", notice.decision_id)
}

/// The payload of a request notice input: the decision, how the request
/// ended, its target, and the issued grant's id or the refusal's reason.
fn notice_payload(notice: &ToolGrantRequestNotice) -> serde_json::Value {
    let mut payload = serde_json::json!({
        "decision_id": notice.decision_id.to_string(),
        "outcome": notice.outcome.as_str(),
        "target": wire_tool_grant_target(&notice.target),
    });
    match &notice.outcome {
        ToolGrantRequestOutcome::Granted { grant_id } => {
            payload["grant_id"] = serde_json::Value::String(grant_id.to_string());
        }
        ToolGrantRequestOutcome::Refused { reason } => {
            payload["reason"] = serde_json::Value::String(reason.clone());
        }
        _ => {}
    }
    payload
}

/// The executor loop: a pass at start, then one per nudge or interval, until
/// the runtime stops it or is gone.
async fn run_tool_grant_executor(
    runtime: Weak<SessionRuntime>,
    mut executor_loop: DecisionExecutorLoop,
    approvers: Arc<ListedOperatorApprovers>,
    nudge: Arc<Notify>,
    mut stop: oneshot::Receiver<()>,
) {
    loop {
        {
            let Some(runtime) = runtime.upgrade() else {
                return;
            };
            let listed = runtime.effective_config().await.ok().map(|config| {
                config
                    .governance
                    .map(|governance| governance.decisions)
                    .unwrap_or_default()
            });
            *approvers.0.write().unwrap_or_else(PoisonError::into_inner) = listed;
        }
        let pass = executor_loop.pass().await;
        for (decision_id, outcome) in &pass.outcomes {
            if let DecisionPassOutcome::Failed(error) = outcome {
                tracing::warn!(
                    %decision_id,
                    %error,
                    "a tool grant request's decision could not be executed now"
                );
            }
        }
        for error in &pass.read_failures {
            tracing::warn!(%error, "the tool grant request executor could not read its decisions");
        }
        tokio::select! {
            _ = &mut stop => return,
            () = nudge.notified() => {}
            () = tokio::time::sleep(TOOL_GRANT_EXECUTOR_INTERVAL) => {}
        }
    }
}

impl SessionRuntime {
    /// The principal `id` of `kind` in the realm's stock trust domain
    /// (`rkat-realm-<realm>`), as a decision names it.
    fn realm_principal_key(
        &self,
        kind: PrincipalKind,
        id: &str,
    ) -> Result<DecisionPrincipalKey, String> {
        let realm = self
            .realm_id()
            .ok_or_else(|| "tool grant requests need the runtime's realm".to_owned())?;
        let domain = TrustDomainId::new(format!("rkat-realm-{}", realm.as_str()))
            .map_err(|error| error.to_string())?;
        let principal =
            PrincipalRef::in_domain(kind, id, domain).map_err(|error| error.to_string())?;
        DecisionPrincipalKey::of(&principal).map_err(|error| error.to_string())
    }

    /// The principal a host's grant request on this runtime names as its
    /// requester: the runtime host `rkat-rpc` in the realm's stock trust
    /// domain.
    ///
    /// # Errors
    /// A runtime without a realm, or a realm id no principal can name.
    pub(crate) fn tool_grant_host_requester(&self) -> Result<DecisionPrincipalKey, String> {
        self.realm_principal_key(PrincipalKind::RuntimeHost, "rkat-rpc")
    }

    /// The principal an agent's grant request on this runtime names as its
    /// requester: the realm's agent executor `rkat-agent`, as the stock
    /// composition names it.
    ///
    /// # Errors
    /// A runtime without a realm, or a realm id no principal can name.
    fn tool_grant_agent_requester(&self) -> Result<DecisionPrincipalKey, String> {
        self.realm_principal_key(PrincipalKind::PersonalAgent, "rkat-agent")
    }

    /// Start the executor of the `tool_grant.issue` decisions of this
    /// runtime's decision store; see the module documentation. Idempotent: a
    /// running executor is kept. A runtime whose persistence keeps no
    /// durable decisions, a candidate launch, and a runtime whose shutdown
    /// began start nothing.
    ///
    /// # Errors
    /// A store that cannot serve an executor claim.
    pub fn start_tool_grant_request_executor(self: &Arc<Self>) -> Result<(), String> {
        let decisions = self.decision_service();
        if !decisions.is_available() {
            return Ok(());
        }
        let mut running = self
            .tool_grant_executor
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        // A pass admits notices into sessions: a runtime that is stopping
        // starts none. Read under the slot, so a start that waited on the
        // shutdown's stop does not start another loop after it.
        if self.runtime_registration_fence.is_closing() {
            return Ok(());
        }
        if running
            .as_ref()
            .is_some_and(|executor| !executor.task.is_finished())
        {
            return Ok(());
        }
        let weak = Arc::downgrade(self);
        let executor = self
            .runtime_adapter()
            .tool_grant_issue_executor(
                Arc::new(RuntimeSubjects(weak.clone())),
                Arc::new(RuntimeNotices(weak.clone())),
            )
            .map_err(|error| error.to_string())?;
        let executor: Arc<dyn DecisionExecutor> = Arc::new(executor);
        let approvers = Arc::new(ListedOperatorApprovers(StdRwLock::new(None)));
        let executor_loop = match DecisionExecutorLoop::start(
            decisions,
            [executor],
            Arc::clone(&approvers) as Arc<dyn DecisionApproverCurrentness>,
        ) {
            Ok(executor_loop) => executor_loop,
            // A candidate launch acts nowhere: it executes no decision.
            Err(DecisionExecutorLoopError::Decision(DecisionError::CandidateLaunch)) => {
                return Ok(());
            }
            Err(error) => return Err(error.to_string()),
        };
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(run_tool_grant_executor(
            weak,
            executor_loop,
            approvers,
            Arc::clone(&self.tool_grant_executor.nudge),
            stopped,
        ));
        *running = Some(RunningExecutor { stop, task });
        drop(running);
        // While the executor runs, the runtime takes its agents' requests,
        // naming the realm's agent as their requester.
        match self.tool_grant_agent_requester() {
            Ok(requester) => {
                self.runtime_adapter()
                    .set_tool_grant_request_host(Some(ToolGrantRequestHost {
                        decisions: self.decision_service(),
                        policy: ToolGrantRequestPolicy::default(),
                        requester,
                    }));
            }
            Err(error) => {
                tracing::warn!(%error, "the runtime's agents cannot be named; they take no grant requests");
            }
        }
        Ok(())
    }

    /// Ask the tool grant request executor for a pass now: a decision of its
    /// kind was recorded here.
    pub(crate) fn nudge_tool_grant_request_executor(&self) {
        self.tool_grant_executor.nudge.notify_one();
    }

    /// Nudge the executor when `decision` is a tool grant request.
    pub(crate) fn nudge_tool_grant_request_executor_for(&self, decision: &StoredDecision) {
        if decision.descriptor.kind.as_str() == TOOL_GRANT_ISSUE_KIND {
            self.nudge_tool_grant_request_executor();
        }
    }

    /// Stop the executor and wait, within its bound, for a pass in flight
    /// to end; a pass that does not end within it is dropped.
    pub(super) async fn shutdown_tool_grant_request_executor(&self) {
        let running = self
            .tool_grant_executor
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let Some(RunningExecutor { stop, mut task }) = running else {
            return;
        };
        self.runtime_adapter().set_tool_grant_request_host(None);
        let _ = stop.send(());
        if tokio::time::timeout(TOOL_GRANT_EXECUTOR_STOP_GRACE, &mut task)
            .await
            .is_err()
        {
            task.abort();
            tracing::warn!(
                "the tool grant request executor did not stop within its bound; its pass was dropped"
            );
        }
    }

    /// What this runtime sees of a grant subject now: a session is held,
    /// running its current run here; a member identity is held while its mob
    /// runs the incarnation the subject names and its roster holds the
    /// identity, running the current run of the session its member holds.
    async fn tool_grant_subject_presence(
        &self,
        subject: &ToolGrantSubject,
    ) -> ToolGrantSubjectPresence {
        match subject {
            ToolGrantSubject::Session { session_id } => ToolGrantSubjectPresence::Held {
                current_run: self.runtime_adapter().current_run(session_id).await,
            },
            ToolGrantSubject::Identity {
                mob_id,
                mob_incarnation,
                identity,
            } => {
                self.member_presence(mob_id, mob_incarnation, identity)
                    .await
            }
        }
    }

    /// The mob `mob_id` while it runs the incarnation `mob_incarnation`.
    #[cfg(feature = "mob")]
    async fn subject_mob(
        &self,
        mob_id: &str,
        mob_incarnation: &str,
    ) -> Result<meerkat_mob::MobHandle, MobAbsence> {
        let Some(state) = self.mob_state() else {
            return Err(MobAbsence::Unknown);
        };
        match state.handle_for(&meerkat_mob::MobId::from(mob_id)).await {
            Ok(handle)
                if handle.live_incarnation().incarnation().to_string() == mob_incarnation =>
            {
                Ok(handle)
            }
            Ok(_) | Err(meerkat_mob::MobError::MobNotFound(_)) => Err(MobAbsence::Ended),
            Err(_) => Err(MobAbsence::Unknown),
        }
    }

    /// What this runtime sees of a member identity now.
    #[cfg(feature = "mob")]
    async fn member_presence(
        &self,
        mob_id: &str,
        mob_incarnation: &str,
        identity: &str,
    ) -> ToolGrantSubjectPresence {
        let handle = match self.subject_mob(mob_id, mob_incarnation).await {
            Ok(handle) => handle,
            Err(MobAbsence::Ended) => return ToolGrantSubjectPresence::Ended,
            Err(MobAbsence::Unknown) => return ToolGrantSubjectPresence::Unknown,
        };
        let member = meerkat_mob::AgentIdentity::from(identity);
        match handle.member(&member).await {
            Ok(_) => {}
            Err(meerkat_mob::MobError::MemberNotFound(_)) => {
                return ToolGrantSubjectPresence::Ended;
            }
            Err(_) => return ToolGrantSubjectPresence::Unknown,
        }
        let current_run = match handle.resolve_bridge_session_id(&member).await {
            Some(session_id) => self.runtime_adapter().current_run(&session_id).await,
            None => None,
        };
        ToolGrantSubjectPresence::Held { current_run }
    }

    /// A server without mobs cannot tell what a member identity is: a
    /// decision about one waits for a host that can.
    #[cfg(not(feature = "mob"))]
    async fn member_presence(
        &self,
        _mob_id: &str,
        _mob_incarnation: &str,
        _identity: &str,
    ) -> ToolGrantSubjectPresence {
        ToolGrantSubjectPresence::Unknown
    }

    /// Deliver a request notice to its subject, once per decision: a
    /// session's as an input of the session, a member identity's through the
    /// mob's member input.
    async fn deliver_tool_grant_request_notice(
        self: &Arc<Self>,
        notice: &ToolGrantRequestNotice,
    ) -> Option<ApprovalConsentDeliveryState> {
        match &notice.subject {
            ToolGrantSubject::Session { session_id } => {
                self.deliver_session_tool_grant_request_notice(session_id, notice)
                    .await
            }
            ToolGrantSubject::Identity {
                mob_id,
                mob_incarnation,
                identity,
            } => {
                self.deliver_member_tool_grant_request_notice(
                    notice,
                    mob_id,
                    mob_incarnation,
                    identity,
                )
                .await
            }
        }
    }

    /// Deliver a member identity's request notice through the mob's member
    /// input for the identity: a keyed member send under the notice's key,
    /// so a repeated delivery answers the first, admitted into the session
    /// the identity's member holds when it is delivered (never one it held
    /// before). Its text is the notice's event type and fields as JSON. An
    /// identity whose mob is gone or was replaced, or that has no member
    /// left, fails the notice; a send committed and not admitted yet, a
    /// member another host serves, or a mob that cannot be read now leaves
    /// it to a later pass.
    #[cfg(feature = "mob")]
    async fn deliver_member_tool_grant_request_notice(
        &self,
        notice: &ToolGrantRequestNotice,
        mob_id: &str,
        mob_incarnation: &str,
        identity: &str,
    ) -> Option<ApprovalConsentDeliveryState> {
        use meerkat_contracts::WireContinuationStatus;
        use meerkat_mob::{MemberSendIdempotencyRefusal, MobError};

        let no_member = || {
            Some(ApprovalConsentDeliveryState::Failed {
                reason: format!("the identity {identity} of mob {mob_id} has no member left"),
            })
        };
        let handle = match self.subject_mob(mob_id, mob_incarnation).await {
            Ok(handle) => handle,
            Err(MobAbsence::Ended) => return no_member(),
            Err(MobAbsence::Unknown) => return None,
        };
        let state = self.mob_state()?;
        let member = meerkat_mob::AgentIdentity::from(identity);
        let content = meerkat_core::types::ContentInput::Text(format!(
            "{TOOL_GRANT_REQUEST_EVENT}: {}",
            notice_payload(notice)
        ));
        let sent = state
            .mob_member_send_keyed(
                &meerkat_mob::MobId::from(mob_id),
                member.clone(),
                content,
                meerkat_core::types::HandlingMode::Queue,
                &notice_key(notice),
            )
            .await;
        match sent {
            Ok(receipt) => match receipt.status {
                WireContinuationStatus::Admitted {
                    session_id,
                    input_id,
                } => SessionId::parse(&session_id).ok().map(|session_id| {
                    ApprovalConsentDeliveryState::Admitted {
                        session_id,
                        input_id,
                    }
                }),
                WireContinuationStatus::Pending => None,
                // The mob incarnation the send was bound to was replaced.
                WireContinuationStatus::Stranded => no_member(),
                WireContinuationStatus::Refused { reason } => {
                    Some(ApprovalConsentDeliveryState::Failed { reason })
                }
            },
            Err(MobError::MemberNotFound(_) | MobError::MobNotFound(_)) => no_member(),
            // Only a first commit captures the member's delivery scope, so no
            // earlier send of the notice was committed: a member that left
            // the roster has no session binding left to scope it to, and one
            // in the roster without a binding here is served elsewhere.
            Err(MobError::DeliveryScopeUnavailable { .. }) => match handle.member(&member).await {
                Err(MobError::MemberNotFound(_)) => no_member(),
                _ => None,
            },
            Err(MobError::MemberSendIdempotency {
                refusal:
                    MemberSendIdempotencyRefusal::Unavailable { .. }
                    | MemberSendIdempotencyRefusal::Inconsistent { .. },
                ..
            }) => None,
            Err(error @ MobError::MemberSendIdempotency { .. }) => {
                Some(ApprovalConsentDeliveryState::Failed {
                    reason: error.to_string(),
                })
            }
            Err(_) => None,
        }
    }

    /// A server without mobs cannot reach a member: the notice stays owed
    /// for a host that can.
    #[cfg(not(feature = "mob"))]
    async fn deliver_member_tool_grant_request_notice(
        &self,
        _notice: &ToolGrantRequestNotice,
        _mob_id: &str,
        _mob_incarnation: &str,
        _identity: &str,
    ) -> Option<ApprovalConsentDeliveryState> {
        None
    }

    /// Deliver a session's request notice to it as an input, like any input
    /// (a dormant session is loaded for it): a repeated delivery answers the
    /// input admitted first. A session that no longer exists fails the
    /// notice; any other refusal (a session another process hosts, a runtime
    /// not accepting input now) leaves it to a later pass.
    async fn deliver_session_tool_grant_request_notice(
        self: &Arc<Self>,
        session_id: &SessionId,
        notice: &ToolGrantRequestNotice,
    ) -> Option<ApprovalConsentDeliveryState> {
        let context = ExternalEventRuntimeContext {
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            idempotency_key: Some(meerkat_runtime::IdempotencyKey::new(notice_key(notice))),
            correlation_id: None,
        };
        match self
            .accept_external_event_via_runtime_with_context(
                session_id,
                TOOL_GRANT_REQUEST_EVENT.to_owned(),
                notice_payload(notice),
                None,
                context,
            )
            .await
        {
            Ok(meerkat_runtime::AcceptOutcome::Accepted { input_id, .. }) => {
                Some(ApprovalConsentDeliveryState::Admitted {
                    session_id: session_id.clone(),
                    input_id: input_id.to_string(),
                })
            }
            Ok(meerkat_runtime::AcceptOutcome::Deduplicated { existing_id, .. }) => {
                Some(ApprovalConsentDeliveryState::Admitted {
                    session_id: session_id.clone(),
                    input_id: existing_id.to_string(),
                })
            }
            Err(refusal) if refusal.code == error::SESSION_NOT_FOUND => {
                Some(ApprovalConsentDeliveryState::Failed {
                    reason: format!("the session {session_id} no longer exists"),
                })
            }
            Ok(_) | Err(_) => None,
        }
    }
}
