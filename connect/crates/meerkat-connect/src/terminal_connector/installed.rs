//! Read-only package discovery for a new setup conversation. Entered package
//! work stays with the terminal presenter until its process owner has joined.
use super::*;
use meerkat::governed_realm::stock::issuance::DEFAULT_STOCK_AGENT;

pub struct Request {
    session: SessionId,
    installs: Arc<CheckedInstalls>,
    action: Action,
    response: oneshot::Sender<Value>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct List { after: Option<String> }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Select { candidate: String }
enum Action { List(List), Select(Select) }

pub(super) fn tools() -> [Arc<ToolDef>; 2] {
    [Arc::new(ToolDef::new(INSTALLED_LIST_TOOL,
        "Find retained service package requests from earlier setup in this realm. Use when credential setup ended before it was saved. Returns only current-session candidate handles and family/profile labels, not installed status or old approval. Select a candidate for current package verification. Pass next as after for another bounded page. Never ask the person for workspace paths or recovery files.",
        json!({"type":"object","additionalProperties":false,"properties":{"after":{"type":"string","maxLength":80}}}))),
    Arc::new(ToolDef::new(INSTALLED_SELECT_TOOL,
        "Verify one retained service candidate from toolkit_installed_service_candidates using the fixed package owner. The result admits its installed job and lists available_setup_tools supported by the exact retained evidence. Choose the public, secret or account route appropriate to the request; an empty list means none is established. Every route still requires its explicit operation proposal, agent, lifetime and fresh protected review; account setup additionally needs the publisher's OAuth descriptor and per-operation requirements. No authentication kind or provider is inferred. Own configured names use current management; inherited names cannot be added here. No installation, approval, native action, account, grant or service-readiness outcome is replayed or inferred.",
        json!({"type":"object","additionalProperties":false,"required":["candidate"],"properties":{"candidate":{"type":"string","minLength":1,"maxLength":80}}})))]
}
fn refused() -> ToolError {
    ToolError::execution_failed("Select a retained candidate from this setup session. No installation or credential action was started.")
}
pub(super) async fn dispatch(owner: &RequestTool, call: ToolCallView<'_>, context: &ToolDispatchContext)
    -> Result<ToolDispatchOutcome, ToolError> {
    if call.args.get().len() > 1024 { return Err(refused()); }
    serde_json::from_str::<crate::forge::UniqueJson>(call.args.get()).map_err(|_| refused())?;
    let session = context.origin_session_id().ok_or_else(refused)?.clone();
    let action = match call.name {
        INSTALLED_LIST_TOOL => {
            let query: List = call.parse_args().map_err(|_| refused())?;
            if query.after.as_ref().is_some_and(|value| value.is_empty() || value.len() > 80) { return Err(refused()); }
            Action::List(query)
        },
        INSTALLED_SELECT_TOOL => {
            let selected: Select = call.parse_args().map_err(|_| refused())?;
            if selected.candidate.is_empty() || selected.candidate.len() > 80 { return Err(refused()); }
            Action::Select(selected)
        },
        _ => return Err(refused()),
    };
    let (response, receive) = oneshot::channel();
    owner.requests.try_send(ConnectorRequest::Installed(Request {
        session, installs:owner.installs.clone(), action, response,
    })).map_err(|_| refused())?;
    let result = receive.await.map_err(|_| ToolError::execution_failed("The retained package read returned no observation. No prior or current native outcome is resolved."))?;
    Ok(ToolResult::new(call.id.into(), result.to_string(), false).into())
}

// The outer conversation awaits this presenter before retiring setup. A caller
// disappearing or interrupting stops admission, but never drops entered work.
enum ReadEnd { Observed(Result<Value, ConnectError>), CallerGone, Stopped }
async fn joined_read<F, I>(operation: F, caller: &mut oneshot::Sender<Value>, interrupt: I)
    -> Result<ReadEnd, ConnectError>
where F: Future<Output = Result<Value, ConnectError>>, I: Future<Output = io::Result<()>> {
    tokio::pin!(operation, interrupt);
    let mut entered = false;
    let stopped = tokio::select! { biased;
        signal = interrupt.as_mut() => Some(signal),
        _ = caller.closed() => None,
        result = std::future::poll_fn(|cx| { entered = true; operation.as_mut().poll(cx) }) => return Ok(ReadEnd::Observed(result)),
    };
    // InstalledResources bounds the operation, signals its fixed owner on
    // timeout and awaits that owner's exit, including its metadata child.
    if entered { let _ = operation.await; }
    match stopped {
        Some(signal) => { signal.map_err(|e| ConnectError::native("Listen for retained package cancellation", e))?; Ok(ReadEnd::Stopped) },
        None => Ok(ReadEnd::CallerGone),
    }
}
fn local_unconfirmed(response: oneshot::Sender<Value>) -> Result<bool, ConnectError> {
    let _ = response.send(json!({"state":"unconfirmed","phase":"retained_package_read",
        "notice":"This package selection is unavailable or changed. No installation, approval or native action was replayed. Other candidates and requested services remain usable; earlier uncertain outcomes remain unresolved."}));
    Ok(true)
}

// This is a projection of the existing route validators, not an authentication
// classifier. The agent still supplies each chosen tool's explicit proposal.
fn selected_result(selected: &crate::service_install::InstalledConnectorSelection) -> Value {
    let mut available_setup_tools = Vec::new();
    if selected.public_configuration(&selected.raw_operations).is_ok() {
        available_setup_tools.push(PLAIN_TOOL);
    }
    if selected.secret_configuration(&selected.raw_operations).is_ok() {
        available_setup_tools.push(SECRET_TOOL);
    }
    if selected.check_current().is_ok()
        && !selected.raw_operations.is_empty() && selected.raw_operations.len() <= 128
        && selected.native_connector.as_ref().is_some_and(|entry| {
            crate::connector_install::native_launch_configuration(
                &selected.installed, &selected.configuration, entry,
            ).is_ok()
        })
    {
        available_setup_tools.push(TOOL);
    }
    json!({"state":"installed_package_selected","job_id":selected.installed.job_id,
        "service":selected.installed.server_name,"family":selected.installed.family,"profile":selected.installed.profile,
        "declared_raw_operations":selected.raw_operations,"default_stock_agent":DEFAULT_STOCK_AGENT,
        "available_setup_tools":available_setup_tools,
        "notice":"Current package evidence is selected for this conversation only. Choose among the available setup tools for the person's request, with an explicit operation subset, agent and lifetime. An empty list means no supported route is established; do not guess authentication or a provider. Account setup still requires an explicit publisher OAuth descriptor and per-operation scopes and destinations; every route requires fresh protected review and any applicable private input. No old approval, install authority, credential, grant, native action outcome or service readiness was restored."})
}

pub(super) async fn present<F: Future<Output = io::Result<()>> + Send>(
    owner: &mut TerminalConnectors, mut request: Request, approvals: &LocalApprovals, interrupt: F,
) -> Result<bool, ConnectError> {
    if request.response.is_closed() { return Ok(true); }
    if approvals.check_session(&request.session).is_err() { return local_unconfirmed(request.response); }
    let realm = &owner.storage.locator;
    let query = match &request.action {
        Action::List(query) => request.installs.retained_list_request(realm, &request.session, query.after.as_deref()),
        Action::Select(selected) => request.installs.retained_select_request(realm, &request.session, &selected.candidate),
    };
    let Ok(query) = query else { return local_unconfirmed(request.response); };
    let resources = match crate::resources::InstalledResources::locate() {
        Ok(value) => value, Err(_) => return local_unconfirmed(request.response),
    };
    let result = joined_read(resources.readback(&owner.storage, &query), &mut request.response, interrupt).await?;
    let result = match result {
        ReadEnd::Stopped => return Ok(false), ReadEnd::CallerGone => return Ok(true),
        ReadEnd::Observed(Err(_)) => return local_unconfirmed(request.response), ReadEnd::Observed(Ok(value)) => value,
    };
    if request.response.is_closed() { return Ok(true); }
    if approvals.check_session(&request.session).is_err() { return local_unconfirmed(request.response); }
    match request.action {
        Action::List(_) => {
            let result = request.installs.retain_candidates(realm, &request.session, result);
            match result { Ok(value) => { let _ = request.response.send(value); Ok(true) }, Err(_) => local_unconfirmed(request.response) }
        },
        Action::Select(selected) => {
            let checked = request.installs.checked_retained(realm, &request.session, &selected.candidate, &result);
            let Ok(checked) = checked else { return local_unconfirmed(request.response); };
            if plain::recheck_retained(&checked.installed, &checked.configuration, checked.selected_inputs()).await.is_err() {
                return local_unconfirmed(request.response);
            }
            // Names already owned by this realm use current management. A
            // matching inherited name never becomes an own initial add.
            let effective = match crate::realm::read_config(&owner.storage).await {
                Ok(value) => value, Err(_) => return local_unconfirmed(request.response),
            };
            let own = match crate::realm::read_own_mcp_config(&owner.storage).await {
                Ok((value, _)) => value, Err(_) => return local_unconfirmed(request.response),
            };
            if request.response.is_closed() { return Ok(true); }
            if approvals.check_session(&request.session).is_err() { return local_unconfirmed(request.response); }
            let service = &checked.installed.server_name;
            if own.iter().any(|config| config.name == *service) {
                let _ = request.response.send(json!({"state":"own_service_exists","service":service,
                    "notice":"This realm already owns that service name. Inspect current credential/account and access inventories, then use their fresh management flow. No old package approval or initial add is restored; this name observation does not verify that the current definition matches the package."}));
                return Ok(true);
            }
            if effective.tools.mcp_servers.iter().any(|config| config.name == *service) {
                let _ = request.response.send(json!({"state":"unavailable","reason":"inherited_service_name","service":service,
                    "notice":"An inherited definition owns this name. Initial setup cannot shadow it and this realm's own-service management cannot change it."}));
                return Ok(true);
            }
            if owner.target_unresolved(service) { return local_unconfirmed(request.response); }
            let admitted = request.installs.admit_retained(realm, &request.session, &selected.candidate, checked);
            let Ok(admitted) = admitted else { return local_unconfirmed(request.response); };
            if admitted.check_current().is_err() { return local_unconfirmed(request.response); }
            let _ = request.response.send(selected_result(&admitted));
            Ok(true)
        },
    }
}

#[cfg(test)]
mod controls {
    use super::*;
    #[tokio::test]
    async fn disappeared_caller_and_interrupt_join_entered_package_read_before_return() {
        let (mut response, _receive)=oneshot::channel();
        let operation=std::future::poll_fn(|_| -> std::task::Poll<Result<Value,ConnectError>> {panic!("pre-entry cancellation must not start package work")});
        assert!(matches!(joined_read(operation,&mut response,async {Ok(())}).await.unwrap(),ReadEnd::Stopped));
        for caller_gone in [false, true] {
            let (release, wait) = oneshot::channel();
            let (mut response, receive) = oneshot::channel();
            let (signal, interrupt) = oneshot::channel();
            let joined = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let done = joined.clone();
            let operation = async move { wait.await.unwrap(); done.store(true, std::sync::atomic::Ordering::SeqCst); Ok(json!({})) };
            let mut held = Box::pin(joined_read(operation, &mut response, async { interrupt.await.unwrap(); Ok(()) }));
            assert!(held.as_mut().now_or_never().is_none());
            if caller_gone { drop(receive); } else { signal.send(()).unwrap(); }
            assert!(held.as_mut().now_or_never().is_none());
            assert!(!joined.load(std::sync::atomic::Ordering::SeqCst));
            release.send(()).unwrap();
            let result = tokio::time::timeout(std::time::Duration::from_secs(5), held.as_mut())
                .await
                .expect("released retained-package fixture read did not settle")
                .unwrap();
            assert!(joined.load(std::sync::atomic::Ordering::SeqCst));
            assert!(matches!((caller_gone, result), (true, ReadEnd::CallerGone) | (false, ReadEnd::Stopped)));
        }
    }
}

#[cfg(test)]
mod returning_account;
