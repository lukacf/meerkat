//! Same installed review/presenter, mounted under the native host route hook.
//! HTTP values select no principal, task, native account, grant or destination.
use crate::{
    ConnectError,
    approval::{ApprovalPresenter, ApprovalPrompt, ApprovalPromptOutcome},
    installed_service_review::account_request::{
        InstalledAccountRequest, InstalledAccountRequests,
        console::{CallbackDelivery, CallbackResult, DecisionCheck, answer_retained_authorized},
        credentials::transport::{CredentialDelivery, answer_retained_credentials},
    },
    stdio::{Exchange, PromptAnswer, PromptBridge},
};
use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, OriginalUri, Path, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures_util::future::BoxFuture;
use meerkat_mobkit::http_console::{ConsoleViewerContext, ConsoleViewerResolver};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicU64, Ordering},
};
use tokio::sync::{mpsc, oneshot};
use toolkit_mobkit::{
    host_gateway::{
        ConsoleAccessPolicy, ConsolePageRead, ConsoleRecoveryPolicy, ConsoleSetupBinding,
        ConsoleStatusPage,
    },
    private_host::{ConsoleLoginHandle, PrivateHostClosed},
    private_originals::InstalledServiceTask,
};

mod http;

/// Pinned by the existing application acquisition artifact. The callback path
/// is fixed; authority never comes from a request's Host header.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsoleSetupSelection {
    pub origin: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_management: Option<ConsoleAccessPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_management: Option<ConsoleRecoveryPolicy>,
}
impl ConsoleSetupSelection {
    /// The supplying application selects this policy when producing its pinned
    /// Console artifact. It is never inferred from a browser or old artifact.
    pub fn for_application_issued_access(origin: String) -> Self {
        Self {
            origin,
            access_management: Some(ConsoleAccessPolicy::RequesterIssuedLeaves),
            recovery_management: None,
        }
    }
    /// Explicit administrator recovery for the operator's own retained intent.
    /// Existing artifacts and the requester-only constructor remain unchanged.
    pub fn with_operator_service_recovery(mut self) -> Self {
        self.recovery_management = Some(ConsoleRecoveryPolicy::OperatorUnpublishedServiceApprovals);
        self
    }
    pub fn validate(&self) -> Result<(), ConnectError> {
        let url = reqwest::Url::parse(&self.origin).map_err(|_| unavailable())?;
        let loopback = matches!(
            url.host_str(),
            Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
        );
        if url.origin().ascii_serialization() != self.origin
            || !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
            || !(url.scheme() == "https" || url.scheme() == "http" && loopback)
        {
            return Err(unavailable());
        }
        Ok(())
    }
    fn callback(&self) -> String {
        format!("{}/host/toolkit/setup/callback", self.origin)
    }
}

