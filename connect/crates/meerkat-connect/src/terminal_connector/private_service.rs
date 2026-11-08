//! Explicit protocol-5 presentation for checked Services/ForgeServices jobs.
//! This module owns neither credentials nor installation/authentication truth.
use super::*;
use crate::stdio::PromptBridge;
use meerkat_core::{AdministrationEntry, AdministrationEntryRefusal, EnteredAdministration};

/// The original private host's admission survives the presentation-only close
/// fence. External logout/expiry/close/output loss retires the same watch. No
/// browser answer can replace its deadline, login or realm selection.
#[derive(Clone)]
pub(crate) struct Admission {
    probe_lifetime: Option<Arc<crate::service_probe::ProbeLifetime>>,
    expires_at_ms: u64,
    retired: watch::Receiver<bool>,
}
impl Admission {
    pub(crate) fn new(expires_at_ms: u64, retired: watch::Receiver<bool>) -> Self {
        Self {
            expires_at_ms,
            retired,
            probe_lifetime: None,
        }
    }
    #[cfg(test)]
    pub(crate) fn with_probe(
        mut self,
        selection: Option<&Arc<crate::service_probe::ServiceProbeSelection>>,
    ) -> Self {
        self.probe_lifetime = selection.map(|selection| selection.lifetime.clone());
        self
    }
    pub(crate) fn with_host(
        mut self,
        host: Option<&Arc<crate::service_probe::ServiceHost>>,
    ) -> Self {
        self.probe_lifetime = host.map(|host| host.lifetime.clone());
        self
    }
    pub(super) fn check(&self) -> Result<(), ConnectError> {
        self.enter()
            .map(|_| ())
            .map_err(|_| unavailable("The original private viewer is no longer current"))
    }
    pub(super) async fn closed(&self) {
        let mut retired = self.retired.clone();
        loop {
            if self.check().is_err() {
                return;
            }
            // Bounded observations avoid overflowing Instant for a large wire
            // deadline. The original absolute expiry is never extended.
            let delay = self.expires_at_ms.saturating_sub(now_ms()).min(60_000);
            let native_retirement = async {
                match &self.probe_lifetime {
                    Some(owner) => owner.closed(self.expires_at_ms).await,
                    None => std::future::pending::<()>().await,
                }
            };
            tokio::select! {
                _=native_retirement=>return,
                changed = retired.changed() => { if changed.is_err() { return; } },
                _ = tokio::time::sleep(Duration::from_millis(delay)) => (),
            }
        }
    }
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or(u64::MAX)
}
impl AdministrationEntry for Admission {
    fn enter(&self) -> Result<EnteredAdministration, AdministrationEntryRefusal> {
        if *self.retired.borrow()
            || self.retired.has_changed().is_err()
            || now_ms() >= self.expires_at_ms
            || self
                .probe_lifetime
                .as_ref()
                .is_some_and(|owner| !owner.current(self.expires_at_ms))
        {
            return Err(AdministrationEntryRefusal::NotAuthenticated);
        }
        Ok(EnteredAdministration::admitted())
    }
}

/// Existing ApprovalPresenter still decides the actual LocalApprovals request.
/// Only the transport of that protected request differs from the terminal.
pub(super) async fn review_transport<W: Write, I: Future<Output = io::Result<()>>>(
    approvals: &LocalApprovals,
    terminal: Option<(&mut ApprovalPresenter, &mut TerminalInput)>,
    output: &mut W,
    request: ApprovalRequest,
    caller: &mut oneshot::Sender<Value>,
    mut interrupt: Pin<&mut I>,
) -> Result<ReviewOutcome, ConnectError> {
    match terminal {
        Some((presenter, input)) => {
            review(
                approvals, presenter, input, output, request, caller, interrupt,
            )
            .await
        }
        None => tokio::select! { biased;
            _ = caller.closed() => Ok(ReviewOutcome::CallerGone),
            stopped = interrupt.as_mut() => {
                stopped.map_err(|error| ConnectError::native("Listen for private access cancellation", error))?;
                Ok(ReviewOutcome::Stopped)
            },
            decision = approvals.review_access(request) => Ok(ReviewOutcome::Decided(Box::new(decision?))),
        },
    }
}

