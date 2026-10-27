//! One composed consumer control. The Console and package are synthetic peers;
//! native input, protected review, close, config/grant finalization and the
//! persisted conversation reopen use their existing production owners.

use super::*;
use crate::forge::{
    FORGE_INSPECT, FORGE_PREPARE, FORGE_SELECT_HOST, FORGE_STATUS, FORGE_SUBMIT, ForgePresenter,
    present_forge_selection,
};
use crate::session::FinalizationClose;
use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, Method, StatusCode, Uri},
    response::IntoResponse,
};
use meerkat_core::service::{SessionHistoryQuery, SessionServiceHistoryExt};
use meerkat_core::{AgentToolDispatcher, ToolCallView, ToolDispatchContext};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    os::{fd::OwnedFd, unix::fs::PermissionsExt},
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const COOKIE: &str = "mobkit_console_0123456789abcdef";
const NONCE: &str = "nnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnn";
const LOGIN: &str = "sssssssssssssssssssssssssssssssssssssssssss";
const PASSWORD: &str = "composition fixture password";
const CHANGE: &str = "composition-change";
const SUBMITTED: &str =
    r#"{"application_result":{"state":"configured","extension":[true,null]},"status":"unknown"}"#;
const OBSERVED: &str =
    r#"{"application_result":{"state":"still-unobserved","extension":{"v":7}},"status":"unknown"}"#;

fn system_rows(messages: &[meerkat_core::Message]) -> Vec<(usize, String)> {
    messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| match message {
            meerkat_core::Message::System(message) => Some((index, message.content.clone())),
            _ => None,
        })
        .collect()
}

fn host() -> Value {
    json!({"protocol":1,"host_id":format!("forge:{}","a".repeat(64)),
    "application_id":"composition-fixture","source_digest":"b".repeat(64)})
}
fn declaration() -> Value {
    json!({"protocol":1,"host":host(),"private_fields":["PRIVATE_TOKEN"]})
}
fn proposal() -> Value {
    json!({"change_id":CHANGE,"configuration":{"mode":"fixture"},
    "expected_revision":"opaque-v1","owner_extension":{"review":"exact-proposal"}})
}

