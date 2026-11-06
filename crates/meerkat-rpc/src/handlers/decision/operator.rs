//! The operator of an `rkat-rpc` stdio connection decides durable decisions.
//!
//! A stdio connection of a Unix build with local authorization has an
//! operator: the process's operating-system user (on a governed pipe, the
//! composition's operator), device trusted, in an authentication scope of the
//! connection's own. The connection presents decisions to it only when its
//! client lists them (`decision/list`, the client's explicit ask): each
//! decision of the returned page that is open, waits for a human, is held for
//! an approver id the realm's configuration lists for the operator
//! (`[governance.decisions] operator_approver_ids`) and was not presented on
//! this connection yet, in page order. For each, the connection reads the
//! clock, writes a `decision/notice` carrying the whole rendering, waits
//! until the notice is on the pipe, and only then records the presentation
//! through the decision owner: to the approver, on the connection's own
//! channel, under the notice's id, at that clock reading. A crash or a closed
//! pipe between the notice and the record leaves a shown but unrecorded
//! presentation, which a decide refuses; never the reverse. The answered
//! page carries each presented decision as recorded. When the realm's
//! configuration cannot be read, the list presents nothing and answers its
//! page as plain dispatch does.
//!
//! The operator decides through `decision/decide` only a decision this
//! connection presented, while the connection is open and the decision's
//! approver id is still listed when the decide arrives. The connection builds
//! the authenticated event itself (the request names only the decision, the
//! answer, the presented digest and a reason) and decides through the same
//! owner path as any host adapter. Every refusal is the decision's own
//! closed vocabulary: `not_eligible`.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use meerkat::governed_realm::stock::StockPrincipals;
use meerkat_contracts::{
    DECISION_NOTICE_NOTIFICATION, DecisionDecideParams, WireDecisionNotice,
    WireDecisionNoticeEvent, WireDecisionNoticePresentation,
};
use meerkat_core::DecisionGovernanceConfig;
use meerkat_core::approval::ApprovalAssurance;
use meerkat_core::approval::consent::{
    AuthenticatedApprover, ConsentEligibility, ConsentIneligible, HostAuthenticationScope,
};
use meerkat_core::approval::decision::{
    AttestedDecision, AuthenticatedDecisionEvent, DecisionContractError, DecisionId,
    DecisionIngress, DecisionPhase, DecisionReference, DecisionStage, PresentationAudience,
    PresentationRecord, Sha256Digest, StoredDecision, canonical_parts,
};
use meerkat_core::connection::RealmId;
use meerkat_runtime::decision::wire::{
    wire_canonical_part, wire_decision_listing, wire_decision_phase,
};
use meerkat_runtime::decision::{DecisionClock, DurableDecisionService, SystemDecisionClock};
use serde_json::value::RawValue;
use tokio::sync::{Mutex, mpsc, oneshot};

use super::{
    decision_error_to_response, decision_owner, handle_decide_event, invalid, parse_decision_id,
    read_list_page,
};
use crate::error;
use crate::handlers::{RpcResponseExt, parse_params};
use crate::protocol::{RpcError, RpcId, RpcNotification, RpcResponse};
use crate::server::AcknowledgedNotification;
use crate::session_runtime::SessionRuntime;

/// How a decision the operator decides on a stdio connection reached the
/// host.
const DECISION_INGRESS: &str = "rkat-rpc-stdio";

/// The prefix of the channel and event references this connection names.
const REFERENCE_PREFIX: &str = "rkat-rpc";

/// One connection's operator for durable decisions; see the module
/// documentation.
pub(crate) struct DecisionOperator {
    /// The operator, device trusted, in this connection's own scope.
    approver: AuthenticatedApprover,
    /// Where this connection's presentations are shown.
    channel_ref: DecisionReference,
    /// Cleared when the connection ends: no later decide on it is eligible.
    open: Arc<AtomicBool>,
    /// The connection's notices whose sending waits until they are written.
    notices: mpsc::Sender<AcknowledgedNotification>,
    /// The decisions this connection presented to its operator. Its lock is
    /// the presentation pass: a decide waits for a pass in flight.
    presented: Mutex<HashSet<DecisionId>>,
}

