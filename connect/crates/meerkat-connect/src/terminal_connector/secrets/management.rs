//! Explicit administration of an unchanged realm-owned service definition.
//! Inventory and presence are current observations, never update receipts.

use super::*;
use serde::Serialize;
use meerkat_core::mcp_config::McpTransportConfig;

const PAGE_SIZE: usize = 16;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Inventory { after: Option<String> }

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Action { Set, Remove }

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Change { service: String, slots: Vec<String>, action: Action }

pub struct Request {
    session: SessionId,
    action: Requested,
    response: oneshot::Sender<Value>,
}
enum Requested { Inventory(Inventory), Change(Change) }

/// Direct local administration. No credential value, model, service launch or
/// setup permission can be supplied through this entry.
pub enum ServiceCredentialCommand {
    Status { after: Option<String> },
    Set { service: String, slots: Vec<String> },
    Remove { service: String, slots: Vec<String> },
}

/// Use the same realm and native persistence as ordinary credential management,
/// without entering the setup conversation or preparing its supplied helper.
pub async fn run_service_credential_command<F: Future<Output = io::Result<()>> + Send, W: Write>(
    storage: ResolvedStorage, command: ServiceCredentialCommand,
    input: &mut TerminalInput, output: &mut W, interrupt: F,
) -> Result<(), ConnectError> {
    let factory = SessionStartup::realm_factory(&storage);
    run_direct(&storage, &factory, command, input, output, interrupt).await
}

async fn run_direct<F: Future<Output = io::Result<()>> + Send, W: Write>(
    storage: &ResolvedStorage, factory: &meerkat::AgentFactory, command: ServiceCredentialCommand,
    input: &mut TerminalInput, output: &mut W, interrupt: F,
) -> Result<(), ConnectError> {
    StockPrincipals::for_local_operator(&storage.locator.realm)
        .map_err(|_| unavailable("The selected realm's local operator is unavailable"))?;
    tokio::pin!(interrupt);
    let change = match command {
        ServiceCredentialCommand::Status { after } => {
            if after.as_ref().is_some_and(|value| value.is_empty() || value.len() > 256 || value.chars().any(char::is_control)) {
                return Err(unavailable("Use an exact service credential inventory cursor"));
            }
            let query = Inventory { after };
            let facts = tokio::select! { biased;
                signal = interrupt.as_mut() => {
                    signal.map_err(|error| ConnectError::native("Listen for credential inventory cancellation", error))?;
                    return Err(unavailable("Credential inventory was cancelled"));
                },
                result = inventory(storage, factory, &query) => result.map_err(unavailable)?,
            };
            return writeln!(output, "{facts}").and_then(|()| output.flush())
                .map_err(|error| ConnectError::native("Display credential inventory", error));
        },
        ServiceCredentialCommand::Set { service, slots } => Change { service, slots, action: Action::Set },
        ServiceCredentialCommand::Remove { service, slots } => Change { service, slots, action: Action::Remove },
    };
    if !valid_change(&change) { return Err(unavailable("Select an exact service and unique declared credential slots")); }
    input.require_terminal().map_err(|error| ConnectError::native("Require protected credential terminal", error))?;
    let artifacts = crate::resources::reserve_setup_artifacts(storage)?;
    let (approvals, mut presenter) = LocalApprovals::create(&artifacts)?;
    // This ID binds only the fresh local decision owner. No agent or
    // native runtime session is created for direct administration.
    let session = SessionId::new();
    approvals.bind_session(&session)?;
    let requester = ApprovalPrincipalId::new("connect-local-operator")
        .map_err(|error| ConnectError::native("Name credential command requester", error))?;
    let (response, mut received) = oneshot::channel();
    let presented = manage(storage, factory, false, &session, change, response,
        Some(requester), &approvals, &mut presenter, input, output, interrupt.as_mut()).await;
    approvals.retire();
    input.check_cleanup().map_err(|error| ConnectError::native("Restore credential terminal", error))?;
    presented?;
    let facts = received.try_recv().map_err(|_| unavailable(
        "Credential change ended without a returned outcome. Do not retry automatically."))?;
    if facts.get("state").and_then(Value::as_str) == Some("updated") { return Ok(()); }
    writeln!(output, "{facts}").and_then(|()| output.flush())
        .map_err(|error| ConnectError::native("Display credential change refusal", error))?;
    Err(unavailable("Credential change was not confirmed. Check current presence; do not retry an uncertain action automatically."))
}

fn refused() -> ToolError {
    ToolError::execution_failed("Use the current service credential inventory and select exact declared slots. Credential values, commands, configuration and grants are not accepted by these tools.")
}

fn request_refused(reason: &'static str, response: oneshot::Sender<Value>, output: &mut impl Write)
    -> Result<bool, ConnectError> {
    let message = "This credential request could not proceed. No credential change was started by this request. Check the selected realm's current credential inventory before choosing a new action; do not repeat an earlier uncertain action automatically.";
    let _ = response.send(json!({"state":"unavailable","phase":"service_credential_management",
        "reason":reason,"instruction":message,"configuration_changed":false,"grant_changed":false}));
    writeln!(output, "{message}").map_err(|error| ConnectError::native("Display credential request refusal", error))?;
    Ok(true)
}

pub(in crate::terminal_connector) fn tools() -> [Arc<ToolDef>; 2] {
    [Arc::new(ToolDef::new(SECRET_STATUS_TOOL,
        "Read one page of current non-OAuth service credential presence in the selected standalone realm. Returns only exact owned stdio service identifiers and declared slot names/presence; no commands, inline environment, credential values or inherited definitions. Use this when the person asks to finish, update or remove a service's credentials, so they need not know technical names. Pass the returned next cursor as after for another page. Stored is presence only, not a vendor account check or proof about any earlier uncertain attempt. This performs no mutation or retry.",
        json!({"type":"object","additionalProperties":false,"properties":{"after":{"type":"string","maxLength":256}}}))),
     Arc::new(ToolDef::new(SECRET_MANAGE_TOOL,
        "Propose an explicit credential change for a service and declared slots returned by toolkit_service_credential_status. set fills or replaces only the selected slots; remove deletes only those selected credentials. The protected terminal shows current presence and consequences, obtains fresh approval and collects replacement values privately. Never ask for or supply credentials in chat. This does not reinstall or change the service definition, agent access, OAuth accounts or frontend roles. It may repair a partially configured service after a stopped initial installation, but creates no missing grant. Earlier uncertain attempts remain uncertain; a new approved change does not settle them. No automatic retry. Existing running processes can retain old environment values.",
        json!({"type":"object","additionalProperties":false,"required":["service","slots","action"],
            "properties":{"service":{"type":"string","minLength":1,"maxLength":256},
                "slots":{"type":"array","minItems":1,"maxItems":256,"uniqueItems":true,
                    "items":{"type":"string","minLength":1,"maxLength":256}},
                "action":{"enum":["set","remove"]}}})))]
}

pub(in crate::terminal_connector) async fn dispatch(owner: &RequestTool, call: ToolCallView<'_>, context: &ToolDispatchContext)
    -> Result<ToolDispatchOutcome, ToolError> {
    if call.args.get().len() > 32 * 1024 { return Err(refused()); }
    serde_json::from_str::<crate::forge::UniqueJson>(call.args.get()).map_err(|_| refused())?;
    let session = context.origin_session_id().ok_or_else(refused)?.clone();
    let action = if call.name == SECRET_STATUS_TOOL {
        let request: Inventory = call.parse_args().map_err(|_| refused())?;
        if request.after.as_ref().is_some_and(|name| name.is_empty() || name.len() > 256 || name.chars().any(char::is_control)) {
            return Err(refused());
        }
        Requested::Inventory(request)
    } else if call.name == SECRET_MANAGE_TOOL {
        let request: Change = call.parse_args().map_err(|_| refused())?;
        if !valid_change(&request) { return Err(refused()); }
        Requested::Change(request)
    } else { return Err(refused()); };
    let (response, receive) = oneshot::channel();
    owner.requests.try_send(ConnectorRequest::CredentialManagement(Request {session, action, response}))
        .map_err(|_| refused())?;
    let result = receive.await.map_err(|_| ToolError::execution_failed(
        "Credential management ended without a returned result. Do not infer that an entered update failed or repeat it automatically."))?;
    Ok(ToolResult::new(call.id.into(), result.to_string(), false).into())
}

fn valid_change(change: &Change) -> bool {
    !change.service.is_empty() && change.service.len() <= 256 && !change.service.chars().any(char::is_control)
        && !change.slots.is_empty() && change.slots.len() <= 256
        && change.slots.iter().collect::<BTreeSet<_>>().len() == change.slots.len()
        && change.slots.iter().all(|slot| !slot.is_empty() && slot.len() <= 256 && !slot.chars().any(char::is_control))
}

fn declared(config: &McpServerConfig) -> Option<&BTreeSet<String>> {
    match &config.transport {
        McpTransportConfig::Stdio(stdio) if !stdio.secret_env.is_empty() => Some(&stdio.secret_env),
        _ => None,
    }
}

async fn owned(storage: &ResolvedStorage) -> Result<Vec<McpServerConfig>, &'static str> {
    let (mut servers, _) = crate::realm::read_own_mcp_config(storage).await
        .map_err(|_| "owned_definitions_unavailable")?;
    servers.sort_by(|left, right| left.name.cmp(&right.name));
    if servers.windows(2).any(|pair| pair[0].name == pair[1].name) {
        return Err("owned_definitions_ambiguous");
    }
    servers.retain(|server| declared(server).is_some());
    Ok(servers)
}

async fn current(storage: &ResolvedStorage, config: &McpServerConfig) -> Result<(), &'static str> {
    let servers = owned(storage).await?;
    if servers.iter().filter(|server| server.name == config.name).eq(std::iter::once(config)) { Ok(()) }
    else { Err("owned_definition_changed_or_absent") }
}