#[derive(Clone)]
pub(crate) struct ConsoleSetup(Arc<Panel>);
struct Panel {
    selection: ConsoleSetupSelection,
    binding: ConsoleSetupBinding,
    viewer: OnceLock<ConsoleViewerResolver>,
    // One open presentation per login: the login that started its request.
    active: Presentations<Active>,
    next: AtomicU64,
    pages: Mutex<Vec<StatusOffer<Arc<ConsoleStatusPage>>>>,
}
// Presentation custody only. Exact login owners, not browser keys or person
// labels, select replacement. At capacity another current owner is never evicted.
const MAX_STATUS_OFFERS: usize = 256;
pub(crate) struct StatusOffer<T> {
    pub(crate) login: ConsoleLoginHandle,
    pub(crate) id: u64,
    pub(crate) value: T,
}
pub(crate) fn replace_status_offer<T>(
    offers: &mut Vec<StatusOffer<T>>,
    login: &ConsoleLoginHandle,
    expected: Option<u64>,
    next: Option<(u64, T)>,
) -> Result<(), ConnectError> {
    // A read selected this exact page before awaiting canonical owners. It may
    // not replace a newer completed read for the same actual login.
    if expected.is_some_and(|id| {
        !offers
            .iter()
            .any(|offer| offer.id == id && offer.login.same_owner(login))
    }) {
        return Err(unavailable());
    }
    use toolkit_mobkit::private_host::ConsoleLoginRefusal;
    offers.retain(|offer| {
        !offer.login.same_owner(login)
            && !matches!(
                offer.login.check(),
                Err(ConsoleLoginRefusal::Retired | ConsoleLoginRefusal::Expired)
            )
    });
    if let Some((id, value)) = next {
        if offers.len() >= MAX_STATUS_OFFERS {
            return Err(unavailable());
        }
        offers.push(StatusOffer {
            login: login.clone(),
            id,
            value,
        });
    }
    Ok(())
}
#[derive(Debug)]
enum StartDisposition {
    Start,
    Obsolete,
}
struct Active {
    id: u64,
    software: bool,
    credential_review: bool,
    task: InstalledServiceTask,
    start: Mutex<Option<oneshot::Sender<StartDisposition>>>,
    prompt: Mutex<Option<Exchange>>,
    callback: Mutex<Option<CallbackDelivery>>,
    credentials: Mutex<Option<CredentialDelivery>>,
    // Exactly the retained carrier of the last accepted action. A GET never
    // writes this slot. No token renewal, global viewer cache or label lookup.
    action: Mutex<Option<Action>>,
}
#[derive(Clone)]
struct Action {
    viewer: ConsoleViewerResolver,
    binding: ConsoleSetupBinding,
    headers: HeaderMap,
    uri: Uri,
    login: ConsoleLoginHandle,
    task: InstalledServiceTask,
    identity: Arc<OnceLock<meerkat_mobkit::identity_first::AgentIdentity>>,
}
impl Action {
    fn current(&self) -> Option<ConsoleViewerContext> {
        let viewer = self.viewer.resolve(&self.headers, &self.uri)?;
        let login = self.binding.current_login(&viewer).ok()?;
        if !self.login.same_owner(&login) {
            return None;
        }
        if self.identity.get().is_some_and(|identity| {
            viewer.access_view.as_ref().is_some_and(|view| {
                view.enforced()
                    && (!view.can_view_agent(identity.as_str())
                        || !view.allows_agent(
                            meerkat_mobkit::access::ACTION_AGENT_SEND,
                            identity.as_str(),
                        ))
            })
        }) {
            return None;
        }
        Some(viewer)
    }
    async fn check(&self) -> Result<(), ConnectError> {
        let identity = self
            .task
            .check_console_viewer(&|| self.current(), &self.login)
            .await
            .map_err(|_| unavailable())?;
        // Bind this action once to the actual current native target. A later
        // check may refuse a changed target but cannot retarget the action.
        if self.identity.get_or_init(|| identity.clone()) != &identity {
            return Err(unavailable());
        }
        self.current().ok_or_else(unavailable)?;
        Ok(())
    }
    fn administration_check(&self) -> Result<crate::installed_service_review::account_request::credentials::administration::AdministrationCheck, ConnectError>{
        // Capture only the already accepted action, before any native wait.
        // The entry below never calls current_login or resolves an enrollment.
        let viewer = self.current().ok_or_else(unavailable)?;
        let human = viewer.human.clone().ok_or_else(unavailable)?;
        let principal = human.principal().map_err(|_| unavailable())?;
        let scope = human.login_scope().ok_or_else(unavailable)?.to_owned();
        let identity = self.identity.get().cloned().ok_or_else(unavailable)?;
        let action = self.clone();
        Ok(Arc::new(move || {
            use meerkat_core::AdministrationEntryRefusal;
            let unavailable = || {
                AdministrationEntryRefusal::Unavailable(
                    "The retained Console credential entry is unavailable".into(),
                )
            };
            action
                .binding
                .with_credential_entry(|| {
                    // This resolver directly rechecks the accepted native carrier and
                    // obtains a fresh view from its same native AccessController. It
                    // does not call Toolkit's blocking login lookup or Action.current.
                    let subject = human
                        .current()
                        .map_err(|_| AdministrationEntryRefusal::NotAuthenticated)?;
                    let viewer = action
                        .viewer
                        .resolve(&action.headers, &action.uri)
                        .ok_or(AdministrationEntryRefusal::NotAuthenticated)?;
                    if viewer.service_identity
                        || viewer.human.as_ref() != Some(&human)
                        || human.principal().ok().as_ref() != Some(&principal)
                        || human.login_scope() != Some(scope.as_str())
                        || viewer.principal.as_ref() != Some(&subject)
                    {
                        return Err(AdministrationEntryRefusal::NotAuthenticated);
                    }
                    let access = viewer
                        .access_view
                        .as_ref()
                        .ok_or(AdministrationEntryRefusal::NotAdministrator)?;
                    if !access.can_administer()
                        || access.enforced()
                            && (!access.can_view_agent(identity.as_str())
                                || !access.allows_agent(
                                    meerkat_mobkit::access::ACTION_AGENT_SEND,
                                    identity.as_str(),
                                ))
                    {
                        return Err(AdministrationEntryRefusal::NotAdministrator);
                    }
                    action.login.try_reserve().map_err(|_| unavailable())
                })
                .map_err(|_| unavailable())?
        }))
    }
    /// The person this accepted answer was made by: the verified human its
    /// retained carrier names now (its current login and access included),
    /// with the login the answer was authenticated in. No current person
    /// answers nothing.
    fn answerer(&self) -> Result<crate::approval::ConsoleAnswerer, ConnectError> {
        let viewer = self.current().ok_or_else(unavailable)?;
        if viewer.service_identity {
            return Err(unavailable());
        }
        let human = viewer.human.as_ref().ok_or_else(unavailable)?;
        let principal = human.principal().map_err(|_| unavailable())?;
        let scope = human.login_scope().ok_or_else(unavailable)?;
        crate::approval::ConsoleAnswerer::new(principal, scope)
    }
    fn decision_check(&self) -> Result<DecisionCheck, ConnectError> {
        let identity = self.identity.get().cloned().ok_or_else(unavailable)?;
        let viewer = self.current().ok_or_else(unavailable)?;
        let human = viewer.human.as_ref().ok_or_else(unavailable)?;
        let principal = human.principal().map_err(|_| unavailable())?;
        let scope = human.login_scope().ok_or_else(unavailable)?.to_owned();
        let action = self.clone();
        Ok(Arc::new(move || {
            // Called under the existing installed owner's mutation guard. Do
            // not reenter that owner or await here. The immutable headers are
            // the accepted answer's carrier, never a later action or token.
            if action.login.check().is_err() {
                return false;
            }
            let Some(viewer) = action.viewer.resolve(&action.headers, &action.uri) else {
                return false;
            };
            let Some(human) = viewer.human.as_ref() else {
                return false;
            };
            !viewer.service_identity
                && human.current().is_ok()
                && human.principal().ok().as_ref() == Some(&principal)
                && human.login_scope() == Some(scope.as_str())
                && !viewer.access_view.as_ref().is_some_and(|view| {
                    view.enforced()
                        && (!view.can_view_agent(identity.as_str())
                            || !view.allows_agent(
                                meerkat_mobkit::access::ACTION_AGENT_SEND,
                                identity.as_str(),
                            ))
                })
        }))
    }
}
impl Active {
    async fn refresh(&self) -> Result<(), ConnectError> {
        let action = self
            .action
            .lock()
            .map_err(|_| unavailable())?
            .clone()
            .ok_or_else(unavailable)?;
        action.check().await
    }
    fn decision_check(&self) -> Result<DecisionCheck, ConnectError> {
        self.action
            .lock()
            .map_err(|_| unavailable())?
            .as_ref()
            .ok_or_else(unavailable)?
            .decision_check()
    }
    /// The person the last accepted answer was made by.
    fn answerer(&self) -> Result<crate::approval::ConsoleAnswerer, ConnectError> {
        self.action
            .lock()
            .map_err(|_| unavailable())?
            .as_ref()
            .ok_or_else(unavailable)?
            .answerer()
    }
    fn administration_check(&self) -> Result<crate::installed_service_review::account_request::credentials::administration::AdministrationCheck, ConnectError>{
        self.action
            .lock()
            .map_err(|_| unavailable())?
            .as_ref()
            .ok_or_else(unavailable)?
            .administration_check()
    }
    fn accept_action(&self, action: Action) -> Result<(), ConnectError> {
        action.current().ok_or_else(unavailable)?;
        if self.software {
            let mut pending = self.start.lock().map_err(|_| unavailable())?;
            let sender = pending.take().ok_or_else(unavailable)?;
            *self.action.lock().map_err(|_| unavailable())? = Some(action);
            sender
                .send(StartDisposition::Start)
                .map_err(|_| unavailable())?;
        } else {
            *self.action.lock().map_err(|_| unavailable())? = Some(action);
        }
        Ok(())
    }
}
impl ConsoleSetup {
    pub(crate) fn new(
        selection: ConsoleSetupSelection,
        binding: ConsoleSetupBinding,
    ) -> Result<Self, ConnectError> {
        selection.validate()?;
        let binding = binding
            .with_access_policy(selection.access_management)
            .with_recovery_policy(selection.recovery_management);
        Ok(Self(Arc::new(Panel {
            selection,
            binding,
            viewer: OnceLock::new(),
            active: Presentations::default(),
            next: AtomicU64::new(1),
            pages: Mutex::new(Vec::new()),
        })))
    }
    pub(crate) fn routes(&self, viewer: ConsoleViewerResolver) -> Result<Router, ConnectError> {
        self.0.viewer.set(viewer).map_err(|_| unavailable())?;
        Ok(http::router(self.clone()))
    }
    /// A read may release obsolete local presentation custody after definite
    /// canonical terminal evidence. It never initiates native account work or
    /// writes a cancellation. Unknown/transient observations leave custody held.
    /// Every login's open presentation is checked, as when one was open.
    async fn reclaim_terminal(&self) -> Result<bool, ConnectError> {
        let mut reclaimed = false;
        for active in self.0.active.all()? {
            reclaimed |= self.reclaim_obsolete(active).await?;
        }
        Ok(reclaimed)
    }
    async fn reclaim_obsolete(&self, active: Arc<Active>) -> Result<bool, ConnectError> {
        if active.start.lock().map_err(|_| unavailable())?.is_none() {
            return Ok(false);
        }
        let terminal = matches!(active.task.console_terminal().await, Ok(true));
        if !terminal {
            return Ok(false);
        }
        // An accepted start wins by consuming this same sender. Never abort
        // or reclaim a driver that has already entered.
        let pending = active.start.lock().map_err(|_| unavailable())?.take();
        if pending.is_none() {
            return Ok(false);
        }
        // Obsolescence ends only this request. It is not presenter failure:
        // the same enrollment can receive a later request from its helper.
        if let Some(pending) = pending {
            let _ = pending.send(StartDisposition::Obsolete);
        }
        self.0.active.release(&active)?;
        Ok(true)
    }
    /// Presents the existing checked-install approval. The prompt's canonical
    /// task was captured from its actual borrowed native dispatch, not HTTP.
    pub(crate) async fn present_software(
        &self,
        mut prompt: ApprovalPrompt,
        closed: &PrivateHostClosed,
    ) -> Result<ApprovalPromptOutcome, ConnectError> {
        // Only failure of shared presentation custody escapes as an owner
        // error. Missing or stale facts of this exact request remain local.
        self.reclaim_terminal().await?;
        let Some(task) = prompt.console_task().cloned() else {
            return Ok(ApprovalPromptOutcome::Refused(unavailable()));
        };
        let Some(login) = task.console_login().cloned() else {
            return Ok(ApprovalPromptOutcome::Refused(unavailable()));
        };
        if login.check().is_err() {
            return Ok(ApprovalPromptOutcome::Refused(unavailable()));
        }
        let id = match self
            .0
            .next
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| {
                (id < 9_007_199_254_740_991).then_some(id + 1)
            }) {
            Ok(id) => id,
            Err(_) => return Ok(ApprovalPromptOutcome::Refused(unavailable())),
        };
        let (pending, obsolete) = oneshot::channel();
        let active = Arc::new(Active {
            id,
            software: true,
            credential_review: false,
            task,
            start: Mutex::new(Some(pending)),
            prompt: Mutex::new(None),
            callback: Mutex::new(None),
            credentials: Mutex::new(None),
            action: Mutex::new(None),
        });
        match self.0.active.claim(&login, active.clone()) {
            Ok(()) => {}
            Err(NotHeld::Taken) => return Ok(ApprovalPromptOutcome::Refused(unavailable())),
            Err(NotHeld::Unavailable) => return Err(unavailable()),
        }
        let result = async {
            let record = prompt.record();
            let allowed = record.allowed_decisions.iter().map(|decision| match decision {
                meerkat_core::ApprovalDecision::Approve => "approve",
                meerkat_core::ApprovalDecision::Deny => "deny",
            }).collect::<Vec<_>>();
            let projection = json!({"kind":"approval","approval_id":record.approval_id,
                "summary":record.proposed_action.summary,"body":record.proposed_action.body,
                "allowed_decisions":allowed,"expires_at":record.expires_at});
            let (bridge, mut prompts, retire) = PromptBridge::channel();
            let answer = bridge.ask(projection);
            tokio::pin!(answer);
            let mut obsolete = Some(obsolete);
            let answer = loop {
                tokio::select! { biased;
                    _ = closed.wait_closed() => { retire.send_replace(true); return Ok(ApprovalPromptOutcome::Stopped); },
                    _ = login.closed() => { retire.send_replace(true); return Ok(ApprovalPromptOutcome::Refused(unavailable())); },
                    _ = prompt.closed() => { retire.send_replace(true); return Ok(ApprovalPromptOutcome::Continue); },
                    result = async { obsolete.as_mut().expect("pending presentation").await }, if obsolete.is_some() => {
                        obsolete = None;
                        match result {
                            Ok(StartDisposition::Start) => {},
                            Ok(StartDisposition::Obsolete) => { retire.send_replace(true); return Ok(ApprovalPromptOutcome::Continue); },
                            Err(_) => return Ok(ApprovalPromptOutcome::Refused(unavailable())),
                        }
                    }
                    result = &mut answer => match result {
                        Ok(answer) => break answer,
                        Err(error) => return Ok(ApprovalPromptOutcome::Refused(error)),
                    },
                    exchange = prompts.recv() => {
                        let Some(exchange) = exchange else { return Ok(ApprovalPromptOutcome::Refused(unavailable())); };
                        let mut held = match active.prompt.lock() {
                            Ok(held) => held,
                            Err(_) => return Ok(ApprovalPromptOutcome::Refused(unavailable())),
                        };
                        if held.is_some() { return Ok(ApprovalPromptOutcome::Refused(unavailable())); }
                        *held = Some(exchange);
                    }
                }
            };
            // HTTP installed this exact accepted answer's carrier before it
            // woke the retained exchange. Capture it once across the await.
            let action = match active.action.lock() {
                Ok(action) => match action.clone() {
                    Some(action) => action,
                    None => return Ok(ApprovalPromptOutcome::Refused(unavailable())),
                },
                Err(_) => return Ok(ApprovalPromptOutcome::Refused(unavailable())),
            };
            if let Err(error) = action.check().await { return Ok(ApprovalPromptOutcome::Refused(error)); }
            let current = match action.decision_check() {
                Ok(current) => current,
                Err(error) => return Ok(ApprovalPromptOutcome::Refused(error)),
            };
            // The person the accepted answer's verified carrier names, read
            // after the last await. No current person decides nothing.
            let answerer = match action.answerer() {
                Ok(answerer) => answerer,
                Err(error) => return Ok(ApprovalPromptOutcome::Refused(error)),
            };
            let PromptAnswer::Approval { decision } = answer else { return Ok(ApprovalPromptOutcome::Refused(unavailable())); };
            if let Err(error) = prompt.decide_console(match decision {
                crate::stdio::wire::Decision::Approve => meerkat_core::ApprovalDecision::Approve,
                crate::stdio::wire::Decision::Deny => meerkat_core::ApprovalDecision::Deny,
            }, current, &answerer) {
                // A native write error can follow a recorded decision. Keep
                // its existing owner and never claim that no effect occurred.
                return Ok(ApprovalPromptOutcome::Refused(error));
            }
            // The existing Attempt now retains the decision predicate before
            // waking dispatch. Its own reservation spans physical settlement;
            // finishing this presentation does not claim installation success.
            Ok(ApprovalPromptOutcome::Continue)
        }.await;
        // Release only this presentation. Shared custody failure is visible
        // and cannot be converted into a request-local success or retry.
        match self.0.active.release(&active) {
            Ok(()) => result,
            Err(_) => match result {
                Ok(ApprovalPromptOutcome::Refused(primary)) | Err(primary) => {
                    Err(ConnectError::Cleanup {
                        primary: Box::new(primary),
                        cleanup: Box::new(unavailable()),
                    })
                }
                _ => Err(unavailable()),
            },
        }
    }
    pub(crate) async fn present(
        &self,
        accounts: &mut InstalledAccountRequests,
        request: InstalledAccountRequest,
        presenter: &mut Option<ApprovalPresenter>,
        closed: &PrivateHostClosed,
    ) -> Result<bool, ConnectError> {
        self.present_request(
            accounts,
            crate::installed_service_review::account_request::InstalledRequest::Account(Box::new(
                request,
            )),
            presenter,
            closed,
        )
        .await
    }

    pub(crate) async fn present_public(
        &self,
        accounts: &mut InstalledAccountRequests,
        request: crate::installed_service_review::account_request::public::InstalledPublicRequest,
        presenter: &mut Option<ApprovalPresenter>,
        closed: &PrivateHostClosed,
    ) -> Result<bool, ConnectError> {
        self.present_request(
            accounts,
            crate::installed_service_review::account_request::InstalledRequest::Public(request),
            presenter,
            closed,
        )
        .await
    }

    pub(crate) async fn present_credentials(
        &self,
        accounts: &mut InstalledAccountRequests,
        request: crate::installed_service_review::account_request::credentials::InstalledCredentialRequest,
        presenter: &mut Option<ApprovalPresenter>,
        closed: &PrivateHostClosed,
    ) -> Result<bool, ConnectError> {
        self.present_request(
            accounts,
            crate::installed_service_review::account_request::InstalledRequest::Credentials(
                Box::new(request),
            ),
            presenter,
            closed,
        )
        .await
    }

    async fn present_request(
        &self,
        accounts: &mut InstalledAccountRequests,
        mut request: crate::installed_service_review::account_request::InstalledRequest,
        presenter: &mut Option<ApprovalPresenter>,
        closed: &PrivateHostClosed,
    ) -> Result<bool, ConnectError> {
        self.reclaim_terminal().await?;
        let task = request.console_task().cloned().ok_or_else(unavailable)?;
        let login = task.console_login().cloned().ok_or_else(unavailable)?;
        login.check().map_err(|_| unavailable())?;
        let id = self
            .0
            .next
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| {
                (id < 9_007_199_254_740_991).then_some(id + 1)
            })
            .map_err(|_| unavailable())?;
        let (credentials, mut values) = match &request {
            crate::installed_service_review::account_request::InstalledRequest::Credentials(
                request,
            ) => {
                let (delivery, receive) = request.credential_delivery()?;
                (Some(delivery), Some(receive))
            }
            _ => (None, None),
        };
        let (start, started) = oneshot::channel();
        let active = Arc::new(Active {
            id,
            software: false,
            credential_review: credentials.is_some(),
            task,
            start: Mutex::new(Some(start)),
            prompt: Mutex::new(None),
            callback: Mutex::new(None),
            credentials: Mutex::new(credentials),
            action: Mutex::new(None),
        });
        self.0
            .active
            .claim(&login, active.clone())
            .map_err(|_| unavailable())?;
        // The sole presenter retains this request. An explicit authenticated
        // POST enters it; GET only projects the already retained request.
        let result = async {
            let closing = async { tokio::select! { biased;
                _ = login.closed() => {}, _ = closed.wait_closed() => {},
            }};
            if !wait_for_start(started, request.closed(), closing).await? { return Ok(true); }
            active.refresh().await?;
            let (bridge, mut prompts, retire) = PromptBridge::channel();
            let (callbacks, mut receive_callback) = mpsc::channel(1);
            let refresh = || -> BoxFuture<'static, Result<(), ConnectError>> {
                let active = active.clone(); Box::pin(async move { active.refresh().await })
            };
            let decision = || active.decision_check();
            let administration = || active.administration_check();
            let answerer = || active.answerer();
            let driver: BoxFuture<'_, Result<bool, ConnectError>> = match request {
                crate::installed_service_review::account_request::InstalledRequest::Account(request) =>
                    Box::pin(accounts.present_console(*request, presenter, bridge, callbacks,
                        self.0.selection.callback(), &refresh, &decision, &answerer, closed.wait_closed())),
                crate::installed_service_review::account_request::InstalledRequest::Credentials(request) => Box::pin(async {
                    let _unused_callbacks = callbacks;
                    accounts.present_credentials_console(*request, presenter, bridge,
                        values.take().ok_or_else(unavailable)?, &refresh, &decision, &administration,
                        &answerer, closed.wait_closed()).await
                }),
                crate::installed_service_review::account_request::InstalledRequest::Public(request) => Box::pin(async {
                    // Keep the shared presentation loop's callback sender alive;
                    // this typed branch never produces a callback or starts auth.
                    let _unused_callbacks = callbacks;
                    accounts.present_public_console(request, presenter, bridge,
                        &refresh, &decision, &answerer, closed.wait_closed()).await
                }),
            };
            tokio::pin!(driver);
            let mut owner_close = None;
            loop {
                tokio::select! { biased;
                    result = &mut driver => break result,
                    prompt = prompts.recv() => {
                        let Some(mut prompt) = prompt else { retire.send_replace(true); break driver.await; };
                        owner_close = prompt.take_owner_close();
                        let occupied = {
                            match active.prompt.lock() {
                                Ok(mut slot) if slot.is_none() => { *slot = Some(prompt); false },
                                _ => true,
                            }
                        };
                        if occupied { retire.send_replace(true); let _ = driver.await; break Err(unavailable()); }
                    }
                    callback = receive_callback.recv() => {
                        let Some(callback) = callback else { retire.send_replace(true); break driver.await; };
                        let occupied = {
                            match active.callback.lock() {
                                Ok(mut slot) if slot.is_none() => { *slot = Some(callback); false },
                                _ => true,
                            }
                        };
                        if occupied { retire.send_replace(true); let _ = driver.await; break Err(unavailable()); }
                    }
                    result = async { match &mut owner_close { Some(close) => close.await,
                        None => std::future::pending().await } }, if owner_close.is_some() => {
                        owner_close = None;
                        let cleared = match active.prompt.lock() {
                            Ok(mut slot) => { slot.take(); true }, Err(_) => false,
                        };
                        if let Ok((_, ack)) = result { let _ = ack.send(()); }
                        if !cleared { retire.send_replace(true); let _ = driver.await; break Err(unavailable()); }
                    }
                }
            }
        }.await;
        // Always retire transport, and release only this actual owner's slot.
        // present_console itself joins entered native account work on close.
        let _ = self.0.active.release(&active);
        result
    }
}