/// What presenting one decision came to.
enum Presented {
    /// Shown and recorded: the decision as the owner recorded it.
    Recorded(Box<StoredDecision>),
    /// Not recorded; the pass goes on with the next decision.
    Skipped,
    /// The connection cannot show anything more; the pass ends.
    Stopped,
}

impl DecisionOperator {
    /// The operator of a plain stdio connection on `realm`: this process's
    /// operating-system user, in a scope of the connection's own. `None`
    /// when the user cannot be named as an approver.
    pub(crate) fn for_local_operator(
        realm: &RealmId,
        notices: mpsc::Sender<AcknowledgedNotification>,
    ) -> Option<Self> {
        let operator = match StockPrincipals::for_local_operator(realm) {
            Ok(principals) => principals.operator,
            Err(error) => {
                tracing::warn!(%error, "the connection's operator cannot be named; it decides nothing");
                return None;
            }
        };
        let scope = HostAuthenticationScope::new(format!(
            "stdio:{}:{}",
            operator.id.as_str(),
            uuid::Uuid::new_v4()
        ));
        match AuthenticatedApprover::new(operator, ApprovalAssurance::DeviceTrusted) {
            Ok(approver) => Self::new(approver.with_authentication_scope(scope), notices),
            Err(error) => {
                tracing::warn!(%error, "the connection's operator is not an approver; it decides nothing");
                None
            }
        }
    }

    /// The operator `approver`, authenticated in its connection's own scope,
    /// presenting through `notices`. `None` for an approver without a scope.
    pub(crate) fn new(
        approver: AuthenticatedApprover,
        notices: mpsc::Sender<AcknowledgedNotification>,
    ) -> Option<Self> {
        let scope = approver.authentication_scope()?;
        let channel_ref =
            DecisionReference::new(format!("{REFERENCE_PREFIX}:{}", scope.as_str())).ok()?;
        Some(Self {
            approver,
            channel_ref,
            open: Arc::new(AtomicBool::new(true)),
            notices,
            presented: Mutex::new(HashSet::new()),
        })
    }

    /// The connection ended: no decide on it is eligible any more, a decide
    /// in flight included when it commits.
    pub(crate) fn close(&self) {
        self.open.store(false, Ordering::SeqCst);
    }

    /// `decision/list` on this connection: the page the client asks for,
    /// after presenting its decisions the operator acts for and was not shown
    /// here yet. Without the realm's configuration it presents nothing.
    pub(crate) async fn handle_list(
        &self,
        id: Option<RpcId>,
        params: Option<&RawValue>,
        runtime: &SessionRuntime,
    ) -> RpcResponse {
        let (owner, mut listing) = match read_list_page(id.clone(), params, runtime) {
            Ok(page) => page,
            Err(response) => return response,
        };
        let mut presented = self.presented.lock().await;
        let waiting: Vec<usize> = listing
            .decisions
            .iter()
            .enumerate()
            .filter(|(_, row)| awaits_the_operator(row) && !presented.contains(&row.decision_id))
            .map(|(index, _)| index)
            .collect();
        if !waiting.is_empty() {
            let listed = match operator_approver_ids(runtime).await {
                Ok(listed) => listed,
                Err(error_value) => {
                    // The operator's approver ids are unknown: the page
                    // answers as on plain dispatch, and nothing is shown.
                    tracing::warn!(
                        error = %error_value.message,
                        "the realm's configuration cannot be read; the list presents nothing"
                    );
                    return RpcResponse::success(id, wire_decision_listing(&listing));
                }
            };
            for index in waiting {
                let Some(row) = listing.decisions.get(index) else {
                    continue;
                };
                if !listed.lists(row.descriptor.approver_id.as_str()) {
                    continue;
                }
                let outcome = self.present(&owner, row).await;
                match outcome {
                    Presented::Recorded(recorded) => {
                        presented.insert(recorded.decision_id.clone());
                        if let Some(slot) = listing.decisions.get_mut(index) {
                            *slot = *recorded;
                        }
                    }
                    Presented::Skipped => {}
                    Presented::Stopped => break,
                }
            }
        }
        drop(presented);
        RpcResponse::success(id, wire_decision_listing(&listing))
    }

