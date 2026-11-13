//! The stdio host's decision verbs.
//!
//! One [`StdioDecisionConnection`] serves the `meta/decisions/*` verbs of
//! one stdio gateway connection, over the runtime's decision owner. The
//! host reads, presents and withdraws decisions. It is told of each open
//! decision once per connection, when it lists it: before a
//! `meta/decisions/list` answers, the connection writes one
//! `meta/decisions/notice` for each open decision of the page it has not
//! noticed yet. A decision the approver map lets the stdio operator decide
//! (`ingress_subjects = [{ ingress = "operator" }]`) that waits for a human
//! is presented to the operator instead: the connection reads its clock,
//! writes a `presented` notice carrying the whole rendering, waits until the
//! notice is on the pipe, and only then records the presentation on its own
//! channel under the notice's id, at that clock reading. Any other decision
//! is noticed `held`, showing nothing.
//!
//! `meta/decisions/decide` without an attestation is the operator's
//! (device trusted, in the connection's own authentication scope), eligible
//! only for a decision this connection presented to the operator, while the
//! map still lists the operator for its approver id when it commits and
//! while the connection is open. A presentation recorded any other way (the
//! host's own `present`, or another surface on the shared store) never makes
//! the operator eligible. With an `attestation`, a declared ingress attests
//! the person who decided; the map must list that person.

use std::collections::HashSet;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use meerkat_contracts::{
    DecisionGetParams, DecisionListParams, DecisionPresentParams, DecisionRenderingParams,
    DecisionRenderingResult, DecisionResult, DecisionWithdrawParams,
    WireDecisionNoticePresentation,
};
use meerkat_core::approval::ApprovalAssurance;
use meerkat_core::approval::consent::{AuthenticatedApprover, HostAuthenticationScope};
use meerkat_core::approval::decision::{
    ApproverId, AttestedDecision, DECISION_PAGE_MAX, DecisionContractError, DecisionId,
    DecisionIngress, DecisionKind, DecisionListFilter, DecisionPhase, DecisionPrincipalKey,
    DecisionReference, DecisionStage, PresentationAudience, PresentationRecord, Sha256Digest,
    StoredDecision, canonical_parts,
};
use meerkat_core::{PrincipalRef, RealmId};
use meerkat_runtime::decision::wire::{
    decision_phase_from_wire, presentation_from_wire, wire_canonical_part, wire_decision,
    wire_decision_phase, wire_unreadable_decision,
};
use meerkat_runtime::decision::{
    DecisionClock, DecisionListing, DecisionWithdrawer, DurableDecisionService, SystemDecisionClock,
};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::Mutex;

use super::approvers::{
    DecideInput, DeciderIdentity, LiveHolderEligibility, decider_principal, holding,
    ingress_person, local_operator_principal,
};
use super::owner::{DecisionEventBinding, RuntimeDecisions};
use super::wire::{
    self, DECIDE_METHOD, DECISIONS_METHOD_PREFIX, DecisionAttestationRequest, GET_METHOD,
    LIST_METHOD, METHOD_NOT_FOUND, MetaDecisionDecideParams, MetaDecisionListResult,
    MetaDecisionNotice, MetaDecisionNoticeEvent, PRESENT_METHOD, RENDERING_METHOD, WITHDRAW_METHOD,
};
use crate::UnifiedRuntime;
use crate::access::OPERATOR_INGRESS;

/// The page size when a listing names none.
const DEFAULT_LIST_PAGE: usize = 100;

/// The prefix of the channel and event references a stdio connection names.
const REFERENCE_PREFIX: &str = "meta-stdio";

/// Writes one `meta/decisions/notice` line to the stdio host.
#[async_trait]
pub trait DecisionNoticeSink: Send + Sync {
    /// Write `line` and answer, once it is known, whether it reached the
    /// pipe (written and flushed).
    async fn deliver(&self, line: String) -> bool;
}

/// The stdio operator of one connection.
struct StdioOperator {
    /// The operating-system user that started the gateway, as the realm's
    /// principal.
    principal: PrincipalRef,
    /// That principal, device trusted, in this connection's own scope.
    approver: AuthenticatedApprover,
}

