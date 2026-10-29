//! Product-managed private-pipe projection of one native setup conversation.
//! This retains request/prompt custody; the native owners retain all execution,
//! completion, decisions and persistence. No browser value is a policy grant.
use crate::{
    ConnectError, ConnectSession, RealmOptions, SessionOptions, SessionStartup,
    approval::{ApprovalPresenter, LocalApprovals},
    provider_setup::SignInDisposition,
    service_preparation::LazySetup,
};
use futures_util::{FutureExt, StreamExt, future::BoxFuture, stream::FuturesUnordered};
use meerkat_core::{AgentEvent, ApprovalDecision, ApprovalPrincipalId};
use meerkat_runtime::{
    completion::{CompletionObservation, CompletionOutcome, CompletionWaitError},
    input_state::InputTerminalOutcome,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt},
    sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot, watch},
};
#[path = "service_stdio.rs"]
mod services;
#[path = "stdio_io.rs"]
mod transport;
#[path = "stdio_wire.rs"]
pub(crate) mod wire;
use wire::*;
pub(crate) use wire::{
    Answer as PromptAnswer, Choice as RecoveryChoice, PairingMethod, ProviderAction,
};

fn ended() -> ConnectError {
    ConnectError::InvalidInput("The protected private-pipe interaction ended".into())
}
pub(crate) struct Exchange {
    pub(crate) prompt: Value,
    pub(crate) response: oneshot::Sender<Result<PromptAnswer, ()>>,
    owner_close: Option<oneshot::Receiver<(SignInDisposition, oneshot::Sender<()>)>>,
}

#[cfg(feature = "console")]
impl Exchange {
    pub(crate) fn take_owner_close(
        &mut self,
    ) -> Option<oneshot::Receiver<(SignInDisposition, oneshot::Sender<()>)>> {
        self.owner_close.take()
    }
}

// This handle closes an informational native-auth prompt. It cannot answer a
// native approval or publish a credential. Completion is acknowledged only
// after the driver has removed the same retained prompt and emitted closure.
pub(crate) struct ProviderNotice {
    cancel_requested: bool,
    response: oneshot::Receiver<Result<PromptAnswer, ()>>,
    owner_close: oneshot::Sender<(SignInDisposition, oneshot::Sender<()>)>,
    bridge: PromptBridge,
}
impl ProviderNotice {
    pub(crate) async fn cancelled(&mut self) {
        tokio::select! { biased;
            _ = self.bridge.closed() => {},
            response = &mut self.response => {
                self.cancel_requested = matches!(response, Ok(Ok(PromptAnswer::Cancel)));
            }
        }
    }
    #[cfg(feature = "console")]
    pub(crate) fn cancel_requested(&self) -> bool {
        self.cancel_requested
    }
    pub(crate) async fn finish(self, disposition: SignInDisposition) -> Result<(), ConnectError> {
        self.finish_with_cancellation(disposition).await.map(|_| ())
    }
    /// Report the explicit request only after this prompt has been removed.
    /// A late click can win prompt custody after native completion has joined;
    /// it still stops this request before its separate access review begins.
    pub(crate) async fn finish_with_cancellation(
        mut self,
        disposition: SignInDisposition,
    ) -> Result<bool, ConnectError> {
        let (ack, joined) = oneshot::channel();
        self.owner_close
            .send((disposition, ack))
            .map_err(|_| ended())?;
        // Keep the response receiver alive until this exact closure is joined.
        let result = tokio::select! {biased;_=self.bridge.closed()=>Err(ended()),r=joined=>r.map_err(|_|ended())};
        self.cancel_requested |= matches!(self.response.try_recv(), Ok(Ok(PromptAnswer::Cancel)));
        drop(self.response);
        result.map(|()| self.cancel_requested)
    }
}
#[derive(Clone)]
pub(crate) struct PromptBridge {
    prompts: mpsc::Sender<Exchange>,
    retired: watch::Receiver<bool>,
}
impl PromptBridge {
    /// A retained protected presentation channel, also used by the application
    /// gateway. It conveys choices only; LocalApprovals owns native decisions.
    #[cfg(any(test, feature = "console"))]
    pub(crate) fn channel() -> (Self, mpsc::Receiver<Exchange>, watch::Sender<bool>) {
        let (prompts, receive) = mpsc::channel(1);
        let (retire, retired) = watch::channel(false);
        (Self { prompts, retired }, receive, retire)
    }
    pub(crate) fn retired(&self) -> bool {
        *self.retired.borrow()
    }
    pub(crate) async fn closed(&self) {
        let mut retired = self.retired.clone();
        while !*retired.borrow_and_update() {
            if retired.changed().await.is_err() {
                break;
            }
        }
    }
    pub(crate) async fn ask(&self, prompt: Value) -> Result<PromptAnswer, ConnectError> {
        if self.retired() {
            return Err(ended());
        }
        let (send, receive) = oneshot::channel();
        tokio::select! {biased;_=self.closed()=>return Err(ended()),r=self.prompts.send(Exchange{prompt,response:send,owner_close:None})=>r.map_err(|_|ended())?}
        tokio::select! {biased;_=self.closed()=>Err(ended()),r=receive=>r.map_err(|_|ended())?.map_err(|_|ended())}
    }
    pub(crate) async fn provider_notice(
        &self,
        prompt: Value,
    ) -> Result<ProviderNotice, ConnectError> {
        if self.retired()
            || !matches!(
                prompt["kind"].as_str(),
                Some("provider_browser" | "provider_device")
            )
        {
            return Err(ended());
        }
        let (send, response) = oneshot::channel();
        let (owner_close, close) = oneshot::channel();
        tokio::select! {biased;_=self.closed()=>return Err(ended()),r=self.prompts.send(Exchange{prompt,response:send,owner_close:Some(close)})=>r.map_err(|_|ended())?}
        Ok(ProviderNotice {
            cancel_requested: false,
            response,
            owner_close,
            bridge: self.clone(),
        })
    }
}
struct Opened {
    session: Arc<ConnectSession>,
    approvals: Arc<LocalApprovals>,
    _lazy: Arc<LazySetup>,
    approval_presenter: ApprovalPresenter,
    forge_presenter: Option<crate::forge::ForgePresenter>,
    paired_host_presenter: Option<crate::host_selection::HostSelectionPresenter>,
    secret_presenter: Option<crate::secret_presenter::SecretEntryPresenter>,
    service_presenter: Option<(
        crate::service_preparation::ServicePreparationPresenter,
        meerkat_core::ResolvedStorage,
    )>,
    capabilities: Value,
    connector: Option<crate::connector_auth::Control>,
}

fn capabilities(
    version: u8,
    adapter: Adapter,
    paired_host: bool,
    tools: &[String],
    service_presenter: Option<&crate::service_preparation::ServicePreparationPresenter>,
) -> Result<Value, &'static str> {
    let services = adapter.is_services();
    if services && (version != 3 || paired_host || service_presenter.is_none())
        || !services && service_presenter.is_some()
        || adapter == Adapter::ForgeApplication && (version != 4 || paired_host)
    {
        return Err("unavailable");
    }
    let forge_enabled = matches!(adapter, Adapter::ForgeReading | Adapter::ForgeApplication);
    let mut prompt_kinds = Vec::new();
    if forge_enabled {
        prompt_kinds.extend(["forge_recovery", "forge_origin", "forge_password"]);
    }
    prompt_kinds.push("approval");
    if services {
        prompt_kinds.extend([
            "provider_target",
            "provider_action",
            "provider_api_key",
            "provider_browser",
            "provider_device",
        ]);
    }
    if paired_host && forge_enabled {
        prompt_kinds.extend([
            "paired_host_target",
            "paired_host_pairing_method",
            "paired_host_pairing_code",
        ]);
    }
    if paired_host {
        prompt_kinds.push("paired_host_notice");
    }
    let mut value = json!({"prompt_kinds":prompt_kinds,"forge_configuration":forge_enabled,
        "service_preparation":services,"paired_host":paired_host});
    if version >= 2 {
        value["tools"] = if services {
            json!(tools.iter().collect::<BTreeSet<_>>())
        } else {
            json!(tools)
        };
    }
    Ok(value)
}