    /// Show `row` to the operator and record that it was shown: the clock is
    /// read first, the notice is written and acknowledged on the pipe, and
    /// only then is the presentation recorded, at that reading.
    async fn present(&self, owner: &DurableDecisionService, row: &StoredDecision) -> Presented {
        let at_ms = match SystemDecisionClock.now_ms() {
            Ok(at_ms) => at_ms,
            Err(error) => {
                tracing::warn!(%error, "no clock reading for a decision presentation");
                return Presented::Stopped;
            }
        };
        let notice_id = uuid::Uuid::new_v4().to_string();
        let message_ref = match DecisionReference::new(notice_id.clone()) {
            Ok(message_ref) => message_ref,
            Err(error) => {
                tracing::warn!(%error, "a decision notice id is not a reference");
                return Presented::Skipped;
            }
        };
        let notice = presented_notice(row, notice_id);
        let notification = match RpcNotification::try_new(DECISION_NOTICE_NOTIFICATION, &notice) {
            Ok(notification) => notification,
            Err(error) => {
                tracing::warn!(%error, "a decision notice could not be admitted");
                return Presented::Skipped;
            }
        };
        let (written, acknowledged) = oneshot::channel();
        if self
            .notices
            .send(AcknowledgedNotification {
                notification,
                written,
            })
            .await
            .is_err()
            || acknowledged.await.is_err()
        {
            // The notice is not known to be on the pipe: nothing is
            // recorded.
            return Presented::Stopped;
        }
        let record = PresentationRecord::complete(
            PresentationAudience::Approver {
                approver_id: row.descriptor.approver_id.clone(),
            },
            &row.descriptor.rendering,
            None,
            self.channel_ref.clone(),
            message_ref,
            at_ms,
        );
        match owner.present(&row.decision_id, &record) {
            Ok(recorded) => Presented::Recorded(Box::new(recorded)),
            Err(error) => {
                tracing::warn!(%error, "a decision shown to the operator was not recorded");
                Presented::Skipped
            }
        }
    }

    /// `decision/decide` by this connection's operator. The connection
    /// builds the authenticated event: the operator in this connection's
    /// scope, the approver id of the stored decision, the request's answer,
    /// presented digest and reason, observed at the server's clock. It is
    /// eligible only for a decision this connection presented whose approver
    /// id the realm's configuration still lists when the decide arrives, and
    /// only while the connection is open.
    pub(crate) async fn handle_decide(
        &self,
        id: Option<RpcId>,
        params: Option<&RawValue>,
        runtime: Arc<SessionRuntime>,
    ) -> RpcResponse {
        let request: DecisionDecideParams = match parse_params(params) {
            Ok(request) => request,
            Err(response) => return response.with_id(id),
        };
        let decision_id = match parse_decision_id(id.clone(), request.decision_id) {
            Ok(decision_id) => decision_id,
            Err(response) => return response,
        };
        let presented_digest = match Sha256Digest::new(request.presented_digest) {
            Ok(digest) => digest,
            Err(error_value) => return invalid(id, &error_value),
        };
        let owner = match decision_owner(id.clone(), &runtime) {
            Ok(owner) => owner,
            Err(response) => return response,
        };
        // A decide waits for a presentation pass in flight on this
        // connection, and reads what it presented.
        let presented = self.presented.lock().await;
        let row = match owner.get(&decision_id) {
            Ok(row) => row,
            Err(error_value) => return decision_error_to_response(id, error_value),
        };
        let listed = match operator_approver_ids(&runtime).await {
            Ok(listed) => listed,
            Err(error_value) => return RpcResponse::from_error(id, error_value),
        };
        let eligible =
            presented.contains(&decision_id) && listed.lists(row.descriptor.approver_id.as_str());
        drop(presented);
        let observed_at_ms = match SystemDecisionClock.now_ms() {
            Ok(now_ms) => now_ms,
            Err(error_value) => {
                return RpcResponse::error(id, error::INTERNAL_ERROR, error_value.to_string());
            }
        };
        let decision = DecisionIngress::new(DECISION_INGRESS).and_then(|ingress| {
            let decision = AttestedDecision::new(
                self.approver.clone(),
                ingress,
                event_ref(id.as_ref())?,
                observed_at_ms,
                row.descriptor.approver_id.clone(),
                presented_digest,
                request.approve,
            )?;
            match request.reason {
                Some(reason) => decision.with_reason(reason),
                None => Ok(decision),
            }
        });
        let decision = match decision {
            Ok(decision) => decision,
            Err(error_value) => return invalid(id, &error_value),
        };
        let event = AuthenticatedDecisionEvent::new(
            decision_id,
            decision,
            Arc::new(OperatorEligibility {
                snapshot: eligible,
                open: Arc::clone(&self.open),
            }),
        );
        handle_decide_event(id, params, runtime, event)
    }
}