/// What this connection told its host.
#[derive(Default)]
struct ConnectionDecisions {
    /// The decisions it noticed, held or presented.
    noticed: HashSet<DecisionId>,
    /// The decisions it presented to its operator.
    presented: HashSet<DecisionId>,
}

/// What presenting one decision to the operator came to.
enum Presented {
    /// Shown and recorded: the decision as the owner recorded it.
    Recorded(Box<StoredDecision>),
    /// Shown but not recorded; the pass goes on with the next decision.
    Skipped,
    /// The connection cannot show anything more; the pass ends.
    Stopped,
}

/// One stdio connection's decision verbs; see the module documentation.
pub struct StdioDecisionConnection {
    operator: Option<StdioOperator>,
    channel_ref: Option<DecisionReference>,
    open: Arc<AtomicBool>,
    notices: Arc<dyn DecisionNoticeSink>,
    /// Its lock is the presentation pass: a decide waits for a pass in
    /// flight.
    state: Mutex<ConnectionDecisions>,
}

impl std::fmt::Debug for StdioDecisionConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StdioDecisionConnection")
            .field("operator", &self.operator.is_some())
            .field("open", &self.open.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

impl StdioDecisionConnection {
    /// The connection of a stdio gateway whose runtime's meerkat-level
    /// realm is `realm`, writing its notices to `notices`. Its operator is
    /// the operating-system user that started the gateway, named in
    /// `realm`; without a realm, or where the user cannot be named, the
    /// connection has no operator, and a decide without an attestation is
    /// method not found.
    pub fn new(realm: Option<&RealmId>, notices: Arc<dyn DecisionNoticeSink>) -> Self {
        let connection = uuid::Uuid::new_v4();
        let operator = realm.and_then(|realm| local_operator(realm, connection));
        let channel_ref = DecisionReference::new(format!("{REFERENCE_PREFIX}:{connection}")).ok();
        Self {
            operator,
            channel_ref,
            open: Arc::new(AtomicBool::new(true)),
            notices,
            state: Mutex::new(ConnectionDecisions::default()),
        }
    }

    /// The connection ended: no decide on it is eligible any more, a decide
    /// in flight included when it commits.
    pub fn close(&self) {
        self.open.store(false, Ordering::SeqCst);
    }

    /// The answer to `request_line` when it calls a `meta/decisions/*`
    /// method, and `None` for any other line.
    pub async fn handle_line(
        &self,
        runtime: &UnifiedRuntime,
        request_line: &str,
    ) -> Option<String> {
        let request: Value = serde_json::from_str(request_line).ok()?;
        let method = request.get("method")?.as_str()?;
        if !method.starts_with(DECISIONS_METHOD_PREFIX) {
            return None;
        }
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let params = request.get("params").cloned().unwrap_or(Value::Null);
        let decisions = runtime.durable_decisions();
        let answer = match method {
            LIST_METHOD => self.list(&id, params, runtime, decisions).await,
            GET_METHOD => get(&id, params, decisions),
            RENDERING_METHOD => rendering(&id, params, decisions),
            PRESENT_METHOD => present(&id, params, decisions),
            WITHDRAW_METHOD => withdraw(&id, params, decisions),
            DECIDE_METHOD => self.decide(&id, params, runtime, decisions).await,
            _ => wire::failure(&id, METHOD_NOT_FOUND, "Method not found", None),
        };
        Some(answer.to_string())
    }

    /// `meta/decisions/list`: the page the host asks for, after noticing
    /// its open decisions this connection did not notice yet, and
    /// presenting to the operator those it may decide.
    async fn list(
        &self,
        id: &Value,
        params: Value,
        runtime: &UnifiedRuntime,
        decisions: &RuntimeDecisions,
    ) -> Value {
        let params: DecisionListParams = match params {
            Value::Null => DecisionListParams::default(),
            params => match parse(id, params) {
                Ok(params) => params,
                Err(answer) => return answer,
            },
        };
        let (owner, mut listing) = match read_page(id, &params, decisions) {
            Ok(page) => page,
            Err(answer) => return answer,
        };
        let config = runtime
            .access_controller()
            .map(|access| access.snapshot().0);
        let mut state = self.state.lock().await;
        for index in 0..listing.decisions.len() {
            let Some(row) = listing.decisions.get(index) else {
                continue;
            };
            if row.state.phase != DecisionPhase::Open {
                continue;
            }
            let for_operator = self.operator.is_some()
                && !decisions.is_candidate()
                && row.state.stage == DecisionStage::Human
                && !state.presented.contains(&row.decision_id)
                && config.as_deref().is_some_and(|config| {
                    holding(
                        config,
                        row.descriptor.approver_id.as_str(),
                        &DeciderIdentity::Operator,
                    )
                    .is_some()
                });
            if for_operator {
                let decision_id = row.decision_id.clone();
                match self.present_to_operator(owner, row).await {
                    Presented::Recorded(recorded) => {
                        state.noticed.insert(decision_id.clone());
                        state.presented.insert(decision_id);
                        if let Some(slot) = listing.decisions.get_mut(index) {
                            *slot = *recorded;
                        }
                    }
                    Presented::Skipped => {
                        state.noticed.insert(decision_id);
                    }
                    Presented::Stopped => break,
                }
            } else if !state.noticed.contains(&row.decision_id) {
                let decision_id = row.decision_id.clone();
                let Ok(line) = wire::notice_line(&held_notice(row)) else {
                    continue;
                };
                if !self.notices.deliver(line).await {
                    break;
                }
                state.noticed.insert(decision_id);
            }
        }
        drop(state);
        wire::success(id, whole_page(&listing))
    }

    /// Show `row` to the operator and record that it was shown: the clock
    /// is read first, the notice is written and acknowledged on the pipe,
    /// and only then is the presentation recorded, at that reading.
    async fn present_to_operator(
        &self,
        owner: &DurableDecisionService,
        row: &StoredDecision,
    ) -> Presented {
        let Some(channel_ref) = self.channel_ref.clone() else {
            return Presented::Stopped;
        };
        let at_ms = match SystemDecisionClock.now_ms() {
            Ok(at_ms) => at_ms,
            Err(error) => {
                tracing::warn!(%error, "no clock reading for a decision presentation");
                return Presented::Stopped;
            }
        };
        let notice_id = uuid::Uuid::new_v4().to_string();
        let Ok(message_ref) = DecisionReference::new(notice_id.clone()) else {
            return Presented::Skipped;
        };
        let Ok(line) = wire::notice_line(&presented_notice(row, notice_id)) else {
            return Presented::Skipped;
        };
        if !self.notices.deliver(line).await {
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
            channel_ref,
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

    /// `meta/decisions/decide`: the operator's, or, with an attestation, the
    /// person a declared ingress attests.
    async fn decide(
        &self,
        id: &Value,
        params: Value,
        runtime: &UnifiedRuntime,
        decisions: &RuntimeDecisions,
    ) -> Value {
        let mut request: MetaDecisionDecideParams = match parse(id, params) {
            Ok(request) => request,
            Err(answer) => return answer,
        };
        let decision_id = match DecisionId::new(request.decision_id.clone()) {
            Ok(decision_id) => decision_id,
            Err(error) => return invalid(id, &error),
        };
        let presented_digest = match Sha256Digest::new(request.presented_digest.clone()) {
            Ok(digest) => digest,
            Err(error) => return invalid(id, &error),
        };
        if !decisions.is_available() {
            return wire::no_decision_store(id);
        }
        match request.attestation.take() {
            Some(attestation) => {
                self.decide_attested(
                    id,
                    runtime,
                    decisions,
                    decision_id,
                    presented_digest,
                    &request,
                    attestation,
                )
                .await
            }
            None => {
                self.decide_as_operator(
                    id,
                    runtime,
                    decisions,
                    decision_id,
                    presented_digest,
                    &request,
                )
                .await
            }
        }
    }

    async fn decide_as_operator(
        &self,
        id: &Value,
        runtime: &UnifiedRuntime,
        decisions: &RuntimeDecisions,
        decision_id: DecisionId,
        presented_digest: Sha256Digest,
        request: &MetaDecisionDecideParams,
    ) -> Value {
        let Some(operator) = self.operator.as_ref() else {
            return wire::failure(
                id,
                METHOD_NOT_FOUND,
                "this connection has no operator: a decide names its attestation",
                None,
            );
        };
        let owner = decisions.owner();
        let access = runtime.access_controller().cloned();
        // A decide waits for a presentation pass in flight on this
        // connection, and reads what it presented.
        let state = self.state.lock().await;
        let presented = state.presented.contains(&decision_id);
        let row = match owner.get(&decision_id) {
            Ok(row) => row,
            Err(error) => return wire::decision_error(id, error),
        };
        drop(state);
        let approver_id = row.descriptor.approver_id.to_string();
        let held = access.as_ref().and_then(|access| {
            holding(
                &access.snapshot().0,
                &approver_id,
                &DeciderIdentity::Operator,
            )
        });
        let decider = decider_principal(
            &row,
            held.as_ref(),
            operator.principal.clone(),
            Some(&operator.principal),
        );
        // The commit reads the map again and refuses a decide whose key it
        // read before the map linked the operator to the requester.
        let input = DecideInput::new(&row, &decider, Some(&operator.principal));
        let approver = if decider == operator.principal {
            operator.approver.clone()
        } else {
            match self.approver_in_connection(decider, ApprovalAssurance::DeviceTrusted) {
                Ok(approver) => approver,
                Err(message) => return wire::failure(id, wire::INTERNAL_ERROR, message, None),
            }
        };
        let observed_at_ms = match SystemDecisionClock.now_ms() {
            Ok(now_ms) => now_ms,
            Err(error) => return wire::failure(id, wire::INTERNAL_ERROR, error.to_string(), None),
        };
        let decision = DecisionIngress::new(OPERATOR_INGRESS).and_then(|ingress| {
            let decision = AttestedDecision::new(
                approver,
                ingress,
                operator_event_ref(id)?,
                observed_at_ms,
                row.descriptor.approver_id.clone(),
                presented_digest,
                request.approve,
            )?;
            match request.reason.clone() {
                Some(reason) => decision.with_reason(reason),
                None => Ok(decision),
            }
        });
        let decision = match decision {
            Ok(decision) => decision,
            Err(error) => return invalid(id, &error),
        };
        let eligibility = LiveHolderEligibility::operator(
            access,
            approver_id,
            input,
            presented,
            Arc::clone(&self.open),
        );
        match owner.decide(&decision_id, &decision, &eligibility) {
            Ok(row) => {
                decisions.notify_work();
                wire::success(
                    id,
                    DecisionResult {
                        decision: wire_decision(&row, false),
                    },
                )
            }
            Err(error) => wire::decision_error(id, error),
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn decide_attested(
        &self,
        id: &Value,
        runtime: &UnifiedRuntime,
        decisions: &RuntimeDecisions,
        decision_id: DecisionId,
        presented_digest: Sha256Digest,
        request: &MetaDecisionDecideParams,
        attestation: DecisionAttestationRequest,
    ) -> Value {
        if attestation.ingress == OPERATOR_INGRESS {
            return wire::invalid_params(
                id,
                "attestation ingress \"operator\" is built in: the operator decides without an \
                 attestation",
            );
        }
        let Some(declaration) = decisions.ingresses().get(&attestation.ingress) else {
            return wire::invalid_params(
                id,
                format!(
                    "attestation ingress {:?} is not declared in runtime_options.decisions",
                    attestation.ingress
                ),
            );
        };
        if attestation.subject.is_empty() || attestation.subject.trim() != attestation.subject {
            return wire::invalid_params(
                id,
                "attestation subject must not be empty or have leading or trailing whitespace",
            );
        }
        if attestation.scope.is_empty() {
            return wire::invalid_params(id, "attestation scope must not be empty");
        }
        let now_ms = match SystemDecisionClock.now_ms() {
            Ok(now_ms) => now_ms,
            Err(error) => return wire::failure(id, wire::INTERNAL_ERROR, error.to_string(), None),
        };
        // A skew is at most an hour, so only a clock near its end overflows:
        // such a reading accepts no event.
        let within_skew = now_ms
            .checked_add(declaration.max_clock_skew_ms())
            .is_some_and(|latest_ms| attestation.observed_at_ms <= latest_ms);
        if !within_skew {
            return wire::invalid_params(
                id,
                format!(
                    "attestation observed_at_ms {} is beyond ingress {:?}'s clock skew: more than \
                     {} ms ahead of this host's clock",
                    attestation.observed_at_ms,
                    attestation.ingress,
                    declaration.max_clock_skew_ms()
                ),
            );
        }
        let person = match ingress_person(&attestation.ingress, &attestation.subject) {
            Ok(person) => person,
            Err(error) => {
                return wire::invalid_params(id, format!("attestation subject: {error}"));
            }
        };
        let (ingress, event_ref) =
            match DecisionIngress::new(attestation.ingress.clone()).and_then(|ingress| {
                Ok((
                    ingress,
                    DecisionReference::new(attestation.event_ref.clone())?,
                ))
            }) {
                Ok(parts) => parts,
                Err(error) => return invalid(id, &error),
            };
        let presentation = match attestation
            .presentation
            .as_ref()
            .map(presentation_from_wire)
        {
            None => None,
            Some(Ok(presentation)) => Some(presentation),
            Some(Err(error)) => return invalid(id, &error),
        };
        let owner = decisions.owner();
        let row = match owner.get(&decision_id) {
            Ok(row) => row,
            Err(error) => return wire::decision_error(id, error),
        };
        if decisions.is_candidate() {
            return wire::decision_error(
                id,
                meerkat_runtime::decision::DecisionError::CandidateLaunch,
            );
        }
        // The event is bound to its decision durably before the decide
        // input: another decision can never use it, and a restart keeps it.
        match decisions
            .bind_event(&ingress, &event_ref, &decision_id)
            .await
        {
            Ok(DecisionEventBinding::Own) => {}
            Ok(DecisionEventBinding::Another) => {
                return wire::invalid_params(
                    id,
                    format!(
                        "attestation event_ref {:?} of ingress {:?} was used by another decision",
                        attestation.event_ref, attestation.ingress
                    ),
                );
            }
            Err(error) => {
                return wire::failure(
                    id,
                    wire::INTERNAL_ERROR,
                    format!("the decision event could not be recorded: {error}"),
                    None,
                );
            }
        }
        let access = runtime.access_controller().cloned();
        let approver_id = row.descriptor.approver_id.to_string();
        let identity = DeciderIdentity::Ingress {
            ingress: attestation.ingress.clone(),
            subject: attestation.subject.clone(),
        };
        let held = access
            .as_ref()
            .and_then(|access| holding(&access.snapshot().0, &approver_id, &identity));
        let operator = self.operator.as_ref().map(|operator| &operator.principal);
        let decider = decider_principal(&row, held.as_ref(), person, operator);
        // The commit reads the map again and refuses a decide whose key it
        // read before the map linked the person to the requester.
        let input = DecideInput::new(&row, &decider, operator);
        let approver =
            match AuthenticatedApprover::new(decider, ApprovalAssurance::PersonAuthenticated) {
                Ok(approver) => approver.with_authentication_scope(HostAuthenticationScope::new(
                    attestation.scope.clone(),
                )),
                Err(error) => {
                    return wire::invalid_params(id, format!("attestation subject: {error}"));
                }
            };
        let decision = AttestedDecision::new(
            approver,
            ingress,
            event_ref,
            attestation.observed_at_ms,
            row.descriptor.approver_id.clone(),
            presented_digest,
            request.approve,
        )
        .and_then(|decision| match request.reason.clone() {
            Some(reason) => decision.with_reason(reason),
            None => Ok(decision),
        })
        .map(|decision| match presentation {
            Some(presentation) => decision.with_presentation(presentation),
            None => decision,
        });
        let decision = match decision {
            Ok(decision) => decision,
            Err(error) => return invalid(id, &error),
        };
        let eligibility = LiveHolderEligibility::attested(access, approver_id, identity, input);
        match owner.decide(&decision_id, &decision, &eligibility) {
            Ok(row) => {
                decisions.notify_work();
                wire::success(
                    id,
                    DecisionResult {
                        decision: wire_decision(&row, false),
                    },
                )
            }
            Err(error) => wire::decision_error(id, error),
        }
    }

    /// `principal`, authenticated at `assurance` in the operator's own
    /// connection scope.
    fn approver_in_connection(
        &self,
        principal: PrincipalRef,
        assurance: ApprovalAssurance,
    ) -> Result<AuthenticatedApprover, String> {
        let scope = self
            .operator
            .as_ref()
            .and_then(|operator| operator.approver.authentication_scope().cloned());
        let approver = AuthenticatedApprover::new(principal, assurance)
            .map_err(|error| format!("the decider is not an approver: {error}"))?;
        Ok(match scope {
            Some(scope) => approver.with_authentication_scope(scope),
            None => approver,
        })
    }
}

/// The operator of a connection on `realm`: this process's
/// operating-system user, device trusted, in a scope of the connection's
/// own. `None` when the user cannot be named as an approver.
fn local_operator(realm: &RealmId, connection: uuid::Uuid) -> Option<StdioOperator> {
    let principal = match local_operator_principal(realm) {
        Ok(principal) => principal,
        Err(error) => {
            tracing::warn!(%error, "the stdio operator cannot be named; it decides nothing");
            return None;
        }
    };
    let scope = HostAuthenticationScope::new(format!(
        "{REFERENCE_PREFIX}:{}:{connection}",
        principal.id.as_str()
    ));
    match AuthenticatedApprover::new(principal.clone(), ApprovalAssurance::DeviceTrusted) {
        Ok(approver) => Some(StdioOperator {
            principal,
            approver: approver.with_authentication_scope(scope),
        }),
        Err(error) => {
            tracing::warn!(%error, "the stdio operator is not an approver; it decides nothing");
            None
        }
    }
}

/// The reference of one operator decide event: the request's id and a
/// nonce, or the id's digest and the nonce when the id itself cannot be
/// part of a reference.
fn operator_event_ref(id: &Value) -> Result<DecisionReference, DecisionContractError> {
    let id = id.to_string();
    let nonce = uuid::Uuid::new_v4();
    DecisionReference::new(format!("{REFERENCE_PREFIX}:{id}:{nonce}")).or_else(|_| {
        DecisionReference::new(format!(
            "{REFERENCE_PREFIX}:{}:{nonce}",
            Sha256Digest::of(id.as_bytes())
        ))
    })
}

fn invalid(id: &Value, error: &DecisionContractError) -> Value {
    wire::invalid_params(id, format!("invalid decision request: {error}"))
}

/// The params of `id`'s request, as `T`; refused as invalid params.
pub(crate) fn parse<T: DeserializeOwned>(id: &Value, params: Value) -> Result<T, Value> {
    serde_json::from_value(params)
        .map_err(|error| wire::invalid_params(id, format!("invalid params: {error}")))
}

/// The listing filter `params` names: every value is validated, and a phase
/// this version does not know is refused, never read as no filter.
pub(crate) fn list_filter(
    id: &Value,
    params: &DecisionListParams,
) -> Result<DecisionListFilter, Value> {
    let mut filter = DecisionListFilter::default();
    if let Some(kind) = &params.kind {
        let kind = DecisionKind::new(kind.clone()).map_err(|error| invalid(id, &error))?;
        filter = filter.with_kind(kind);
    }
    if let Some(phase) = params.phase {
        let Some(phase) = decision_phase_from_wire(phase) else {
            return Err(wire::invalid_params(id, "unknown decision phase"));
        };
        filter = filter.with_phase(phase);
    }
    if let Some(approver_id) = &params.approver_id {
        let approver_id =
            ApproverId::new(approver_id.clone()).map_err(|error| invalid(id, &error))?;
        filter = filter.with_approver_id(approver_id);
    }
    if let Some(requester) = &params.requester {
        let requester =
            DecisionPrincipalKey::new(requester.clone()).map_err(|error| invalid(id, &error))?;
        filter = filter.with_requester(requester);
    }
    Ok(filter)
}

/// The page size `params` name: the default when they name none. A page of
/// no decisions is refused; a larger page than the store's reads as the
/// store's.
pub(crate) fn page_limit(id: &Value, params: &DecisionListParams) -> Result<usize, Value> {
    match params.limit {
        None => Ok(DEFAULT_LIST_PAGE),
        Some(0) => Err(wire::invalid_params(
            id,
            "a decision page limit must be at least 1",
        )),
        Some(limit) => Ok(usize::try_from(limit)
            .unwrap_or(DECISION_PAGE_MAX)
            .min(DECISION_PAGE_MAX)),
    }
}

/// The page `params` names, and the owner it was read through.
pub(crate) fn read_page<'a>(
    id: &Value,
    params: &DecisionListParams,
    decisions: &'a RuntimeDecisions,
) -> Result<(&'a DurableDecisionService, DecisionListing), Value> {
    let filter = list_filter(id, params)?;
    let limit = page_limit(id, params)?;
    if !decisions.is_available() {
        return Err(wire::no_decision_store(id));
    }
    let owner = decisions.owner();
    owner
        .list(&filter, params.after.as_deref(), limit)
        .map(|listing| (owner, listing))
        .map_err(|error| wire::decision_error(id, error))
}

/// A page the caller reads whole.
fn whole_page(listing: &DecisionListing) -> MetaDecisionListResult {
    MetaDecisionListResult {
        decisions: listing
            .decisions
            .iter()
            .map(|row| wire_decision(row, false))
            .collect(),
        statuses: Vec::new(),
        unreadable: listing
            .unreadable
            .iter()
            .map(wire_unreadable_decision)
            .collect(),
        next_after: listing.next_after.clone(),
    }
}

/// The decision id `params` name, read from `T`'s `decision_id`.
fn decision_id_of(id: &Value, decision_id: String) -> Result<DecisionId, Value> {
    DecisionId::new(decision_id).map_err(|error| invalid(id, &error))
}

/// `meta/decisions/get` for the host: the decision whole.
fn get(id: &Value, params: Value, decisions: &RuntimeDecisions) -> Value {
    let params: DecisionGetParams = match parse(id, params) {
        Ok(params) => params,
        Err(answer) => return answer,
    };
    let decision_id = match decision_id_of(id, params.decision_id) {
        Ok(decision_id) => decision_id,
        Err(answer) => return answer,
    };
    if !decisions.is_available() {
        return wire::no_decision_store(id);
    }
    match decisions.owner().get(&decision_id) {
        Ok(row) => wire::success(
            id,
            DecisionResult {
                decision: wire_decision(&row, params.include_request),
            },
        ),
        Err(error) => wire::decision_error(id, error),
    }
}

/// The rendering of `row` and its canonical parts for `max_part_bytes`.
pub(crate) fn rendering_of(
    row: &StoredDecision,
    max_part_bytes: Option<NonZeroU32>,
) -> DecisionRenderingResult {
    let rendering = row.descriptor.rendering.as_str();
    DecisionRenderingResult {
        decision_id: row.decision_id.to_string(),
        rendering: rendering.to_owned(),
        renderer: row.descriptor.renderer.to_string(),
        subject_digest: row.descriptor.subject_digest.to_string(),
        max_part_bytes: max_part_bytes.map(NonZeroU32::get),
        parts: canonical_parts(rendering, max_part_bytes)
            .iter()
            .map(wire_canonical_part)
            .collect(),
    }
}

/// The part size `params` name: none, or at least one byte.
pub(crate) fn part_size(
    id: &Value,
    max_part_bytes: Option<u32>,
) -> Result<Option<NonZeroU32>, Value> {
    match max_part_bytes {
        None => Ok(None),
        Some(bytes) => NonZeroU32::new(bytes)
            .map(Some)
            .ok_or_else(|| wire::invalid_params(id, "a part size is at least one byte")),
    }
}

/// `meta/decisions/rendering` for the host.
fn rendering(id: &Value, params: Value, decisions: &RuntimeDecisions) -> Value {
    let params: DecisionRenderingParams = match parse(id, params) {
        Ok(params) => params,
        Err(answer) => return answer,
    };
    let decision_id = match decision_id_of(id, params.decision_id) {
        Ok(decision_id) => decision_id,
        Err(answer) => return answer,
    };
    let max_part_bytes = match part_size(id, params.max_part_bytes) {
        Ok(size) => size,
        Err(answer) => return answer,
    };
    if !decisions.is_available() {
        return wire::no_decision_store(id);
    }
    match decisions.owner().get(&decision_id) {
        Ok(row) => wire::success(id, rendering_of(&row, max_part_bytes)),
        Err(error) => wire::decision_error(id, error),
    }
}

/// `meta/decisions/present`: what the host presenter showed on its channel.
/// It counts only when it shows every canonical part of the held rendering
/// to the party that decides now; it never makes the operator eligible.
fn present(id: &Value, params: Value, decisions: &RuntimeDecisions) -> Value {
    let params: DecisionPresentParams = match parse(id, params) {
        Ok(params) => params,
        Err(answer) => return answer,
    };
    let decision_id = match decision_id_of(id, params.decision_id) {
        Ok(decision_id) => decision_id,
        Err(answer) => return answer,
    };
    let presentation = match presentation_from_wire(&params.presentation) {
        Ok(presentation) => presentation,
        Err(error) => return invalid(id, &error),
    };
    if !decisions.is_available() {
        return wire::no_decision_store(id);
    }
    match decisions.owner().present(&decision_id, &presentation) {
        Ok(row) => wire::success(
            id,
            DecisionResult {
                decision: wire_decision(&row, false),
            },
        ),
        Err(error) => wire::decision_error(id, error),
    }
}

/// `meta/decisions/withdraw`: the host withdraws an open decision.
fn withdraw(id: &Value, params: Value, decisions: &RuntimeDecisions) -> Value {
    let params: DecisionWithdrawParams = match parse(id, params) {
        Ok(params) => params,
        Err(answer) => return answer,
    };
    let decision_id = match decision_id_of(id, params.decision_id) {
        Ok(decision_id) => decision_id,
        Err(answer) => return answer,
    };
    if !decisions.is_available() {
        return wire::no_decision_store(id);
    }
    match decisions
        .owner()
        .withdraw(&decision_id, &DecisionWithdrawer::Host)
    {
        Ok(row) => {
            decisions.notify_work();
            wire::success(
                id,
                DecisionResult {
                    decision: wire_decision(&row, false),
                },
            )
        }
        Err(error) => wire::decision_error(id, error),
    }
}

/// The `held` notice of `row`: nothing of its content.
fn held_notice(row: &StoredDecision) -> MetaDecisionNotice {
    notice(
        row,
        uuid::Uuid::new_v4().to_string(),
        MetaDecisionNoticeEvent::Held,
    )
}

/// The `presented` notice of `row`: the whole rendering, as one canonical
/// part.
fn presented_notice(row: &StoredDecision, notice_id: String) -> MetaDecisionNotice {
    let descriptor = &row.descriptor;
    let rendering = descriptor.rendering.as_str();
    MetaDecisionNotice {
        presentation: Some(WireDecisionNoticePresentation {
            rendering: rendering.to_owned(),
            renderer: descriptor.renderer.to_string(),
            subject_digest: descriptor.subject_digest.to_string(),
            parts: canonical_parts(rendering, None)
                .iter()
                .map(wire_canonical_part)
                .collect(),
        }),
        ..notice(row, notice_id, MetaDecisionNoticeEvent::Presented)
    }
}

fn notice(
    row: &StoredDecision,
    notice_id: String,
    event: MetaDecisionNoticeEvent,
) -> MetaDecisionNotice {
    let descriptor = &row.descriptor;
    MetaDecisionNotice {
        notice_id,
        decision_id: row.decision_id.to_string(),
        kind: descriptor.kind.to_string(),
        event,
        phase: wire_decision_phase(row.state.phase),
        approver_id: descriptor.approver_id.to_string(),
        requester: descriptor.requester.to_string(),
        rendering_ref: descriptor.subject_digest.to_string(),
        presentation: None,
    }
}