/// Accept one fully bound native composite at its first poll. No await or
/// alternate selection occurs between this currentness check and native entry.
/// Once accepted, joined_effect retains this exact future across cancellation.
pub(super) async fn accepted_effect<I, F, T>(
    interrupt: Pin<&mut I>,
    stopped: &mut Option<secrets::Stop>,
    admission: &Admission,
    effect: impl FnOnce() -> F,
) -> Option<Result<T, ConnectError>>
where
    I: Future<Output = io::Result<()>>,
    F: Future<Output = T>,
{
    secrets::joined_effect(interrupt, stopped, || async {
        admission.check()?;
        Ok(effect().await)
    })
    .await
}

pub(crate) const PRIVATE_TOOLS: &[&str] = &[
    PLAIN_TOOL,
    SECRET_TOOL,
    TOOL,
    INSTALLED_LIST_TOOL,
    INSTALLED_SELECT_TOOL,
];
struct PrivateRequestTool(RequestTool);
#[async_trait]
impl AgentToolDispatcher for PrivateRequestTool {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.0.tools().iter().filter(|tool| PRIVATE_TOOLS.contains(&tool.name.as_str())).map(|tool| {
            let mut tool = (**tool).clone();
            tool.description = tool.description.replace("protected terminal", "protected host");
            if tool.name.as_str() == TOOL {
                tool.description = "Ask the protected host to review access for an already native-authenticated account and this exact checked installed job. If sign-in is required, the host's separate installation-bound connector login must finish first. A named profile must already exist in the selected native realm; transient initialization profiles do not satisfy finalization. Supply only the publisher's exact descriptor, operation requirements, agent and explicit lifetime. No credential, command, grant or readiness assertion is accepted. The final result arrives only after setup closes.".into();
                tool.input_schema["properties"]["connector"]["properties"]["client_auth_profile"]["description"] = json!("Existing native realm profile ID, or omit for the native public-client default. No profile settings or secrets belong in model arguments.");
            }
            Arc::new(tool)
        }).collect::<Vec<_>>().into()
    }
    fn tool_catalog_capabilities(&self) -> meerkat_core::ToolCatalogCapabilities {
        self.0.tool_catalog_capabilities()
    }
    fn tool_mutation_class(&self, name: &str) -> ToolMutationClass {
        if PRIVATE_TOOLS.contains(&name) {
            self.0.tool_mutation_class(name)
        } else {
            ToolMutationClass::Unknown
        }
    }
    async fn dispatch(&self, _: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        Err(refused())
    }
    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        if !PRIVATE_TOOLS.contains(&call.name) {
            return Err(refused());
        }
        self.0.dispatch_with_context(call, context).await
    }
}
impl TerminalConnectors {
    pub(crate) fn private_control(&self) -> Control {
        Control::new(self.auth.clone())
    }
    pub(crate) fn private_services_dispatcher_factory(
        &self,
    ) -> crate::service_preparation::TerminalToolFactory {
        let storage = self.storage.clone();
        let requests = self.send.clone();
        Arc::new(move |installs| {
            Arc::new(PrivateRequestTool(RequestTool {
                installs,
                storage: storage.clone(),
                requests: requests.clone(),
            }))
        })
    }
    pub(crate) async fn present_private_services(
        &mut self,
        request: ConnectorRequest,
        approvals: &LocalApprovals,
        bridge: &PromptBridge,
        admission: Admission,
    ) -> Result<Option<plain::PlainHandoff>, ConnectError> {
        admission.check()?;
        let mut handoff = None;
        let stop = admission.clone();
        let interrupt = async {
            tokio::select! { _ = bridge.closed() => (), _ = stop.closed() => () };
            Ok(())
        };
        match request {
            ConnectorRequest::Plain(request) => {
                plain::present_private_admitted(
                    self,
                    *request,
                    approvals,
                    interrupt,
                    &mut handoff,
                    admission,
                )
                .await?;
            }
            ConnectorRequest::Secret(request) => {
                secrets::present_private(
                    self,
                    *request,
                    approvals,
                    bridge,
                    admission,
                    &mut handoff,
                )
                .await?;
            }
            ConnectorRequest::Account(request) => {
                tokio::pin!(interrupt);
                self.present_private_account(
                    *request,
                    approvals,
                    admission,
                    interrupt.as_mut(),
                    &mut handoff,
                )
                .await?;
            }
            ConnectorRequest::Installed(request) => {
                installed::present(self, request, approvals, interrupt).await?;
            }
            _ => {
                return Err(unavailable(
                    "This private protocol does not supply service management",
                ));
            }
        }
        if handoff.is_some() {
            self.requests.close();
        }
        Ok(handoff)
    }