#[derive(Clone)]
struct HttpState {
    origin: String,
    // Fixture-only HTTP observations, never a second application status owner.
    calls: Arc<Mutex<Vec<(String, String, Value)>>>,
}
struct Server {
    state: HttpState,
    stop: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
}
impl Server {
    async fn open() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let state = HttpState {
            origin: format!("http://{}", listener.local_addr().unwrap()),
            calls: Default::default(),
        };
        let app = Router::new().fallback(http).with_state(state.clone());
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
        });
        Self { state, stop, task }
    }
    async fn finish(self) -> Result<Vec<(String, String, Value)>, ConnectError> {
        let Self {
            state,
            stop,
            mut task,
        } = self;
        let _ = stop.send(());
        let observed = tokio::time::timeout(Duration::from_secs(5), &mut task).await;
        match observed {
            Ok(result) => result.map_err(|e| ConnectError::native("Join fixture server", e))?,
            Err(error) => {
                // An observation timeout does not release request workers.
                // Join this same graceful owner before reporting the timeout.
                let joined = task
                    .await
                    .map_err(|e| ConnectError::native("Join fixture server after timeout", e));
                return combine(
                    Err(ConnectError::native(
                        "Observe fixture server shutdown",
                        error,
                    )),
                    joined,
                );
            }
        }
        let calls = state.calls.lock().unwrap().clone();
        Ok(calls)
    }
}
async fn http(
    State(state): State<HttpState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> axum::response::Response {
    let path = uri.path();
    let parsed = serde_json::from_slice(&body).unwrap_or(Value::Null);
    state
        .calls
        .lock()
        .unwrap()
        .push((method.to_string(), path.into(), parsed));
    if headers.get("host").and_then(|v| v.to_str().ok()) != state.origin.strip_prefix("http://")
        || uri.query().is_some()
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    if method == Method::GET && path == "/login" {
        let html = format!(
            "<form method=\"post\" action=\"/login\"><input type=\"hidden\" name=\"nonce\" value=\"{NONCE}\"><input type=\"password\" name=\"password\"></form>"
        );
        let mut response = (
            StatusCode::OK,
            [("content-type", "text/html"), ("cache-control", "no-store")],
            html,
        )
            .into_response();
        response.headers_mut().insert(
            "set-cookie",
            format!("{COOKIE}_login={NONCE}; HttpOnly; Max-Age=300; Path=/; SameSite=Strict")
                .parse()
                .unwrap(),
        );
        return response;
    }
    if method == Method::POST && path == "/login" {
        let form = reqwest::Url::parse(&format!(
            "http://127.0.0.1/?{}",
            String::from_utf8_lossy(&body)
        ))
        .unwrap();
        let values = form.query_pairs().into_owned().collect::<BTreeMap<_, _>>();
        if values.get("password").map(String::as_str) != Some(PASSWORD)
            || values.get("nonce").map(String::as_str) != Some(NONCE)
            || headers.get("cookie").and_then(|v| v.to_str().ok())
                != Some(format!("{COOKIE}_login={NONCE}").as_str())
            || headers.get("origin").and_then(|v| v.to_str().ok()) != Some(state.origin.as_str())
        {
            return StatusCode::FORBIDDEN.into_response();
        }
        let mut response = (
            StatusCode::SEE_OTHER,
            [("location", "/console"), ("cache-control", "no-store")],
        )
            .into_response();
        response.headers_mut().append(
            "set-cookie",
            format!("{COOKIE}={LOGIN}; HttpOnly; Max-Age=28800; Path=/; SameSite=Strict")
                .parse()
                .unwrap(),
        );
        response.headers_mut().append(
            "set-cookie",
            format!("{COOKIE}_login=\"\"; Max-Age=0; Path=/; SameSite=Lax")
                .parse()
                .unwrap(),
        );
        return response;
    }
    if headers.get("cookie").and_then(|v| v.to_str().ok())
        != Some(format!("{COOKIE}={LOGIN}").as_str())
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if method == Method::POST
        && headers.get("origin").and_then(|v| v.to_str().ok()) != Some(state.origin.as_str())
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    let envelope=match (method.as_str(),path) {
        ("GET","/console/forge/frontends/descriptor")=>declaration().to_string(),
        ("GET","/console/forge/frontends")=>json!({"protocol":1,"host":host(),"observation":{"revision":"opaque-v1"}}).to_string(),
        ("POST","/console/forge/frontends/prepare")=>json!({"protocol":1,"host":host(),"proposal":proposal(),
            "approval":{"proof":"c".repeat(64),"expires_at":SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()+300}}).to_string(),
        ("POST","/console/forge/frontends/submit")=>format!("{{\"protocol\":1,\"host\":{},\"observation\":{SUBMITTED}}}",host()),
        ("GET","/console/forge/frontends/changes/composition-change")=>format!("{{\"protocol\":1,\"host\":{},\"observation\":{OBSERVED}}}",host()),
        _=>return StatusCode::NOT_FOUND.into_response(),
    };
    (
        StatusCode::OK,
        [
            ("content-type", "application/json"),
            ("cache-control", "no-store"),
        ],
        envelope,
    )
        .into_response()
}

// The real protected presenter still owns its PTY, including hidden password
// entry. Answer only each prompt actually rendered, never approval IDs guessed
// from native state. The second round must render neither origin nor password.
struct Human {
    master: OwnedFd,
    text: String,
    origin: String,
    origin_sent: bool,
    password_sent: bool,
    approved: BTreeSet<String>,
}
impl Write for Human {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.text.push_str(&String::from_utf8_lossy(bytes));
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        let mut answers = Vec::new();
        if !self.origin_sent && self.text.contains("Forge Console origin") {
            self.origin_sent = true;
            answers.push(self.origin.clone());
        }
        if !self.password_sent && self.text.contains("Forge Console password (hidden)") {
            self.password_sent = true;
            answers.push(PASSWORD.into());
        }
        for part in self.text.split("Type 'approve ").skip(1) {
            let Some((id, _)) = part.split_once('\'') else {
                continue;
            };
            if self.approved.insert(id.into()) {
                answers.push(format!("approve {id}"));
            }
        }
        for answer in answers {
            let line = format!("{answer}\n");
            let written =
                nix::unistd::write(&self.master, line.as_bytes()).map_err(io::Error::other)?;
            if written != line.len() {
                return Err(io::Error::other("short fixture terminal answer"));
            }
        }
        Ok(())
    }
}
fn terminal(origin: &str) -> (TerminalInput, Human) {
    let pair = nix::pty::openpty(None, None).unwrap();
    (
        TerminalInput::from_owned_fd(pair.slave),
        Human {
            master: pair.master,
            text: String::new(),
            origin: origin.into(),
            origin_sent: false,
            password_sent: false,
            approved: BTreeSet::new(),
        },
    )
}
fn script(client: &ScriptedProvider, calls: &[(&str, &str, Value)]) {
    let mut pending = client.1.lock().unwrap();
    assert!(pending.is_empty());
    pending.extend(
        calls
            .iter()
            .map(|(id, name, args)| ((*id).into(), (*name).into(), args.clone())),
    );
}
fn result(client: &ScriptedProvider, id: &str) -> Result<ObservedToolResult, ConnectError> {
    client
        .2
        .lock()
        .unwrap()
        .get(id)
        .cloned()
        .ok_or_else(|| invalid("The actual model did not receive the tool result"))
}
fn value(result: &ObservedToolResult) -> Result<Value, ConnectError> {
    require(
        !result.is_error,
        "Expected a successful actual model tool result",
    )?;
    serde_json::from_str(&result.text_content())
        .map_err(|e| ConnectError::native("Read fixture tool result", e))
}