/// What one presentation returned, by the kind of request it presented.
enum Presented {
    /// An account, credential or public service request.
    Request(Result<bool, ConnectError>),
    /// A software approval.
    Software(Result<ApprovalPromptOutcome, ConnectError>),
}

type Presentation = BoxFuture<
    'static,
    (
        Option<ConsoleLoginHandle>,
        super::setup_presenter::Enrollment,
        Presented,
    ),
>;

/// The login a setup request belongs to: the Console login of the task that
/// started it. `None` for a request no Console login started, which the
/// panel refuses at once.
fn request_login(event: &super::setup_presenter::SetupEvent) -> Option<ConsoleLoginHandle> {
    use super::setup_presenter::SetupEvent;
    match event {
        SetupEvent::Approval(prompt) => prompt.console_task(),
        SetupEvent::Account(request) => request.console_task(),
        SetupEvent::Public(request) => request.console_task(),
        SetupEvent::Credentials(request) => request.console_task(),
    }
    .and_then(|task| task.console_login())
    .cloned()
}

/// Present one setup request on its enrollment's panel, keeping the
/// enrollment with the result.
fn present_one(
    login: Option<ConsoleLoginHandle>,
    mut enrollment: super::setup_presenter::Enrollment,
    event: super::setup_presenter::SetupEvent,
    closed: PrivateHostClosed,
) -> Presentation {
    use super::setup_presenter::SetupEvent;
    Box::pin(async move {
        let presented = match event {
            SetupEvent::Account(request) => Presented::Request(
                async {
                    let panel = enrollment.console().ok_or_else(unavailable)?;
                    panel
                        .present(
                            enrollment.accounts.as_mut().ok_or_else(unavailable)?,
                            *request,
                            &mut enrollment.presenter,
                            &closed,
                        )
                        .await
                }
                .await,
            ),
            SetupEvent::Credentials(request) => Presented::Request(
                async {
                    let panel = enrollment.console().ok_or_else(unavailable)?;
                    panel
                        .present_credentials(
                            enrollment.accounts.as_mut().ok_or_else(unavailable)?,
                            *request,
                            &mut enrollment.presenter,
                            &closed,
                        )
                        .await
                }
                .await,
            ),
            SetupEvent::Public(request) => Presented::Request(
                async {
                    let panel = enrollment.console().ok_or_else(unavailable)?;
                    panel
                        .present_public(
                            enrollment.accounts.as_mut().ok_or_else(unavailable)?,
                            request,
                            &mut enrollment.presenter,
                            &closed,
                        )
                        .await
                }
                .await,
            ),
            SetupEvent::Approval(prompt) => Presented::Software(match enrollment.console() {
                Some(panel) => panel.present_software(*prompt, &closed).await,
                None => Ok(ApprovalPromptOutcome::Refused(unavailable())),
            }),
        };
        (login, enrollment, presented)
    })
}