fn exact_observation(observed: &meerkat::McpServiceSecretObservation, storage: &ResolvedStorage, config: &McpServerConfig) -> bool {
    observed.owner == storage.locator.realm && declared(config).is_some_and(|names|
        observed.slots.keys().collect::<BTreeSet<_>>() == names.iter().collect())
}

fn source(storage: &ResolvedStorage, persistence: &meerkat_auth_core::auth_store::ProviderAuthPersistence)
    -> Result<meerkat::RealmMcpServiceSecrets, &'static str> {
    Ok(meerkat::RealmMcpServiceSecrets::from_realm_source(crate::realm::config_source(storage).map_err(|_| "realm_source_unavailable")?,
        storage.locator.realm.clone(), persistence.token_store().clone()))
}

fn persistence(factory: &meerkat::AgentFactory) -> Result<meerkat_auth_core::auth_store::ProviderAuthPersistence, &'static str> {
    factory.resolution_provider_auth_persistence()
        .map_err(|_| "original_persistence_unavailable")?
        .ok_or("original_persistence_unavailable")
}

async fn inventory(storage: &ResolvedStorage, factory: &meerkat::AgentFactory, request: &Inventory)
    -> Result<Value, &'static str> {
    let persistence = persistence(factory)?;
    let source = source(storage, &persistence)?;
    let servers = owned(storage).await?;
    let selected = servers.iter().filter(|server| request.after.as_ref().is_none_or(|after| server.name > *after))
        .take(PAGE_SIZE + 1).collect::<Vec<_>>();
    let mut rows = Vec::new();
    for config in selected.iter().take(PAGE_SIZE) {
        let observed = source.observe(config).await;
        current(storage, config).await?;
        let presence = match observed {
            Ok(observed) if exact_observation(&observed, storage, config) => observation_facts(&Some(Ok(observed))),
            _ => json!({"state":"unavailable","scope":"presence_only_not_update_receipt_or_vendor_authentication"}),
        };
        rows.push(json!({"service":config.name,"declared_slots":declared(config),"presence":presence}));
    }
    let next = (selected.len() > PAGE_SIZE).then(|| selected[PAGE_SIZE - 1].name.clone());
    Ok(json!({"realm":storage.locator.realm.as_str(),"services":rows,"next":next,
        "scope":"current_owned_definitions_and_presence_only",
        "notice":"Presence is not vendor authentication or evidence about an earlier uncertain update. Credential changes do not install, grant access or manage a frontend."}))
}

pub(in crate::terminal_connector) async fn present<F: Future<Output = io::Result<()>> + Send, W: Write>(
    owner: &mut TerminalConnectors, mut request: Request, approvals: &LocalApprovals,
    presenter: &mut ApprovalPresenter, input: &mut TerminalInput, output: &mut W, interrupt: F,
) -> Result<bool, ConnectError> {
    if request.response.is_closed() { return Ok(true); }
    if approvals.check_session(&request.session).is_err() {
        return request_refused("review_owner_closed", request.response, output);
    }
    if StockPrincipals::for_local_operator(&owner.storage.locator.realm).is_err() {
        return request_refused("selected_realm_operator_unavailable", request.response, output);
    }
    tokio::pin!(interrupt);
    match request.action {
        Requested::Inventory(query) => {
            let result = tokio::select! { biased;
                signal = interrupt.as_mut() => { signal.map_err(|e| ConnectError::native("Listen for credential inventory cancellation", e))?; return Ok(false); },
                _ = request.response.closed() => return Ok(true),
                result = inventory(&owner.storage, &owner.factory, &query) => match result {
                    Ok(value) => value,
                    Err(reason) => return request_refused(reason, request.response, output),
                },
            };
            if approvals.check_session(&request.session).is_err() {
                return request_refused("review_owner_closed", request.response, output);
            }
            let _ = request.response.send(result);
            Ok(true)
        },
        Requested::Change(change) => manage(&owner.storage, &owner.factory, owner.reviewed.is_some(), &request.session, change, request.response, None,
            approvals, presenter, input, output, interrupt.as_mut()).await,
    }
}

// Management is deliberately separate from initial-install replay prevention:
// the original finalizer results remain untouched by any later approved action.
async fn manage<F: Future<Output = io::Result<()>> + Send, W: Write>(
    storage: &ResolvedStorage, factory: &meerkat::AgentFactory, finalization_waiting: bool,
    session: &SessionId, change: Change, mut response: oneshot::Sender<Value>,
    requester: Option<ApprovalPrincipalId>,
    approvals: &LocalApprovals, presenter: &mut ApprovalPresenter, input: &mut TerminalInput,
    output: &mut W, mut interrupt: Pin<&mut F>,
) -> Result<bool, ConnectError> {
    if finalization_waiting { return request_refused("reviewed_finalization_waiting_for_setup_close", response, output); }
    let servers = match owned(storage).await {
        Ok(servers) => servers,
        Err(reason) => return request_refused(reason, response, output),
    };
    let Some(config) = servers.into_iter().find(|server| server.name == change.service) else {
        return request_refused("select_current_owned_service_from_inventory", response, output);
    };
    if !valid_change(&change) || !change.slots.iter().all(|slot| declared(&config).is_some_and(|names| names.contains(slot))) {
        return request_refused("select_exact_declared_credential_slots", response, output);
    }
    let persistence = match persistence(factory) {
        Ok(value) => value, Err(reason) => return request_refused(reason, response, output),
    };
    let source = match source(storage, &persistence) {
        Ok(value) => value, Err(reason) => return request_refused(reason, response, output),
    };
    let before = match source.observe(&config).await {
        Ok(observed) if exact_observation(&observed, storage, &config) => observed,
        _ => return request_refused("current_credential_owner_observation_unavailable", response, output),
    };
    if let Err(reason) = current(storage, &config).await { return request_refused(reason, response, output); }
    if approvals.check_session(session).is_err() { return request_refused("review_owner_closed", response, output); }
    if response.is_closed() { return Ok(true); }
    if input.require_terminal().is_err() { return request_refused("protected_terminal_required", response, output); }
    let body = json!({"phase":"service_credential_management","realm":storage.locator.realm.as_str(),
        "service":config.name,"action":change.action,"slots":change.slots,"current_presence":observation_facts(&Some(Ok(before))),
        "notice":"Change only these credentials for the unchanged service definition. Set fills or replaces the selected values; remove deletes the selected values. Other slots, service configuration and agent grants stay as they are. Later connections use the changed credentials; running processes can retain old environment values. This does not revoke provider credentials or prove vendor account readiness. Any earlier uncertain attempt remains uncertain. Cancellation can leave some selected updates completed."});
    let mut request = review_request(session, "Change these service credentials?", body)?;
    if let Some(requester) = requester { request.requester = requester; }
    let decision = match review(approvals, presenter, input, output,
        request, &mut response, interrupt.as_mut()).await? {
        ReviewOutcome::Decided(value) => value,
        ReviewOutcome::CallerGone => return Ok(true),
        ReviewOutcome::Stopped => return Ok(false),
    };
    if decision.status != ApprovalStatus::Approved {
        let _ = response.send(json!({"state":"denied","phase":"service_credential_management","updates":[],"grant_changed":false}));
        return Ok(true);
    }
    let mut values = Vec::new();
    if change.action == Action::Set {
        for slot in &change.slots {
            if let Err(reason) = current(storage, &config).await { return request_refused(reason, response, output); }
            if approvals.check_session(session).is_err() { return request_refused("review_owner_closed", response, output); }
            if response.is_closed() { return Ok(true); }
            writeln!(output, "New credential for {} ({})", review_text(&config.name), review_text(slot))
                .map_err(|e| ConnectError::native("Display selected credential slot", e))?;
            let value = tokio::select! { biased;
                signal = interrupt.as_mut() => { signal.map_err(|e| ConnectError::native("Listen for credential input cancellation", e))?; return Ok(false); },
                _ = response.closed() => return Ok(true),
                value = input.read_service_secret(output) => value.map_err(|e| ConnectError::native("Read hidden replacement credential", e))?,
            };
            let Some(value) = value else { return Ok(false); };
            values.push(value);
        }
    }
    if let Err(reason) = current(storage, &config).await { return request_refused(reason, response, output); }
    if approvals.check_session(session).is_err() { return request_refused("review_owner_closed", response, output); }
    let mut stop_cause = None;
    let outcome = {
        let stopped = stop_signal(interrupt.as_mut(), &mut response, input.wait_for_end(), &mut stop_cause);
        tokio::pin!(stopped);
        administer(storage, &persistence, &config, &change, &values,
            || approvals.check_session(session), stopped.as_mut()).await
    };
    display_settlement(&outcome, &config, &change, response, output)?;
    if let Some(Stop::ListenerFailed(error)) = outcome.stopped {
        return Err(ConnectError::native("Listen for credential management cancellation", error));
    }
    Ok(continue_after(stop_cause))
}

