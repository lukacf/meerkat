//! Protected initial installation of a checked stdio service's named secrets.
//! This path never repairs or rotates an existing configured installation.

pub(super) mod management;

use super::*;
use crate::{service_install::{InstalledConnectorSelection, ServiceCredentialSlot, SelectedServiceInputs}, terminal::ServiceSecret};
use std::collections::BTreeSet;
#[cfg(test)]
use crate::service_install::ServiceCredentialReference;
use meerkat_core::{ConfigDelta, McpRealmPersistTarget};
use meerkat::governed_realm::stock::issuance::{ReviewedMcpInstallation, grant_stock_agent_for_reviewed_installation};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Proposal {
    job_id: String,
    operations: Vec<String>,
    agent: String,
    expires_in_ms: Option<u64>,
}

pub struct SecretRequest {
    selection: InstalledConnectorSelection,
    config: McpServerConfig,
    grant: StockGrantRequest,
    response: oneshot::Sender<Value>,
}

/// The same immutable checked installation and real protected approval survive
/// setup close. Only this value retains the hidden input, never its tool reply.
pub struct ReviewedSecretInstallation {
    storage: ResolvedStorage,
    factory: meerkat::AgentFactory,
    installed: InstalledSelection,
    document: Arc<Value>,
    selected_inputs: Option<SelectedServiceInputs>,
    references: Vec<ServiceCredentialSlot>,
    config: McpServerConfig,
    own_servers: Vec<McpServerConfig>,
    expected_generation: u64,
    request: StockGrantRequest,
    decision: ApprovalRecord,
    values: Vec<ServiceSecret>,
}

fn refused() -> ToolError {
    ToolError::execution_failed("A current reconciled installation, exact raw operations and complete checked secret environment requirements are required. Use only protected terminal input for credential values. No secret or grant was written.")
}

pub(super) fn tool() -> Arc<ToolDef> {
    Arc::new(ToolDef::new(SECRET_TOOL,
        "Ask the protected terminal to initially save a checked stdio service, store its declared credential slots in the selected realm vault, and grant one agent exact raw operations. Library profiles require setup_check, install/reconcile and setup_help after the check with complete credential_environment_references. Outside-catalog Python/Node packages require setup_selected_check, install/reconcile and explicit environment_secret_slots plus operation_tools in the checked selected portable entry; the terminal asks for values privately. A current toolkit_select_installed_service result can supply the same verified package and complete metadata after an earlier credential setup ended, without reinstalling. Supply only job id, actual raw operations, consumer agent and explicit expires_in_ms (null means until revoked). Never request or supply credentials in chat, environment values, commands, native account identities or grants. Existing configured targets and credential rotation are not supported here. Finalization runs after setup closes and does not invoke the service or prove its account/readiness.",
        json!({"type":"object","additionalProperties":false,
            "required":["job_id","operations","agent","expires_in_ms"],
            "properties":{"job_id":{"type":"string"},
                "operations":{"type":"array","minItems":1,"maxItems":128,"uniqueItems":true,
                    "items":{"type":"string","minLength":1,"maxLength":128}},
                "agent":{"type":"string"},"expires_in_ms":{"type":["integer","null"],"minimum":1}}})))
}

pub(super) async fn dispatch(owner: &RequestTool, call: ToolCallView<'_>, context: &ToolDispatchContext)
    -> Result<ToolDispatchOutcome, ToolError>
{
    if call.args.get().len() > 32 * 1024 { return Err(refused()); }
    serde_json::from_str::<crate::forge::UniqueJson>(call.args.get()).map_err(|_| refused())?;
    let raw: Value = call.parse_args().map_err(|_| refused())?;
    if raw.get("expires_in_ms").is_none() { return Err(refused()); }
    let proposal: Proposal = serde_json::from_value(raw).map_err(|_| refused())?;
    let operations: BTreeSet<_> = proposal.operations.iter().cloned().collect();
    if proposal.expires_in_ms == Some(0) || operations.is_empty() || operations.len() > 128
        || operations.len() != proposal.operations.len()
        || operations.iter().any(|name| name.is_empty() || name.len() > 128
            || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b)))
    { return Err(refused()); }
    let session = context.origin_session_id().ok_or_else(refused)?;
    stock_agent_principal(&owner.storage.locator.realm, &proposal.agent).map_err(|_| refused())?;
    let selection = owner.installs.connector_selection(&owner.storage.locator, session, &proposal.job_id)
        .map_err(|_| refused())?;
    let config = selection.secret_configuration(&operations).map_err(|_| refused())?;
    let grant = StockGrantRequest { agent:proposal.agent, expires_in_ms:proposal.expires_in_ms,
        target:StockGrantTarget::Mcp {server:config.name.clone(),operations:McpOperations::Exact(operations)} };
    let (response, receive) = oneshot::channel();
    owner.requests.try_send(ConnectorRequest::Secret(SecretRequest {selection,config,grant,response}))
        .map_err(|_| refused())?;
    let result = receive.await.map_err(|_| ToolError::execution_failed(
        "Protected service-credential review ended without a completed handoff. No readiness is established."))?;
    Ok(ToolResult::new(call.id.into(), result.to_string(), false).into())
}

fn require_absent(servers: &[McpServerConfig], config: &McpServerConfig) -> Result<(), ConnectError> {
    if servers.iter().any(|server| server.name == config.name) {
        return Err(unavailable("This service name is already configured. Initial secret setup cannot reuse, repair or rotate it."));
    }
    Ok(())
}

async fn initial_snapshot(storage: &ResolvedStorage, config: &McpServerConfig)
    -> Result<(Vec<McpServerConfig>, u64), ConnectError>
{
    initial_snapshot_if_absent(storage, config).await?.ok_or_else(|| unavailable(
        "This service name is already configured. Initial secret setup cannot reuse, repair or rotate it."))
}

// Only an occupied name is a request-local refusal. Native configuration read
// failures retain their error path; the post-close finalizer keeps its contract.
async fn initial_snapshot_if_absent(storage: &ResolvedStorage, config: &McpServerConfig)
    -> Result<Option<(Vec<McpServerConfig>, u64)>, ConnectError>
{
    // Do not silently shadow an inherited installation either. This effective
    // read is a preflight; only the own-document expected generation admits add.
    let effective = crate::realm::read_config(storage).await?;
    if effective.tools.mcp_servers.iter().any(|server| server.name == config.name) { return Ok(None); }
    let current = crate::realm::read_own_mcp_config(storage).await?;
    if current.0.iter().any(|server| server.name == config.name) { return Ok(None); }
    Ok(Some(current))
}

async fn preview(storage: &ResolvedStorage, servers: &[McpServerConfig], config: &McpServerConfig)
    -> Result<(), ConnectError>
{
    require_absent(servers, config)?;
    let mut candidate = servers.to_vec();
    candidate.push(config.clone());
    crate::realm::own_config_runtime(storage)?.patch_preview(&ConfigDelta(json!({"tools":{"mcp_servers":candidate}}))).await
        .map_err(|e| ConnectError::native("Preview initial service configuration", e))?
        .validate_for_write(meerkat_models::canonical(), servers)
        .map_err(|e| ConnectError::native("Validate initial service configuration", e))?;
    Ok(())
}