/// Present the Console's setup requests: every login's at once, and one
/// login's one at a time, in the order they arrived. An enrollment (one
/// agent session's setup owner) yields its next request only after its
/// current one is presented, so requests presented at once come from
/// different enrollments. Host closure, a presentation that reports the host
/// stopped and a shared custody failure each end the intake: waiting
/// requests retire with their enrollments, and every running presentation
/// is joined, never dropped (on closure each observes it and ends).
pub async fn serve_console_setup(
    reviews: super::LocalSetupReviews,
    closed: PrivateHostClosed,
) -> Result<(), ConnectError> {
    serve_setup_presentations(reviews, closed, request_login, present_one).await
}

/// [`serve_console_setup`] with its two per-request hooks passed in: the
/// login a request belongs to (`login_of`) and the presentation of one
/// request (`present`).
async fn serve_setup_presentations(
    reviews: super::LocalSetupReviews,
    closed: PrivateHostClosed,
    login_of: impl Fn(&super::setup_presenter::SetupEvent) -> Option<ConsoleLoginHandle>,
    present: impl Fn(
        Option<ConsoleLoginHandle>,
        super::setup_presenter::Enrollment,
        super::setup_presenter::SetupEvent,
        PrivateHostClosed,
    ) -> Presentation,
) -> Result<(), ConnectError> {
    use futures_util::StreamExt as _;
    enum Step {
        Closed,
        Next(
            Option<(
                super::setup_presenter::Enrollment,
                super::setup_presenter::SetupEvent,
            )>,
        ),
        Finished(
            Option<(
                Option<ConsoleLoginHandle>,
                super::setup_presenter::Enrollment,
                Presented,
            )>,
        ),
    }
    let mut presenters = Some(super::setup_presenter::SetupPresenters::new(reviews));
    let mut turns = LoginTurns::default();
    let mut running = futures_util::stream::FuturesUnordered::<Presentation>::new();
    // No enrollment is waiting for its next request now: those presenting
    // return to the presenters when they finish.
    let mut idle = false;
    let mut outcome = Ok(());
    loop {
        if presenters.is_none() && running.is_empty() {
            return outcome;
        }
        let step = tokio::select! { biased;
            _ = closed.wait_closed(), if presenters.is_some() => Step::Closed,
            finished = running.next(), if !running.is_empty() => Step::Finished(finished),
            next = async {
                match presenters.as_mut() {
                    Some(presenters) => presenters.next().await,
                    None => std::future::pending().await,
                }
            }, if presenters.is_some() && !idle => Step::Next(next),
        };
        match step {
            Step::Closed => {
                presenters = None;
                turns.clear();
            }
            Step::Next(None) if running.is_empty() => {
                presenters = None;
                turns.clear();
            }
            Step::Next(None) => idle = true,
            Step::Next(Some((enrollment, event))) => {
                let login = login_of(&event);
                if let Some((enrollment, event)) = turns.offer(login.clone(), (enrollment, event)) {
                    running.push(present(login, enrollment, event, closed.clone()));
                }
            }
            Step::Finished(None) => {}
            Step::Finished(Some((login, enrollment, presented))) => {
                idle = false;
                let next = turns.finished(login.as_ref());
                // Request failure must not retire this enrollment or another
                // helper. Host closure still ends the intake.
                let finished = match presented {
                    Presented::Request(result) => {
                        super::finish_account_presentation(presenters.as_mut(), enrollment, result)
                            .map(|_| true)
                    }
                    Presented::Software(result) => {
                        super::finish_software_presentation(presenters.as_mut(), enrollment, result)
                    }
                };
                match finished {
                    Ok(true) => {}
                    Ok(false) => {
                        presenters = None;
                        turns.clear();
                    }
                    Err(error) => {
                        if outcome.is_ok() {
                            outcome = Err(error);
                        }
                        presenters = None;
                        turns.clear();
                    }
                }
                if presenters.is_some()
                    && let Some((login, (enrollment, event))) = next
                {
                    running.push(present(login, enrollment, event, closed.clone()));
                }
            }
        }
    }
}
fn unavailable() -> ConnectError {
    ConnectError::InvalidInput("The protected Console setup interaction is unavailable".into())
}