fn display_settlement(outcome: &ManagedOutcome, config: &McpServerConfig, change: &Change,
    response: oneshot::Sender<Value>, output: &mut impl Write,
) -> Result<(), ConnectError> {
    let facts = outcome.facts(config, change);
    let _ = response.send(facts.clone());
    writeln!(output, "{}", facts).and_then(|()| output.flush())
        .map_err(|error| ConnectError::native("Display credential management result", error))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StopCause { Requested, CallerGone, ListenerFailed }

fn continue_after(cause: Option<StopCause>) -> bool {
    matches!(cause, None | Some(StopCause::CallerGone))
}

async fn stop_signal<I: Future<Output = io::Result<()>>, E: Future<Output = io::Result<()>>>(
    mut interrupt: Pin<&mut I>, caller: &mut oneshot::Sender<Value>, end: E,
    cause: &mut Option<StopCause>,
) -> io::Result<()> {
    let (selected, result) = tokio::select! { biased;
        signal = interrupt.as_mut() => (StopCause::Requested, signal),
        _ = caller.closed() => (StopCause::CallerGone, Ok(())),
        signal = end => (StopCause::Requested, signal),
    };
    *cause = Some(if result.is_err() { StopCause::ListenerFailed } else { selected });
    result
}

#[derive(Default)]
struct ManagedOutcome {
    updates: Vec<(String, Result<Option<meerkat_core::auth::McpServiceSecretRef>, meerkat::McpServiceSecretUpdateError>, bool)>,
    stopped: Option<Stop>,
    refusal: Option<&'static str>,
}

impl ManagedOutcome {
    fn facts(&self, config: &McpServerConfig, change: &Change) -> Value {
        let uncertain = self.updates.iter().any(|(_, result, confirmed)|
            matches!(result, Err(meerkat::McpServiceSecretUpdateError::OutcomeUnknown(_))) || (result.is_ok() && !*confirmed));
        let complete = self.refusal.is_none() && self.updates.len() == change.slots.len()
            && self.updates.iter().all(|(_, result, confirmed)| result.is_ok() && *confirmed);
        json!({"phase":"service_credential_management","service":config.name,"action":change.action,
            "state":if uncertain {"uncertain"} else if self.stopped.is_some() {"stopped"} else if complete {"updated"} else {"incomplete"},
            "updates":self.updates.iter().map(|(slot,result,confirmed)| match result {
                Ok(_) if !*confirmed => json!({"slot":slot,"state":"uncertain","error_kind":"UnexpectedReference"}),
                Ok(Some(reference)) => json!({"slot":slot,"state":"stored","reference":{"realm":reference.realm().as_str(),
                    "server":reference.server(),"env":reference.env(),"meaning":reference.meaning()}}),
                Ok(None) => json!({"slot":slot,"state":"removed"}),
                Err(error) => json!({"slot":slot,"state":if matches!(error,meerkat::McpServiceSecretUpdateError::OutcomeUnknown(_)) {"uncertain"} else {"refused"},"error_kind":update_kind(error)}),
            }).collect::<Vec<_>>(),"refusal":self.refusal,
            "stop":match &self.stopped {None => None,Some(Stop::Cancelled) => Some("requested_or_caller_gone"),Some(Stop::ListenerFailed(_)) => Some("listener_failed")},
            "configuration_changed":false,"grant_changed":false,
            "notice":"These are results of this explicit action only. Earlier uncertain attempts remain uncertain. Do not retry automatically. Inspect current presence when needed; it cannot identify which earlier write landed. Existing processes may retain old environment values; no provider revocation, agent access change or service readiness is claimed."})
    }
}

async fn administer<I: Future<Output = io::Result<()>>>(storage: &ResolvedStorage,
    persistence: &meerkat_auth_core::auth_store::ProviderAuthPersistence, config: &McpServerConfig,
    change: &Change, values: &[ServiceSecret], check_session: impl Fn() -> Result<(), ConnectError>,
    mut interrupt: Pin<&mut I>,
) -> ManagedOutcome {
    let mut outcome = ManagedOutcome::default();
    if !valid_change(change) || change.service != config.name
        || !change.slots.iter().all(|slot| declared(config).is_some_and(|names| names.contains(slot))) {
        outcome.refusal = Some("selected_definition_or_slots_mismatch"); return outcome;
    }
    if change.action == Action::Set && values.len() != change.slots.len() {
        outcome.refusal = Some("replacement_values_incomplete"); return outcome;
    }
    for (index, slot) in change.slots.iter().enumerate() {
        if check_session().is_err() { outcome.refusal = Some("review_owner_closed"); break; }
        if current(storage, config).await.is_err() { outcome.refusal = Some("definition_changed"); break; }
        if check_session().is_err() { outcome.refusal = Some("review_owner_closed"); break; }
        // Caller disappearance and interruption stop the next entry. Once a
        // mutation starts, the existing joined-effect helper retains its future.
        let result = joined_effect(interrupt.as_mut(), &mut outcome.stopped, || async {
            match change.action {
                Action::Set => meerkat::set_mcp_service_secret(storage, persistence, config, slot, values[index].expose()).await.map(Some),
                Action::Remove => meerkat::remove_mcp_service_secret(storage, persistence, config, slot,
                    meerkat::McpServiceSecretRemoval::Current).await.map(|()| None),
            }
        }).await;
        let Some(result) = result else { break; };
        let confirmed = result.as_ref().is_ok_and(|reference| reference.as_ref().is_none_or(|reference|
            reference.realm() == &storage.locator.realm && reference.server() == config.name && reference.env() == slot));
        outcome.updates.push((slot.clone(), result, confirmed));
        if !confirmed { break; }
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_auth_core::auth_store::{FileLockCoordinator, FileTokenStore, ProviderAuthPersistence,
        PersistedTokens, TokenKey, TokenStore, TokenStoreError, TokenStoreObservation};
    use meerkat_core::{ConfigStore, FileConfigStore};
    use std::{path::{Path, PathBuf}, sync::atomic::{AtomicBool, AtomicUsize, Ordering}};
    use tokio::sync::Semaphore;

    struct ControlledStore {
        inner: FileTokenStore,
        saves: AtomicUsize,
        clears: AtomicUsize,
        hold_next: AtomicBool,
        lose_next: AtomicBool,
        fail_load: AtomicBool,
        entered: Semaphore,
        release: Semaphore,
    }
    #[async_trait]
    impl TokenStore for ControlledStore {
        async fn load(&self, key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError> {
            if self.fail_load.load(Ordering::SeqCst) { return Err(TokenStoreError::Unavailable("fixture-private-store-detail".into())); }
            self.inner.load(key).await
        }
        async fn observe_status(&self, key: &TokenKey) -> Result<TokenStoreObservation, TokenStoreError> {
            if self.fail_load.load(Ordering::SeqCst) { return Err(TokenStoreError::Unavailable("fixture-private-store-detail".into())); }
            self.inner.observe_status(key).await
        }
        async fn save(&self, key: &TokenKey, value: &PersistedTokens) -> Result<(), TokenStoreError> {
            self.saves.fetch_add(1, Ordering::SeqCst);
            if self.hold_next.swap(false, Ordering::SeqCst) {
                self.entered.add_permits(1);
                self.release.acquire().await.unwrap().forget();
            }
            self.inner.save(key, value).await?;
            if self.lose_next.swap(false, Ordering::SeqCst) { return Err(TokenStoreError::Unavailable("fixture lost acknowledgement".into())); }
            Ok(())
        }
        async fn clear(&self, key: &TokenKey) -> Result<(), TokenStoreError> {
            self.clears.fetch_add(1, Ordering::SeqCst);
            if self.hold_next.swap(false, Ordering::SeqCst) {
                self.entered.add_permits(1);
                self.release.acquire().await.unwrap().forget();
            }
            self.inner.clear(key).await?;
            if self.lose_next.swap(false, Ordering::SeqCst) { return Err(TokenStoreError::Unavailable("fixture lost acknowledgement".into())); }
            Ok(())
        }
        async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> { self.inner.list().await }
        fn backend_name(&self) -> &'static str { "service-management-fixture-file" }
    }
    struct Release(Arc<ControlledStore>);
    impl Drop for Release { fn drop(&mut self) { self.0.release.add_permits(1); } }

    struct Fixture {
        storage: ResolvedStorage,
        factory: meerkat::AgentFactory,
        store: Arc<ControlledStore>,
        config: McpServerConfig,
        config_path: PathBuf,
        vault_path: PathBuf,
    }
    impl Fixture {
        async fn new(root: &Path) -> Self {
            let storage = crate::RealmOptions {context_root:root.into(), state_root:Some(root.join("state")),
                user_config_root:Some(root.join("user"))}.resolve("management-fixture").unwrap();
            let config_path = crate::realm::config_source(&storage).unwrap().config_doc_path(&storage.locator.realm);
            let mut config = McpServerConfig::stdio("cards", "/fixture/private-command", vec!["private-argument".into()],
                [("INLINE_PRIVATE".into(), "private-inline-value".into())].into_iter().collect());
            let McpTransportConfig::Stdio(stdio) = &mut config.transport else { unreachable!() };
            stdio.secret_env = BTreeSet::from(["KEY".into(),"TOKEN".into()]);
            FileConfigStore::new(config_path.clone(), meerkat_models::canonical())
                .patch(ConfigDelta(json!({"max_tokens":1234,"tools":{"shell_enabled":false,"mcp_servers":[config]}})))
                .await.unwrap();
            let vault_path = root.join("credentials");
            let store = Arc::new(ControlledStore {inner:FileTokenStore::new(vault_path.clone()), saves:AtomicUsize::new(0),
                clears:AtomicUsize::new(0),hold_next:AtomicBool::new(false),lose_next:AtomicBool::new(false),fail_load:AtomicBool::new(false),
                entered:Semaphore::new(0),release:Semaphore::new(0)});
            let factory = meerkat::AgentFactory::new(root.join("factory")).with_provider_auth_persistence(
                ProviderAuthPersistence::new(store.clone(),Arc::new(FileLockCoordinator::new(root.join("locks")))));
            Self {storage,factory,store,config,config_path,vault_path}
        }
        fn persistence(&self) -> ProviderAuthPersistence { persistence(&self.factory).unwrap() }
        async fn presence(&self) -> meerkat::McpServiceSecretObservation {
            source(&self.storage, &self.persistence()).unwrap().observe(&self.config).await.unwrap()
        }
        async fn run(&self, change: &Change, values: &[ServiceSecret]) -> ManagedOutcome {
            let signal = std::future::pending::<io::Result<()>>(); tokio::pin!(signal);
            administer(&self.storage,&self.persistence(),&self.config,change,values,|| Ok(()),signal.as_mut()).await
        }
    }
    fn change(action: Action, slots: &[&str]) -> Change {
        Change {service:"cards".into(),slots:slots.iter().map(|slot| (*slot).into()).collect(),action}
    }
    fn values(values: &[&str]) -> Vec<ServiceSecret> {
        values.iter().map(|value| ServiceSecret::from_hidden_input((*value).into()).unwrap()).collect()
    }

    #[derive(Clone, Default)]
    struct Seen(Arc<std::sync::Mutex<Vec<u8>>>);
    impl Seen {
        fn text(&self) -> String { String::from_utf8(self.0.lock().unwrap().clone()).unwrap() }
    }
    impl Write for Seen {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes); Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> { Ok(()) }
    }

    #[tokio::test]
    async fn direct_status_uses_owned_inventory_without_setup_or_model_configuration() {
        let root = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(root.path()).await;
        let original = std::fs::read(&fixture.config_path).unwrap();
        let global = fixture.storage.layout.user_rkat_root().unwrap().join("config.toml");
        std::fs::create_dir_all(global.parent().unwrap()).unwrap();
        std::fs::write(&global, "not a valid model configuration [").unwrap();
        let (_send, receive) = mpsc::channel(1);
        let mut input = TerminalInput::scripted(receive);
        let mut output = Vec::new();
        run_direct(&fixture.storage, &fixture.factory, ServiceCredentialCommand::Status { after: None },
            &mut input, &mut output, std::future::pending::<io::Result<()>>()).await.unwrap();
        let facts: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(facts["realm"], "management-fixture");
        assert_eq!(facts["services"][0]["service"], "cards");
        assert_eq!(facts["services"].as_array().unwrap().len(), 1);
        assert_eq!(std::fs::read(&fixture.config_path).unwrap(), original);
        assert!(!fixture.storage.locator.state_root.join(".meerkat-connect").exists());
        assert!(!fixture.storage.locator.state_root.join("management-fixture/runtime.sqlite").exists());
        for private in ["private-command", "private-argument", "INLINE_PRIVATE", "private-inline-value"] {
            assert!(!String::from_utf8_lossy(&output).contains(private));
        }
        assert_eq!(fixture.store.saves.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.store.clears.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn direct_changes_require_exact_selection_and_a_protected_terminal() {
        use nix::pty::openpty;
        let root = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(root.path()).await;
        let original = std::fs::read(&fixture.config_path).unwrap();
        for set in [false, true] {
            let command = |service: &str, slots: Vec<&str>| {
                let service = service.to_owned();
                let slots = slots.into_iter().map(str::to_owned).collect();
                if set { ServiceCredentialCommand::Set { service, slots } }
                else { ServiceCredentialCommand::Remove { service, slots } }
            };
            for (service, slots) in [("missing", vec!["KEY"]), ("cards", vec!["OTHER"]),
                ("cards", vec!["KEY", "KEY"]), ("cards", vec![])] {
                let pair = openpty(None, None).unwrap();
                let mut input = TerminalInput::from_owned_fd(pair.slave);
                let mut output = Vec::new();
                let result = run_direct(&fixture.storage, &fixture.factory, command(service, slots),
                    &mut input, &mut output, std::future::pending::<io::Result<()>>()).await;
                assert!(result.is_err());
                assert!(!String::from_utf8_lossy(&output).contains("Type 'approve "));
            }
            let (_send, receive) = mpsc::channel(1);
            let mut input = TerminalInput::scripted(receive);
            assert!(run_direct(&fixture.storage, &fixture.factory, command("cards", vec!["KEY"]),
                &mut input, &mut Vec::new(), std::future::pending::<io::Result<()>>()).await.is_err());
        }
        assert_eq!(fixture.store.saves.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.store.clears.load(Ordering::SeqCst), 0);
        assert_eq!(std::fs::read(&fixture.config_path).unwrap(), original);
    }

    #[tokio::test]
    async fn direct_remove_reuses_review_currentness_and_joined_native_removal() {
        use nix::{pty::openpty, unistd::write};
        tokio::time::timeout(Duration::from_secs(15), async {
            for mode in ["approve", "deny", "quit", "cancel", "stale"] {
                let root = tempfile::tempdir().unwrap();
                let fixture = Fixture::new(root.path()).await;
                let seed = change(Action::Set, &["KEY", "TOKEN"]);
                let seeded = fixture.run(&seed, &values(&["fixture-key", "fixture-token"])).await;
                assert_eq!(seeded.facts(&fixture.config, &seed)["state"], "updated");
                let original_references = ["KEY", "TOKEN"].map(|slot| meerkat_core::auth::McpServiceSecretRef::for_definition(
                    fixture.storage.locator.realm.clone(), &fixture.config, slot).unwrap());
                assert_ne!(TokenKey::mcp_service_secret(&original_references[0]), TokenKey::mcp_service_secret(&original_references[1]));
                for reference in &original_references {
                    assert_eq!(meerkat_core::auth::observe_mcp_service_secret(fixture.store.as_ref(), reference).await.unwrap(),
                        meerkat_core::auth::McpServiceSecretPresence::Stored);
                }
                let original = std::fs::read(&fixture.config_path).unwrap();
                let realm = meerkat_store::realm_paths_in(&fixture.storage.locator.state_root, fixture.storage.locator.realm.as_str());
                let grants = meerkat_store::grant_owner_store_path(&realm.root);
                let grants_before = std::fs::read(&grants).ok();
                let pair = openpty(None, None).unwrap();
                let mut input = TerminalInput::from_owned_fd(pair.slave);
                let mut output = Seen::default(); let seen = output.clone();
                let (cancel, cancelled) = oneshot::channel();
                let signal = async { let _ = cancelled.await; Ok(()) };
                let human = async {
                    while !seen.text().contains("Type 'approve ") { tokio::task::yield_now().await; }
                    let text = seen.text();
                    assert!(text.contains("service_credential_management"));
                    assert!(text.contains("\"service\": \"cards\""));
                    assert!(!text.contains("supplied setup helper"));
                    let id = text.split("Type 'approve ").last().unwrap().split('\'').next().unwrap();
                    if mode == "stale" {
                        let mut changed = fixture.config.clone();
                        let McpTransportConfig::Stdio(stdio) = &mut changed.transport else { unreachable!() };
                        stdio.args.push("changed-before-removal".into());
                        FileConfigStore::new(fixture.config_path.clone(), meerkat_models::canonical())
                            .patch(ConfigDelta(json!({"tools":{"mcp_servers":[changed]}}))).await.unwrap();
                    }
                    if mode == "cancel" { cancel.send(()).unwrap(); None }
                    else {
                        let answer = match mode { "deny" => format!("deny {id}\n"), "quit" => "/quit\n".into(), _ => format!("approve {id}\n") };
                        write(&pair.master, answer.as_bytes()).unwrap();
                        // The join retains this output until the command settles.
                        Some(cancel)
                    }
                };
                let (result, _retained_cancel) = tokio::join!(run_direct(&fixture.storage, &fixture.factory,
                    ServiceCredentialCommand::Remove { service: "cards".into(), slots: vec!["KEY".into()] },
                    &mut input, &mut output, signal), human);
                assert_eq!(result.is_ok(), mode == "approve", "{mode}");
                input.check_cleanup().unwrap();
                assert_eq!(fixture.store.clears.load(Ordering::SeqCst), usize::from(mode == "approve"));
                let current = owned(&fixture.storage).await.unwrap().remove(0);
                let observed = source(&fixture.storage, &fixture.persistence()).unwrap().observe(&current).await.unwrap();
                if mode == "stale" {
                    assert_ne!(current.stdio_launch_meaning(), fixture.config.stdio_launch_meaning());
                    for slot in ["KEY", "TOKEN"] {
                        assert_eq!(observed.slots[slot], meerkat_core::auth::McpServiceSecretPresence::NotStored);
                    }
                    // The changed definition cannot inherit the original references, which remain stored.
                    for reference in &original_references {
                        assert_eq!(meerkat_core::auth::observe_mcp_service_secret(fixture.store.as_ref(), reference).await.unwrap(),
                            meerkat_core::auth::McpServiceSecretPresence::Stored);
                    }
                } else {
                    assert_eq!(observed.slots["KEY"], if mode == "approve" { meerkat_core::auth::McpServiceSecretPresence::NotStored }
                        else { meerkat_core::auth::McpServiceSecretPresence::Stored });
                    assert_eq!(observed.slots["TOKEN"], meerkat_core::auth::McpServiceSecretPresence::Stored);
                    assert_eq!(std::fs::read(&fixture.config_path).unwrap(), original);
                }
                assert_eq!(std::fs::read(&grants).ok(), grants_before);
                assert!(!realm.root.join("runtime.sqlite").exists());
                assert_eq!(fixture.store.saves.load(Ordering::SeqCst), 2);
                for secret in ["fixture-key", "fixture-token"] { assert!(!seen.text().contains(secret)); }
            }
        }).await.expect("direct protected removal must settle without setup or a model");
    }

    #[tokio::test]
    async fn direct_set_uses_protected_input_and_preserves_exact_native_outcomes() {
        use nix::{pty::openpty, sys::termios::{tcgetattr, LocalFlags}, unistd::write};
        let progress = std::cell::Cell::new(("not-started", "initializing"));
        tokio::time::timeout(Duration::from_secs(30), async {
            for mode in ["corrupt_global", "fill", "replace", "deny", "cancel", "quit", "stale", "uncertain", "entered_cancel"] {
                progress.set((mode, "initializing"));
                let root = tempfile::tempdir().unwrap();
                let fixture = Fixture::new(root.path()).await;
                fixture.run(&change(Action::Set, &["TOKEN"]), &values(&["fixture-sibling-token"])).await;
                if mode == "replace" {
                    fixture.run(&change(Action::Set, &["KEY"]), &values(&["fixture-old-key"])).await;
                }
                let saves_before = fixture.store.saves.load(Ordering::SeqCst);
                let key = |slot| TokenKey::mcp_service_secret(&meerkat_core::auth::McpServiceSecretRef::for_definition(
                    fixture.storage.locator.realm.clone(), &fixture.config, slot).unwrap());
                let sibling_before = fixture.store.load(&key("TOKEN")).await.unwrap();
                let config_before = std::fs::read(&fixture.config_path).unwrap();
                let realm = meerkat_store::realm_paths_in(&fixture.storage.locator.state_root, fixture.storage.locator.realm.as_str());
                let grants = meerkat_store::grant_owner_store_path(&realm.root);
                let grants_before = std::fs::read(&grants).ok();
                // Model setup is absent for direct management. Corrupt global
                // TOML is different: native custody must still read that chain.
                let global = fixture.storage.layout.user_rkat_root().unwrap().join("config.toml");
                assert!(!global.exists());
                if mode == "corrupt_global" {
                    std::fs::create_dir_all(global.parent().unwrap()).unwrap();
                    std::fs::write(&global, "not a valid model configuration [").unwrap();
                }
                fixture.store.lose_next.store(mode == "uncertain", Ordering::SeqCst);
                fixture.store.hold_next.store(mode == "entered_cancel", Ordering::SeqCst);
                let _release = Release(fixture.store.clone());
                let pair = openpty(None, None).unwrap();
                let observer = pair.slave.try_clone().unwrap();
                let terminal_before = tcgetattr(&observer).unwrap();
                let mut input = TerminalInput::from_owned_fd(pair.slave);
                let mut output = Seen::default();
                let seen = output.clone();
                let (cancel, cancelled) = oneshot::channel();
                let signal = async { let _ = cancelled.await; Ok(()) };
                let completed = AtomicBool::new(false);
                let human = async {
                    if mode == "corrupt_global" { return Some(cancel); }
                    progress.set((mode, "waiting-for-review"));
                    while !seen.text().contains("Type 'approve ") {
                        assert!(!completed.load(Ordering::SeqCst), "{mode}: command ended before review");
                        tokio::task::yield_now().await;
                    }
                    let text = seen.text();
                    assert!(text.contains("\"service\": \"cards\"") && text.contains("\"action\": \"set\""));
                    assert!(text.contains("\"KEY\""));
                    assert!(!text.contains("supplied setup helper"));
                    let id = text.split("Type 'approve ").last().unwrap().split('\'').next().unwrap();
                    if mode == "cancel" {
                        cancel.send(()).unwrap();
                        return None;
                    }
                    if mode == "stale" {
                        let mut changed = fixture.config.clone();
                        let McpTransportConfig::Stdio(stdio) = &mut changed.transport else { unreachable!() };
                        stdio.args.push("changed-before-set".into());
                        FileConfigStore::new(fixture.config_path.clone(), meerkat_models::canonical())
                            .patch(ConfigDelta(json!({"tools":{"mcp_servers":[changed]}}))).await.unwrap();
                    }
                    let answer = if mode == "deny" { format!("deny {id}\n") } else { format!("approve {id}\n") };
                    write(&pair.master, answer.as_bytes()).unwrap();
                    if matches!(mode, "deny" | "stale") { return Some(cancel); }
                    progress.set((mode, "waiting-for-hidden-input"));
                    while !seen.text().contains("Service credential (hidden)")
                        || tcgetattr(&observer).unwrap().local_flags.contains(LocalFlags::ECHO) {
                        assert!(!completed.load(Ordering::SeqCst), "{mode}: command ended before hidden input");
                        tokio::task::yield_now().await;
                    }
                    assert_eq!(fixture.store.saves.load(Ordering::SeqCst), saves_before);
                    write(&pair.master, if mode == "quit" { b"/quit\n".as_slice() }
                        else { b"fixture-replacement-key\n".as_slice() }).unwrap();
                    if mode == "entered_cancel" {
                        progress.set((mode, "waiting-for-store-entry"));
                        loop {
                            if let Ok(permit) = fixture.store.entered.try_acquire() {
                                permit.forget();
                                break;
                            }
                            assert!(!completed.load(Ordering::SeqCst), "{mode}: command ended before store entry");
                            tokio::task::yield_now().await;
                        }
                        cancel.send(()).unwrap();
                        tokio::task::yield_now().await;
                        assert!(!seen.text().contains("\"updates\""), "entered write must settle before reporting");
                        fixture.store.release.add_permits(1);
                        return None;
                    }
                    // Keep the interrupt sender alive until the command returns.
                    Some(cancel)
                };
                let command = async {
                    let result = run_direct(&fixture.storage, &fixture.factory,
                        ServiceCredentialCommand::Set { service: "cards".into(), slots: vec!["KEY".into()] },
                        &mut input, &mut output, signal).await;
                    completed.store(true, Ordering::SeqCst);
                    eprintln!("direct set {mode}: command returned {}", if result.is_ok() { "success" } else { "error" });
                    result
                };
                let (result, _retained_cancel) = tokio::join!(command, human);
                progress.set((mode, "checking-outcome"));
                assert_eq!(result.is_ok(), matches!(mode, "fill" | "replace"), "{mode}");
                input.check_cleanup().unwrap();
                assert_eq!(tcgetattr(&observer).unwrap(), terminal_before);
                let entered = matches!(mode, "fill" | "replace" | "uncertain" | "entered_cancel");
                assert_eq!(fixture.store.saves.load(Ordering::SeqCst), saves_before + usize::from(entered), "{mode}");
                assert_eq!(fixture.store.clears.load(Ordering::SeqCst), 0);
                let reopened = FileTokenStore::new(fixture.vault_path.clone());
                let selected = reopened.load(&key("KEY")).await.unwrap();
                assert_eq!(selected.as_ref().and_then(|value| value.primary_secret.as_deref()),
                    entered.then_some("fixture-replacement-key"), "{mode}");
                assert_eq!(reopened.load(&key("TOKEN")).await.unwrap(), sibling_before);
                assert_eq!(std::fs::read(&grants).ok(), grants_before);
                assert!(!realm.root.join("runtime.sqlite").exists());
                if mode == "stale" {
                    let current = owned(&fixture.storage).await.unwrap().remove(0);
                    assert_ne!(current.stdio_launch_meaning(), fixture.config.stdio_launch_meaning());
                    let observed = source(&fixture.storage, &fixture.persistence()).unwrap().observe(&current).await.unwrap();
                    assert!(observed.slots.values().all(|value| *value == meerkat_core::auth::McpServiceSecretPresence::NotStored));
                } else {
                    assert_eq!(std::fs::read(&fixture.config_path).unwrap(), config_before);
                }
                let text = seen.text();
                if mode == "corrupt_global" {
                    assert!(!text.contains("Type 'approve ") && !text.contains("Service credential (hidden)"));
                    assert!(text.contains("\"reason\":\"current_credential_owner_observation_unavailable\""));
                }
                if mode == "uncertain" { assert!(text.contains("\"state\":\"uncertain\"")); }
                if mode == "entered_cancel" { assert!(text.contains("\"state\":\"stopped\"")); }
                for secret in ["fixture-sibling-token", "fixture-old-key", "fixture-replacement-key"] {
                    assert!(!text.contains(secret));
                }
            }
        }).await.unwrap_or_else(|_| {
            let (mode, phase) = progress.get();
            panic!("direct protected set must settle without setup or a model: {mode} at {phase}");
        });
    }

    #[tokio::test]
    async fn fills_remaining_slot_then_replaces_only_explicitly_selected_credentials() {
        tokio::time::timeout(Duration::from_secs(30), async {
            let root = tempfile::tempdir().unwrap();
            let fixture = Fixture::new(root.path()).await;
            let original = std::fs::read(&fixture.config_path).unwrap();
            let original_generation = crate::realm::read_own_mcp_config(&fixture.storage).await.unwrap().1;
            let first = fixture.run(&change(Action::Set,&["KEY"]), &values(&["fixture-initial-key"])).await;
            assert_eq!(first.facts(&fixture.config,&change(Action::Set,&["KEY"]))["state"],"updated");
            assert_eq!(fixture.presence().await.slots["TOKEN"],meerkat_core::auth::McpServiceSecretPresence::NotStored);
            let filled = fixture.run(&change(Action::Set,&["TOKEN"]), &values(&["fixture-new-token"])).await;
            assert_eq!(filled.updates.len(),1);
            let replaced = fixture.run(&change(Action::Set,&["KEY"]), &values(&["fixture-replacement-key"])).await;
            let facts = replaced.facts(&fixture.config,&change(Action::Set,&["KEY"]));
            assert_eq!(facts["state"],"updated");
            assert_eq!(facts["configuration_changed"],false);
            assert_eq!(facts["grant_changed"],false);
            assert_eq!(fixture.store.saves.load(Ordering::SeqCst),3);
            assert_eq!(fixture.store.clears.load(Ordering::SeqCst),0);
            assert_eq!(std::fs::read(&fixture.config_path).unwrap(),original);
            assert_eq!(crate::realm::read_own_mcp_config(&fixture.storage).await.unwrap().1,original_generation);
            let reopened = meerkat::RealmMcpServiceSecrets::from_realm_source(crate::realm::config_source(&fixture.storage).unwrap(),
                fixture.storage.locator.realm.clone(),Arc::new(FileTokenStore::new(fixture.vault_path.clone())));
            let observed = reopened.observe(&fixture.config).await.unwrap();
            assert!(observed.slots.values().all(|presence| *presence == meerkat_core::auth::McpServiceSecretPresence::Stored));
            assert!(!facts.to_string().contains("fixture-replacement-key"));
        }).await.expect("native managed set controls must settle");
    }

    #[tokio::test]
    async fn current_presence_and_new_action_do_not_resolve_an_uncertain_earlier_write() {
        tokio::time::timeout(Duration::from_secs(30), async {
            let root = tempfile::tempdir().unwrap();
            let fixture = Fixture::new(root.path()).await;
            fixture.store.lose_next.store(true,Ordering::SeqCst);
            let request = change(Action::Set,&["KEY","TOKEN"]);
            let earlier = fixture.run(&request,&values(&["fixture-first-key","fixture-unentered-token"])).await;
            let saved_facts = earlier.facts(&fixture.config,&request);
            assert_eq!(saved_facts["state"],"uncertain");
            assert_eq!(earlier.updates.len(),1);
            let observed = fixture.presence().await;
            assert_eq!(observed.slots["KEY"],meerkat_core::auth::McpServiceSecretPresence::Stored);
            assert_eq!(observed.slots["TOKEN"],meerkat_core::auth::McpServiceSecretPresence::NotStored);
            let later = change(Action::Set,&["TOKEN"]);
            let new_action = fixture.run(&later,&values(&["fixture-approved-later-token"])).await;
            assert_eq!(new_action.facts(&fixture.config,&later)["state"],"updated");
            assert_eq!(earlier.facts(&fixture.config,&request),saved_facts);
            assert_eq!(fixture.store.saves.load(Ordering::SeqCst),2,"no replay or implicit retry");
        }).await.expect("uncertain and later managed writes must stay separate");
    }

    #[tokio::test]
    async fn current_removal_clears_only_the_selected_slot_and_keeps_the_definition() {
        tokio::time::timeout(Duration::from_secs(30), async {
            let root = tempfile::tempdir().unwrap();
            let fixture = Fixture::new(root.path()).await;
            fixture.run(&change(Action::Set,&["KEY","TOKEN"]), &values(&["fixture-key","fixture-token"])).await;
            let original = std::fs::read(&fixture.config_path).unwrap();
            let request = change(Action::Remove,&["KEY"]);
            let removed = fixture.run(&request,&[]).await;
            assert_eq!(removed.facts(&fixture.config,&request)["updates"][0]["state"],"removed");
            assert_eq!(removed.facts(&fixture.config,&request)["state"],"updated");
            let observed = fixture.presence().await;
            assert_eq!(observed.slots["KEY"],meerkat_core::auth::McpServiceSecretPresence::NotStored);
            assert_eq!(observed.slots["TOKEN"],meerkat_core::auth::McpServiceSecretPresence::Stored);
            assert_eq!(std::fs::read(&fixture.config_path).unwrap(),original);
            assert_eq!(fixture.store.clears.load(Ordering::SeqCst),1);
            let explicit_absent_removal = fixture.run(&request,&[]).await;
            assert_eq!(explicit_absent_removal.facts(&fixture.config,&request)["state"],"updated");
            assert_eq!(fixture.store.saves.load(Ordering::SeqCst),2);
        }).await.expect("selected Current removal and absent removal must settle");
    }

    #[tokio::test]
    async fn unknown_removal_is_not_resolved_by_absence_or_another_approved_change() {
        tokio::time::timeout(Duration::from_secs(30), async {
            let root = tempfile::tempdir().unwrap();
            let fixture = Fixture::new(root.path()).await;
            fixture.run(&change(Action::Set,&["KEY","TOKEN"]), &values(&["fixture-key","fixture-token"])).await;
            fixture.store.lose_next.store(true,Ordering::SeqCst);
            let request = change(Action::Remove,&["KEY","TOKEN"]);
            let earlier = fixture.run(&request,&[]).await;
            let saved = earlier.facts(&fixture.config,&request);
            assert_eq!(saved["state"],"uncertain");
            assert_eq!(fixture.store.clears.load(Ordering::SeqCst),1);
            let observed = fixture.presence().await;
            assert_eq!(observed.slots["KEY"],meerkat_core::auth::McpServiceSecretPresence::NotStored);
            assert_eq!(observed.slots["TOKEN"],meerkat_core::auth::McpServiceSecretPresence::Stored);
            let later = change(Action::Set,&["KEY"]);
            assert_eq!(fixture.run(&later,&values(&["fixture-later-key"])).await.facts(&fixture.config,&later)["state"],"updated");
            assert_eq!(earlier.facts(&fixture.config,&request),saved);
            assert_eq!(fixture.store.clears.load(Ordering::SeqCst),1,"no retry or implicit deletion");
        }).await.expect("unknown removal must remain a distinct attempt");
    }

    #[tokio::test]
    async fn changed_definition_foreign_realm_and_closed_review_refuse_before_writing() {
        tokio::time::timeout(Duration::from_secs(30), async {
            for (reason,action) in ["changed","foreign","review_closed","slot","service"].into_iter()
                .flat_map(|reason| [Action::Set,Action::Remove].into_iter().map(move |action| (reason,action))) {
                let root = tempfile::tempdir().unwrap();
                let fixture = Fixture::new(root.path()).await;
                let mut storage = fixture.storage.clone();
                if reason == "foreign" { storage.locator.realm=meerkat_core::RealmId::parse("foreign").unwrap(); }
                if reason == "changed" {
                    let mut changed = fixture.config.clone();
                    let McpTransportConfig::Stdio(stdio) = &mut changed.transport else { unreachable!() };
                    stdio.args.push("changed-after-review".into());
                    FileConfigStore::new(fixture.config_path.clone(),meerkat_models::canonical())
                        .patch(ConfigDelta(json!({"tools":{"mcp_servers":[changed]}}))).await.unwrap();
                }
                let signal = std::future::pending::<io::Result<()>>(); tokio::pin!(signal);
                let mut request = change(action, if reason == "slot" {&["UNDECLARED"]} else {&["KEY"]});
                if reason == "service" { request.service="different-service".into(); }
                let secret_values = if action == Action::Set {values(&["fixture-unentered-key"])} else {Vec::new()};
                let result = administer(&storage,&fixture.persistence(),&fixture.config,&request,
                    &secret_values,|| if reason == "review_closed" {Err(unavailable("closed fixture"))} else {Ok(())},signal.as_mut()).await;
                assert!(result.updates.is_empty());
                assert_eq!(fixture.store.saves.load(Ordering::SeqCst),0);
                assert_eq!(fixture.store.clears.load(Ordering::SeqCst),0);
            }
        }).await.expect("review/currentness refusals must settle");
    }

    #[tokio::test]
    async fn cancellation_or_caller_loss_before_entry_writes_nothing() {
        tokio::time::timeout(Duration::from_secs(30), async {
            let root = tempfile::tempdir().unwrap();
            let fixture = Fixture::new(root.path()).await;
            let signal = std::future::ready(Ok(())); tokio::pin!(signal);
            let result = administer(&fixture.storage,&fixture.persistence(),&fixture.config,&change(Action::Set,&["KEY"]),
                &values(&["fixture-unentered-key"]),|| Ok(()),signal.as_mut()).await;
            assert!(result.updates.is_empty());
            assert!(matches!(result.stopped,Some(Stop::Cancelled)));
            let (mut sender,receiver) = oneshot::channel::<Value>(); drop(receiver);
            let signal = async { sender.closed().await; Ok(()) }; tokio::pin!(signal);
            let gone = administer(&fixture.storage,&fixture.persistence(),&fixture.config,&change(Action::Set,&["KEY"]),
                &values(&["fixture-unentered-key"]),|| Ok(()),signal.as_mut()).await;
            assert!(gone.updates.is_empty());
            assert_eq!(fixture.store.saves.load(Ordering::SeqCst),0);
        }).await.expect("pre-entry stop controls must settle");
    }

    #[tokio::test]
    async fn interruption_after_entry_joins_that_write_and_prevents_the_next_slot() {
        tokio::time::timeout(Duration::from_secs(30), async {
            let root = tempfile::tempdir().unwrap();
            let fixture = Fixture::new(root.path()).await;
            fixture.store.hold_next.store(true,Ordering::SeqCst);
            let _release = Release(fixture.store.clone());
            let request = change(Action::Set,&["KEY","TOKEN"]);
            let secret_values = values(&["fixture-entered-key","fixture-unentered-token"]);
            let persistence = fixture.persistence();
            let (cancel,receive) = oneshot::channel();
            let signal = async { let _ = receive.await; Ok(()) }; tokio::pin!(signal);
            let mut running = Box::pin(administer(&fixture.storage,&persistence,&fixture.config,&request,&secret_values,|| Ok(()),signal.as_mut()));
            tokio::select! {
                permit = fixture.store.entered.acquire() => permit.unwrap().forget(),
                _ = running.as_mut() => panic!("write did not reach the held native store"),
            }
            cancel.send(()).unwrap();
            assert!(running.as_mut().now_or_never().is_none(),"entered write must remain joined");
            fixture.store.release.add_permits(1);
            let result = running.await;
            assert!(matches!(result.stopped,Some(Stop::Cancelled)));
            assert_eq!(result.updates.len(),1);
            assert_eq!(fixture.store.saves.load(Ordering::SeqCst),1);
            let observed = fixture.presence().await;
            assert_eq!(observed.slots["KEY"],meerkat_core::auth::McpServiceSecretPresence::Stored);
            assert_eq!(observed.slots["TOKEN"],meerkat_core::auth::McpServiceSecretPresence::NotStored);
        }).await.expect("entered native write must be joined before stopping");
    }

    #[tokio::test]
    async fn selected_interrupt_survives_later_caller_loss_and_output_failure_after_settlement() {
        struct BrokenOutput;
        impl Write for BrokenOutput {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {Err(io::Error::new(io::ErrorKind::BrokenPipe,"fixture closed output"))}
            fn flush(&mut self) -> io::Result<()> {Ok(())}
        }
        tokio::time::timeout(Duration::from_secs(30), async {
            for action in [Action::Set,Action::Remove] {
                let root = tempfile::tempdir().unwrap();
                let fixture = Fixture::new(root.path()).await;
                if action == Action::Remove {fixture.run(&change(Action::Set,&["KEY","TOKEN"]),&values(&["fixture-key","fixture-token"])).await;}
                fixture.store.hold_next.store(true,Ordering::SeqCst);
                let _release = Release(fixture.store.clone());
                let request = change(action,&["KEY","TOKEN"]);
                let secret_values = if action == Action::Set {values(&["fixture-entered-key","fixture-unentered-token"])} else {Vec::new()};
                let persistence = fixture.persistence();
                let (cancel,receive) = oneshot::channel();
                let signal = async {let _ = receive.await; Ok(())}; tokio::pin!(signal);
                let (mut caller,receiver) = oneshot::channel();
                let mut cause = None;
                let result = {
                    let stopped = stop_signal(signal.as_mut(), &mut caller, std::future::pending(), &mut cause);
                    tokio::pin!(stopped);
                    let mut running = Box::pin(administer(&fixture.storage,&persistence,&fixture.config,&request,&secret_values,|| Ok(()),stopped.as_mut()));
                    tokio::select! {
                        permit = fixture.store.entered.acquire() => permit.unwrap().forget(),
                        _ = running.as_mut() => panic!("mutation did not enter"),
                    }
                    cancel.send(()).unwrap();
                    assert!(running.as_mut().now_or_never().is_none());
                    drop(receiver);
                    fixture.store.release.add_permits(1);
                    running.await
                };
                assert!(matches!(cause,Some(StopCause::Requested)));
                assert!(!continue_after(cause),"late caller loss cannot erase selected Ctrl-C");
                assert_eq!(result.updates.len(),1,"second slot never entered");
                let observed = fixture.presence().await;
                let expected = if action == Action::Set {meerkat_core::auth::McpServiceSecretPresence::Stored} else {meerkat_core::auth::McpServiceSecretPresence::NotStored};
                assert_eq!(observed.slots["KEY"],expected);
                assert!(display_settlement(&result,&fixture.config,&request,caller,&mut BrokenOutput).is_err());
                assert_eq!(fixture.store.clears.load(Ordering::SeqCst),if action == Action::Remove {1} else {0});
                assert_eq!(fixture.store.saves.load(Ordering::SeqCst),if action == Action::Remove {2} else {1});
            }
        }).await.expect("stop cause and output failure must preserve native settlement");
    }

    #[tokio::test]
    async fn ordinary_selection_and_presence_refusals_leave_management_available() {
        use std::os::unix::fs::PermissionsExt;
        tokio::time::timeout(Duration::from_secs(30), async {
            let root = tempfile::Builder::new().permissions(std::fs::Permissions::from_mode(0o700)).tempdir().unwrap();
            let fixture = Fixture::new(root.path()).await;
            let (approvals,mut presenter) = LocalApprovals::create_paired_host(root.path()).unwrap();
            let session = SessionId::new(); approvals.bind_session(&session).unwrap();
            let (_input_send,input_receive) = mpsc::channel(1);
            let mut input = TerminalInput::scripted(input_receive);
            let mut output = Vec::new();
            for reason in ["missing","slot","unavailable","protected_terminal"] {
                fixture.store.fail_load.store(reason == "unavailable",Ordering::SeqCst);
                let mut request = change(Action::Set,&["KEY"]);
                if reason == "missing" {request.service="absent".into();}
                if reason == "slot" {request.slots=vec!["UNDECLARED".into()];}
                let (caller,receive) = oneshot::channel();
                let signal = std::future::pending::<io::Result<()>>(); tokio::pin!(signal);
                assert!(manage(&fixture.storage,&fixture.factory,false,&session,request,caller,None,&approvals,
                    &mut presenter,&mut input,&mut output,signal.as_mut()).await.unwrap());
                let facts = receive.await.unwrap();
                assert_eq!(facts["state"],"unavailable");
                assert_eq!(facts["phase"],"service_credential_management");
                if reason == "unavailable" {
                    // The owner may refuse the read or report NotObserved.
                    // Both leave this request local and private; neither can
                    // bypass the protected terminal or imply an empty slot.
                    assert!(matches!(facts["reason"].as_str(), Some("current_credential_owner_observation_unavailable" | "protected_terminal_required")));
                } else {
                    let expected = match reason {"missing" => "select_current_owned_service_from_inventory",
                        "slot" => "select_exact_declared_credential_slots",_ => "protected_terminal_required"};
                    assert_eq!(facts["reason"],expected);
                }
            }
            assert!(!String::from_utf8(output).unwrap().contains("fixture-private-store-detail"));
            assert_eq!(inventory(&fixture.storage,&fixture.factory,&Inventory {after:None}).await.unwrap()["services"][0]["service"],"cards");
            assert_eq!(fixture.store.saves.load(Ordering::SeqCst),0);
            assert_eq!(fixture.store.clears.load(Ordering::SeqCst),0);
        }).await.expect("ordinary refusal must return to the same live conversation");
    }

    #[tokio::test]
    async fn inventory_projects_owned_names_and_presence_without_launch_material() {
        tokio::time::timeout(Duration::from_secs(30), async {
            let root = tempfile::tempdir().unwrap();
            let fixture = Fixture::new(root.path()).await;
            let global = fixture.storage.layout.user_rkat_root().unwrap().join("config.toml");
            let mut inherited = fixture.config.clone(); inherited.name="global-only".into();
            FileConfigStore::new(global,meerkat_models::canonical()).patch(ConfigDelta(json!({"tools":{"mcp_servers":[inherited]}}))).await.unwrap();
            let result = inventory(&fixture.storage,&fixture.factory,&Inventory {after:None}).await.unwrap();
            assert_eq!(result["services"].as_array().unwrap().len(),1);
            assert_eq!(result["services"][0]["service"],"cards");
            let printed = result.to_string();
            for excluded in ["global-only","private-command","private-argument","INLINE_PRIVATE","private-inline-value"] {
                assert!(!printed.contains(excluded));
            }
            assert!(printed.contains("KEY") && printed.contains("TOKEN"));
            assert_eq!(fixture.store.saves.load(Ordering::SeqCst),0);
            assert_eq!(fixture.store.clears.load(Ordering::SeqCst),0);
        }).await.expect("owned inventory projection must settle");
    }

    #[tokio::test]
    async fn inventory_pages_owned_services_without_repeating_or_dropping_the_next_row() {
        tokio::time::timeout(Duration::from_secs(30), async {
            let root = tempfile::tempdir().unwrap();
            let fixture = Fixture::new(root.path()).await;
            let servers = (0..PAGE_SIZE+1).map(|index| {let mut config=fixture.config.clone(); config.name=format!("service-{index:02}"); config}).collect::<Vec<_>>();
            FileConfigStore::new(fixture.config_path.clone(),meerkat_models::canonical())
                .patch(ConfigDelta(json!({"tools":{"mcp_servers":servers}}))).await.unwrap();
            let first = inventory(&fixture.storage,&fixture.factory,&Inventory {after:None}).await.unwrap();
            assert_eq!(first["services"].as_array().unwrap().len(),PAGE_SIZE);
            assert_eq!(first["next"],"service-15");
            let second = inventory(&fixture.storage,&fixture.factory,&Inventory {after:Some(first["next"].as_str().unwrap().into())}).await.unwrap();
            assert_eq!(second["services"].as_array().unwrap().len(),1);
            assert_eq!(second["services"][0]["service"],"service-16");
            assert!(second["next"].is_null());
            assert_eq!(fixture.store.saves.load(Ordering::SeqCst),0);
            assert_eq!(fixture.store.clears.load(Ordering::SeqCst),0);
        }).await.expect("bounded names-only inventory pages must settle");
    }

    #[test]
    fn management_arguments_cannot_supply_secrets_config_or_duplicate_slots() {
        for key in ["value","credentials","config","command","agent","grant"] {
            let mut supplied = json!({"service":"cards","slots":["KEY"],"action":"set"});
            supplied[key]=json!("fixture-private");
            assert!(serde_json::from_value::<Change>(supplied).is_err());
        }
        assert!(!valid_change(&change(Action::Set,&["KEY","KEY"])));
        assert!(!valid_change(&change(Action::Set,&[])));
        assert!(valid_change(&change(Action::Set,&["KEY"])));
    }
}