// Project only closed typed labels from this exact completion. Operator text,
// provider/model identifiers and nested reason payloads remain undisclosed.
fn turn_error_labels(
    error: &meerkat_core::TurnErrorMetadata,
) -> (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
) {
    use meerkat_core::error::{
        LlmProviderErrorKind as ProviderKind, LlmProviderErrorRetryability as Retryability,
    };
    use meerkat_core::{
        TurnTerminalCauseKind as Kind, TurnTerminalOutcome as Outcome,
        event::AgentErrorReason as Reason,
    };
    let kind = match error.kind {
        Kind::Unknown => "Unknown",
        Kind::HookDenied => "HookDenied",
        Kind::HookFailure => "HookFailure",
        Kind::LlmFailure => "LlmFailure",
        Kind::ToolFailure => "ToolFailure",
        Kind::StructuredOutputValidationFailed => "StructuredOutputValidationFailed",
        Kind::BudgetExhausted => "BudgetExhausted",
        Kind::TimeBudgetExceeded => "TimeBudgetExceeded",
        Kind::RetryExhausted => "RetryExhausted",
        Kind::TurnLimitReached => "TurnLimitReached",
        Kind::RuntimeApplyFailure => "RuntimeApplyFailure",
        Kind::FatalFailure => "FatalFailure",
    };
    let outcome = match error.outcome {
        None => "Absent",
        Some(Outcome::None) => "None",
        Some(Outcome::Completed) => "Completed",
        Some(Outcome::Failed) => "Failed",
        Some(Outcome::Cancelled) => "Cancelled",
        Some(Outcome::BudgetExhausted) => "BudgetExhausted",
        Some(Outcome::TimeBudgetExceeded) => "TimeBudgetExceeded",
        Some(Outcome::StructuredOutputValidationFailed) => "StructuredOutputValidationFailed",
    };
    let reason = match error.reason.as_ref() {
        None => "Absent",
        Some(Reason::LlmRateLimited { .. }) => "LlmRateLimited",
        Some(Reason::LlmContextExceeded { .. }) => "LlmContextExceeded",
        Some(Reason::LlmAuthError) => "LlmAuthError",
        Some(Reason::ContentFiltered { .. }) => "ContentFiltered",
        Some(Reason::LlmInvalidModel { .. }) => "LlmInvalidModel",
        Some(Reason::LlmProviderError { .. }) => "LlmProviderError",
        Some(Reason::LlmNetworkTimeout { .. }) => "LlmNetworkTimeout",
        Some(Reason::LlmCallTimeout { .. }) => "LlmCallTimeout",
        Some(Reason::HookDenied { .. }) => "HookDenied",
        Some(Reason::HookTimeout { .. }) => "HookTimeout",
        Some(Reason::HookExecutionFailed { .. }) => "HookExecutionFailed",
        Some(Reason::HookConfigInvalid { .. }) => "HookConfigInvalid",
        Some(Reason::StructuredOutputValidationFailed { .. }) => "StructuredOutputValidationFailed",
        Some(Reason::InvalidOutputSchema { .. }) => "InvalidOutputSchema",
        Some(Reason::AuthReauthRequired { .. }) => "AuthReauthRequired",
        Some(Reason::CallbackPending { .. }) => "CallbackPending",
        Some(Reason::TurnTerminalCause { .. }) => "TurnTerminalCause",
        Some(Reason::ModelFallbackResumeHeld { .. }) => "ModelFallbackResumeHeld",
        Some(Reason::HookLaunchRefused { .. }) => "HookLaunchRefused",
    };
    let (provider_kind, provider_retryability) = match error.reason.as_ref() {
        Some(Reason::LlmProviderError {
            provider_error_kind,
            provider_error_retryability,
            ..
        }) => {
            let kind = match provider_error_kind {
                ProviderKind::InvalidRequest => "InvalidRequest",
                ProviderKind::OperationRefused => "OperationRefused",
                ProviderKind::OperationObservationUnavailable => "OperationObservationUnavailable",
                ProviderKind::OperationAuthorizationUnavailable => {
                    "OperationAuthorizationUnavailable"
                }
                ProviderKind::AuthorizationRouteChanged => "AuthorizationRouteChanged",
                ProviderKind::RequestTooLarge => "RequestTooLarge",
                ProviderKind::QuotaExhausted => "QuotaExhausted",
                ProviderKind::ContentFiltered => "ContentFiltered",
                ProviderKind::PolicyStop => "PolicyStop",
                ProviderKind::ServerError => "ServerError",
                ProviderKind::ServerOverloaded => "ServerOverloaded",
                ProviderKind::ConnectionReset => "ConnectionReset",
                ProviderKind::Unknown => "Unknown",
                ProviderKind::StreamParseError => "StreamParseError",
                ProviderKind::IncompleteResponse => "IncompleteResponse",
            };
            let retryability = match provider_error_retryability {
                Retryability::Retryable => "Retryable",
                Retryability::NonRetryable => "NonRetryable",
            };
            (kind, retryability)
        }
        _ => ("Absent", "Absent"),
    };
    (
        kind,
        if error.terminal { "true" } else { "false" },
        outcome,
        reason,
        provider_kind,
        provider_retryability,
    )
}