/// The presentations a panel holds, each with the exact login owner that
/// started the request it shows. Presentation custody only: holding one is
/// never a grant or a decision. A login holds one presentation at a time,
/// and logins hold theirs at once: one person's open prompt never holds back
/// another login's.
pub(crate) struct Presentations<T> {
    held: Mutex<Vec<(ConsoleLoginHandle, Arc<T>)>>,
}

/// Why a presentation was not held.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum NotHeld {
    /// The login holds another presentation.
    Taken,
    /// The panel's presentation custody is unavailable.
    Unavailable,
}

impl<T> Default for Presentations<T> {
    fn default() -> Self {
        Self {
            held: Mutex::new(Vec::new()),
        }
    }
}

impl<T> Presentations<T> {
    /// Hold `value` for `login`; refused while `login` holds another.
    pub(crate) fn claim(&self, login: &ConsoleLoginHandle, value: Arc<T>) -> Result<(), NotHeld> {
        let mut held = self.held.lock().map_err(|_| NotHeld::Unavailable)?;
        if held.iter().any(|(owner, _)| owner.same_owner(login)) {
            return Err(NotHeld::Taken);
        }
        held.push((login.clone(), value));
        Ok(())
    }

    /// The presentation `login` holds, if any.
    pub(crate) fn of(&self, login: &ConsoleLoginHandle) -> Result<Option<Arc<T>>, ConnectError> {
        Ok(self
            .held
            .lock()
            .map_err(|_| unavailable())?
            .iter()
            .find(|(owner, _)| owner.same_owner(login))
            .map(|(_, value)| value.clone()))
    }