    async fn present_private_account<I: Future<Output = io::Result<()>>>(
        &mut self,
        request: AccountRequest,
        approvals: &LocalApprovals,
        admission: Admission,
        mut interrupt: Pin<&mut I>,
        handoff: &mut Option<plain::PlainHandoff>,
    ) -> Result<bool, ConnectError> {
        let mut output = io::sink();
        if request.response.is_closed() {
            return Ok(true);
        }
        if self.reviewed.is_some() {
            let _ = request.response.send(json!({"state":"pending", "reason":"reviewed_finalization_waiting_for_setup_close", "grant_issued":false}));
            return Ok(true);
        }
        if self.target_unresolved(&request.installed.server_name) {
            return selection_refused(
                "prior_finalization_unresolved",
                request.response,
                &mut output,
            );
        }
        admission.check()?;
        approvals.check_session(&request.installed.session)?;
        if let Err(reason) = request.selection.check_target(&request.target) {
            return selection_refused(reason, request.response, &mut output);
        }
        // The post-close native finalizer rereads this actual realm source.
        // A transient initializer profile cannot satisfy that later check.
        let profile = match profiles::selected(&self.profiles, &request.target).await {
            Ok(profile) => profile,
            Err(error) => return error.respond(request.response, &mut output),
        };
        admission.check()?;
        if request.target.client_auth_profile.is_some() != profile.is_some() {
            return selection_refused(
                "realm_client_profile_required",
                request.response,
                &mut output,
            );
        }
        let status = self
            .auth
            .facade
            .connector_status(&request.target.slot)
            .await
            .map_err(|_| unavailable("The native connector account observation is unavailable"))?;
        admission.check()?;
        approvals.check_session(&request.installed.session)?;
        // Existing connect/connector/login/* owns any required ceremony. This
        // presenter never starts a second login, copies a token or promotes a
        // native login observation into permission/readiness.
        if status.phase != meerkat::ConnectorAuthPhase::Authorized {
            return selection_refused("native_sign_in_required", request.response, &mut output);
        }
        let installed = match request.selection.join_authorized(&request.target, &status) {
            Ok(value) => value,
            Err(reason) => return selection_refused(reason, request.response, &mut output),
        };
        let account = match crate::connector_install::authorized_account(&request.target, &status) {
            Ok(value) => value.clone(),
            Err(reason) => return selection_refused(reason, request.response, &mut output),
        };
        let config = match reviewed_installation_configuration(&installed) {
            Ok(value) => value,
            Err(reason) => return selection_refused(reason, request.response, &mut output),
        };
        let selected = request.selection;
        let guard = admission.clone();
        let session = request.installed.session.clone();
        self.review_account_common(
            AccountReview {
                installed: request.installed,
                document: request.document,
                target: request.target,
                account,
                config,
                grant: request.grant,
                response: request.response,
                login_completed: false,
                client_profile: profile,
            },
            approvals,
            None,
            &mut output,
            || {
                guard.check().map_err(|_| "private_admission_retired")?;
                approvals
                    .check_session(&session)
                    .map_err(|_| "private_approval_retired")?;
                selected.check_current()
            },
            interrupt.as_mut(),
            Some(admission),
            Some(handoff),
        )
        .await
    }
}

#[cfg(test)]
mod tests;