async fn drive(
    phase: &'static str,
    session: &ConnectSession,
    admission: &AcceptOutcome,
    completion: CompletionHandle,
    approvals: &LocalApprovals,
    approval_presenter: &mut ApprovalPresenter,
    forge: &mut ForgePresenter,
    input: &mut TerminalInput,
    output: &mut Human,
) -> Result<String, ConnectError> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let wait = completion.try_wait_with_terminal_outcome();
    tokio::pin!(wait);
    let mut completion_observed = false;
    let observed=async {
        loop {
            tokio::select! {biased;
                _=tokio::time::sleep_until(deadline)=>return Err(invalid("Composed input observation timed out")),
                result=wait.as_mut()=>{
                    completion_observed=true;
                    let (outcome,terminal)=result.map_err(|e|ConnectError::native("Observe actual input completion",e))?.into_parts();
                    let CompletionOutcome::Completed(result)=outcome else {
                        let completion_kind=match &outcome {
                            CompletionOutcome::Completed(_)=>"Completed",
                            CompletionOutcome::CompletedWithoutResult=>"CompletedWithoutResult",
                            CompletionOutcome::CallbackPending {..}=>"CallbackPending",
                            CompletionOutcome::CallbackBatchPending {..}=>"CallbackBatchPending",
                            CompletionOutcome::Cancelled=>"Cancelled",
                            CompletionOutcome::Abandoned {..}=>"Abandoned",
                            CompletionOutcome::AbandonedWithError {..}=>"AbandonedWithError",
                            CompletionOutcome::CompletedWithFinalizationFailure {..}=>"CompletedWithFinalizationFailure",
                            CompletionOutcome::RuntimeTerminated {..}=>"RuntimeTerminated",
                            CompletionOutcome::OutcomeUnknown {..}=>"OutcomeUnknown",
                        };
                        let terminal_kind=match &terminal {
                            None=>"None",
                            Some(InputTerminalOutcome::Consumed)=>"Consumed",
                            Some(InputTerminalOutcome::Superseded {..})=>"Superseded",
                            Some(InputTerminalOutcome::Coalesced {..})=>"Coalesced",
                            Some(InputTerminalOutcome::Abandoned {..})=>"Abandoned",
                            Some(_)=>"Unknown",
                        };
                        let (error_kind,error_terminal,error_outcome,error_reason,provider_kind,provider_retryability)=match &outcome {
                            CompletionOutcome::Abandoned {error,..}
                            | CompletionOutcome::AbandonedWithError {error,..}
                            | CompletionOutcome::CompletedWithFinalizationFailure {error}
                            | CompletionOutcome::RuntimeTerminated {error,..}=>turn_error_labels(error),
                            CompletionOutcome::Completed(_)
                            | CompletionOutcome::CompletedWithoutResult
                            | CompletionOutcome::CallbackPending {..}
                            | CompletionOutcome::CallbackBatchPending {..}
                            | CompletionOutcome::Cancelled
                            | CompletionOutcome::OutcomeUnknown {..}=>("Absent","Absent","Absent","Absent","Absent","Absent"),
                        };
                        return Err(invalid(&format!("The actual {phase} input did not complete: completion={completion_kind}; terminal={terminal_kind}; error_kind={error_kind}; error_terminal={error_terminal}; error_outcome={error_outcome}; error_reason={error_reason}; provider_kind={provider_kind}; provider_retryability={provider_retryability}")));
                    };
                    require(terminal==Some(InputTerminalOutcome::Consumed),"Live completion lacks exact Consumed outcome")?;
                    return Ok(result.text);
                },
                prompt=forge.next()=>{
                    let prompt=prompt.ok_or_else(||invalid("The retained Forge presenter ended"))?;
                    let interrupt=async {tokio::time::sleep_until(deadline).await;Ok(())};
                    let presented=present_forge_selection(prompt,input,output,interrupt).await;
                    // Use the exact CLI mapping and exact native admission.
                    if !matches!(presented,Ok(true)) {completion_observed=true;}
                    if !crate::main_connector::continue_after_presentation(presented,session,admission,approvals,wait.as_mut()).await? {
                        return Err(invalid("The composed presenter stopped the conversation"));
                    }
                },
                prompt=approval_presenter.next()=>{
                    let prompt=prompt.ok_or_else(||invalid("The retained native approval presenter ended"))?;
                    let interrupt=async {tokio::time::sleep_until(deadline).await;Ok(())};
                    if !present_approval(prompt,input,output,interrupt).await? {
                        return Err(invalid("The actual dispatch approval stopped"));
                    }
                },
            }
        }
    }.await;
    if observed.is_err() && !completion_observed {
        // The same completion remains borrowed until native cancellation has
        // settled. No replacement input or retry is submitted during cleanup.
        let cancelled = session.cancel(admission).await.map(|_| ());
        let settled = wait
            .as_mut()
            .await
            .map_err(|e| ConnectError::native("Join actual input after fixture failure", e))
            .map(|_| ());
        combine(observed, combine(cancelled, settled))
    } else {
        observed
    }
}