async fn open(mut binding: Initialize, bridge: PromptBridge) -> Result<Opened, &'static str> {
    // Validate and construct the selected generic frontend before native auth,
    // filesystem setup or any private prompt. The actual driver validated the
    // same closed trusted initialization before scheduling this future.
    let application_forge = if binding.adapter == Adapter::ForgeApplication {
        if !binding.valid() {
            return Err("invalid_binding");
        }
        let selected = binding.forge.take().ok_or("invalid_binding")?;
        Some(
            LazySetup::private_forge_application(selected.host, selected.private_fields)
                .map_err(|_| "invalid_binding")?,
        )
    } else {
        None
    };
    // The selected .51 MCP close does not establish active setup-worker
    // settlement. Refuse this new composition before credentials, artifacts or
    // native startup until the actual owner path and qualification are present.
    if binding.adapter.is_services() {
        if binding.services.as_ref().is_none_or(|services| {
            option_env!("MEERKAT_CONNECT_RESOURCES_SHA256")
                != Some(services.resource_manifest_sha256.as_str())
        }) {
            return Err("invalid_binding");
        }
        return Err("unavailable");
    }
    let include_paired_host = binding.paired_host.is_some();
    let include_forge = matches!(
        binding.adapter,
        Adapter::ForgeReading | Adapter::ForgeApplication
    );
    let auth_binding = binding
        .auth_binding
        .map(|v| {
            serde_json::from_value::<meerkat_core::AuthBindingRef>(
                json!({"realm":v.realm,"binding":v.binding,"profile":v.profile}),
            )
            .map_err(|_| "invalid_binding")
        })
        .transpose()?;
    let storage = RealmOptions {
        context_root: binding.realm.context_root.into(),
        state_root: Some(binding.realm.state_root.into()),
        user_config_root: Some(binding.realm.user_config_root.into()),
    }
    .resolve(&binding.realm.realm_id)
    .map_err(|_| "invalid_binding")?;
    let mut startup = SessionStartup::open(storage.clone())
        .await
        .map_err(|_| "native_auth_required")?;
    let model = meerkat::CreateSessionModelResolutionRequest {
        model: binding.model,
        auth_binding,
        ..Default::default()
    };
    let model = if binding.adapter.is_services() {
        crate::provider_setup::private::present(&mut startup, model, bridge)
            .await
            .map_err(|_| "native_auth_required")?
            .ok_or("binding_retired")?
    } else {
        if !crate::provider_setup::ready_for_open(&mut startup, &model)
            .await
            .map_err(|_| "native_auth_required")?
        {
            return Err("native_auth_required");
        }
        model
    };
    let connector = if binding.adapter.is_services() {
        let profiles = binding
            .connector_auth
            .take()
            .map(ConnectorProfiles::into_native)
            .unwrap_or_default();
        Some(crate::connector_auth::Control::new(
            startup
                .configure_connectors(profiles)
                .await
                .map_err(|_| "connector_unavailable")?,
        ))
    } else {
        None
    };
    let artifacts =
        crate::resources::reserve_setup_artifacts(&storage).map_err(|_| "unavailable")?;
    let actor =
        ApprovalPrincipalId::new(binding.viewer.principal_id).map_err(|_| "invalid_binding")?;
    let (approvals, approval_presenter) = LocalApprovals::create_browser(
        &artifacts,
        actor,
        std::time::UNIX_EPOCH + Duration::from_millis(binding.viewer.expires_at_ms),
    )
    .map_err(|_| "unavailable")?;
    let mut service_presenter = None;
    let (lazy, paired_host_presenter, secret_presenter) = if binding.adapter.is_services() {
        let router = connector.as_ref().ok_or("connector_unavailable")?.router();
        let (lazy, presenter) = LazySetup::private_services_with_router(router);
        service_presenter = Some((presenter, storage.clone()));
        (lazy, None, None)
    } else if let Some(lazy) = application_forge {
        (lazy, None, None)
    } else if binding.adapter == Adapter::PairedHost {
        let credential = binding
            .paired_host
            .as_mut()
            .and_then(|paired_host| paired_host.credential.take())
            .ok_or("invalid_binding")?;
        let client = crate::paired_host::PairedHostClient::for_presenter(
            &binding.application.origin,
            &credential.header,
            &credential.value,
        )
        .map_err(|_| "invalid_binding")?;
        drop(credential);
        let paired_host = crate::paired_host_tool::PairedHostFrontend::private_preparation(client);
        let presenter = paired_host
            .take_secret_entry_presenter()
            .map_err(|_| "unsupported_presenter")?;
        (
            LazySetup::paired_host_preparation(paired_host),
            None,
            Some(presenter),
        )
    } else if include_paired_host {
        let (paired_host, presenter) =
            crate::paired_host_tool::PairedHostFrontend::deferred_with_research();
        let secrets = paired_host
            .take_secret_entry_presenter()
            .map_err(|_| "unsupported_presenter")?;
        (
            LazySetup::private_frontends(paired_host, true, include_forge),
            Some(presenter),
            Some(secrets),
        )
    } else {
        (LazySetup::reading(), None, None)
    };
    let forge_presenter = if include_forge {
        lazy.set_forge_artifacts(&artifacts)
            .map_err(|_| "unavailable")?;
        Some(
            lazy.take_forge_presenter()
                .map_err(|_| "unsupported_presenter")?,
        )
    } else {
        None
    };
    let mut options = lazy
        .session_options(SessionOptions {
            model,
            ..Default::default()
        })
        .map_err(|_| "unavailable")?;
    approvals
        .attach_lazy_setup(&mut options, &lazy)
        .map_err(|_| "required_context_missing")?;
    let tools = lazy.declared_tools();
    // Capture the actual negotiated composition while its presenters are
    // still retained. Never downgrade a service bootstrap to frontend-only
    // capabilities or construct a second tool-name table for the response.
    let capabilities = capabilities(
        binding.protocol_version,
        binding.adapter,
        include_paired_host,
        &tools,
        service_presenter.as_ref().map(|(presenter, _)| presenter),
    )?;
    let session = startup
        .open_lazy_setup(options, &lazy)
        .await
        .map_err(|_| "native_auth_required")?;
    if approvals.bind_session(session.id()).is_err() {
        let _ = session.close().await;
        return Err("invalid_binding");
    }
    Ok(Opened {
        session: Arc::new(session),
        approvals: Arc::new(approvals),
        _lazy: Arc::new(lazy),
        approval_presenter,
        forge_presenter,
        paired_host_presenter,
        secret_presenter,
        // Existing v1/v2 frontend bootstraps do not authorize service resource
        // effects. A separately admitted composition must supply this owner.
        service_presenter,
        capabilities,
        connector,
    })
}
struct ActiveSend {
    provisional_closed: bool,
    id: String,
    cancel: watch::Sender<bool>,
    receipt: bool,
    cancel_result: Option<Result<bool, ()>>,
    cancel_requests: Vec<String>,
}
enum Work {
    Opened(Box<Result<Opened, &'static str>>),
    History(String, Result<Value, ConnectError>),
    Sent(String, TurnCompletion, Option<Result<bool, ()>>),
    Presenter(bool),
    Service(Result<(), ConnectError>),
    Connector(
        String,
        crate::connector_auth::Control,
        Result<crate::connector_auth::Reply, &'static str>,
        Option<Result<(), &'static str>>,
        bool,
    ),
    Closed(Result<(), ConnectError>),
}
enum ConnectorCommand {
    Start(
        meerkat::ConnectorOAuthTarget,
        String,
        Box<crate::connector_install::InstallationLogin>,
    ),
    Complete(String, String, Option<String>),
    Cancel,
}
enum Notice {
    Admitted(String),
    Cancel(String, Result<bool, ()>),
    Conversation(String, Value),
}
// Output is bounded by both frames and bytes. Provisional deltas may stop;
// canonical history remains the source of the completed answer.
const OUTPUT_FRAMES: usize = 64;
const CONTROL_RESERVE: usize = 16;
struct OutputFrame {
    bytes: Vec<u8>,
    _budget: OwnedSemaphorePermit,
}
struct TurnCompletion {
    outcome: Result<&'static str, &'static str>,
    uncertain: bool,
    settled: bool,
}
impl TurnCompletion {
    fn unentered() -> Self {
        Self {
            outcome: Err("unavailable"),
            uncertain: false,
            settled: true,
        }
    }
    fn unknown() -> Self {
        Self {
            outcome: Err("turn_uncertain"),
            uncertain: true,
            settled: false,
        }
    }
    fn observed(outcome: &CompletionOutcome, terminal: Option<&InputTerminalOutcome>) -> Self {
        let settled = matches!(
            outcome,
            CompletionOutcome::Completed(_)
                | CompletionOutcome::CompletedWithoutResult
                | CompletionOutcome::Cancelled
        ) || crate::terminal::failed_turn_allows_new_input(outcome, terminal, false);
        Self {
            outcome: Ok(completion(outcome)),
            uncertain: false,
            settled,
        }
    }
    fn observation(result: Result<CompletionObservation, CompletionWaitError>) -> Self {
        match result {
            Ok(value) => Self::observed(value.outcome(), value.input_terminal_outcome()),
            Err(_) => Self::unknown(),
        }
    }
}
async fn before_entry<T>(
    retired: watch::Receiver<bool>,
    operation: impl std::future::Future<Output = Result<T, &'static str>>,
) -> Result<T, &'static str> {
    if *retired.borrow() {
        return Err("binding_retired");
    }
    // Once the exact operation is first-polled, the caller retains it through
    // settlement. Retirement cannot replace it with an unentered retry.
    operation.await
}
struct PendingPrompt {
    id: String,
    prompt: Value,
    // None is output-only notice custody, never a fabricated native waiter.
    response: Option<oneshot::Sender<Result<PromptAnswer, ()>>>,
    owner_close: Option<oneshot::Receiver<(SignInDisposition, oneshot::Sender<()>)>>,
}
enum PromptClosure {
    Owner(Result<(SignInDisposition, oneshot::Sender<()>), oneshot::error::RecvError>),
    Response,
}
async fn pending_prompt_closed(pending: &mut Option<PendingPrompt>) -> PromptClosure {
    let Some(PendingPrompt {
        owner_close,
        response,
        ..
    }) = pending.as_mut()
    else {
        return std::future::pending().await;
    };
    // Borrow the retained prompt once. Cancelling this wait leaves both
    // channels in that same prompt for the next driver iteration.
    tokio::select! {biased;
        closed=async{if let Some(close)=owner_close.as_mut(){close.await}else{std::future::pending().await}}=>PromptClosure::Owner(closed),
        _=async{if let Some(response)=response.as_mut(){response.closed().await}else{std::future::pending::<()>().await}}=>PromptClosure::Response,
    }
}
struct Driver {
    binding: Option<Initialize>,
    initialize_id: Option<String>,
    scope: Option<String>,
    opening: bool,
    session: Option<Arc<ConnectSession>>,
    approvals: Option<Arc<LocalApprovals>>,
    lazy: Option<Arc<LazySetup>>,
    secret_presenter: Option<crate::secret_presenter::SecretEntryPresenter>,
    private_notices: VecDeque<(String, crate::secret_presenter::SecretEntryNotice)>,
    active: Option<ActiveSend>,
    last: Option<Value>,
    uncertain: bool,
    retired: bool,
    failed: bool,
    closing: bool,
    closed: bool,
    eof: bool,
    workers: usize,
    connector: Option<crate::connector_auth::Control>,
    connector_busy: bool,
    connector_cancel: Option<oneshot::Sender<()>>,
    connector_cancel_id: Option<String>,
    histories: usize,
    stop: watch::Sender<bool>,
    output: mpsc::Sender<OutputFrame>,
    output_budget: Arc<Semaphore>,
    output_failed: bool,
    pending: Option<PendingPrompt>,
    prompt_counter: u64,
    seen: BTreeSet<String>,
    retirements: BTreeMap<String, Target>,
    retire_replies: Vec<(String, String, Target)>,
    close_replies: Vec<(String, Target)>,
    tasks: FuturesUnordered<BoxFuture<'static, Work>>,
    notices: mpsc::Sender<Notice>,
    bridge: PromptBridge,
}
impl Driver {
    fn notices_pending(&self) -> bool {
        !self.private_notices.is_empty()
            || self.pending.as_ref().is_some_and(|p| p.response.is_none())
    }
    fn state(&self) -> Value {
        let mut state = json!({"available":self.session.is_some()&&!self.retired&&!self.closed,
        "can_send":self.session.is_some()&&!self.retired&&!self.closed&&!self.uncertain&&self.active.is_none()&&!self.notices_pending(),
        "can_stop":self.active.as_ref().is_some_and(|a|a.receipt&&!a.cancel.borrow().to_owned())&&!self.retired,
        "outcome_uncertain":self.uncertain,
        "active_send":self.active.as_ref().map(|a|json!({"send_request_id":a.id,"cancellation_requested":*a.cancel.borrow()})),
        "last_completion":self.last});
        if self
            .binding
            .as_ref()
            .is_some_and(|binding| binding.protocol_version >= 2)
        {
            state["private_notice_pending"] = json!(self.notices_pending());
        }
        state
    }
    fn output(&mut self, value: Value) {
        if self.output_failed {
            return;
        }
        let queued = (|| {
            let mut frame = transport::BoundedFrame {
                bytes: Vec::new(),
                limit: MAX_FRAME,
            };
            serde_json::to_writer(&mut frame, &value).map_err(|_| ())?;
            let mut bytes = frame.bytes;
            bytes.push(b'\n');
            let budget = self
                .output_budget
                .clone()
                .try_acquire_many_owned(bytes.len() as u32)
                .map_err(|_| ())?;
            self.output
                .try_send(OutputFrame {
                    bytes,
                    _budget: budget,
                })
                .map_err(|_| ())
        })();
        if queued.is_err() {
            self.output_failed = true;
            self.failed = true;
            self.fence("failed");
            self.closing = true;
        }
    }
    fn event_target(&self) -> Option<Value> {
        self.scope
            .as_ref()
            .map(|scope| json!({"scope":scope}))
            .or_else(|| {
                self.initialize_id
                    .as_ref()
                    .map(|id| json!({"initialize_request_id":id}))
            })
    }
    fn event(&mut self, mut value: Value) {
        if self
            .binding
            .as_ref()
            .is_some_and(|b| b.protocol_version == 3)
        {
            if let Some(target) = self.event_target() {
                value["target"] = target;
                self.output(json!({"jsonrpc":"2.0","method":"connect/event","params":value}));
            }
        } else if let Some(scope) = &self.scope {
            value["scope"] = json!(scope);
            self.output(json!({"jsonrpc":"2.0","method":"connect/event","params":value}));
        }
    }
    fn state_event(&mut self) {
        self.event(json!({"kind":"state","state":self.state()}));
    }
    fn fence(&mut self, reason: &str) {
        if self.retired {
            return;
        }
        self.retired = true;
        self.stop.send_replace(true);
        if self.secret_presenter.is_some()
            && self
                .lazy
                .as_ref()
                .is_some_and(|lazy| lazy.paired_host().retire_selection().is_err())
        {
            self.failed = true;
        }
        self.secret_presenter.take();
        if let Some(control) = &mut self.connector
            && control.retire().is_err()
        {
            self.failed = true;
        }
        self.private_notices.clear();
        if self
            .approvals
            .as_ref()
            .is_some_and(|a| a.retire_checked().is_err())
        {
            self.failed = true;
        }
        if let Some(active) = &self.active {
            active.cancel.send_replace(true);
        }
        if let Some(prompt) = self.pending.take() {
            drop(prompt.response);
            let disposition = if reason == "expired"
                && self
                    .binding
                    .as_ref()
                    .is_some_and(|binding| binding.protocol_version == 3)
            {
                "expired"
            } else {
                "retired"
            };
            self.event(
                json!({"kind":"prompt_closed","prompt_id":prompt.id,"disposition":disposition}),
            );
        }
        self.event(json!({"kind":"unavailable","reason":reason}));
        self.state_event();
    }
    fn valid_scope(&self, scope: &str) -> Result<(), &'static str> {
        if self.scope.as_deref() != Some(scope) {
            Err("invalid_binding")
        } else if self.retired {
            Err("binding_retired")
        } else if self.session.is_none() {
            Err("unavailable")
        } else {
            Ok(())
        }
    }
    fn connector_binding(&self, scope: &str) -> Result<(), &'static str> {
        self.valid_scope(scope)?;
        if !self
            .binding
            .as_ref()
            .is_some_and(|b| b.protocol_version == 3 && b.adapter.is_services())
        {
            return Err("unavailable");
        }
        Ok(())
    }
    fn connector_scope(&self, scope: &str) -> Result<(), &'static str> {
        self.connector_binding(scope)?;
        if self.connector_busy {
            return Err("busy");
        }
        if self.connector.is_none() {
            return Err("unavailable");
        }
        Ok(())
    }
    fn connector_work(&mut self, id: String, command: ConnectorCommand) {
        let Some(mut control) = self.connector.take() else {
            self.bad(&id, "unavailable");
            return;
        };
        self.connector_busy = true;
        let (signal, cancel) = oneshot::channel();
        if matches!(&command, ConnectorCommand::Complete(..)) {
            self.connector_cancel = Some(signal);
        }
        // Increment before the future can be polled. Retirement and close use
        // this existing worker count and cannot overtake entered native work.
        self.workers += 1;
        let mut retired = self.stop.subscribe();
        self.tasks.push(
            async move {
                let stopped = *retired.borrow();
                let (result, cancellation, cancellation_failed) = if stopped {
                    (Err("binding_retired"), None, false)
                } else {
                    match command {
                        ConnectorCommand::Start(target, redirect, selection) => (
                            control.start(target, redirect, *selection).await,
                            None,
                            false,
                        ),
                        ConnectorCommand::Complete(state, code, handle) => {
                            control
                                .complete(state, code, handle, &mut retired, cancel)
                                .await
                        }
                        ConnectorCommand::Cancel => (control.cancel(), None, false),
                    }
                };
                // Discovery can yield a new attempt after logout. Join start, then
                // retire that exact attempt. Do not retry a failed native cancel.
                let cleanup_failed =
                    cancellation_failed || *retired.borrow() && control.retire().is_err();
                Work::Connector(id, control, result, cancellation, cleanup_failed)
            }
            .boxed(),
        );
    }
    fn target(&self, target: &Target) -> bool {
        match target {
            Target::Scope(v) => self.scope.as_deref() == Some(&v.scope),
            Target::Initialize(v) => {
                self.initialize_id.as_deref() == Some(&v.initialize_request_id)
            }
        }
    }
    fn bad(&mut self, id: &str, code: &str) {
        self.output(failed(id, code));
    }
    fn params<T: serde::de::DeserializeOwned>(&mut self, id: &str, value: Value) -> Option<T> {
        match serde_json::from_value(value) {
            Ok(v) => Some(v),
            Err(_) => {
                self.output(error(json!(id), -32602, "Invalid params", None));
                None
            }
        }
    }
    fn frame(&mut self, bytes: &[u8]) {
        if serde_json::from_slice::<crate::forge::UniqueJson>(bytes).is_err() {
            self.output(error(Value::Null, -32700, "Parse error", None));
            return;
        }
        let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
            self.output(error(Value::Null, -32700, "Parse error", None));
            return;
        };
        if value.get("method").is_none() {
            self.answer(value);
            return;
        }
        let request = match serde_json::from_value::<Request>(value) {
            Ok(r) if r.jsonrpc == "2.0" && request_id(&r.id, "h:") => r,
            _ => {
                self.output(error(Value::Null, -32600, "Invalid Request", None));
                return;
            }
        };
        if self.seen.len() >= 65_536 || !self.seen.insert(request.id.clone()) {
            self.bad(&request.id, "unavailable");
            return;
        }
        let id = request.id;
        if self
            .binding
            .as_ref()
            .is_some_and(|b| b.viewer.expires_at_ms <= now())
        {
            self.fence("expired");
        }
        match request.method.as_str() {
            "connect/initialize" => {
                if self.initialize_id.is_some() {
                    self.bad(&id, "invalid_binding");
                    return;
                }
                let Some(binding) = self.params::<Initialize>(&id, request.params) else {
                    return;
                };
                if !binding.valid() {
                    self.bad(&id, "invalid_binding");
                    return;
                }
                self.initialize_id = Some(id);
                let mut retained_binding = binding.clone();
                retained_binding.connector_auth = None;
                if let Some(paired_host) = &mut retained_binding.paired_host {
                    paired_host.credential = None;
                }
                self.binding = Some(retained_binding);
                self.opening = true;
                if self
                    .binding
                    .as_ref()
                    .is_some_and(|b| b.protocol_version == 3)
                {
                    self.state_event();
                }
                let retired = self.stop.subscribe();
                let bridge = self.bridge.clone();
                self.tasks.push(
                    async move {
                        Work::Opened(Box::new(before_entry(retired, open(binding, bridge)).await))
                    }
                    .boxed(),
                );
            }
            "connect/connector/login/start" => {
                let Some(params) = self.params::<ConnectorStart>(&id, request.params) else {
                    return;
                };
                if let Err(code) = self.connector_scope(&params.scope) {
                    self.bad(&id, code);
                    return;
                }
                let realm = self.connector.as_ref().unwrap().realm().clone();
                let target = match params.connector.into_native(realm) {
                    Ok(target) => target,
                    Err(code) => {
                        self.bad(&id, code);
                        return;
                    }
                };
                let selected_host = (|| {
                    let binding = self.binding.as_ref().ok_or("invalid_binding")?;
                    let storage = RealmOptions {
                        context_root: binding.realm.context_root.clone().into(),
                        state_root: Some(binding.realm.state_root.clone().into()),
                        user_config_root: Some(binding.realm.user_config_root.clone().into()),
                    }
                    .resolve(&binding.realm.realm_id)
                    .map_err(|_| "invalid_binding")?;
                    let owner = self
                        .lazy
                        .as_ref()
                        .and_then(|lazy| lazy.installs())
                        .ok_or("installation_unconfirmed")?;
                    let session = self.session.as_ref().ok_or("unavailable")?.id().clone();
                    Ok::<_, &'static str>((owner, storage.locator, session))
                })();
                let (owner, realm, session) = match selected_host {
                    Ok(selected) => selected,
                    Err(code) => {
                        self.bad(&id, code);
                        return;
                    }
                };
                let selected = match owner.connector_selection(&realm, &session, &params.job_id) {
                    Ok(selected) => selected,
                    Err(crate::service_install::ConnectorSelectionError::MetadataRequired {
                        family,
                        profile,
                    }) => {
                        let mut reply = failed(&id, "installation_metadata_required");
                        reply["error"]["data"]["next_action"] = json!({
                            "tool":"setup_help", "arguments":{"family":family,"profile":profile},
                            "then":{"method":"connect/connector/login/start","job_id":params.job_id},
                            "instruction":"Ask the setup agent to call setup_help for this selected profile, then retry ConnectorStart for this same job with explicit operation requirements."
                        });
                        self.output(reply);
                        return;
                    }
                    Err(crate::service_install::ConnectorSelectionError::Unconfirmed) => {
                        self.bad(&id, "installation_unconfirmed");
                        return;
                    }
                };
                let family = selected.installed.family.clone();
                let profile = selected.installed.profile.clone();
                let selection = match crate::connector_install::InstallationLogin::prepare(
                    selected,
                    params.operations,
                ) {
                    Ok(selection) => selection,
                    Err("unsupported_native_entry") => {
                        let mut reply = failed(&id, "unsupported_native_entry");
                        reply["error"]["data"]["next_action"] = json!({
                            "family":family, "profile":profile, "job_id":params.job_id,
                            "instruction":"Ask the setup agent to select and install a compatible integration whose verified package declares native connector support, then reconcile that new job and retry ConnectorStart with its job_id. The existing ordinary installation remains unchanged."
                        });
                        self.output(reply);
                        return;
                    }
                    Err(code) => {
                        self.bad(&id, code);
                        return;
                    }
                };
                self.connector_work(
                    id,
                    ConnectorCommand::Start(target, params.redirect_uri, Box::new(selection)),
                );
            }
            "connect/connector/login/complete" => {
                let Some(params) = self.params::<ConnectorComplete>(&id, request.params) else {
                    return;
                };
                if let Err(code) = self.connector_scope(&params.scope) {
                    self.bad(&id, code);
                    return;
                }
                self.connector_work(
                    id,
                    ConnectorCommand::Complete(
                        params.state,
                        params.code,
                        params.host_attempt_handle,
                    ),
                );
            }
            "connect/connector/login/cancel" => {
                let Some(params) = self.params::<ConnectorCancel>(&id, request.params) else {
                    return;
                };
                if let Err(code) = self.connector_binding(&params.scope) {
                    self.bad(&id, code);
                    return;
                }
                if self.connector_busy {
                    let Some(cancel) = self.connector_cancel.take() else {
                        self.bad(&id, "busy");
                        return;
                    };
                    self.connector_cancel_id = Some(id);
                    // A closed receiver means completion already resolved. Its
                    // retained Control handles cancellation when Work is read.
                    let _ = cancel.send(());
                } else {
                    if self.connector.is_none() {
                        self.bad(&id, "unavailable");
                        return;
                    }
                    self.connector_work(id, ConnectorCommand::Cancel);
                }
            }
            "connect/history" => {
                let Some(params) = self.params::<History>(&id, request.params) else {
                    return;
                };
                if let Err(code) = self.valid_scope(&params.scope) {
                    self.bad(&id, code);
                    return;
                }
                if params
                    .offset
                    .is_some_and(|n| n > MAX_SAFE || usize::try_from(n).is_err())
                {
                    self.bad(&id, "invalid_message");
                    return;
                }
                let session = self.session.as_ref().unwrap().clone();
                self.histories += 1;
                self.tasks.push(
                    async move {
                        Work::History(
                            id,
                            session
                                .public_history(params.offset.map(|n| n as usize))
                                .await,
                        )
                    }
                    .boxed(),
                );
            }
            "connect/send" => {
                let Some(params) = self.params::<Send>(&id, request.params) else {
                    return;
                };
                if let Err(code) = self.valid_scope(&params.scope) {
                    self.bad(&id, code);
                    return;
                }
                if params.text.trim().is_empty() {
                    self.bad(&id, "invalid_message");
                    return;
                }
                if self.uncertain {
                    self.bad(&id, "turn_uncertain");
                    return;
                }
                if self.active.is_some() || self.notices_pending() {
                    self.bad(&id, "busy");
                    return;
                }
                let (cancel, receive) = watch::channel(false);
                self.active = Some(ActiveSend {
                    provisional_closed: false,
                    id: id.clone(),
                    cancel,
                    receipt: false,
                    cancel_result: None,
                    cancel_requests: Vec::new(),
                });
                let session = self.session.as_ref().unwrap().clone();
                let notices = self.notices.clone();
                self.tasks
                    .push(send(session, id, params.text, receive, notices).boxed());
                self.state_event();
            }
            "connect/cancel" => {
                let Some(params) = self.params::<Cancel>(&id, request.params) else {
                    return;
                };
                if let Err(code) = self.valid_scope(&params.scope) {
                    self.bad(&id, code);
                    return;
                }
                let Some(active) = self
                    .active
                    .as_mut()
                    .filter(|a| a.id == params.send_request_id)
                else {
                    self.output(result(&id,json!({"scope":params.scope,"send_request_id":params.send_request_id,"status":"not_current"})));
                    return;
                };
                if let Some(outcome) = active.cancel_result {
                    self.cancel_reply(id, params.send_request_id, outcome);
                } else {
                    active.cancel_requests.push(id);
                    active.cancel.send_replace(true);
                    self.state_event();
                }
            }
            "connect/retire" => {
                let Some(params) = self.params::<Retire>(&id, request.params) else {
                    return;
                };
                if !self.target(&params.target) || !opaque(&params.retirement_id) {
                    self.bad(&id, "invalid_binding");
                    return;
                }
                if self.retired
                    && !self.retirements.is_empty()
                    && !self.retirements.contains_key(&params.retirement_id)
                {
                    self.bad(&id, "binding_retired");
                    return;
                }
                if self
                    .retirements
                    .get(&params.retirement_id)
                    .is_some_and(|t| t != &params.target)
                {
                    self.bad(&id, "invalid_binding");
                    return;
                }
                self.retirements
                    .insert(params.retirement_id.clone(), params.target.clone());
                self.retire_replies
                    .push((id, params.retirement_id, params.target));
                let reason = if matches!(params.reason, RetireReason::LoginExpired) {
                    "expired"
                } else {
                    "retired"
                };
                self.fence(reason);
            }
            "connect/close" => {
                let Some(params) = self.params::<Close>(&id, request.params) else {
                    return;
                };
                if !self.target(&params.target) {
                    self.bad(&id, "invalid_binding");
                    return;
                }
                self.close_replies.push((id, params.target));
                self.fence("closing");
                self.closing = true;
            }
            _ => self.output(error(json!(id), -32601, "Method not found", None)),
        }
    }
    fn answer(&mut self, value: Value) {
        if self
            .binding
            .as_ref()
            .is_some_and(|b| b.viewer.expires_at_ms <= now())
        {
            self.fence("expired");
        }
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        if !request_id(&id, "p:") {
            self.output(error(Value::Null, -32600, "Invalid Request", None));
            return;
        }
        if self.retired {
            self.bad(&id, "binding_retired");
            return;
        }
        if self.pending.as_ref().is_none_or(|p| {
            p.id != id || p.response.as_ref().is_some_and(oneshot::Sender::is_closed)
        }) {
            self.bad(&id, "stale_prompt");
            return;
        }
        if value.get("error").is_some() {
            let valid = serde_json::from_value::<ResponseError>(value).is_ok_and(|r| {
                r.jsonrpc == "2.0"
                    && r.id == id
                    && r.error.message.len() <= 4096
                    && r.error.code < 0
            });
            if !valid {
                self.bad(&id, "invalid_answer");
                return;
            }
            let prompt = self.pending.take().unwrap();
            let display_only = prompt.response.is_none();
            drop(prompt.response);
            self.event(json!({"kind":"prompt_closed","prompt_id":id,"disposition":"refused"}));
            if display_only {
                self.state_event();
            } else {
                self.cancel_current();
            }
            return;
        }
        let response = match serde_json::from_value::<Response>(value) {
            Ok(r) if r.jsonrpc == "2.0" && r.id == id => r,
            _ => {
                self.bad(&id, "invalid_answer");
                return;
            }
        };
        let answer = match serde_json::from_value::<AnswerEnvelope>(response.result) {
            Ok(v) => v.answer,
            Err(_) => {
                self.bad(&id, "invalid_answer");
                return;
            }
        };
        if !valid_answer(&self.pending.as_ref().unwrap().prompt, &answer) {
            self.bad(&id, "invalid_answer");
            return;
        }
        let prompt = self.pending.take().unwrap();
        let cancelled = matches!(answer, Answer::Cancel);
        let display_only = prompt.response.is_none();
        if let Some(response) = prompt.response {
            if cancelled {
                drop(response);
                self.cancel_current();
            } else if response.send(Ok(answer)).is_err() {
                self.bad(&id, "stale_prompt");
                return;
            }
        }
        // Answer delivery is not proof that the native decision was recorded.
        self.event(json!({"kind":"prompt_closed","prompt_id":id,"disposition":if cancelled{"cancelled"}else{"answered"}}));
        if display_only {
            self.state_event();
        }
    }
    fn cancel_current(&mut self) {
        if self.opening
            && self
                .binding
                .as_ref()
                .is_some_and(|b| b.protocol_version == 3)
        {
            self.fence("retired");
            return;
        }
        if let Some(active) = &self.active {
            active.cancel.send_replace(true);
        }
        self.state_event();
    }
    fn cancel_reply(&mut self, id: String, send_id: String, outcome: Result<bool, ()>) {
        match outcome{
        Ok(current)=>self.output(result(&id,json!({"scope":self.scope,"send_request_id":send_id,"status":if current{"cancellation_requested"}else{"not_current"}}))),
        Err(())=>self.bad(&id,"stop_uncertain"),
    }
    }
    fn notice(&mut self, notice: Notice) {
        match notice {
            Notice::Admitted(id) => {
                if let Some(active) = self.active.as_mut().filter(|a| a.id == id) {
                    active.receipt = true;
                }
                self.state_event();
            }
            Notice::Cancel(id, outcome) => {
                if let Some(active) = self.active.as_mut().filter(|a| a.id == id) {
                    active.cancel_result = Some(outcome);
                    let send = active.id.clone();
                    let requests = std::mem::take(&mut active.cancel_requests);
                    for request in requests {
                        self.cancel_reply(request, send.clone(), outcome);
                    }
                }
                self.state_event();
            }
            Notice::Conversation(id, event) => {
                if !self.retired
                    && let Some(active) = self.active.as_mut().filter(|a| a.id == id)
                {
                    if active.provisional_closed {
                        return;
                    }
                    if self.output.capacity() <= CONTROL_RESERVE
                        || self.output_budget.available_permits() < 512 * 1024
                    {
                        active.provisional_closed = true;
                        return;
                    }
                    self.event(json!({"kind":"conversation","send_request_id":id,"event":event}));
                }
            }
        }
    }
    fn prompt(&mut self, exchange: Exchange) {
        if self
            .binding
            .as_ref()
            .is_some_and(|b| b.viewer.expires_at_ms <= now())
        {
            self.fence("expired");
        }
        let initializing = self.opening
            && self
                .binding
                .as_ref()
                .is_some_and(|b| b.protocol_version == 3);
        let provider_prompt = matches!(
            exchange.prompt["kind"].as_str(),
            Some(
                "provider_target"
                    | "provider_action"
                    | "provider_api_key"
                    | "provider_browser"
                    | "provider_device"
            )
        );
        if provider_prompt
            && (!initializing
                || self
                    .binding
                    .as_ref()
                    .is_none_or(|b| !b.adapter.is_services()))
        {
            self.cancel_current();
            return;
        }
        if self.retired || (!initializing && self.active.is_none()) || self.pending.is_some() {
            drop(exchange.response);
            self.cancel_current();
            return;
        }
        self.prompt_counter += 1;
        let id = format!("p:{}", self.prompt_counter);
        let params = if self
            .binding
            .as_ref()
            .is_some_and(|b| b.protocol_version == 3)
        {
            json!({"target":self.event_target(),"prompt":exchange.prompt})
        } else {
            json!({"scope":self.scope,"prompt":exchange.prompt})
        };
        self.output(json!({"jsonrpc":"2.0","id":id,"method":"connect/prompt","params":params}));
        if self.retired {
            return;
        }
        self.pending = Some(PendingPrompt {
            id,
            prompt: exchange.prompt,
            response: Some(exchange.response),
            owner_close: exchange.owner_close,
        });
    }

    fn owner_closed(
        &mut self,
        closed: Result<(SignInDisposition, oneshot::Sender<()>), oneshot::error::RecvError>,
    ) {
        if let Some(prompt) = self.pending.take() {
            let disposition = match closed.as_ref().map(|(disposition, _)| disposition) {
                Ok(SignInDisposition::Completed) => "owner_completed",
                Ok(SignInDisposition::Expired) => "expired",
                Ok(SignInDisposition::Refused) | Err(_) => "refused",
            };
            self.event(
                json!({"kind":"prompt_closed","prompt_id":prompt.id,"disposition":disposition}),
            );
            if let Ok((_, ack)) = closed {
                let _ = ack.send(());
            }
        }
    }

    fn present_notice(&mut self) {
        if self.retired || self.active.is_some() || self.pending.is_some() {
            return;
        }
        if self
            .binding
            .as_ref()
            .is_some_and(|b| b.viewer.expires_at_ms <= now())
        {
            self.fence("expired");
            return;
        }
        let Some((send_id, notice)) = self.private_notices.pop_front() else {
            return;
        };
        self.prompt_counter += 1;
        let id = format!("p:{}", self.prompt_counter);
        let prompt = json!({"kind":"paired_host_notice","send_request_id":send_id,"notice":notice.into_private()});
        // Set the guard before output: no interleaved request or state may make
        // this private output eligible for a different native send.
        self.pending = Some(PendingPrompt {
            id: id.clone(),
            prompt: prompt.clone(),
            response: None,
            owner_close: None,
        });
        self.output(json!({"jsonrpc":"2.0","id":id,"method":"connect/prompt","params":{"scope":self.scope,"prompt":prompt}}));
    }
    fn work(&mut self, work: Work) {
        match work {
            Work::Opened(result_open) => {
                self.opening = false;
                let id = self.initialize_id.clone().unwrap();
                match *result_open {
                    Ok(opened) => {
                        if !self.retired
                            || self
                                .binding
                                .as_ref()
                                .is_none_or(|b| b.protocol_version != 3)
                        {
                            self.scope =
                                Some(format!("connect:{}", meerkat_core::SessionId::new()));
                        }
                        self.connector = opened.connector;
                        self.session = Some(opened.session);
                        self.approvals = Some(opened.approvals);
                        self.lazy = Some(opened._lazy);
                        self.secret_presenter = opened.secret_presenter;
                        if self.retired {
                            if self.approvals.as_ref().unwrap().retire_checked().is_err() {
                                self.failed = true;
                            }
                            drop(opened.approval_presenter);
                            drop(opened.forge_presenter);
                            drop(opened.paired_host_presenter);
                            drop(opened.service_presenter);
                            self.secret_presenter.take();
                            self.bad(&id, "binding_retired");
                        } else {
                            self.workers = 1;
                            self.tasks.push(
                                approvals(opened.approval_presenter, self.bridge.clone()).boxed(),
                            );
                            let binding = self.binding.as_ref().unwrap();
                            if let Some(presenter) = opened.forge_presenter {
                                self.workers += 1;
                                self.tasks.push(
                                    forge(
                                        presenter,
                                        self.bridge.clone(),
                                        binding.application.application_id.clone(),
                                        binding.application.origin.clone(),
                                    )
                                    .boxed(),
                                );
                            }
                            if let Some(presenter) = opened.paired_host_presenter {
                                self.workers += 1;
                                self.tasks.push(
                                    paired_host(
                                        presenter,
                                        self.bridge.clone(),
                                        binding.paired_host.as_ref().unwrap().targets.clone(),
                                    )
                                    .boxed(),
                                );
                            }
                            if let Some((presenter, storage)) = opened.service_presenter {
                                self.workers += 1;
                                let bridge = self.bridge.clone();
                                self.tasks.push(
                                    async move {
                                        Work::Service(
                                            services::run(presenter, storage, bridge).await,
                                        )
                                    }
                                    .boxed(),
                                );
                            }
                            self.output(result(&id,json!({"protocol_version":binding.protocol_version,"scope":self.scope,"session_id":self.session.as_ref().unwrap().id().to_string(),"capabilities":opened.capabilities,"state":self.state()})));
                        }
                    }
                    Err(code) => {
                        self.failed |= code != "binding_retired";
                        if self
                            .binding
                            .as_ref()
                            .is_some_and(|b| b.protocol_version == 3)
                        {
                            let code = if self.retired {
                                "binding_retired"
                            } else {
                                code
                            };
                            self.fence("failed");
                            self.bad(&id, code);
                        } else {
                            self.bad(&id, code);
                            self.fence("failed");
                        }
                        self.closing = true;
                    }
                }
            }
            Work::History(id, history) => {
                self.histories -= 1;
                if self.retired {
                    self.bad(&id, "binding_retired")
                } else {
                    match history {
                        Ok(history) => self
                            .output(result(&id, json!({"history":history,"state":self.state()}))),
                        Err(_) => self.bad(&id, "history_unavailable"),
                    }
                }
            }
            Work::Sent(id, observed, cancelled) => {
                // All queue writes occur synchronously before native tools
                // return. Capture both slots for this joined send, with no
                // await and before publishing any can_send state.
                if let Some(presenter) = self.secret_presenter.as_mut() {
                    let pending = presenter.take_pending();
                    if observed.settled && !self.retired {
                        self.private_notices.extend(
                            pending
                                .into_iter()
                                .flatten()
                                .map(|notice| (id.clone(), notice)),
                        );
                    }
                }
                if let Some(active) = self.active.take() {
                    for request in active.cancel_requests {
                        self.cancel_reply(
                            request,
                            id.clone(),
                            cancelled.or(active.cancel_result).unwrap_or(Ok(false)),
                        );
                    }
                }
                self.uncertain |= observed.uncertain;
                // A known attempt result is retained even when the exact input is
                // requeued or finalization failed. It cannot attest joined work.
                if !observed.settled
                    || self
                        .lazy
                        .as_ref()
                        .is_some_and(|lazy| lazy.owner().requires_shutdown())
                {
                    self.failed = true;
                    self.fence("failed");
                    self.closing = true;
                }
                match observed.outcome {
                    Ok(completion) => {
                        self.last = Some(json!({"send_request_id":id,"completion":completion}));
                        self.output(result(&id,json!({"scope":self.scope,"send_request_id":id,"completion":completion,"state":self.state()})));
                    }
                    Err(code) => self.bad(&id, code),
                }
                self.state_event();
            }
            Work::Presenter(failed) => {
                self.workers -= 1;
                if failed && !self.retired {
                    self.failed = true;
                    self.fence("failed");
                    self.closing = true;
                }
            }
            Work::Service(result) => {
                self.workers -= 1;
                if result.is_err() {
                    // An entered preparation cleanup failure cannot disappear
                    // merely because retirement was requested while it ran.
                    self.failed = true;
                    self.fence("failed");
                    self.closing = true;
                }
            }
            Work::Connector(id, mut control, reply, cancellation, cleanup_failed) => {
                self.workers -= 1;
                self.connector_busy = false;
                self.connector_cancel.take();
                let cancel_reply = self.connector_cancel_id.take().map(|cancel_id| {
                    // If the completion won before the signal was observed,
                    // cancel through the same owner now that its join is held.
                    (
                        cancel_id,
                        cancellation.unwrap_or_else(|| control.cancel().map(|_| ())),
                    )
                });
                let cleanup_failed = cleanup_failed
                    || cancel_reply
                        .as_ref()
                        .is_some_and(|(_, reply)| matches!(reply, Err("connector_unavailable")));
                // Retirement may have won after the future returned but before
                // this driver observed it. Do not publish a late start secret.
                let cleanup_failed = cleanup_failed || self.retired && control.retire().is_err();
                if cleanup_failed {
                    self.failed = true;
                    // Keep the Control in this joined Work while fencing, so
                    // fence cannot retry a failed native cancellation.
                    self.fence("failed");
                    self.closing = true;
                }
                self.connector = Some(control);
                match reply {
                    Ok(crate::connector_auth::Reply::Started(_)) if self.retired => self.bad(&id,
                        if cleanup_failed { "shutdown_unconfirmed" } else { "binding_retired" }),
                    Ok(crate::connector_auth::Reply::Started(started)) => self.output(result(&id,
                        json!({"scope":self.scope,"started":started}))),
                    Ok(crate::connector_auth::Reply::Completed(completed)) => self.output(result(&id,
                        json!({"scope":self.scope,"completed":meerkat::connector_ready_to_wire(&completed.native)}))),
                    Ok(crate::connector_auth::Reply::Cancelled) => self.output(result(&id,
                        json!({"scope":self.scope,"cancelled":true}))),
                    Err(code) => self.bad(&id, code),
                }
                if let Some((cancel_id, reply)) = cancel_reply {
                    match reply {
                        Ok(()) => self.output(result(
                            &cancel_id,
                            json!({"scope":self.scope,"cancelled":true}),
                        )),
                        Err(code) => self.bad(&cancel_id, code),
                    }
                }
            }
            Work::Closed(outcome) => {
                self.closed = true;
                if outcome.is_err() {
                    self.failed = true;
                }
                let replies = std::mem::take(&mut self.close_replies);
                for (id, target) in replies {
                    if self.failed {
                        self.bad(&id, "shutdown_unconfirmed")
                    } else {
                        self.output(result(
                            &id,
                            json!({"target":target,"resources_closed":true}),
                        ))
                    }
                }
            }
        }
    }
    fn settle(&mut self) {
        self.present_notice();
        if self.retired
            && !self.opening
            && self.active.is_none()
            && self.workers == 0
            && self.histories == 0
        {
            let replies = std::mem::take(&mut self.retire_replies);
            for (id, retirement_id, target) in replies {
                if self.failed {
                    self.bad(&id, "shutdown_unconfirmed")
                } else {
                    self.output(result(&id,json!({"target":target,"retirement_id":retirement_id,"admission_closed":true,"prompts_retired":true,"entered_work":"settled"})));
                }
            }
            if self.closing && !self.closed {
                if let Some(session) = self.session.take() {
                    self.connector.take();
                    self.lazy.take();
                    self.approvals.take();
                    match Arc::try_unwrap(session) {
                        Ok(session) => self
                            .tasks
                            .push(async move { Work::Closed(session.close().await) }.boxed()),
                        Err(_) => {
                            self.failed = true;
                            self.work(Work::Closed(Err(ended())));
                        }
                    }
                } else if self.tasks.is_empty() {
                    self.work(Work::Closed(Ok(())));
                }
            }
        }
    }
}

