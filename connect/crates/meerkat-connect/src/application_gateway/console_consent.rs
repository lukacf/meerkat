//! Explicit installed R3 consent. The actual Meta callback supplies the durable
//! resolver. Only a decision on an existing approval crosses HTTP. Core retains
//! the exact operation/arguments; approval does not execute or prove recovery.
use crate::application_consent::{
    ApplicationConsentOwner, ApplicationOperationReview, ConsentApplicationIdentity,
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, OriginalUri, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::post,
};
use meerkat_core::{
    ApprovalAssurance, ApprovalDecision, ApprovalId, ApprovalPrincipalId, ApprovalRecord,
    ApprovalRisk, OperationExecutionScope, PrincipalKind, PrincipalRef,
    approval::{
        ApprovalConsentScope,
        consent::{
            AuthenticatedApprover, AuthenticatedConsentDecision, AuthenticatedConsentEvent,
            ConsentEligibility, ConsentIneligible, ConsentNotice, ConsentPresentation,
            ConsentPresentationCandidate, ConsentRouteUnavailable, HostAuthenticationScope,
            HumanConsentRoute,
        },
        review::OperationReviewer,
    },
};
use meerkat_mobkit::{
    http_console::ConsoleViewerResolver, identity_first::AgentIdentity,
    runtime::CurrentConsoleHumanEvidence,
};
use meerkat_toolkit::host_lifetime::control::HostLifetimeError;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, OnceLock, Weak},
    time::Duration,
};
use toolkit_mobkit::host_gateway::{
    ConsoleSetupBinding, ObservedConsoleConsent, ToolkitGatewayHost,
};