pub(super) async fn present<F: Future<Output = io::Result<()>> + Send, W: Write>(
    owner: &mut TerminalConnectors, mut request: SecretRequest, approvals: &LocalApprovals,
    presenter: &mut ApprovalPresenter, input: &mut TerminalInput, output: &mut W, interrupt: F,
) -> Result<bool, ConnectError> {
    if request.response.is_closed() { return Ok(true); }
    if owner.reviewed.is_some() {
        let _ = request.response.send(json!({"state":"pending","reason":"reviewed_finalization_waiting_for_setup_close","grant_issued":false}));
        return Ok(true);
    }
    if owner.target_unresolved(&request.config.name) {
        return selection_refused("prior_finalization_unresolved", request.response, output);
    }
    if input.require_terminal().is_err() {
        return selection_refused("protected_terminal_required", request.response, output);
    }
    let check = || -> Result<bool, ConnectError> {
        approvals.check_session(&request.selection.installed.session)?;
        let current = request.selection.check_current();
        approvals.check_session(&request.selection.installed.session)?;
        Ok(current.is_ok())
    };
    if !check()? { return selection_refused("installation_changed", request.response, output); }
    let documents = request_documents(plain::recheck_retained(
        &request.selection.installed, &request.selection.configuration, request.selection.selected_inputs(),
    )).await;
    if !check()? { return selection_refused("installation_changed", request.response, output); }
    if let Err(reason) = documents { return selection_refused(reason, request.response, output); }
    let snapshot = initial_snapshot_if_absent(&owner.storage, &request.config).await?;
    approvals.check_session(&request.selection.installed.session)?;
    let Some((own_servers, expected_generation)) = snapshot else {
        return selection_refused("server_name_already_selected", request.response, output);
    };
    preview(&owner.storage, &own_servers, &request.config).await?;
    owner.factory.resolution_provider_auth_persistence()
        .map_err(|_| unavailable("The original native credential persistence is unavailable."))?
        .ok_or_else(|| unavailable("The original native credential persistence is unavailable."))?;
    let references = request.selection.secret_slots().map_err(unavailable)?;
    // This display does not replace the typed request retained for native issuance.
    let grant = match &request.grant.target {
        StockGrantTarget::Mcp {server,operations:McpOperations::Exact(operations)} => json!({
            "agent":request.grant.agent,"expires_in_ms":request.grant.expires_in_ms,
            "target":{"kind":"mcp","server":server,"operations":{"kind":"exact","names":operations}}}),
        _ => return selection_refused("unsupported_service_grant", request.response, output),
    };
    let body = json!({"phase":"initial_service_secret_installation","realm":owner.storage.locator.realm.as_str(),
        "job_id":request.selection.installed.job_id,"configuration":request.config,"expected_config_generation":expected_generation,
        "credential_slots":references,"operation_basis":request.selection.operation_basis(),"grant":grant,
        "authentication":"opaque service credentials; no verified vendor account is asserted",
        "notice":"Save this service in the selected realm, privately store its requested credentials, and give the chosen agent the listed access. Cancellation or failure can leave saved configuration or credentials. The connection will be checked when used."});
    tokio::pin!(interrupt);
    let decision = match review(approvals, presenter, input, output,
        review_request(&request.selection.installed.session, "Store credentials and enable this new service?", body)?,
        &mut request.response, interrupt.as_mut()).await? {
        ReviewOutcome::Decided(value) => value,
        ReviewOutcome::CallerGone => return Ok(true),
        ReviewOutcome::Stopped => return Ok(false),
    };
    if decision.status != ApprovalStatus::Approved {
        let _ = request.response.send(json!({"state":"denied","phase":"service_credentials","grant_issued":false}));
        return Ok(true);
    }
    let mut values = Vec::new();
    for reference in &references {
        if !check()? { return selection_refused("installation_changed", request.response, output); }
        if request.response.is_closed() { return Ok(true); }
        writeln!(output, "Credential for {} ({})", review_text(&reference.label()), review_text(reference.environment_variable()))
            .map_err(|e| ConnectError::native("Display service credential slot", e))?;
        let value = tokio::select! { biased;
            signal = interrupt.as_mut() => {signal.map_err(|e| ConnectError::native("Listen for service credential cancellation", e))?; return Ok(false)},
            _ = request.response.closed() => return Ok(true),
            value = input.read_service_secret(output) => value.map_err(|e| ConnectError::native("Read hidden service credential", e))?,
        };
        let Some(value) = value else { return Ok(false); };
        values.push(value);
    }
    if !check()? { return selection_refused("installation_changed", request.response, output); }
    let documents = request_documents(plain::recheck_retained(
        &request.selection.installed, &request.selection.configuration, request.selection.selected_inputs(),
    )).await;
    if !check()? { return selection_refused("installation_changed", request.response, output); }
    if let Err(reason) = documents { return selection_refused(reason, request.response, output); }
    let snapshot = initial_snapshot_if_absent(&owner.storage, &request.config).await?;
    approvals.check_session(&request.selection.installed.session)?;
    match snapshot {
        None => return selection_refused("server_name_already_selected", request.response, output),
        Some(current) if current != (own_servers.clone(), expected_generation) =>
            return selection_refused("reviewed_realm_changed", request.response, output),
        Some(_) => (),
    }
    if request.response.is_closed() { return Ok(true); }
    if let Some(signal) = interrupt.as_mut().now_or_never() {
        signal.map_err(|e| ConnectError::native("Listen for service credential cancellation", e))?;
        return Ok(false);
    }
    let selected_inputs = request.selection.selected_inputs().cloned();
    owner.reviewed = Some(ReviewedInstallation::Secret(ReviewedSecretInstallation {
        storage:owner.storage.clone(),factory:owner.factory.clone(),installed:request.selection.installed,
        document:request.selection.configuration,selected_inputs,references,config:request.config,own_servers,expected_generation,
        request:request.grant,decision,values,
    }).into());
    let _ = request.response.send(json!({"state":"pending","phase":"initial_service_secret_installation",
        "reason":"reviewed_finalization_waiting_for_setup_close","grant_issued":false,"credentials_stored":false,
        "instruction":"The protected review and hidden values are retained. Finish this setup turn so the existing owner can close and finalize. Do not request credentials in chat, repeat this action or claim service readiness."}));
    Ok(true)
}

type GrantResult = Result<meerkat::governed_realm::stock::issuance::StockGrantOutcome, StockGrantError>;
type Observation = Result<meerkat::McpServiceSecretObservation, meerkat_core::auth::McpServiceSecretRefusal>;
type SlotResult = Result<meerkat_core::auth::McpServiceSecretRef, meerkat::McpServiceSecretUpdateError>;

#[derive(Debug)]
pub(in crate::terminal_connector) enum Stop { Cancelled, ListenerFailed(io::Error) }

/// The existing caller owns this future. A signal prevents the next effect;
/// it never drops an already-entered native mutation or fabricates rollback.
pub(in crate::terminal_connector) async fn joined_effect<I, F, T>(mut interrupt: Pin<&mut I>, stop: &mut Option<Stop>, effect: impl FnOnce() -> F)
    -> Option<T>
where I: Future<Output = io::Result<()>>, F: Future<Output = T> {
    if stop.is_some() { return None; }
    if let Some(signal) = interrupt.as_mut().now_or_never() {
        *stop = Some(match signal { Ok(()) => Stop::Cancelled, Err(error) => Stop::ListenerFailed(error) });
        return None;
    }
    let running = effect();
    tokio::pin!(running);
    let mut polled = false;
    let completed = tokio::select! { biased;
        signal = interrupt.as_mut() => Err(signal),
        result = std::future::poll_fn(|cx| {
            polled = true;
            running.as_mut().poll(cx)
        }) => Ok(result),
    };
    match completed {
        Ok(result) => Some(result),
        Err(signal) => {
            *stop = Some(match signal { Ok(()) => Stop::Cancelled, Err(error) => Stop::ListenerFailed(error) });
            // If select observed the signal before ever polling the native
            // future, no administration entered. Otherwise retain that exact
            // future through settlement, even if its first await was a read.
            if polled { Some(running.await) } else { None }
        },
    }
}

/// This invocation's actual results, not a recovery owner or a tuple receipt.
/// Native payloads remain in memory and are never rendered through Debug.
#[derive(Default)]
pub(super) struct Outcome {
    persistence: Option<Result<u64, meerkat_core::mcp_config::McpRealmPersistError>>,
    before: Option<Observation>,
    slots: Vec<(String, SlotResult)>,
    after: Option<Observation>,
    grant: Option<GrantResult>,
    stopped: Option<Stop>,
    refusal: Option<&'static str>,
}
impl std::fmt::Debug for Outcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceSecretFinalization").field("observed_slot_results", &self.slots.len())
            .field("stopped", &self.stopped.is_some()).finish_non_exhaustive()
    }
}

fn exact_presence(observed: &meerkat::McpServiceSecretObservation, realm: &meerkat_core::RealmId,
    references: &[ServiceCredentialSlot], expected: meerkat_core::auth::McpServiceSecretPresence) -> bool {
    &observed.owner == realm && observed.slots.len() == references.len()
        && references.iter().all(|reference| observed.slots.get(reference.environment_variable()) == Some(&expected))
}

fn update_kind(error: &meerkat::McpServiceSecretUpdateError) -> &'static str {
    use meerkat::McpServiceSecretUpdateError as Error;
    match error {
        Error::Invalid(_) => "Invalid", Error::DefinitionChanged(_) => "DefinitionChanged",
        Error::StillCurrent(_) => "StillCurrent", Error::ConfigurationUnavailable(_) => "ConfigurationUnavailable",
        Error::Ownership(_) => "Ownership", Error::NotAdmitted(_) => "NotAdmitted",
        Error::OutcomeUnknown(_) => "OutcomeUnknown",
    }
}