async fn approvals(presenter: ApprovalPresenter, bridge: PromptBridge) -> Work {
    Work::Presenter(present_approvals(presenter, bridge).await)
}

pub(crate) async fn present_approvals(
    mut presenter: ApprovalPresenter,
    bridge: PromptBridge,
) -> bool {
    loop {
        let prompt =
            tokio::select! {biased;_=bridge.closed()=>return false,prompt=presenter.next()=>prompt};
        let Some(mut prompt) = prompt else {
            return !bridge.retired();
        };
        let record = prompt.record();
        let projection = json!({"kind":"approval","approval_id":record.approval_id,"summary":record.proposed_action.summary,"body":record.proposed_action.body,"allowed_decisions":["approve","deny"],"expires_at":record.expires_at});
        let answer = tokio::select! {biased;_=prompt.closed()=>continue,answer=bridge.ask(projection)=>answer};
        match answer {
            Ok(Answer::Approval { decision }) => {
                if bridge.retired() {
                    continue;
                }
                if let Err(error) = prompt.decide(match decision {
                    Decision::Approve => ApprovalDecision::Approve,
                    Decision::Deny => ApprovalDecision::Deny,
                }) {
                    // Expired/stale eligibility is local refusal. A failed native
                    // decision write or poisoned custody is infrastructure failure.
                    if !matches!(error, ConnectError::InvalidInput(_)) {
                        return true;
                    }
                }
            }
            _ if bridge.retired() => return false,
            _ => {}
        }
    }
}
async fn forge(
    mut presenter: crate::forge::ForgePresenter,
    bridge: PromptBridge,
    application: String,
    origin: String,
) -> Work {
    loop {
        let prompt = tokio::select! {biased;_=bridge.closed()=>return Work::Presenter(false),prompt=presenter.next()=>prompt};
        let Some(prompt) = prompt else {
            return Work::Presenter(!bridge.retired());
        };
        // Presenter failure retires the exact native tool waiter. It does not
        // retry login or manufacture a denial. Native completion stays decisive.
        let _ = crate::forge::present_stdio(prompt, &bridge, &application, &origin).await;
    }
}
async fn paired_host(
    mut presenter: crate::host_selection::HostSelectionPresenter,
    bridge: PromptBridge,
    targets: Vec<crate::paired_host_stdio::Target>,
) -> Work {
    loop {
        let prompt = tokio::select! {biased;_=bridge.closed()=>return Work::Presenter(false),prompt=presenter.next()=>prompt};
        let Some(prompt) = prompt else {
            return Work::Presenter(!bridge.retired());
        };
        let _ = crate::paired_host_stdio::present(prompt, &bridge, &targets).await;
    }
}
fn completion(outcome: &CompletionOutcome) -> &'static str {
    match outcome {
        CompletionOutcome::Completed(_) => "completed",
        CompletionOutcome::CompletedWithoutResult => "completed_without_result",
        CompletionOutcome::CallbackPending { .. } => "callback_pending",
        CompletionOutcome::CallbackBatchPending { .. } => "callback_batch_pending",
        CompletionOutcome::Cancelled => "cancelled",
        CompletionOutcome::Abandoned { .. } => "abandoned",
        CompletionOutcome::AbandonedWithError { .. } => "abandoned_with_error",
        CompletionOutcome::CompletedWithFinalizationFailure { .. } => {
            "completed_with_finalization_failure"
        }
        CompletionOutcome::RuntimeTerminated { .. } => "runtime_terminated",
        CompletionOutcome::OutcomeUnknown { .. } => "outcome_unknown",
    }
}
async fn send(
    session: Arc<ConnectSession>,
    id: String,
    text: String,
    mut cancel: watch::Receiver<bool>,
    notices: mpsc::Sender<Notice>,
) -> Work {
    if *cancel.borrow() {
        return Work::Sent(id, TurnCompletion::unentered(), None);
    }
    let mut events = match session.events().await {
        Ok(events) => events,
        Err(_) => return Work::Sent(id, TurnCompletion::unentered(), None),
    };
    if *cancel.borrow() {
        return Work::Sent(id, TurnCompletion::unentered(), None);
    }
    // Once entered, neither admission nor completion is dropped by a control.
    let (admission, handle) = match session.submit(text).await {
        Ok(value) => value,
        Err(error) => {
            if let ConnectError::MissingCompletion { admission } = error {
                let _ = session.cancel(&admission).await;
            }
            return Work::Sent(id, TurnCompletion::unknown(), None);
        }
    };
    let _ = notices.send(Notice::Admitted(id.clone())).await;
    let wait = handle.try_wait_with_terminal_outcome();
    tokio::pin!(wait);
    let mut cancelled = false;
    let mut cancel_result = None;
    let mut remaining = 256 * 1024usize;
    loop {
        if *cancel.borrow() && !cancelled {
            cancelled = true;
            let outcome = session.cancel(&admission).await.map_err(|_| ());
            cancel_result = Some(outcome);
            let _ = notices.send(Notice::Cancel(id.clone(), outcome)).await;
        }
        tokio::select! {biased;
            _=cancel.changed(),if !cancelled=>{},
            result=&mut wait=>{
                if *cancel.borrow()&&!cancelled {let outcome=session.cancel(&admission).await.map_err(|_|());cancel_result=Some(outcome);let _=notices.send(Notice::Cancel(id.clone(),outcome)).await;}
                let observed=TurnCompletion::observation(result);
                if !observed.settled&&cancel_result.is_none(){
                    let outcome=session.cancel(&admission).await.map_err(|_|());
                    cancel_result=Some(outcome);let _=notices.send(Notice::Cancel(id.clone(),outcome)).await;
                }
                return Work::Sent(id,observed,cancel_result)
            },
            event=events.next(),if remaining>0=>{
                let Some(event)=event else{remaining=0;continue};
                if event.source.session_id()!=Some(session.id()){continue}
                let (mut projected,message_id)=match event.payload{
                    AgentEvent::RunStarted{..}=>(json!({"kind":"run_observed"}),None),
                    AgentEvent::TurnStarted{assistant_message_id,..}=>(json!({"kind":"text_reset"}),assistant_message_id),
                    AgentEvent::TextDelta{delta,assistant_message_id}=>(json!({"kind":"text_delta","text":delta}),assistant_message_id),
                    AgentEvent::TextComplete{content,assistant_message_id}=>(json!({"kind":"text_complete","text":content}),assistant_message_id),
                    _=>continue,
                };
                if let Some(id)=message_id{projected["messageId"]=json!(id);}
                let size=projected.to_string().len();if size>remaining{remaining=0;continue}remaining-=size;
                if notices.try_send(Notice::Conversation(id.clone(),projected)).is_err(){remaining=0;}
            }
        }
    }
}