    /// Every presentation held now.
    pub(crate) fn all(&self) -> Result<Vec<Arc<T>>, ConnectError> {
        Ok(self
            .held
            .lock()
            .map_err(|_| unavailable())?
            .iter()
            .map(|(_, value)| value.clone())
            .collect())
    }

    /// Release exactly `value`, if it is still held.
    pub(crate) fn release(&self, value: &Arc<T>) -> Result<(), ConnectError> {
        self.held
            .lock()
            .map_err(|_| unavailable())?
            .retain(|(_, held)| !Arc::ptr_eq(held, value));
        Ok(())
    }
}

/// The Console presenter loop's turns over requests and the login each one
/// belongs to: one presentation per login at a time, and every login's at
/// once. A request whose login has one running waits for that login's turn,
/// in the order it arrived. A request without a login is refused at once, so
/// it never waits.
pub(crate) struct LoginTurns<T> {
    running: Vec<Option<ConsoleLoginHandle>>,
    waiting: std::collections::VecDeque<(Option<ConsoleLoginHandle>, T)>,
}

impl<T> Default for LoginTurns<T> {
    fn default() -> Self {
        Self {
            running: Vec::new(),
            waiting: std::collections::VecDeque::new(),
        }
    }
}

impl<T> LoginTurns<T> {
    /// A request of `login` arrived: it starts now (returned) or waits for
    /// its turn.
    pub(crate) fn offer(&mut self, login: Option<ConsoleLoginHandle>, request: T) -> Option<T> {
        let busy = login.as_ref().is_some_and(|login| {
            self.running
                .iter()
                .any(|running| same_login(running.as_ref(), Some(login)))
        });
        if busy {
            self.waiting.push_back((login, request));
            return None;
        }
        self.running.push(login);
        Some(request)
    }