fn observation_facts(observation: &Option<Observation>) -> Value {
    match observation {
        None => Value::Null,
        Some(Err(_)) => json!({"state":"unavailable"}),
        Some(Ok(observed)) => json!({"owner":observed.owner.as_str(),
            "slots":observed.slots.iter().map(|(name,presence)| (name, match presence {
                meerkat_core::auth::McpServiceSecretPresence::Stored => "Stored",
                meerkat_core::auth::McpServiceSecretPresence::NotStored => "NotStored",
                meerkat_core::auth::McpServiceSecretPresence::NotObserved => "NotObserved",
            })).collect::<BTreeMap<_,_>>(),
            "scope":"presence_only_not_update_receipt_or_vendor_authentication"}),
    }
}

impl Outcome {
    pub(super) fn stopped(&self) -> bool { self.stopped.is_some() }
    pub(super) fn grant(&self) -> Option<&GrantResult> { self.grant.as_ref() }
    pub(super) fn has_unconfirmed_effect(&self) -> bool {
        self.persistence.as_ref().is_some_and(|result|
            result.as_ref().is_err_and(public_persistence_may_have_changed))
            || self.slots.iter().any(|(_, result)|
                matches!(result, Err(meerkat::McpServiceSecretUpdateError::OutcomeUnknown(_))))
            || self.grant.as_ref().is_some_and(|result| unconfirmed_grant_effect(result.as_ref()))
    }
    pub(super) fn blocks_reentry(&self) -> bool {
        match &self.persistence {
            None => false,
            Some(Ok(_)) => true,
            Some(Err(error)) => public_persistence_may_have_changed(error),
        }
    }
    pub(super) fn message(&self, target: &FinalizationTarget) -> Result<String, ConnectError> {
        if let (Some(Ok(generation)), Some(grant)) = (&self.persistence, &self.grant) {
            return configured_grant_message_ref(&format!(
                "Saved {} in realm {} at generation {} and privately stored the requested credentials. The connection will be checked when used",
                target.server, target.realm.realm.as_str(), generation), &target.agent, grant.as_ref());
        }
        let saved = match &self.persistence {
            None => "No service configuration or credential write was entered.",
            Some(Ok(_)) => "The service configuration was saved; some credentials may also be saved.",
            Some(Err(error)) if public_persistence_may_have_changed(error) => "The configuration write has an uncertain outcome; no credentials were written.",
            Some(Err(_)) => "The configuration writer refused; no credentials were written.",
        };
        Err(unavailable(&format!("Service setup {}. {saved} No new access was requested. Do not repeat this installation automatically.",
            if self.stopped.is_some() { "stopped" } else { "did not complete" })))
    }
    pub(super) fn facts(&self, grant: Value) -> Value {
        let persistence = match &self.persistence {
            None => json!({"state":"not_entered"}),
            Some(Ok(generation)) => json!({"state":"saved","generation":generation}),
            Some(Err(error)) => json!({"state":if public_persistence_may_have_changed(error) {"uncertain"} else {"refused"},
                "error":public_persistence_details(error)}),
        };
        json!({"persistence":persistence,"initial_presence":observation_facts(&self.before),
            "slot_writes":self.slots.iter().map(|(env,result)| match result {
                Ok(reference) => json!({"env":env,"state":"stored","reference":{"realm":reference.realm().as_str(),
                    "server":reference.server(),"env":reference.env(),"meaning":reference.meaning()}}),
                Err(error) => json!({"env":env,"state":if matches!(error,meerkat::McpServiceSecretUpdateError::OutcomeUnknown(_)) {"uncertain"} else {"refused"},"error_kind":update_kind(error)}),
            }).collect::<Vec<_>>(),"final_presence":observation_facts(&self.after),"grant":grant,
            "stop":self.stopped.as_ref().map(|stop| match stop { Stop::Cancelled => "cancelled",
                Stop::ListenerFailed(error) => { let _ = error.kind(); "listener_failed" } }),
            "refusal":self.refusal,"vendor_account":"not_verified"})
    }
}

impl ReviewedSecretInstallation {
    pub(super) async fn finalize_outcome_after_close<I: Future<Output = io::Result<()>>>(self,
        mut interrupt: Pin<&mut I>) -> FinalizedInstallation {
        let target = FinalizationTarget::new(&self.storage, &self.config, &self.request,
            &self.installed.job_id, &self.installed.session);
        let outcome = match self.finalize_effect_after_close(interrupt.as_mut()).await {
            Ok(outcome) => FinalizationOutcome::Secret(outcome),
            Err(error) => FinalizationOutcome::BeforeAdministrationRefused(error),
        };
        FinalizedInstallation::new(target, outcome)
    }