fn new_driver(
    output: mpsc::Sender<OutputFrame>,
    output_budget: Arc<Semaphore>,
) -> (Driver, mpsc::Receiver<Notice>, mpsc::Receiver<Exchange>) {
    let (stop, retired) = watch::channel(false);
    let (prompts, prompt_rx) = mpsc::channel(1);
    let bridge = PromptBridge { prompts, retired };
    let (notices, notice_rx) = mpsc::channel(32);
    let driver = Driver {
        binding: None,
        initialize_id: None,
        scope: None,
        opening: false,
        session: None,
        approvals: None,
        lazy: None,
        secret_presenter: None,
        private_notices: VecDeque::new(),
        active: None,
        last: None,
        uncertain: false,
        retired: false,
        failed: false,
        closing: false,
        closed: false,
        eof: false,
        workers: 0,
        connector: None,
        connector_busy: false,
        connector_cancel: None,
        connector_cancel_id: None,
        histories: 0,
        stop,
        output,
        output_budget,
        output_failed: false,
        pending: None,
        prompt_counter: 0,
        seen: BTreeSet::new(),
        retirements: BTreeMap::new(),
        retire_replies: Vec::new(),
        close_replies: Vec::new(),
        tasks: FuturesUnordered::new(),
        notices,
        bridge,
    };
    (driver, notice_rx, prompt_rx)
}