#[tokio::test]
async fn generic_submit_survives_native_service_finalization_in_same_conversation() {
    // Non-hex opaque auth canaries cannot overlap the displayed hex identities.
    for secret in [NONCE, LOGIN, PASSWORD] {
        assert!(!host().to_string().contains(secret));
    }
    let server = Server::open().await;
    let (front, presenter) = crate::main_connector::standalone_forge(None).unwrap();
    let mut owner: Option<Opened> = None;
    let mut failed_closed: Option<crate::session::ClosedSetup> = None;
    let mut retained_directory: Option<tempfile::TempDir> = None;
    let mut retained_model_peer: Option<ModelPeer> = None;
    // Retain the current native owner outside the caught assertion schedule.
    // The continuation below replaces it only after exact close and cleanup.
    let mut resumed_owner: Option<(
        ConnectSession,
        Arc<NativeSetup>,
        crate::terminal_connector::TerminalConnectors,
    )> = None;
    let checked=std::panic::AssertUnwindSafe(async {
        owner=Some(Opened::open_with_forge(Some((front.clone(),presenter))).await?);
        let fixture=owner.as_mut().unwrap();
        let id=fixture.session()?.id().clone();
        let root=fixture._directory.path().to_owned();
        let storage=crate::RealmOptions {context_root:root.clone(),state_root:Some(root.join("state")),user_config_root:Some(root.join("home"))}.resolve(REALM)?;
        let artifacts=root.join("setup-composition");std::fs::create_dir(&artifacts).unwrap();
        std::fs::set_permissions(&artifacts,std::fs::Permissions::from_mode(0o700)).unwrap();
        let (lazy,services)=fixture.setup.take_composition()?;
        lazy.set_forge_artifacts(&artifacts)
            .map_err(|error| ConnectError::native("Prepare initial fixture Forge recovery", error))?;
        let mut forge=lazy.take_forge_presenter()?;
        let (mut input,mut human)=terminal(&server.state.origin);
        script(&fixture.client,&[("select-first",FORGE_SELECT_HOST,json!({})),
            ("inspect-first",FORGE_INSPECT,json!({})),
            ("prepare-first",FORGE_PREPARE,json!({"configuration":{"mode":"fixture"},"expected_revision":"opaque-v1"})),
            ("submit-once",FORGE_SUBMIT,json!({"change_id":CHANGE}))]);
        let (admission,completion)=fixture.session()?.submit("Configure the selected generic application once, then keep the service setup conversation available.".into()).await?;
        let answer=drive("original",fixture.session.as_ref().unwrap(),&admission,completion,&fixture.approvals,
            &mut fixture.approval_presenter,&mut forge,&mut input,&mut human).await?;
        let AcceptOutcome::Accepted {input_id:first_id,..}=&admission else {return Err(invalid("Original input was not newly admitted"));};
        let first_id=first_id.clone();
        let runtime=LogicalRuntimeId::for_session(&id);
        let rows=settled_history(fixture.store.as_ref(),&runtime,std::slice::from_ref(&first_id)).await?;
        require(rows[0].seed.terminal_outcome==Some(InputTerminalOutcome::Consumed),"Original durable row is not Consumed")?;
        let row=serde_json::to_value(&rows[0]).map_err(|e|ConnectError::native("Read durable original",e))?;
        let outcome:CompletionOutcome=serde_json::from_value(row["terminal_completion"]["outcome"].clone())
            .map_err(|e|ConnectError::native("Read original terminal completion",e))?;
        require(matches!(outcome,CompletionOutcome::Completed(result) if result.text==answer),"Durable original disagrees with live completion")?;
        let submitted=value(&result(&fixture.client,"submit-once")?)?;
        require(submitted["observation_json"]==SUBMITTED && submitted["recovery_only"]==true,
            "The opaque generic observation was changed or treated as fresh write authority")?;
        require(!fixture.approvals.has_uncertain_dispatch(),"Completed generic HTTP dispatch was misclassified as uncertain native dispatch")?;
        require(human.approved.len()==1,"The actual generic submit did not receive exactly one protected approval")?;

        // Reuse the checked-package consumer fixture. Its launch descriptor is
        // a separate real line-protocol peer; no actual service effect is claimed.
        let service_calls=root.join("service-peer-calls");
        let peer=MCP_PEER.replace("setup_help","actual_read");
        let package=crate::stdio::tests::selected_public_fixture_at(&root,
            Some(json!({"authentication":{"kind":"none"},"operation_tools":{"declared/read":"actual_read"}})),
            false,id.clone(),REALM,json!({"command":"/bin/sh","args":["-c",peer,"service-composition",service_calls]})).await;
        let dispatcher=fixture.connectors.dispatcher(package.owner.clone());
        let args=serde_json::value::to_raw_value(&json!({"job_id":"3".repeat(32),"operations":["actual_read"],"agent":"rkat-agent","expires_in_ms":null})).unwrap();
        let context=ToolDispatchContext::default().with_runtime_identity(id.clone(),None);
        let request=dispatcher.dispatch_with_context(ToolCallView{id:"service-review",name:crate::terminal_connector::PLAIN_TOOL,args:&args},&context);
        let show=async {
            let request=tokio::time::timeout(Duration::from_secs(10),fixture.connectors.next()).await
                .map_err(|e|ConnectError::native("Observe real service request",e))?
                .ok_or_else(||invalid("The real service request did not reach its presenter"))?;
            fixture.connectors.present(request,&fixture.approvals,&mut fixture.approval_presenter,&mut input,&mut human,
                async {tokio::time::sleep(Duration::from_secs(10)).await;Ok(())}).await
        };
        let (reply,presented)=tokio::join!(request,show);
        require(presented? && reply.is_ok(),"Actual public-service protected review did not settle")?;
        let reviewed=fixture.connectors.take_reviewed_action().ok_or_else(||invalid("No real reviewed service handoff"))?;
        require(!fixture.approvals.has_uncertain_dispatch(),"The review changed dispatch settlement")?;
        require(human.approved.len()==2,"The service handoff reused the earlier Forge approval")?;
        input.check_cleanup().map_err(|e|ConnectError::native("Restore protected terminal",e))?;
        drop(dispatcher);drop(package);drop(forge);drop(services);drop(lazy);

        let mut old=owner.take().unwrap();
        let session=old.session.take().unwrap();
        match session.try_close_for_finalization().await {
            FinalizationClose::Closed(closed)=>{
                old.approvals.retire();
                if let Err(error)=old.connectors.retire() {
                    failed_closed=Some(closed);owner=Some(old);return Err(error);
                }
                match closed.finish_host_cleanup().await {
                    Ok(())=>(),
                    Err((closed,error))=>{
                        // Retain both the exact cleanup owner and its directory
                        // outside this schedule. Failure cannot admit finalization.
                        failed_closed=Some(closed);owner=Some(old);return Err(error);
                    }
                }
            },
            FinalizationClose::Busy(session)|FinalizationClose::Unconfirmed {session,..}=>{
                old.session=Some(session);owner=Some(old);
                return Err(invalid("Actual native close did not report Closed"));
            }
        }
        old.setup.retire();
        let Opened {continuation,client,model_peer,_directory:directory,mcp_calls,stock,store,setup,approvals,
            connectors,approval_presenter,refused_close,session:_,..}=old;
        drop((stock,store,setup,approvals,connectors,approval_presenter,refused_close));
        retained_directory=Some(directory);
        retained_model_peer=model_peer;
        let interrupt=std::future::pending::<io::Result<()>>();tokio::pin!(interrupt);
        let mut finalized=reviewed.finalize_outcome_unless_cancelled(interrupt.as_mut()).await?
            .ok_or_else(||invalid("The real native finalizer never entered"))?;
        finalized.display_for_continuation(&mut human)?;
        let installation=finalized.installation().ok_or_else(||invalid("The real finalizer returned no installation outcome"))?;
        let finalization_facts=installation.notice();
        require(finalization_facts["facts"]["persistence"]=="saved"
            && finalization_facts["facts"]["grant"]["state"]=="selected"
            && finalization_facts["server"]=="outside-server"
            && finalization_facts["consumer_agent"]=="rkat-agent"
            && finalization_facts["service_readiness"]=="not_observed",
            "The actual native outcome did not save and select the reviewed service")?;
        let (servers,_)=crate::realm::read_own_mcp_config(&storage).await?;
        require(servers.iter().any(|server|server.name=="outside-server" && server.access.as_ref()
            .is_some_and(|access|access.allowed_operations==BTreeSet::from(["actual_read".into()]))),"The actual config did not retain the reviewed service ceiling")?;
        let notice=finalized.take_continuation_notice().ok_or_else(||invalid("The actual finalizer omitted its one notice"))?;
        require(finalized.take_continuation_notice().is_none(),"The finalizer occurrence was reusable")?;
        let finalizations=vec![installation];
        let (fresh,presenter)=crate::main_connector::standalone_forge(Some(front))?;
        let options=SessionOptions {model:native_model_peer::model_request(REALM,MODEL)
            .map_err(|e|ConnectError::native("Select resumed native fixture binding",e))?,build:Default::default()};
        let mut resumed=continuation.reopen().await?;
        let saved_message_count=resumed.session.messages().len();
        let saved_system_rows=system_rows(resumed.session.messages());
        let (approvals,mut approval_presenter)=LocalApprovals::create_paired_host(&artifacts)?;
        let approvals=Arc::new(approvals);approvals.bind_session(&id)?;
        let connectors=crate::terminal_connector::TerminalConnectors::open_with_finalizations(resumed.startup(),finalizations.into()).await?;
        let (paired_host,_)=crate::paired_host_tool::PairedHostFrontend::deferred();
        let setup=NativeSetup::with_forge(id.clone(),storage.locator.clone(),paired_host,&connectors,approvals.clone(),(fresh.clone(),presenter))?;
        let resumed_store=resumed.startup().persistence.as_ref()
            .ok_or_else(||invalid("Resumed native persistence is missing"))?.runtime_store();
        let session=resumed.open_native_setup(options,setup.clone(),&mut human,interrupt.as_mut()).await?
            .ok_or_else(||invalid("The exact conversation did not reopen"))?;
        resumed_owner=Some((session,setup,connectors));
        let (session,setup,_)=resumed_owner.as_ref().unwrap();
        require(session.id()==&id,"Continuation created a different native conversation")?;
        require(saved_system_rows.first().is_some_and(|(index, text)| *index==0
            && text.contains(crate::session::SETUP_INSTRUCTIONS)),"Saved conversation lost its original setup instructions")?;
        let reopened_history=session.service.read_history(&id,SessionHistoryQuery {
            offset:0,limit:Some(saved_message_count+1),
        }).await.map_err(|e|ConnectError::native("Read reopened setup instructions",e))?;
        require(!reopened_history.has_more && system_rows(&reopened_history.messages)==saved_system_rows,
            "Reopen changed the saved System instructions or appended a new prompt")?;
        let (lazy,services)=setup.take_composition()?;
        lazy.set_forge_artifacts(&artifacts)
            .map_err(|error| ConnectError::native("Prepare continued fixture Forge recovery", error))?;
        let mut forge=lazy.take_forge_presenter()?;
        let start=human.text.len();
        let first_resumed_request=client.3.lock().unwrap().len();
        // Borrow this one process-owned occurrence. Native's complete model
        // projection includes both its summary and structured event block.
        let expected_notice=match meerkat_runtime::input::runtime_input_run_started_content(notice.input_for_test()) {
            Some(meerkat_core::types::ContentInput::Text(text)) if !text.is_empty()=>text,
            _=>return Err(invalid("The actual finalization notice has no single native text projection")),
        };
        *client.4.lock().unwrap()=Some((SUBMITTED.into(),expected_notice.clone()));
        script(&client,&[("before-selection",FORGE_INSPECT,json!({})),
            ("select-fresh",FORGE_SELECT_HOST,json!({})),
            ("submit-once",FORGE_SUBMIT,json!({"change_id":CHANGE})),
            ("submit-new-call",FORGE_SUBMIT,json!({"change_id":CHANGE})),
            ("foreign-status",FORGE_STATUS,json!({"change_id":"foreign-change"})),
            ("prepare-again",FORGE_PREPARE,json!({"configuration":{},"expected_revision":"opaque-v1"})),
            ("status-after-finalization",FORGE_STATUS,json!({"change_id":CHANGE}))]);
        let (admission,completion)=session.submit_finalization(notice).await?;
        let AcceptOutcome::Accepted {input_id:next_id,..}=&admission else {return Err(invalid("Continuation notice was not newly admitted"));};
        require(next_id!=&first_id,"Continuation reused the original input occurrence")?;
        let next_id=next_id.clone();
        let next_answer=drive("resumed-finalization",session,&admission,completion,&approvals,&mut approval_presenter,&mut forge,&mut input,&mut human).await;
        if next_answer.is_err() {
            let observed_requests=client.3.lock().unwrap().len();
            let resumed_request_delta=observed_requests-first_resumed_request;
            let remaining_scripted_calls=client.1.lock().unwrap().len();
            let restored_context_check_armed=client.4.lock().unwrap().is_some();
            let _=writeln!(io::stderr(),"forge resumed-finalization fixture: request_delta={resumed_request_delta}; remaining_scripted_calls={remaining_scripted_calls}; restored_context_check_armed={restored_context_check_armed}");
        }
        let next_answer=next_answer?;
        require(next_answer==ANSWER,"Continued native input did not reach the scripted end turn")?;
        require(client.3.lock().unwrap().get(first_resumed_request)
            .is_some_and(|messages|resumed_context_matches(messages,SUBMITTED,&expected_notice)),
            "The first resumed model request did not contain the original observation and one actual finalization notice")?;
        require(client.1.lock().unwrap().is_empty(),"The continued native input skipped scripted operations")?;
        let rows=settled_history(resumed_store.as_ref(),&runtime,&[first_id.clone(),next_id.clone()]).await?;
        for (expected_id,expected_answer) in [(&first_id,&answer),(&next_id,&next_answer)] {
            let row=rows.iter().find(|row|&row.state.input_id==expected_id).ok_or_else(||invalid("Missing exact composed input"))?;
            require(row.seed.terminal_outcome==Some(InputTerminalOutcome::Consumed),"Composed input has no durable Consumed outcome")?;
            let row=serde_json::to_value(row).map_err(|e|ConnectError::native("Read composed completion",e))?;
            let outcome:CompletionOutcome=serde_json::from_value(row["terminal_completion"]["outcome"].clone())
                .map_err(|e|ConnectError::native("Decode composed completion",e))?;
            require(matches!(outcome,CompletionOutcome::Completed(result) if &result.text==expected_answer),"Composed durable completion disagrees with live result")?;
        }
        drop(resumed_store);
        for denied in ["before-selection","submit-once","submit-new-call","foreign-status","prepare-again"] {
            require(result(&client,denied)?.is_error,"Old authority or a foreign status request became executable")?;
        }
        let status=value(&result(&client,"status-after-finalization")?)?;
        require(status["observation_json"]==OBSERVED && status["recovery_only"]==true,"The retained status was invented, lost or made executable")?;
        require(!human.text[start..].contains("Forge Console origin") && !human.text[start..].contains("Forge Console password"),"Continuation discarded or replaced the original private client")?;
        require(human.approved.len()==2 && !approvals.has_uncertain_dispatch(),"Continuation restored or requested obsolete approval")?;
        require(!human.text.contains(PASSWORD) && !human.text.contains(LOGIN),"Private fixture credentials escaped the protected client")?;
        require(value(&result(&client,"select-fresh")?).is_ok(),"Fresh protected selection was not observed by the resumed model")?;
        input.check_cleanup().map_err(|e|ConnectError::native("Restore continued terminal",e))?;
        drop(forge);drop(services);drop(lazy);drop(fresh);drop(approval_presenter);drop(approvals);
        let (session,setup,mut connectors)=resumed_owner.take().unwrap();
        let cleanup=session.close().await;setup.retire();let cleanup=combine(cleanup,connectors.retire());
        drop((setup,connectors,client,continuation,finalized));
        cleanup?;
        for path in [mcp_calls,service_calls] {
            let bytes=std::fs::read(path).or_else(|error|if error.kind()==io::ErrorKind::NotFound {Ok(Vec::new())} else {Err(error)}).unwrap();
            require(bytes.is_empty(),"The composition unexpectedly invoked a synthetic MCP service")?;
        }
        Ok::<_,ConnectError>(())
    }).catch_unwind().await;
    // A failed finalization may retain a separate native cleanup owner. Keep
    // its peer alive through both ordinary and retained cleanup below.
    if retained_model_peer.is_none() {
        retained_model_peer = owner.as_mut().and_then(|owner| owner.model_peer.take());
    }
    let first_cleanup = match owner.as_mut() {
        Some(owner) => owner.finish().await,
        None => Ok(()),
    };
    let next_cleanup = match resumed_owner.take() {
        Some((session, setup, mut connectors)) => {
            let result = session.close().await;
            setup.retire();
            combine(result, connectors.retire())
        }
        None => Ok(()),
    };
    let failed_cleanup = match failed_closed.take() {
        Some(closed) => match closed.finish_host_cleanup().await {
            Ok(()) => Ok(()),
            Err((held, error)) => {
                failed_closed = Some(held);
                Err(error)
            }
        },
        None => Ok(()),
    };
    let mut calls = None;
    let server_cleanup = match server.finish().await {
        Ok(observed) => {
            calls = Some(observed);
            Ok(())
        }
        Err(error) => Err(error),
    };
    let peer_cleanup = match retained_model_peer.take() {
        Some(peer) => peer
            .finish()
            .await
            .map_err(|e| ConnectError::native("Join continued native model fixture peer", e)),
        None => Ok(()),
    };
    let cleanup = combine(
        first_cleanup,
        combine(
            next_cleanup,
            combine(failed_cleanup, combine(server_cleanup, peer_cleanup)),
        ),
    );
    drop(retained_directory);
    match checked {
        Ok(result) => combine(result, cleanup)
            .expect("the real composed finalization and continuation, including retained cleanup"),
        Err(panic) => {
            if let Err(cleanup) = cleanup {
                eprintln!(
                    "Fixture cleanup also failed: {cleanup:?}; preserving the original panic."
                );
            }
            std::panic::resume_unwind(panic)
        }
    }
    let calls = calls.expect("successful graceful server join retained its observations");
    let matching = |verb: &str, path: &str| {
        calls
            .iter()
            .filter(|(v, p, _)| v == verb && p == path)
            .collect::<Vec<_>>()
    };
    assert_eq!(matching("POST", "/login").len(), 1);
    assert!(matching("GET", "/console/forge/frontends/descriptor").len() >= 2);
    assert_eq!(
        matching("POST", "/console/forge/frontends/prepare").len(),
        1
    );
    let submits = matching("POST", "/console/forge/frontends/submit");
    assert_eq!(submits.len(), 1);
    assert_eq!(submits[0].2["proposal"], proposal());
    assert_eq!(submits[0].2["approval"]["approved"], true);
    assert_eq!(submits[0].2["approval"]["proof"], "c".repeat(64));
    assert_eq!(
        matching("GET", "/console/forge/frontends/changes/composition-change").len(),
        1
    );
    assert!(
        !calls
            .iter()
            .any(|(_, path, _)| path.contains("foreign-change"))
    );
}