    async fn finalize_effect_after_close<I: Future<Output = io::Result<()>>>(self, mut interrupt: Pin<&mut I>)
        -> Result<Outcome, ConnectError> {
        let StockGrantTarget::Mcp { server, operations: McpOperations::Exact(operations) } = &self.request.target else {
            return Err(unavailable("The exact protected service review is required."));
        };
        if self.decision.status != ApprovalStatus::Approved || self.installed.realm != self.storage.locator
            || self.installed.server_name != self.config.name || server != &self.config.name
            || self.references.len() != self.values.len()
            || crate::service_install::secret_launch_configuration(&self.installed, &self.document, operations, &self.references)
                .map_err(unavailable)? != self.config
        { return Err(unavailable("The exact protected service review changed. Nothing was finalized.")); }
        plain::recheck_retained(&self.installed, &self.document, self.selected_inputs.as_ref()).await?;
        if initial_snapshot(&self.storage, &self.config).await? != (self.own_servers.clone(), self.expected_generation) {
            return Err(unavailable("The reviewed realm changed. No service configuration or credential was written."));
        }
        preview(&self.storage, &self.own_servers, &self.config).await?;
        let persistence = self.factory.resolution_provider_auth_persistence()
            .map_err(|_| unavailable("The original native credential persistence is unavailable."))?
            .ok_or_else(|| unavailable("The original native credential persistence is unavailable."))?;
        let source = meerkat::RealmMcpServiceSecrets::from_realm_source(crate::realm::config_source(&self.storage)?,
            self.storage.locator.realm.clone(), persistence.token_store().clone());
        let principals = StockPrincipals::for_local_operator(&self.storage.locator.realm)
            .map_err(|error| ConnectError::native("Identify selected realm operator", error))?;
        let runtime = crate::realm::own_config_runtime(&self.storage)?;
        let target = McpRealmPersistTarget::new(&runtime);
        let mut outcome = Outcome::default();
        // Only the create-only generation winner may reach any slot setter.
        // An existing exact definition is not a retry or a repair permission.
        let Some(saved) = joined_effect(interrupt.as_mut(), &mut outcome.stopped, ||
            target.add(self.config.clone(), Some(self.expected_generation))).await else { return Ok(outcome); };
        let generation = match saved {
            Ok(saved) => { outcome.persistence = Some(Ok(saved.generation)); saved.generation },
            Err(error) => { outcome.persistence = Some(Err(error)); return Ok(outcome); },
        };
        outcome.before = joined_effect(interrupt.as_mut(), &mut outcome.stopped, || source.observe(&self.config)).await;
        if outcome.before.as_ref().is_none_or(|result| !result.as_ref().is_ok_and(|observed|
            exact_presence(observed, &self.storage.locator.realm, &self.references, meerkat_core::auth::McpServiceSecretPresence::NotStored))) {
            if outcome.stopped.is_none() { outcome.refusal = Some("initial_slots_not_confirmed_empty"); }
            return Ok(outcome);
        }
        for (reference, value) in self.references.iter().zip(&self.values) {
            let Some(result) = joined_effect(interrupt.as_mut(), &mut outcome.stopped, ||
                meerkat::set_mcp_service_secret(&self.storage, &persistence, &self.config,
                    reference.environment_variable(), value.expose())).await else { return Ok(outcome); };
            let valid = result.as_ref().is_ok_and(|observed| observed.realm() == &self.storage.locator.realm
                && observed.server() == self.config.name.as_str() && observed.env() == reference.environment_variable());
            outcome.slots.push((reference.environment_variable().to_owned(), result));
            if !valid { outcome.refusal = Some("slot_write_not_confirmed"); return Ok(outcome); }
        }
        outcome.after = joined_effect(interrupt.as_mut(), &mut outcome.stopped, || source.observe(&self.config)).await;
        if outcome.after.as_ref().is_none_or(|result| !result.as_ref().is_ok_and(|observed|
            exact_presence(observed, &self.storage.locator.realm, &self.references, meerkat_core::auth::McpServiceSecretPresence::Stored))) {
            if outcome.stopped.is_none() { outcome.refusal = Some("current_slot_presence_unavailable"); }
            return Ok(outcome);
        }
        let installation = ReviewedMcpInstallation { server: self.config.clone(), generation };
        outcome.grant = joined_effect(interrupt.as_mut(), &mut outcome.stopped, ||
            grant_stock_agent_for_reviewed_installation(&self.storage, &principals, &self.request, &installation)).await;
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_exit_distinguishes_unknown_write_from_stopped_before_grant() {
        let root = tempfile::tempdir().unwrap();
        let storage = crate::RealmOptions { context_root:root.path().to_owned(),
            state_root:Some(root.path().join("state")), user_config_root:Some(root.path().join("home")) }
            .resolve("exit-fixture").unwrap();
        for (outcome, expected) in [
            (Outcome { stopped:Some(Stop::Cancelled), ..Default::default() }, false),
            (Outcome { persistence:Some(Ok(7)), stopped:Some(Stop::Cancelled), ..Default::default() }, false),
            (Outcome { persistence:Some(Ok(7)), slots:vec![("TOKEN".into(),
                Err(meerkat::McpServiceSecretUpdateError::OutcomeUnknown("PRIVATE_SLOT_DIAGNOSTIC".into())))],
                stopped:Some(Stop::Cancelled), ..Default::default() }, true),
            (Outcome { persistence:Some(Ok(7)), slots:vec![("TOKEN".into(),
                Err(meerkat::McpServiceSecretUpdateError::NotAdmitted(
                    meerkat_core::AdministrationEntryRefusal::NotAdministrator)))],
                stopped:Some(Stop::Cancelled), ..Default::default() }, false),
        ] {
            let installation = FinalizedInstallation::new(FinalizationTarget {
                realm:storage.locator.clone(), server:"mail".into(), agent:"fixture".into(),
                job_id:"reviewed-job".into(), session:SessionId::new(),
            }, FinalizationOutcome::Secret(outcome));
            let action = FinalizedAction { outcome:ActionOutcome::Installation(Arc::new(installation)),
                continuation:None };
            assert!(action.stopped_during_finalization());
            let result = FinalizedAction::finish_invocation(&[action], Ok(()));
            assert_eq!(result.is_err(), expected);
            if let Err(error) = result { assert!(!error.to_string().contains("PRIVATE_SLOT_DIAGNOSTIC")); }
        }
    }
    use meerkat_auth_core::auth_store::{FileLockCoordinator, FileTokenStore, ProviderAuthPersistence,
        PersistedTokens, TokenKey, TokenStore, TokenStoreError};
    use meerkat_core::{ApprovalService, ConfigStore, FileConfigStore};
    use sha2::{Digest, Sha256};
    use std::{path::{Path, PathBuf}, sync::atomic::{AtomicUsize, Ordering}};
    use tokio::sync::Semaphore;

    // Real file-vault delegation with a bounded effect schedule. It does not
    // stand in for provider authentication or a successfully launched service.
    struct ControlledStore {
        inner: FileTokenStore,
        saves: AtomicUsize,
        hold_first: bool,
        fail_after: Option<usize>,
        entered: Semaphore,
        release: Semaphore,
    }
    struct ReleaseOnDrop(Arc<ControlledStore>);
    impl Drop for ReleaseOnDrop {
        fn drop(&mut self) { self.0.release.add_permits(1); }
    }
    #[async_trait]
    impl TokenStore for ControlledStore {
        async fn load(&self, key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError> {
            self.inner.load(key).await
        }
        async fn save(&self, key: &TokenKey, value: &PersistedTokens) -> Result<(), TokenStoreError> {
            let ordinal = self.saves.fetch_add(1, Ordering::SeqCst) + 1;
            if self.hold_first && ordinal == 1 {
                self.entered.add_permits(1);
                self.release.acquire().await.unwrap().forget();
            }
            self.inner.save(key, value).await?;
            if self.fail_after == Some(ordinal) {
                return Err(TokenStoreError::Unavailable("controlled lost store acknowledgement".into()));
            }
            Ok(())
        }
        async fn clear(&self, key: &TokenKey) -> Result<(), TokenStoreError> { self.inner.clear(key).await }
        async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> { self.inner.list().await }
        fn backend_name(&self) -> &'static str { "controlled-native-service-secret-file" }
    }

    struct Fixture {
        storage: ResolvedStorage,
        factory: meerkat::AgentFactory,
        store: Arc<ControlledStore>,
        installed: InstalledSelection,
        document: Arc<Value>,
        config: McpServerConfig,
        references: Vec<ServiceCredentialSlot>,
        own: Vec<McpServerConfig>,
        generation: u64,
        config_path: PathBuf,
    }
    impl Fixture {
        async fn new(root: &Path, hold_first: bool, fail_after: Option<usize>) -> Self {
            let storage = crate::RealmOptions { context_root:root.to_owned(), state_root:Some(root.join("state")),
                user_config_root:Some(root.join("home")) }.resolve("secret-work").unwrap();
            let config_path = crate::realm::config_source(&storage).unwrap().config_doc_path(&storage.locator.realm);
            FileConfigStore::new(config_path.clone(), meerkat_models::canonical())
                .patch(ConfigDelta(json!({"max_tokens":1234,"tools":{"shell_enabled":false,"mcp_servers":[]}})))
                .await.unwrap();
            let (_, bundle) = meerkat::storage_provider::open_realm_persistence_with_layout(
                storage.layout.clone(), storage.locator.realm.as_str(), None,
                Some(meerkat_store::RealmOrigin::Explicit)).await.unwrap();
            drop(bundle);
            let principals = StockPrincipals::for_local_operator(&storage.locator.realm).unwrap();
            meerkat::governed_realm::stock::govern_stock_realm(&storage.locator.state_root,
                storage.locator.realm.as_str(), &principals).await.unwrap();
            let store = Arc::new(ControlledStore { inner:FileTokenStore::new(root.join("credentials")),
                saves:AtomicUsize::new(0),hold_first,fail_after,entered:Semaphore::new(0),release:Semaphore::new(0) });
            let factory = meerkat::AgentFactory::new(root.join("factory")).with_provider_auth_persistence(
                ProviderAuthPersistence::new(store.clone(),Arc::new(FileLockCoordinator::new(root.join("credential-locks")))));
            let installed_root = root.join("installed");
            std::fs::create_dir_all(installed_root.join("ready")).unwrap();
            std::fs::create_dir_all(installed_root.join("configuration")).unwrap();
            let selected = br#"{"profile":"trello","config":{"api_key_env":"FIXTURE_KEY","token_env":"FIXTURE_TOKEN"}}"#;
            let receipt = br#"{"scope":"inert package fixture"}"#;
            // The executable is never run. These controls exercise retained
            // documents and the real realm/vault/grant owners only.
            let document = Arc::new(json!({"mcpServers":{"cards":{"command":installed_root.join("runtime/bin/python"),
                "args":["-I","-m","meerkat_toolkit_services.adapters.agno_mcp","serve","--config",installed_root.join("configuration/selected.json")]}}}));
            let document_bytes = serde_json::to_vec(document.as_ref()).unwrap();
            std::fs::write(installed_root.join("ready/installed.json"),receipt).unwrap();
            std::fs::write(installed_root.join("configuration/selected.json"),selected).unwrap();
            std::fs::write(installed_root.join("ready/mcp.json"),&document_bytes).unwrap();
            let installed = InstalledSelection { realm:storage.locator.clone(),session:SessionId::new(),job_id:"1".repeat(32),
                validation_id:"2".repeat(32),family:"agno".into(),profile:"trello".into(),server_name:"cards".into(),
                mcp_path:installed_root.join("ready/mcp.json"),receipt_path:installed_root.join("ready/installed.json"),
                receipt_sha256:Some(format!("{:x}",Sha256::digest(receipt))),
                configuration_sha256:Some(format!("{:x}",Sha256::digest(selected))),
                mcp_document_sha256:format!("{:x}",Sha256::digest(&document_bytes)),recovery_request_sha256:Some("3".repeat(64)) };
            let references = vec![ServiceCredentialReference {field_path:vec!["config".into(),"api_key_env".into()],environment_variable:"FIXTURE_KEY".into()},
                ServiceCredentialReference {field_path:vec!["config".into(),"token_env".into()],environment_variable:"FIXTURE_TOKEN".into()}].into_iter().map(ServiceCredentialSlot::LibraryField).collect::<Vec<_>>();
            let config = crate::service_install::secret_launch_configuration(&installed,&document,
                &BTreeSet::from(["list_cards".into()]),&references).unwrap();
            let (own,generation) = initial_snapshot(&storage,&config).await.unwrap();
            Self {storage,factory,store,installed,document,config,references,own,generation,config_path}
        }
        fn review(&self) -> ReviewedSecretInstallation {
            let approvals = ApprovalService::new();
            let requested = approvals.request(review_request(&self.installed.session,"Controlled service-secret review",
                json!({"scope":"post-close realm/vault boundary only"})).unwrap()).unwrap();
            let decision = approvals.decide(&requested.approval_id,ApprovalDecision::Approve,
                ApprovalPrincipalId::new("fixture-operator").unwrap(),None,None).unwrap();
            ReviewedSecretInstallation {storage:self.storage.clone(),factory:self.factory.clone(),installed:self.installed.clone(),
                document:self.document.clone(),selected_inputs:None,references:self.references.clone(),config:self.config.clone(),own_servers:self.own.clone(),
                expected_generation:self.generation,request:StockGrantRequest {agent:"rkat-agent".into(),expires_in_ms:Some(60000),
                    target:StockGrantTarget::Mcp {server:self.config.name.clone(),operations:McpOperations::Exact(BTreeSet::from(["list_cards".into()]))}},
                decision,values:vec![ServiceSecret::from_hidden_input("fixture-key-value".into()).unwrap(),
                    ServiceSecret::from_hidden_input("fixture-token-value".into()).unwrap()]}
        }
        fn observer(&self) -> meerkat::RealmMcpServiceSecrets {
            meerkat::RealmMcpServiceSecrets::from_realm_source(crate::realm::config_source(&self.storage).unwrap(),
                self.storage.locator.realm.clone(),self.store.clone())
        }
    }
    async fn selected_review(root: &Path, profile: &str, software: bool) -> (Fixture, ReviewedSecretInstallation) {
        let mut fixture = Fixture::new(root,false,None).await;
        let installed_root = fixture.installed.mcp_path.parent().unwrap().parent().unwrap();
        std::fs::remove_file(installed_root.join("configuration/selected.json")).unwrap();
        std::fs::remove_dir(installed_root.join("configuration")).unwrap();
        std::fs::create_dir(installed_root.join("inputs")).unwrap();
        let package = br#"{"scope":"inert selected package bytes"}"#;
        let source = br#"{"scope":"inert selected declaration bytes"}"#;
        let document_name = if software { "declaration.json" } else { "activation.json" };
        std::fs::write(installed_root.join("inputs/package.json"),package).unwrap();
        std::fs::write(installed_root.join("inputs").join(document_name),source).unwrap();
        let source_hash = format!("{:x}",Sha256::digest(source));
        let mut checked = json!({"profile":profile,"selection_sha256":source_hash,"effect_sha256":"f".repeat(64),
            "files":{"package.json":format!("{:x}",Sha256::digest(package))},
            "declared_service":{"authentication":{"kind":"binding","owner":"process","requirement":"key","credential_audience":"service"},
                "operation_tools":{"read":"list_cards"},"environment_secret_slots":["FIXTURE_KEY","FIXTURE_TOKEN"]}});
        checked["files"][document_name]=json!(source_hash);
        if software { checked["document_kind"]=json!("local_software_declaration");checked["declaration_digest"]=json!("d".repeat(64)); }
        else { checked["manifest_digest"]=json!("d".repeat(64)); }
        let inputs = crate::service_install::selected_inputs_for_test(&checked);
        fixture.installed.family = format!("selected-{profile}"); fixture.installed.profile=profile.into();
        fixture.installed.configuration_sha256=None;
        fixture.references=inputs.environment_secret_slots.as_ref().unwrap().names().iter().cloned()
            .map(ServiceCredentialSlot::SelectedEnvironment).collect();
        let mut document=fixture.document.as_ref().clone();
        document["mcpServers"]["cards"]["args"] = if profile=="python" {json!(["-I","-B","-m","selected_service"])} else {json!(["/fixture/selected_service/index.js"])};
        document["mcpServers"]["cards"]["command"]=json!(if profile=="python" {"/fixture/python"} else {"/fixture/node"});
        let bytes=serde_json::to_vec(&document).unwrap();
        std::fs::write(&fixture.installed.mcp_path,&bytes).unwrap();
        fixture.installed.mcp_document_sha256=format!("{:x}",Sha256::digest(bytes));
        fixture.document=Arc::new(document);
        fixture.config=crate::service_install::secret_launch_configuration(&fixture.installed,&fixture.document,
            &BTreeSet::from(["list_cards".into()]),&fixture.references).unwrap();
        let mut reviewed=fixture.review();reviewed.selected_inputs=Some(inputs);
        (fixture,reviewed)
    }

    #[tokio::test]
    async fn selected_secret_finalization_uses_both_document_forms_and_native_reopened_vault() {
        tokio::time::timeout(Duration::from_secs(30),async {
            for profile in ["python","node"] { for software in [false,true] {
                let root=tempfile::tempdir().unwrap();
                let (fixture,review)=selected_review(root.path(),profile,software).await;
                assert!(fixture.references.iter().all(|slot| matches!(slot,ServiceCredentialSlot::SelectedEnvironment(_))));
                assert!(!fixture.installed.mcp_path.parent().unwrap().parent().unwrap().join("configuration").exists());
                let result=finish(review).await;let outcome=secret_outcome(&result);
                assert!(outcome.grant.as_ref().is_some_and(Result::is_ok));
                assert_eq!(fixture.store.saves.load(Ordering::SeqCst),2);
                let (saved,_)=crate::realm::read_own_mcp_config(&fixture.storage).await.unwrap();
                assert_eq!(saved,vec![fixture.config.clone()]);
                let reopened=meerkat::RealmMcpServiceSecrets::from_realm_source(crate::realm::config_source(&fixture.storage).unwrap(),
                    fixture.storage.locator.realm.clone(),Arc::new(FileTokenStore::new(root.path().join("credentials"))));
                assert!(exact_presence(&reopened.observe(&fixture.config).await.unwrap(),&fixture.storage.locator.realm,
                    &fixture.references,meerkat_core::auth::McpServiceSecretPresence::Stored));
                let notice=result.private_observation().to_string();
                assert!(!notice.contains("fixture-key-value") && !notice.contains("fixture-token-value"));
            }}
        }).await.expect("selected native owner controls must settle");
    }

    #[tokio::test]
    async fn selected_secret_document_changes_after_setup_close_never_reach_native_writes() {
        tokio::time::timeout(Duration::from_secs(30),async {
            for profile in ["python","node"] { for software in [false,true] { for file in ["package","selection","launch","receipt"] {
                let root=tempfile::tempdir().unwrap();let (fixture,review)=selected_review(root.path(),profile,software).await;
                let installed_root=fixture.installed.mcp_path.parent().unwrap().parent().unwrap();
                let path=match file {"package"=>installed_root.join("inputs/package.json"),
                    "selection"=>installed_root.join("inputs").join(if software {"declaration.json"} else {"activation.json"}),
                    "launch"=>fixture.installed.mcp_path.clone(),_=>fixture.installed.receipt_path.clone()};
                std::fs::write(path,b"changed after protected review").unwrap();
                let result=finish(review).await;
                assert!(matches!(result.outcome,FinalizationOutcome::BeforeAdministrationRefused(_)));
                assert_eq!(fixture.store.saves.load(Ordering::SeqCst),0);
                assert_eq!(crate::realm::read_own_mcp_config(&fixture.storage).await.unwrap(),(fixture.own.clone(),fixture.generation));
            }}}
        }).await.expect("selected document refusal controls must settle");
    }

    async fn finish(reviewed: ReviewedSecretInstallation) -> FinalizedInstallation {
        ReviewedInstallation::Secret(reviewed).finalize_outcome_after_close().await
    }

    #[tokio::test]
    async fn cancellation_before_first_native_poll_does_not_enter_the_effect() {
        let mut polls = 0;
        let interrupt = std::future::poll_fn(|cx| {
            polls += 1;
            if polls == 1 { cx.waker().wake_by_ref(); std::task::Poll::Pending }
            else { std::task::Poll::Ready(Ok(())) }
        });
        tokio::pin!(interrupt);
        let effects = AtomicUsize::new(0);
        let mut stop = None;
        let result = joined_effect(interrupt.as_mut(),&mut stop,|| async {
            effects.fetch_add(1,Ordering::SeqCst);
            42
        }).await;
        assert!(result.is_none());
        assert!(matches!(stop,Some(Stop::Cancelled)));
        assert_eq!(effects.load(Ordering::SeqCst),0);
    }
    fn secret_outcome(result: &FinalizedInstallation) -> &Outcome {
        let FinalizationOutcome::Secret(outcome) = &result.outcome else { panic!("expected native secret finalization") };
        outcome
    }

    // The fixed-reader responses below are synthetic. The package controls own
    // real installation/discovery; this control joins actual PTY review to the
    // existing native finalizer across genuinely fresh selection owners.
    struct NoSetupDispatch(AtomicUsize);
    #[async_trait]
    impl AgentToolDispatcher for NoSetupDispatch {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> { Arc::from([]) }
        async fn dispatch(&self, _:ToolCallView<'_>) -> Result<ToolDispatchOutcome,ToolError> {
            self.0.fetch_add(1,Ordering::SeqCst);
            panic!("returning credential setup must not dispatch installation")
        }
    }
    #[derive(Clone,Default)]
    struct Seen(Arc<std::sync::Mutex<Vec<u8>>>);
    impl Seen { fn text(&self)->String {String::from_utf8(self.0.lock().unwrap().clone()).unwrap()} }
    impl Write for Seen {
        fn write(&mut self,bytes:&[u8])->io::Result<usize>{self.0.lock().unwrap().extend_from_slice(bytes);Ok(bytes.len())}
        fn flush(&mut self)->io::Result<()>{Ok(())}
    }
    fn returning_observation(fixture: &Fixture, request_digest: &str) -> (Value,Value) {
        let locator=json!({"setup":"setup-credential-fixture","workspace":format!("work-{}","a".repeat(32)),
            "job_id":fixture.installed.job_id,"request_sha256":request_digest,"checkpoint_sha256":"b".repeat(64)});
        let metadata=json!({"kind":"library","selection":{"family":"agno","profile":"trello","operations":["cards"],
                "values":{"api_key_env":"FIXTURE_KEY","token_env":"FIXTURE_TOKEN"}},
            "help":{"family":"agno","profile":"trello","operations":[{"id":"cards","tool":"list_cards"}],
                "fields":[{"name":"api_key_env","kind":"string","credential_reference":true},
                    {"name":"token_env","kind":"string","credential_reference":true}]},
            "configuration_check":{"credential_environment_references":[
                {"field_path":["config","api_key_env"],"environment_variable":"FIXTURE_KEY"},
                {"field_path":["config","token_env"],"environment_variable":"FIXTURE_TOKEN"}]}});
        (json!({"kind":"retained_installation_page","candidates":[{"locator":locator,"family":"agno","profile":"trello"}],"next_cursor":null,"unconfirmed":false}),
         json!({"kind":"retained_installation_readback","locator":locator,"metadata":metadata,
            "installation":{"scope":"retained_payload_and_launch_binding_only","family":"agno","profile":"trello",
                "job_id":fixture.installed.job_id,"validation_id":fixture.installed.validation_id,"recovery_request_sha256":request_digest,
                "mcp_path":fixture.installed.mcp_path,"receipt_path":fixture.installed.receipt_path,
                "receipt_sha256":fixture.installed.receipt_sha256,"configuration_sha256":fixture.installed.configuration_sha256,
                "mcp_configuration":fixture.document}}))
    }
    async fn quit_then_fresh_credential_review(fixture: &Fixture, root: &Path) -> ReviewedSecretInstallation {
        use nix::{pty::openpty,sys::termios::{tcgetattr,LocalFlags},unistd::write};
        let package_request=br#"{"scope":"inert retained package request"}"#;
        let request_path=root.join("original-package-request.json");std::fs::write(&request_path,package_request).unwrap();
        let request_digest=format!("{:x}",Sha256::digest(package_request));
        let receipt_before=std::fs::read(&fixture.installed.receipt_path).unwrap();
        let config_before=std::fs::read(&fixture.config_path).unwrap();
        let realm_path=meerkat_store::realm_paths_in(&fixture.storage.locator.state_root,fixture.storage.locator.realm.as_str());
        let grants=meerkat_store::grant_owner_store_path(&realm_path.root);let grants_before=std::fs::read(&grants).ok();
        let (page,readback)=returning_observation(fixture,&request_digest);
        let dispatcher=Arc::new(NoSetupDispatch(AtomicUsize::new(0)));
        let mut previous_handle: Option<String> = None;
        let mut previous_decision=None;
        for quit in [true,false] {
            let session=SessionId::new();
            let installs=CheckedInstalls::fixture(dispatcher.clone());installs.bind_realm(&fixture.storage.locator).unwrap();
            if let Some(key)=&previous_handle { assert!(installs.retained_select_request(&fixture.storage.locator,&session,key).is_err()); }
            let public=installs.retain_candidates(&fixture.storage.locator,&session,page.clone()).unwrap();
            let handle=public["candidates"][0]["candidate"].as_str().unwrap().to_owned();
            if !quit {
                installs.retained_select_request(&fixture.storage.locator,&session,&handle).unwrap();
                let mut changed=readback.clone();changed["installation"]["recovery_request_sha256"]=json!("0".repeat(64));
                assert!(installs.checked_retained(&fixture.storage.locator,&session,&handle,&changed).is_err());
                assert!(installs.connector_selection(&fixture.storage.locator,&session,&fixture.installed.job_id).is_err());
                assert!(installs.checked_retained(&fixture.storage.locator,&session,&handle,&json!({"state":"unavailable"})).is_err());
            }
            installs.retained_select_request(&fixture.storage.locator,&session,&handle).unwrap();
            let verified=installs.checked_retained(&fixture.storage.locator,&session,&handle,&readback).unwrap();
            let selection=installs.admit_retained(&fixture.storage.locator,&session,&handle,verified).unwrap();
            let config=selection.secret_configuration(&BTreeSet::from(["list_cards".into()])).unwrap();
            let mut startup=SessionStartup {storage:fixture.storage.clone(),config:meerkat::Config::default(),factory:fixture.factory.clone(),
                persistence:None,connector_auth:None,standalone:None};
            let mut owner=Box::pin(TerminalConnectors::open(&mut startup)).await.unwrap();
            let (approvals,mut presenter)=LocalApprovals::create_paired_host(root).unwrap();approvals.bind_session(&session).unwrap();
            let (response,receive)=oneshot::channel();
            let request=SecretRequest {selection:selection.clone(),config,grant:StockGrantRequest {agent:"rkat-agent".into(),expires_in_ms:Some(60000),
                target:StockGrantTarget::Mcp {server:"cards".into(),operations:McpOperations::Exact(BTreeSet::from(["list_cards".into()]))}},response};
            let pair=openpty(None,None).unwrap();let observer=pair.slave.try_clone().unwrap();let original=tcgetattr(&observer).unwrap();
            let mut input=TerminalInput::from_owned_fd(pair.slave);let mut output=Seen::default();let seen=output.clone();
            let human=async {
                while !seen.text().contains("Type 'approve ") {tokio::task::yield_now().await;}
                let text=seen.text();let id=text.split("Type 'approve ").last().unwrap().split('\'').next().unwrap().to_owned();
                write(&pair.master,format!("approve {id}\n").as_bytes()).unwrap();
                while !seen.text().contains("Credential for config.api_key_env")
                    || tcgetattr(&observer).unwrap().local_flags.contains(LocalFlags::ECHO) {tokio::task::yield_now().await;}
                write(&pair.master,b"fixture-key-value\n").unwrap();
                while !seen.text().contains("Credential for config.token_env")
                    || tcgetattr(&observer).unwrap().local_flags.contains(LocalFlags::ECHO) {tokio::task::yield_now().await;}
                assert_eq!(fixture.store.saves.load(Ordering::SeqCst),0);
                write(&pair.master,if quit {b"/quit\n".as_slice()} else {b"fixture-token-value\n".as_slice()}).unwrap();
                id
            };
            let (presented,decision)=tokio::join!(present(&mut owner,request,&approvals,&mut presenter,&mut input,&mut output,
                std::future::pending::<io::Result<()>>()),human);
            assert_eq!(presented.unwrap(),!quit);input.check_cleanup().unwrap();assert_eq!(tcgetattr(&observer).unwrap(),original);
            assert!(!seen.text().contains("fixture-key-value") && !seen.text().contains("fixture-token-value"));
            assert_ne!(previous_decision.as_ref(),Some(&decision));previous_decision=Some(decision);
            assert_eq!(fixture.store.saves.load(Ordering::SeqCst),0);assert!(fixture.store.list().await.unwrap().is_empty());
            assert_eq!(std::fs::read(&fixture.config_path).unwrap(),config_before);assert_eq!(std::fs::read(&grants).ok(),grants_before);
            assert_eq!(std::fs::read(&request_path).unwrap(),package_request);assert_eq!(std::fs::read(&fixture.installed.receipt_path).unwrap(),receipt_before);
            assert_eq!(dispatcher.0.load(Ordering::SeqCst),0);
            approvals.retire();installs.retire();owner.retire().unwrap();assert!(selection.check_current().is_err());
            if quit { assert!(!owner.has_reviewed());assert!(receive.await.is_err());previous_handle=Some(handle); }
            else {
                assert_eq!(receive.await.unwrap()["state"],"pending");
                let Some(ReviewedInstallation::Secret(reviewed))=owner.take_reviewed() else {panic!("fresh review must be retained")};
                assert_eq!(reviewed.decision.proposed_action.body.as_ref().unwrap()["grant"], json!({
                    "agent":"rkat-agent","expires_in_ms":60000,
                    "target":{"kind":"mcp","server":"cards","operations":{"kind":"exact","names":["list_cards"]}}}));
                return reviewed;
            }
            // These component owners are gone before the new session starts.
            // This fixture never entered a native setup agent runtime.
            drop(owner);drop(installs);drop(approvals);drop(input);
        }
        unreachable!()
    }

    #[tokio::test]
    async fn initial_secret_finalizer_uses_paired_vault_exact_realm_and_finite_grant() {
        tokio::time::timeout(Duration::from_secs(30),async {
            let root = tempfile::tempdir().unwrap();
            let fixture = Fixture::new(root.path(),false,None).await;
            let reviewed = quit_then_fresh_credential_review(&fixture,root.path()).await;
            let result = finish(reviewed).await;
            let outcome = secret_outcome(&result);
            assert_eq!(fixture.store.saves.load(Ordering::SeqCst),2);
            assert_eq!(outcome.slots.len(),2);
            assert!(outcome.slots.iter().all(|(_,result)| result.is_ok()));
            assert!(outcome.grant.as_ref().is_some_and(Result::is_ok));
            let (saved,generation) = crate::realm::read_own_mcp_config(&fixture.storage).await.unwrap();
            assert_eq!(saved,vec![fixture.config.clone()]);
            assert!(generation > fixture.generation);
            assert!(exact_presence(&fixture.observer().observe(&fixture.config).await.unwrap(),
                &fixture.storage.locator.realm,&fixture.references,meerkat_core::auth::McpServiceSecretPresence::Stored));
            let reopened = meerkat::RealmMcpServiceSecrets::from_realm_source(crate::realm::config_source(&fixture.storage).unwrap(),
                fixture.storage.locator.realm.clone(),Arc::new(FileTokenStore::new(root.path().join("credentials"))));
            assert!(exact_presence(&reopened.observe(&fixture.config).await.unwrap(),&fixture.storage.locator.realm,
                &fixture.references,meerkat_core::auth::McpServiceSecretPresence::Stored));
            let foreign = meerkat::RealmMcpServiceSecrets::from_realm_source(crate::realm::config_source(&fixture.storage).unwrap(),
                meerkat_core::RealmId::parse("other").unwrap(),fixture.store.clone());
            assert!(foreign.observe(&fixture.config).await.is_err());
            let notice = result.private_observation().to_string();
            assert!(!notice.contains("fixture-key-value") && !notice.contains("fixture-token-value"));
            assert_eq!(result.notice()["service_readiness"],"not_observed");
            assert!(result.message().unwrap().contains("fresh session"));
        }).await.expect("initial native realm/vault finalization must settle");
    }

    #[tokio::test]
    async fn secret_finalizer_refuses_changed_documents_realm_and_existing_exact_configuration() {
        tokio::time::timeout(Duration::from_secs(30),async {
            for change in ["document","realm","existing","inherited"] {
                let root = tempfile::tempdir().unwrap();
                let fixture = Fixture::new(root.path(),false,None).await;
                let mut reviewed = fixture.review();
                match change {
                    "document" => std::fs::write(&fixture.installed.receipt_path,b"changed").unwrap(),
                    "realm" => reviewed.storage = crate::RealmOptions {context_root:root.path().to_owned(),
                        state_root:Some(root.path().join("state")),user_config_root:Some(root.path().join("home"))}.resolve("other").unwrap(),
                    "inherited" => {
                        let global=fixture.storage.layout.user_rkat_root().unwrap().join("config.toml");
                        FileConfigStore::new(global,meerkat_models::canonical()).patch(ConfigDelta(json!({"tools":{"mcp_servers":[fixture.config.clone()]}}))).await.unwrap();
                        assert!(crate::realm::read_own_mcp_config(&fixture.storage).await.unwrap().0.is_empty());
                        assert!(initial_snapshot(&fixture.storage,&fixture.config).await.is_err());
                    },
                    _ => { let runtime = crate::realm::own_config_runtime(&fixture.storage).unwrap();
                        McpRealmPersistTarget::new(&runtime).add(fixture.config.clone(),Some(fixture.generation)).await.unwrap(); },
                }
                let before = std::fs::read(&fixture.config_path).unwrap();
                let result = finish(reviewed).await;
                assert!(matches!(result.outcome,FinalizationOutcome::BeforeAdministrationRefused(_)));
                assert_eq!(fixture.store.saves.load(Ordering::SeqCst),0);
                assert_eq!(std::fs::read(&fixture.config_path).unwrap(),before);
            }
        }).await.expect("pre-effect refusal must settle");
    }

    #[tokio::test]
    async fn uncertain_slot_write_stops_pair_and_cannot_be_recovered_from_presence() {
        tokio::time::timeout(Duration::from_secs(30),async {
            let root = tempfile::tempdir().unwrap();
            let fixture = Fixture::new(root.path(),false,Some(1)).await;
            let result = finish(fixture.review()).await;
            let outcome = secret_outcome(&result);
            assert_eq!(fixture.store.saves.load(Ordering::SeqCst),1);
            assert!(matches!(&outcome.slots[0].1,Err(meerkat::McpServiceSecretUpdateError::OutcomeUnknown(_))));
            assert!(outcome.after.is_none() && outcome.grant.is_none());
            let observed = fixture.observer().observe(&fixture.config).await.unwrap();
            assert_eq!(observed.slots["FIXTURE_KEY"],meerkat_core::auth::McpServiceSecretPresence::Stored);
            assert_eq!(observed.slots["FIXTURE_TOKEN"],meerkat_core::auth::McpServiceSecretPresence::NotStored);
            assert_eq!(result.notice()["facts"]["slot_writes"][0]["state"],"uncertain");
            assert!(result.blocks_reentry(&fixture.storage.locator,&fixture.config.name));
            let retry = finish(fixture.review()).await;
            assert!(matches!(retry.outcome,FinalizationOutcome::BeforeAdministrationRefused(_)));
            assert_eq!(fixture.store.saves.load(Ordering::SeqCst),1);
        }).await.expect("uncertain slot result and refusal must settle");
    }

    #[tokio::test]
    async fn cancellation_joins_first_native_slot_and_stops_the_second_and_grant() {
        tokio::time::timeout(Duration::from_secs(30),async {
            let root = tempfile::tempdir().unwrap();
            let fixture = Fixture::new(root.path(),true,None).await;
            let _release_on_failure = ReleaseOnDrop(fixture.store.clone());
            let first = fixture.review();
            let second = fixture.review();
            let (cancel,receive) = oneshot::channel();
            let interrupt = async {receive.await.map_err(io::Error::other)};
            tokio::pin!(interrupt);
            let finalizing = ReviewedInstallation::Secret(first).finalize_outcome_unless_cancelled(interrupt.as_mut());
            tokio::pin!(finalizing);
            // Poll the actual finalizer until its native setter reaches the
            // held vault. Always release before any assertions or teardown.
            let entered = tokio::select! {
                value = finalizing.as_mut() => Err(value),
                permit = fixture.store.entered.acquire() => {permit.unwrap().forget(); Ok(())},
            };
            if entered.is_err() { fixture.store.release.add_permits(1); panic!("native slot did not enter"); }
            let sent = cancel.send(());
            let early = finalizing.as_mut().now_or_never();
            let stayed_owned = early.is_none();
            fixture.store.release.add_permits(1);
            let result = match early { Some(result) => result, None => finalizing.await }.unwrap().unwrap();
            let second_result = finish(second).await;
            assert!(sent.is_ok() && stayed_owned);
            assert!(matches!(second_result.outcome,FinalizationOutcome::BeforeAdministrationRefused(_)));
            assert!(result.stopped_during_finalization());
            let outcome = secret_outcome(&result);
            assert_eq!(outcome.slots.len(),1);
            assert!(outcome.slots[0].1.is_ok() && outcome.grant.is_none());
            assert_eq!(fixture.store.saves.load(Ordering::SeqCst),1);
            let observed = fixture.observer().observe(&fixture.config).await.unwrap();
            assert_eq!(observed.slots["FIXTURE_KEY"],meerkat_core::auth::McpServiceSecretPresence::Stored);
            assert_eq!(observed.slots["FIXTURE_TOKEN"],meerkat_core::auth::McpServiceSecretPresence::NotStored);
            assert!(result.blocks_reentry(&fixture.storage.locator,&fixture.config.name));
        }).await.expect("entered slot cancellation must join");
    }

    #[tokio::test]
    async fn preentry_cancellation_and_stale_create_generation_never_reach_slots() {
        tokio::time::timeout(Duration::from_secs(30),async {
            let root = tempfile::tempdir().unwrap();
            let fixture = Fixture::new(root.path(),false,None).await;
            let before = std::fs::read(&fixture.config_path).unwrap();
            let interrupt = std::future::ready(Ok(()));
            tokio::pin!(interrupt);
            let stopped = ReviewedInstallation::Secret(fixture.review())
                .finalize_outcome_unless_cancelled(interrupt.as_mut()).await.unwrap();
            assert!(stopped.is_none());
            assert_eq!(std::fs::read(&fixture.config_path).unwrap(),before);
            assert_eq!(fixture.store.saves.load(Ordering::SeqCst),0);
            // Both reviewed snapshots are absent. Only one native add using
            // that generation can succeed; neither path reuses a definition.
            require_absent(&fixture.own,&fixture.config).unwrap();
            let left = crate::realm::own_config_runtime(&fixture.storage).unwrap();
            let right = crate::realm::own_config_runtime(&fixture.storage).unwrap();
            let left = McpRealmPersistTarget::new(&left);
            let right = McpRealmPersistTarget::new(&right);
            let (a,b) = tokio::join!(left.add(fixture.config.clone(),Some(fixture.generation)),
                right.add(fixture.config.clone(),Some(fixture.generation)));
            assert_ne!(a.is_ok(),b.is_ok());
            assert_eq!(fixture.store.saves.load(Ordering::SeqCst),0);
            let attempted = finish(fixture.review()).await;
            assert!(matches!(attempted.outcome,FinalizationOutcome::BeforeAdministrationRefused(_)));
            assert_eq!(fixture.store.saves.load(Ordering::SeqCst),0);
        }).await.expect("cancel and native create exclusion must settle");
    }

    #[tokio::test]
    async fn secret_lowering_preserves_checked_adapter_and_rejects_inline_or_predeclared_secrets() {
        tokio::time::timeout(Duration::from_secs(30),async {
            let root = tempfile::tempdir().unwrap();
            let fixture = Fixture::new(root.path(),false,None).await;
            let meerkat_core::mcp_config::McpTransportConfig::Stdio(launch) = &fixture.config.transport else { panic!("stdio") };
            assert!(launch.env.is_empty());
            assert_eq!(launch.secret_env,BTreeSet::from(["FIXTURE_KEY".into(),"FIXTURE_TOKEN".into()]));
            assert_eq!(launch.args[3],"serve");
            assert!(fixture.config.connector.is_none());
            assert_eq!(fixture.config.access.as_ref().unwrap().allowed_operations,BTreeSet::from(["list_cards".into()]));
            let operations = BTreeSet::from(["list_cards".into()]);
            for (field,value) in [("env",json!({"FIXTURE_KEY":"never-inline"})),
                ("secret_env",json!(["EXTRA_SLOT"])),("command",json!("relative-command"))] {
                let mut document = fixture.document.as_ref().clone(); document["mcpServers"]["cards"][field] = value;
                assert!(crate::service_install::secret_launch_configuration(&fixture.installed,&document,&operations,&fixture.references).is_err());
            }
            let mut duplicate = fixture.references.clone(); duplicate[1] = duplicate[0].clone();
            assert!(crate::service_install::secret_launch_configuration(&fixture.installed,&fixture.document,&operations,&duplicate).is_err());
            assert!(crate::service_install::secret_launch_configuration(&fixture.installed,&fixture.document,&BTreeSet::new(),&fixture.references).is_err());
        }).await.expect("retained launch projection must settle");
    }

    #[test]
    fn secret_update_refusal_and_unknown_preserve_partial_configuration_without_payloads() {
        let root = tempfile::tempdir().unwrap();
        let storage = crate::RealmOptions {
            context_root: root.path().to_owned(),
            state_root: Some(root.path().join("state")),
            user_config_root: Some(root.path().join("home")),
        }.resolve("work").unwrap();
        let private = "PRIVATE_SECRET_UPDATE_DIAGNOSTIC";
        for (index, (error, state, kind)) in [
            (meerkat::McpServiceSecretUpdateError::NotAdmitted(
                meerkat_core::AdministrationEntryRefusal::Unavailable(private.into())),
                "refused", "NotAdmitted"),
            (meerkat::McpServiceSecretUpdateError::OutcomeUnknown(private.into()),
                "uncertain", "OutcomeUnknown"),
        ].into_iter().enumerate() {
            // Typed projection inputs only. The completed configuration stays
            // saved; an unconfirmed slot neither proves absence nor issues a grant.
            let outcome = Outcome { persistence: Some(Ok(7)),
                slots: vec![("FIXTURE_KEY".into(), Err(error))], ..Default::default() };
            let observed = FinalizedInstallation::new(FinalizationTarget {
                realm: storage.locator.clone(), server: "cards".into(), agent: "fixture".into(),
                job_id: "reviewed-job".into(), session: SessionId::new(),
            }, FinalizationOutcome::Secret(outcome));
            let notice = observed.notice();
            assert_eq!(notice["facts"]["persistence"], json!({"state":"saved","generation":7}));
            assert_eq!(notice["facts"]["slot_writes"][0],
                json!({"env":"FIXTURE_KEY","state":state,"error_kind":kind}));
            assert!(notice["facts"]["initial_presence"].is_null());
            assert!(notice["facts"]["final_presence"].is_null());
            assert_eq!(notice["facts"]["grant"]["state"], "not_requested");
            assert_eq!(notice["facts"]["vendor_account"], "not_verified");
            assert_eq!(notice["automatic_reentry_blocked"], true);
            assert_eq!(notice["service_readiness"], "not_observed");
            observed.retain(root.path(), index + 1).unwrap();
            let retained = std::fs::read_to_string(root.path()
                .join(format!("standalone-finalization-{}.txt", index + 1))).unwrap();
            let mut output = Vec::new();
            observed.display_for_continuation(&mut output).unwrap();
            assert!(!notice.to_string().contains(private));
            assert!(!retained.contains(private));
            assert!(!String::from_utf8(output).unwrap().contains(private));
        }
    }

    #[test]
    fn exact_presence_rejects_foreign_missing_extra_and_unobserved_slots() {
        let realm = meerkat_core::RealmId::parse("work").unwrap();
        let references = vec![ServiceCredentialReference {field_path:vec!["config".into(),"key".into()],environment_variable:"KEY".into()}].into_iter().map(ServiceCredentialSlot::LibraryField).collect::<Vec<_>>();
        let exact = meerkat::McpServiceSecretObservation {owner:realm.clone(),
            slots:BTreeMap::from([("KEY".into(),meerkat_core::auth::McpServiceSecretPresence::NotStored)])};
        assert!(exact_presence(&exact,&realm,&references,meerkat_core::auth::McpServiceSecretPresence::NotStored));
        let mut wrong = exact.clone(); wrong.owner = meerkat_core::RealmId::parse("other").unwrap();
        assert!(!exact_presence(&wrong,&realm,&references,meerkat_core::auth::McpServiceSecretPresence::NotStored));
        let mut wrong = exact.clone(); wrong.slots.clear();
        assert!(!exact_presence(&wrong,&realm,&references,meerkat_core::auth::McpServiceSecretPresence::NotStored));
        let mut wrong = exact.clone(); wrong.slots.insert("OTHER".into(),meerkat_core::auth::McpServiceSecretPresence::NotStored);
        assert!(!exact_presence(&wrong,&realm,&references,meerkat_core::auth::McpServiceSecretPresence::NotStored));
        for presence in [meerkat_core::auth::McpServiceSecretPresence::Stored,meerkat_core::auth::McpServiceSecretPresence::NotObserved] {
            let mut wrong = exact.clone(); wrong.slots.insert("KEY".into(),presence);
            assert!(!exact_presence(&wrong,&realm,&references,meerkat_core::auth::McpServiceSecretPresence::NotStored));
        }
    }

    #[tokio::test]
    async fn secret_operation_is_absent_from_nonterminal_private_composition() {
        let (setup,_presenter) = crate::service_preparation::LazySetup::private_services();
        assert!(!setup.owner().allows_terminal_connectors());
        assert!(!setup.allowed_tools().iter().any(|name| TOOLS.contains(&name.as_str())));
        assert!(!setup.declared_tools().iter().any(|name| TOOLS.contains(&name.as_str())));
        assert_eq!(tool().name,SECRET_TOOL);
        assert!(TOOLS.contains(&SECRET_TOOL));
    }
}