/// Trusted policy for a durable target's exact retained native operation.
/// Retired sessions retain their canonical identity; this does not select only
/// its current Active incarnation. Core separately fences transfer and entry.
/// Visibility is never this policy's source. Not deserializable from a request.
#[derive(Clone)]
pub struct ConsoleConsentRule {
    target: AgentIdentity,
    approvers: Vec<PrincipalRef>,
}
impl ConsoleConsentRule {
    pub fn for_target(
        target: AgentIdentity,
        approvers: Vec<PrincipalRef>,
    ) -> Result<Self, HostLifetimeError> {
        if approvers.is_empty()
            || approvers
                .iter()
                .any(|p| p.kind != PrincipalKind::Human || p.validate_qualified().is_err())
            || approvers
                .iter()
                .enumerate()
                .any(|(i, p)| approvers[..i].contains(p))
        {
            return Err(HostLifetimeError::InvalidSelection);
        }
        Ok(Self { target, approvers })
    }
}
/// Constructs the route and review together from one persistent issuer.
/// Mutually exclusive with a separately supplied generic operation review.
pub struct ConsoleConsentSelection {
    owner: Arc<ApplicationConsentOwner>,
    reviewer: Arc<dyn OperationReviewer>,
    review_deadline: Duration,
    consent_validity: Duration,
    origin: String,
    rules: Vec<ConsoleConsentRule>,
    control: ConsoleConsentControl,
    #[cfg(test)]
    observer: Option<Arc<dyn meerkat_core::approval::review::OperationReviewObserver>>,
}
impl ConsoleConsentSelection {
    pub fn new(
        owner: Arc<ApplicationConsentOwner>,
        reviewer: Arc<dyn OperationReviewer>,
        review_deadline: Duration,
        consent_validity: Duration,
        origin: String,
        rules: Vec<ConsoleConsentRule>,
    ) -> Result<Self, HostLifetimeError> {
        super::console_setup::ConsoleSetupSelection {
            origin: origin.clone(),
            access_management: None,
            recovery_management: None,
        }
        .validate()
        .map_err(|_| HostLifetimeError::InvalidSelection)?;
        validate_rules(&rules)?;
        if review_deadline.is_zero() || consent_validity.is_zero() {
            return Err(HostLifetimeError::InvalidSelection);
        }
        owner.check().map_err(|_| HostLifetimeError::Unavailable)?;
        Ok(Self {
            owner,
            reviewer,
            review_deadline,
            consent_validity,
            origin,
            rules,
            control: ConsoleConsentControl::default(),
            #[cfg(test)]
            observer: None,
        })
    }
    #[cfg(test)]
    pub(crate) fn with_test_observer(
        mut self,
        observer: Arc<dyn meerkat_core::approval::review::OperationReviewObserver>,
    ) -> Self {
        self.observer = Some(observer);
        self
    }
    pub fn control(&self) -> ConsoleConsentControl {
        self.control.clone()
    }
    pub(crate) fn compose(
        self,
        host: ToolkitGatewayHost,
    ) -> Result<
        (
            ToolkitGatewayHost,
            Arc<ConsoleConsentRoute>,
            Arc<ApplicationConsentOwner>,
        ),
        HostLifetimeError,
    > {
        let binding = host.console_setup_binding()?;
        let identity = binding.with_consent_host(|actual| {
            ConsentApplicationIdentity::from_installed(actual)
                .map_err(|_| HostLifetimeError::Unavailable)
        })?;
        if &identity != self.owner.identity() {
            return Err(HostLifetimeError::WrongOwner);
        }
        let route = Arc::new(ConsoleConsentRoute {
            owner: Arc::downgrade(&self.owner),
            binding,
            origin: self.origin,
            selected: Mutex::new(Some(Selected {
                id: uuid::Uuid::new_v4().to_string(),
                rules: self.rules,
                resolver: None,
            })),
            events: Mutex::new(BTreeMap::new()),
            notices: tokio::sync::broadcast::channel(128).0,
        });
        self.control
            .0
            .set(Arc::downgrade(&route))
            .map_err(|_| HostLifetimeError::InvalidSelection)?;
        let review =
            ApplicationOperationReview::new(self.owner, self.reviewer, self.review_deadline)
                .with_human_consent(route.clone(), self.consent_validity);
        #[cfg(test)]
        let review = match self.observer {
            Some(observer) => review.with_test_observer(observer),
            None => review,
        };
        let (host, owner) = review.compose_installed_with_owner(host)?;
        Ok((host, route, owner))
    }
}
/// Retains no gateway owner. Available only to the trusted selection caller,
/// after its actual route has been composed; no HTTP data can construct it.
#[derive(Clone, Default)]
pub struct ConsoleConsentControl(Arc<OnceLock<Weak<ConsoleConsentRoute>>>);
impl ConsoleConsentControl {
    pub fn route(&self) -> Result<Arc<ConsoleConsentRoute>, HostLifetimeError> {
        self.0
            .get()
            .and_then(Weak::upgrade)
            .ok_or(HostLifetimeError::Unavailable)
    }
}
fn validate_rules(rules: &[ConsoleConsentRule]) -> Result<(), HostLifetimeError> {
    if rules.is_empty()
        || rules
            .iter()
            .enumerate()
            .any(|(i, row)| rules[..i].iter().any(|p| p.target == row.target))
    {
        return Err(HostLifetimeError::InvalidSelection);
    }
    Ok(())
}
struct Selected {
    id: String,
    rules: Vec<ConsoleConsentRule>,
    resolver: Option<ConsoleViewerResolver>,
}
/// Owns explicit route selection. No issuer or verifier is silently replaced.
/// Retire first; a successor must receive its own actual Meta callback.
pub struct ConsoleConsentRoute {
    owner: Weak<ApplicationConsentOwner>,
    binding: ConsoleSetupBinding,
    origin: String,
    selected: Mutex<Option<Selected>>,
    events: Mutex<BTreeMap<String, AcceptedEvent>>,
    notices: tokio::sync::broadcast::Sender<ConsentNotice>,
}
struct AcceptedEvent {
    approver: AuthenticatedApprover,
    approval: ApprovalRecord,
    proof: Arc<Eligible>,
}
struct Eligible {
    route: Arc<ConsoleConsentRoute>,
    human: CurrentConsoleHumanEvidence,
    native: ObservedConsoleConsent,
}
impl ConsentEligibility for Eligible {
    fn check_current(&self) -> Result<(), ConsentIneligible> {
        // Atomic poison/channel state, bounded time and owner stamps only.
        // No lock, allocation, callback, payload borrow or fresh observation.
        if self.route.owner.strong_count() == 0
            || self.route.selected.is_poisoned()
            || self.route.events.is_poisoned()
        {
            return Err(ConsentIneligible);
        }
        self.human.check_current()?;
        self.native.check_current()
    }
}
impl ConsoleConsentRoute {
    fn owner(&self) -> Result<Arc<ApplicationConsentOwner>, HostLifetimeError> {
        self.owner.upgrade().ok_or(HostLifetimeError::Unavailable)
    }
    pub(crate) fn routes(
        self: &Arc<Self>,
        resolver: ConsoleViewerResolver,
    ) -> Result<Router, HostLifetimeError> {
        if !resolver.session_fences().is_durable() {
            return Err(HostLifetimeError::InvalidSelection);
        }
        self.binding.change_consent_selection(|| {
            let mut selected = self
                .selected
                .lock()
                .map_err(|_| HostLifetimeError::Unavailable)?;
            let current = selected.as_mut().ok_or(HostLifetimeError::Denied)?;
            if current.resolver.is_some() {
                return Err(HostLifetimeError::InvalidSelection);
            }
            current.resolver = Some(resolver);
            Ok(())
        })?;
        Ok(Router::new()
            .route("/toolkit/consent/read", post(read))
            .route("/toolkit/consent/decide", post(decide))
            .layer(DefaultBodyLimit::max(64 * 1024))
            .with_state(self.clone()))
    }
    /// Trusted replacement publishes before visibility, even for equal labels.
    pub fn replace_rules(&self, rules: Vec<ConsoleConsentRule>) -> Result<(), HostLifetimeError> {
        validate_rules(&rules)?;
        self.binding.change_consent_selection(|| {
            let mut selected = self
                .selected
                .lock()
                .map_err(|_| HostLifetimeError::Unavailable)?;
            let selected = selected.as_mut().ok_or(HostLifetimeError::Denied)?;
            selected.id = uuid::Uuid::new_v4().to_string();
            selected.rules = rules;
            Ok(())
        })
    }
    /// Required before selecting another issuer, verifier or fence record.
    pub fn retire(&self) -> Result<(), HostLifetimeError> {
        self.binding.change_consent_selection(|| {
            self.selected
                .lock()
                .map_err(|_| HostLifetimeError::Unavailable)?
                .take();
            Ok(())
        })
    }
    /// Exact native notice identity for trusted projection. Lagging readers
    /// reread the authoritative approval; no notice authorizes execution.
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<ConsentNotice> {
        self.notices.subscribe()
    }