    /// A presentation of `login` ended. Returns the waiting request whose
    /// turn came, if any; it starts now.
    pub(crate) fn finished(
        &mut self,
        login: Option<&ConsoleLoginHandle>,
    ) -> Option<(Option<ConsoleLoginHandle>, T)> {
        if let Some(index) = self
            .running
            .iter()
            .position(|running| same_login(running.as_ref(), login))
        {
            self.running.remove(index);
        }
        // Only this login's turn came: a request waits only for its own login.
        let login = login?;
        let index = self
            .waiting
            .iter()
            .position(|(waiting, _)| same_login(waiting.as_ref(), Some(login)))?;
        let (login, request) = self.waiting.remove(index)?;
        self.running.push(login.clone());
        Some((login, request))
    }

    /// Drop every waiting request.
    pub(crate) fn clear(&mut self) {
        self.waiting.clear();
    }
}

fn same_login(left: Option<&ConsoleLoginHandle>, right: Option<&ConsoleLoginHandle>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left.same_owner(right),
        (None, None) => true,
        _ => false,
    }
}

async fn wait_for_start(
    started: oneshot::Receiver<StartDisposition>,
    request_closed: impl std::future::Future<Output = ()>,
    closed: impl std::future::Future<Output = ()>,
) -> Result<bool, ConnectError> {
    tokio::select! { biased;
        _ = closed => Err(unavailable()),
        _ = request_closed => Ok(false),
        result = started => result.map(|outcome| matches!(outcome, StartDisposition::Start)).map_err(|_| unavailable()),
    }
}

#[cfg(test)]
mod tests;