/// Whether `row` waits for a human decision now.
fn awaits_the_operator(row: &StoredDecision) -> bool {
    row.state.phase == DecisionPhase::Open && row.state.stage == DecisionStage::Human
}

/// The approver ids the realm's configuration lists for the operator now:
/// the runtime's effective configuration (on a governed pipe, its host's
/// configuration as it started).
async fn operator_approver_ids(
    runtime: &SessionRuntime,
) -> Result<DecisionGovernanceConfig, RpcError> {
    runtime.effective_config().await.map(|config| {
        config
            .governance
            .map(|governance| governance.decisions)
            .unwrap_or_default()
    })
}

/// The `presented` notice of `row`: the whole rendering, as one canonical
/// part.
fn presented_notice(row: &StoredDecision, notice_id: String) -> WireDecisionNotice {
    let descriptor = &row.descriptor;
    let rendering = descriptor.rendering.as_str();
    WireDecisionNotice {
        notice_id,
        decision_id: row.decision_id.to_string(),
        kind: descriptor.kind.to_string(),
        event: WireDecisionNoticeEvent::Presented,
        phase: wire_decision_phase(row.state.phase),
        approver_id: descriptor.approver_id.to_string(),
        requester: descriptor.requester.to_string(),
        presentation: Some(WireDecisionNoticePresentation {
            rendering: rendering.to_owned(),
            renderer: descriptor.renderer.to_string(),
            subject_digest: descriptor.subject_digest.to_string(),
            parts: canonical_parts(rendering, None)
                .iter()
                .map(wire_canonical_part)
                .collect(),
        }),
    }
}

/// The reference of one decide event: the request's id and a nonce, or the
/// id's digest and the nonce when the id itself cannot be part of a
/// reference.
fn event_ref(id: Option<&RpcId>) -> Result<DecisionReference, DecisionContractError> {
    let id = serde_json::to_string(&id).unwrap_or_default();
    let nonce = uuid::Uuid::new_v4();
    DecisionReference::new(format!("{REFERENCE_PREFIX}:{id}:{nonce}")).or_else(|_| {
        DecisionReference::new(format!(
            "{REFERENCE_PREFIX}:{}:{nonce}",
            Sha256Digest::of(id.as_bytes())
        ))
    })
}

/// The eligibility a decide on this connection carries: whether the
/// connection presented the decision and still listed its approver id when
/// the decide arrived, and, when the decision commits, whether the
/// connection is still open.
struct OperatorEligibility {
    snapshot: bool,
    open: Arc<AtomicBool>,
}

impl ConsentEligibility for OperatorEligibility {
    fn check_current(&self) -> Result<(), ConsentIneligible> {
        if self.snapshot && self.open.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(ConsentIneligible)
        }
    }
}