/// The executable's inherited private stdin/stdout. No terminal or provider
/// sign-in reader is constructed on this path.
pub async fn run() -> Result<(), ConnectError> {
    let input = transport::Pipe::inherited(std::io::stdin()).map_err(|_| ended())?;
    let output = transport::Pipe::inherited(std::io::stdout()).map_err(|_| ended())?;
    run_io(input, output).await
}
async fn run_io<R: AsyncRead + Unpin, W: AsyncWrite + Unpin + std::marker::Send + 'static>(
    input: R,
    mut output: W,
) -> Result<(), ConnectError> {
    let (write_tx, mut write_rx) = mpsc::channel::<OutputFrame>(OUTPUT_FRAMES);
    let output_budget = Arc::new(Semaphore::new(MAX_FRAME * 2));
    let (writer_stop, mut writer_cancel) = watch::channel(false);
    let mut writer = tokio::spawn(async move {
        while let Some(frame) = write_rx.recv().await {
            tokio::select! {biased;_=writer_cancel.changed()=>return Err(()),result=tokio::time::timeout(Duration::from_secs(30),output.write_all(&frame.bytes))=>result.map_err(|_|())?.map_err(|_|())?}
        }
        output.flush().await.map_err(|_| ())
    });
    let (mut driver, mut notice_rx, mut prompt_rx) = new_driver(write_tx, output_budget);
    let mut frames = transport::Frames::new(input);
    let mut writer_done = false;
    loop {
        driver.settle();
        if driver.closed && driver.tasks.is_empty() {
            break;
        }
        let expiry = driver
            .binding
            .as_ref()
            .map(|b| b.viewer.expires_at_ms)
            .unwrap_or(u64::MAX);
        let delay = Duration::from_millis(expiry.saturating_sub(now()));
        tokio::select! {biased;
            result=&mut writer,if !writer_done=>{writer_done=true;driver.output_failed=true;driver.failed|=!matches!(result,Ok(Ok(())));driver.fence("failed");driver.closing=true;},
            _=tokio::time::sleep(delay),if !driver.retired&&driver.binding.is_some()=>driver.fence("expired"),
            frame=frames.next(),if !driver.eof=>match frame {
                Ok(Some(bytes))=>driver.frame(&bytes),
                Ok(None)|Err(_)=>{driver.eof=true;driver.fence("closing");driver.closing=true;},
            },
            closed=pending_prompt_closed(&mut driver.pending)=>match closed {
                PromptClosure::Owner(closed)=>driver.owner_closed(closed),
                PromptClosure::Response=>{
                    if let Some(prompt)=driver.pending.take(){driver.event(json!({"kind":"prompt_closed","prompt_id":prompt.id,"disposition":"cancelled"}));}
                },
            },
            Some(work)=driver.tasks.next(),if !driver.tasks.is_empty()=>driver.work(work),
            Some(notice)=notice_rx.recv()=>driver.notice(notice),
            Some(exchange)=prompt_rx.recv()=>driver.prompt(exchange),
        }
        if driver.output_failed {
            writer_stop.send_replace(true);
        }
        if driver.eof && driver.initialize_id.is_none() {
            driver.closed = true;
        }
    }
    let failed = driver.failed;
    drop(driver);
    if !writer_done && writer.await.map_err(|_| ended())?.is_err() {
        return Err(ended());
    }
    if failed { Err(ended()) } else { Ok(()) }
}

#[cfg(test)]
#[path = "stdio_tests.rs"]
pub(crate) mod tests;