    async fn observe(
        self: &Arc<Self>,
        headers: &HeaderMap,
        uri: &Uri,
        approval: &ApprovalRecord,
    ) -> Result<(AuthenticatedApprover, Arc<Eligible>), HostLifetimeError> {
        let (selection_id, rules, scope) = {
            let selected = self
                .selected
                .try_lock()
                .map_err(|_| HostLifetimeError::Unavailable)?;
            let selected = selected.as_ref().ok_or(HostLifetimeError::Denied)?;
            (
                selected.id.clone(),
                selected.rules.clone(),
                selected.scope_for_record(approval)?.clone(),
            )
        };
        // The issuer supplied this exact domain/session. The selected factory
        // preserves durable member identity across resume; reset gets a fresh
        // session. No request target, cached roster or new claim participates.
        let identity = self.binding.consent_session_owner(&scope).await?;
        let rule = rules
            .into_iter()
            .find(|rule| rule.target == identity)
            .ok_or(HostLifetimeError::Denied)?;
        let owner = self.owner()?;
        let ((human, principal), native) = self.binding.observe_consent_event(|| {
            owner.check().map_err(|_| HostLifetimeError::Unavailable)?;
            let selected = self
                .selected
                .try_lock()
                .map_err(|_| HostLifetimeError::Unavailable)?;
            let selected = selected.as_ref().ok_or(HostLifetimeError::Denied)?;
            let resolver = selected
                .resolver
                .as_ref()
                .ok_or(HostLifetimeError::Unavailable)?;
            let viewer = resolver
                .resolve(headers, uri)
                .ok_or(HostLifetimeError::Denied)?;
            if viewer.service_identity {
                return Err(HostLifetimeError::Denied);
            }
            let human = viewer.human.as_ref().ok_or(HostLifetimeError::Denied)?;
            let principal = human.principal().map_err(|_| HostLifetimeError::Denied)?;
            if selected.id != selection_id || selected.scope_for_record(approval)? != &scope {
                return Err(HostLifetimeError::Denied);
            }
            if !rule.approvers.contains(&principal) {
                return Err(HostLifetimeError::Denied);
            }
            // Live access is conjoined, never interpreted as consent eligibility.
            if viewer.access_view.as_ref().is_some_and(|view| {
                view.enforced()
                    && (!view.can_view_agent(rule.target.as_str())
                        || !view.allows_agent(
                            meerkat_mobkit::access::ACTION_AGENT_SEND,
                            rule.target.as_str(),
                        ))
            }) {
                return Err(HostLifetimeError::Denied);
            }
            let current = resolver
                .session_fences()
                .observe_current_human(human)
                .map_err(|_| HostLifetimeError::Denied)?;
            Ok(((current, principal), viewer))
        })?;
        let token_scope = format!(
            "connect-console:{}:{}",
            uuid::Uuid::new_v4(),
            human
                .human()
                .token_digest()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        let approver =
            AuthenticatedApprover::new(principal, ApprovalAssurance::PersonAuthenticated)
                .map_err(|_| HostLifetimeError::Denied)?
                .with_authentication_scope(HostAuthenticationScope::new(token_scope));
        let proof = Arc::new(Eligible {
            route: self.clone(),
            human,
            native,
        });
        proof
            .check_current()
            .map_err(|_| HostLifetimeError::Denied)?;
        Ok((approver, proof))
    }
    async fn decide_event(
        self: &Arc<Self>,
        headers: &HeaderMap,
        uri: &Uri,
        request: Decision,
    ) -> Result<ApprovalRecord, HostLifetimeError> {
        let owner = self.owner()?;
        let record = owner
            .approvals()
            .get(&request.approval_id)
            .map_err(|_| HostLifetimeError::Denied)?;
        let (approver, proof) = self.observe(headers, uri, &record).await?;
        let key = approver
            .authentication_scope()
            .ok_or(HostLifetimeError::Denied)?
            .as_str()
            .to_owned();
        let _event = EventGuard {
            route: self.clone(),
            key: key.clone(),
        };
        self.events
            .lock()
            .map_err(|_| HostLifetimeError::Unavailable)?
            .insert(
                key,
                AcceptedEvent {
                    approver: approver.clone(),
                    approval: record,
                    proof,
                },
            );
        let event = AuthenticatedConsentEvent::new(
            request.approval_id,
            AuthenticatedConsentDecision {
                approver,
                decision: request.decision,
                reason: request.reason,
            },
        );
        self.binding
            .with_current_consent(|| owner.approvals().decide_consent_event(event))?
            .map_err(|_| HostLifetimeError::Denied)
    }
}
struct EventGuard {
    route: Arc<ConsoleConsentRoute>,
    key: String,
}
impl Drop for EventGuard {
    fn drop(&mut self) {
        // Recover poison solely to remove this exact event and break retained
        // custody cycles. No authorization path ever recovers the mutex.
        self.route
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.key);
    }
}
impl Selected {
    fn scope_for_record<'a>(
        &self,
        record: &'a ApprovalRecord,
    ) -> Result<&'a ApprovalConsentScope, HostLifetimeError> {
        let scope = record
            .consent
            .as_ref()
            .and_then(|c| c.scope.as_ref())
            .ok_or(HostLifetimeError::Denied)?;
        let data = record
            .metadata
            .app_context
            .as_ref()
            .ok_or(HostLifetimeError::Denied)?;
        if data
            .get("connect_console_selection")
            .and_then(serde_json::Value::as_str)
            != Some(self.id.as_str())
            || data.get("session")
                != Some(
                    &serde_json::to_value(&scope.session_id)
                        .map_err(|_| HostLifetimeError::Unavailable)?,
                )
        {
            return Err(HostLifetimeError::Denied);
        }
        Ok(scope)
    }
}
impl HumanConsentRoute for ConsoleConsentRoute {
    fn present(
        &self,
        candidate: &ConsentPresentationCandidate<'_>,
    ) -> Result<ConsentPresentation, ConsentRouteUnavailable> {
        self.owner()
            .map_err(|_| ConsentRouteUnavailable)?
            .check()
            .map_err(|_| ConsentRouteUnavailable)?;
        let selected = self
            .selected
            .try_lock()
            .map_err(|_| ConsentRouteUnavailable)?;
        let selected = selected.as_ref().ok_or(ConsentRouteUnavailable)?;
        if selected.resolver.is_none() {
            return Err(ConsentRouteUnavailable);
        }
        let OperationExecutionScope::RuntimeInput {
            owner_session_id, ..
        } = &candidate.facts().execution_scope
        else {
            return Err(ConsentRouteUnavailable);
        };
        Ok(ConsentPresentation {
            requester: ApprovalPrincipalId::new("installed-console-consent")
                .map_err(|_| ConsentRouteUnavailable)?,
            risk: ApprovalRisk::High,
            metadata: meerkat_core::SurfaceMetadata {
                labels: Default::default(),
                // Attribution only. Exact action, arguments and current native
                // authorization remain owned and checked by Core.
                app_context: Some(serde_json::json!({"connect_console_selection": selected.id,
                    "session": owner_session_id})),
            },
        })
    }
    fn eligibility(
        &self,
        approver: &AuthenticatedApprover,
        approval: &ApprovalRecord,
    ) -> Result<Arc<dyn ConsentEligibility>, ConsentIneligible> {
        let key = approver
            .authentication_scope()
            .ok_or(ConsentIneligible)?
            .as_str();
        let events = self.events.try_lock().map_err(|_| ConsentIneligible)?;
        let event = events.get(key).ok_or(ConsentIneligible)?;
        if &event.approval != approval
            || event.approver.principal() != approver.principal()
            || event.approver.assurance() != approver.assurance()
            || event.approver.authentication_scope() != approver.authentication_scope()
        {
            return Err(ConsentIneligible);
        }
        event.proof.check_current()?;
        Ok(event.proof.clone())
    }
    fn notice(&self, notice: ConsentNotice) {
        let _ = self.notices.send(notice);
    }
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Read {
    approval_id: ApprovalId,
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Decision {
    approval_id: ApprovalId,
    decision: ApprovalDecision,
    reason: Option<String>,
}
fn request_allowed(route: &ConsoleConsentRoute, headers: &HeaderMap, uri: &Uri) -> bool {
    uri.query().is_none()
        && headers.get_all("origin").iter().count() == 1
        && headers.get("origin").and_then(|v| v.to_str().ok()) == Some(route.origin.as_str())
        && headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .map(str::trim)
            == Some("application/json")
}
fn response(result: Result<ApprovalRecord, HostLifetimeError>) -> Response {
    let (status, body) = match result {
        Ok(record) => (StatusCode::OK, serde_json::json!({"approval":record})),
        Err(HostLifetimeError::Unavailable) => (
            StatusCode::SERVICE_UNAVAILABLE,
            serde_json::json!({"error":"Consent unavailable"}),
        ),
        Err(_) => (
            StatusCode::FORBIDDEN,
            serde_json::json!({"error":"Consent refused"}),
        ),
    };
    (
        status,
        [
            ("cache-control", "no-store"),
            ("referrer-policy", "no-referrer"),
            ("x-content-type-options", "nosniff"),
        ],
        Json(body),
    )
        .into_response()
}
mod recovery_guidance;

async fn read(
    State(route): State<Arc<ConsoleConsentRoute>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    Json(request): Json<Read>,
) -> Response {
    if !request_allowed(&route, &headers, &uri) {
        return response(Err(HostLifetimeError::Denied));
    }
    recovery_guidance::read_response(
        async {
            let owner = route.owner()?;
            let record = owner
                .approvals()
                .get(&request.approval_id)
                .map_err(|_| HostLifetimeError::Denied)?;
            let (_, proof) = route.observe(&headers, &uri, &record).await?;
            proof
                .check_current()
                .map_err(|_| HostLifetimeError::Denied)?;
            Ok(record)
        }
        .await,
    )
}
async fn decide(
    State(route): State<Arc<ConsoleConsentRoute>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    Json(request): Json<Decision>,
) -> Response {
    if !request_allowed(&route, &headers, &uri)
        || request.reason.as_ref().is_some_and(|s| s.len() > 4096)
        || !matches!(
            request.decision,
            ApprovalDecision::Approve | ApprovalDecision::Deny
        )
    {
        return response(Err(HostLifetimeError::Denied));
    }
    response(route.decide_event(&headers, &uri, request).await)
}
#[cfg(test)]
pub(crate) mod tests;

// The resource fixture needs a real helper session before final host adoption.
// Its early native factory retains one review owner; only presentation is
// deferred until that SAME application's real Console route is composed.
#[cfg(test)]
struct DeferredConsoleRouteForTest(OnceLock<Weak<ConsoleConsentRoute>>);
#[cfg(test)]
impl DeferredConsoleRouteForTest {
    fn actual(&self) -> Result<Arc<ConsoleConsentRoute>, HostLifetimeError> {
        self.0
            .get()
            .and_then(Weak::upgrade)
            .ok_or(HostLifetimeError::Unavailable)
    }
}
#[cfg(test)]
impl HumanConsentRoute for DeferredConsoleRouteForTest {
    fn present(
        &self,
        candidate: &ConsentPresentationCandidate<'_>,
    ) -> Result<ConsentPresentation, ConsentRouteUnavailable> {
        self.actual()
            .map_err(|_| ConsentRouteUnavailable)?
            .present(candidate)
    }
    fn eligibility(
        &self,
        approver: &AuthenticatedApprover,
        approval: &ApprovalRecord,
    ) -> Result<Arc<dyn ConsentEligibility>, ConsentIneligible> {
        self.actual()
            .map_err(|_| ConsentIneligible)?
            .eligibility(approver, approval)
    }
    fn notice(&self, notice: ConsentNotice) {
        if let Ok(actual) = self.actual() {
            actual.notice(notice);
        }
    }
}
#[cfg(test)]
pub(crate) struct PreparedConsoleConsentForTest {
    selection: ConsoleConsentSelection,
    retained: Arc<crate::application_consent::RetainedApplicationOperationReview>,
    deferred: Arc<DeferredConsoleRouteForTest>,
}
#[cfg(test)]
impl ConsoleConsentSelection {
    pub(crate) fn prepare_for_test(
        self,
        prepared: &toolkit_mobkit::installed_host::preparation::PreparedInstallationOwner,
    ) -> Result<PreparedConsoleConsentForTest, HostLifetimeError> {
        let deferred = Arc::new(DeferredConsoleRouteForTest(OnceLock::new()));
        let review = ApplicationOperationReview::new(
            self.owner.clone(),
            self.reviewer.clone(),
            self.review_deadline,
        )
        .with_human_consent(deferred.clone(), self.consent_validity);
        let review = match &self.observer {
            Some(observer) => review.with_test_observer(observer.clone()),
            None => review,
        };
        let retained = Arc::new(
            review
                .bind(prepared)
                .map_err(|_| HostLifetimeError::WrongOwner)?,
        );
        Ok(PreparedConsoleConsentForTest {
            selection: self,
            retained,
            deferred,
        })
    }
}
#[cfg(test)]
impl PreparedConsoleConsentForTest {
    pub(crate) fn review(
        &self,
    ) -> Result<Arc<meerkat_core::approval::review::BoundOperationReview>, HostLifetimeError> {
        self.retained
            .current()
            .map_err(|_| HostLifetimeError::Unavailable)
    }
    pub(crate) fn route(&self) -> Result<Arc<ConsoleConsentRoute>, HostLifetimeError> {
        self.deferred.actual()
    }
    pub(crate) fn compose(
        self,
        host: ToolkitGatewayHost,
    ) -> Result<
        (
            ToolkitGatewayHost,
            Arc<ConsoleConsentRoute>,
            Arc<ApplicationConsentOwner>,
        ),
        HostLifetimeError,
    > {
        let binding = host.console_setup_binding()?;
        let identity = binding.with_consent_host(|actual| {
            ConsentApplicationIdentity::from_installed(actual)
                .map_err(|_| HostLifetimeError::Unavailable)
        })?;
        if &identity != self.selection.owner.identity() {
            return Err(HostLifetimeError::WrongOwner);
        }
        self.review()?;
        let route = Arc::new(ConsoleConsentRoute {
            owner: Arc::downgrade(&self.selection.owner),
            binding,
            origin: self.selection.origin,
            selected: Mutex::new(Some(Selected {
                id: uuid::Uuid::new_v4().to_string(),
                rules: self.selection.rules,
                resolver: None,
            })),
            events: Mutex::new(BTreeMap::new()),
            notices: tokio::sync::broadcast::channel(128).0,
        });
        self.selection
            .control
            .0
            .set(Arc::downgrade(&route))
            .map_err(|_| HostLifetimeError::InvalidSelection)?;
        // Bind the real route only after exact installed identity succeeded.
        // Missing resolver/login/eligibility still refuses in that real route.
        self.deferred
            .0
            .set(Arc::downgrade(&route))
            .map_err(|_| HostLifetimeError::InvalidSelection)?;
        let host = self.retained.compose_same_installed_for_test(host)?;
        Ok((host, route, self.selection.owner))
    }
}
