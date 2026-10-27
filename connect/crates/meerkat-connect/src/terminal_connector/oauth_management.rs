//! Standalone OAuth lifecycle presentation over the existing native owner.
//! No configuration, grants or credential values are written by this adapter.
use super::*;
use crate::connector_auth::{LoginSelection, existing::ExistingConnector};
use meerkat::ConnectorClientAuthSource;
use std::collections::BTreeSet;

const PAGE_SIZE: usize = 16;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Inventory { after: Option<String> }

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Change {
    Reconnect { service: String, scopes: Option<Vec<String>>, client_auth_profile: Option<String> },
    Disconnect { service: String },
}
impl Change {
    fn service(&self) -> &str {
        match self { Self::Reconnect { service, .. } | Self::Disconnect { service } => service }
    }
}
enum Requested { Inventory(Inventory), Change(Change) }
pub struct Request { session: SessionId, action: Requested, response: oneshot::Sender<Value> }

fn refused() -> ToolError {
    ToolError::execution_failed("Select an exact existing OAuth service from the standalone account inventory. Reconnect omits both scopes and client_auth_profile to recover native defaults, or supplies both as explicitly new choices (null profile selects the public client). Account identities, credentials, callbacks, install jobs, configurations and grants are not accepted.")
}

fn request_refused(reason: &'static str, response: oneshot::Sender<Value>, output: &mut impl Write)
    -> Result<bool, ConnectError> {
    let message = "This account request could not proceed. No sign-in or logout was started by this request. Read the current account inventory and review a new action; do not repeat an earlier uncertain action automatically.";
    writeln!(output, "{message}").map_err(|error| ConnectError::native("Display account request refusal", error))?;
    let _ = response.send(json!({"state":"unavailable","phase":"account_management","reason":reason,
        "instruction":message,"configuration_written":false,"grants_written":false}));
    Ok(true)
}

pub(super) fn tools() -> [Arc<ToolDef>; 2] {
    [Arc::new(ToolDef::new(OAUTH_STATUS_TOOL,
        "Read a page of existing OAuth service definitions, current native account status and available reconnect defaults in this standalone realm. Supply {} or the returned next name as after. Reconnect defaults come only from native stored requested scopes and an exactly matching current client profile. Operation requirements and granted scopes are separate observations. None proves agent access or authorizes changes. Inherited credential owners are identified without being changed. No token records, credentials, commands or full configuration are returned.",
        json!({"type":"object","additionalProperties":false,"properties":{"after":{"type":"string","minLength":1,"maxLength":256}}}))),
    Arc::new(ToolDef::new(OAUTH_MANAGE_TOOL,
        "Ask the protected terminal to reconnect the same account or disconnect an existing OAuth credential slot. Select the service from toolkit_service_account_status. reconnect uses the exact account/descriptor from the current native own definition. Omit scopes and client_auth_profile to recover native defaults. Only when defaults are missing or unusable, or the user explicitly wants new choices, supply both newly chosen scopes and a matching client profile ID (null selects the public client). The terminal reviews and rechecks these inputs. disconnect clears the selected slot and can affect other services sharing it. Native logout drains current native flows but does not join lost host callback tasks, prevent future login, revoke independent agent access, change account, reinstall, change configuration or guarantee provider-side revocation. Account replacement and lost-attempt recovery are separate steps not exposed here. Never retry an uncertain effect automatically.",
        // The model-facing root is one object; decode still enforces each action's fields.
        json!({"type":"object","additionalProperties":false,"required":["action","service"],
            "properties":{"action":{"type":"string","enum":["reconnect","disconnect"]},
                "service":{"type":"string","minLength":1,"maxLength":256},
                "scopes":{"type":"array","maxItems":128,"uniqueItems":true,"items":{"type":"string","minLength":1,"maxLength":2048}},
                "client_auth_profile":{"type":["string","null"],"minLength":1,"maxLength":256}}})))]
}

fn name(value: &str, limit: usize) -> bool {
    !value.is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}

fn decode(tool: &str, raw: &str) -> Result<Requested, ToolError> {
    if raw.len() > 32 * 1024 { return Err(refused()); }
    serde_json::from_str::<crate::forge::UniqueJson>(raw).map_err(|_| refused())?;
    let raw: Value = serde_json::from_str(raw).map_err(|_| refused())?;
    if tool == OAUTH_STATUS_TOOL {
        let query: Inventory = serde_json::from_value(raw).map_err(|_| refused())?;
        if query.after.as_ref().is_some_and(|value| !name(value, 256)) { return Err(refused()); }
        return Ok(Requested::Inventory(query));
    }
    if tool != OAUTH_MANAGE_TOOL { return Err(refused()); }
    if raw.get("action").and_then(Value::as_str) == Some("reconnect") {
        if raw.get("scopes").is_some() != raw.get("client_auth_profile").is_some()
            || raw.get("scopes").is_some_and(Value::is_null) { return Err(refused()); }
    }
    let change: Change = serde_json::from_value(raw).map_err(|_| refused())?;
    if !name(change.service(), 256) { return Err(refused()); }
    if let Change::Reconnect { scopes, client_auth_profile, .. } = &change {
        if scopes.as_ref().is_some_and(|scopes| scopes.len() > 128
            || scopes.iter().any(|scope| !name(scope, 2048))
            || scopes.iter().collect::<BTreeSet<_>>().len() != scopes.len())
            || client_auth_profile.as_ref().is_some_and(|profile| !name(profile, 256)) {
            return Err(refused());
        }
    }
    Ok(Requested::Change(change))
}

pub(super) async fn dispatch(owner: &RequestTool, call: ToolCallView<'_>, context: &ToolDispatchContext)
    -> Result<ToolDispatchOutcome, ToolError> {
    let action = decode(call.name, call.args.get())?;
    let session = context.origin_session_id().ok_or_else(refused)?.clone();
    let (response, receive) = oneshot::channel();
    owner.requests.try_send(ConnectorRequest::AccountManagement(Request {session, action, response}))
        .map_err(|_| refused())?;
    let value = receive.await.map_err(|_| ToolError::execution_failed(
        "Account management ended without a returned result. An entered native effect may have completed; do not repeat it automatically."))?;
    Ok(ToolResult::new(call.id.into(), value.to_string(), false).into())
}

fn phase(status: &meerkat::ConnectorAuthStatus) -> &'static str {
    match status.phase {
        meerkat::ConnectorAuthPhase::Authorized => "authorized",
        meerkat::ConnectorAuthPhase::AuthorizationRequired => "authorization_required",
        meerkat::ConnectorAuthPhase::ReauthRequired => "reauth_required",
        _ => "other_native_phase",
    }
}

fn observation(status: Result<meerkat::ConnectorAuthStatus, meerkat::HostAuthError>,
    slot: &meerkat_core::CredentialAccountRef) -> Value {
    match status {
        Ok(status) if &status.slot == slot => json!({"phase":phase(&status),
            "verified_account":status.verified_account.as_ref().map(|account| json!({
                "issuer":account.issuer,"strategy":account.strategy_id,"subject":account.subject})),
            "observed_scopes":status.scopes,
            "notice":"Current account observation only; not agent access, original requested scopes or proof about an earlier uncertain attempt."}),
        _ => json!({"phase":"unavailable"}),
    }
}

#[derive(Clone, PartialEq, Eq)]
struct ReconnectInputs {
    scopes: BTreeSet<String>,
    profile_id: Option<String>,
    profile_binding: Option<String>,
}

fn matching_profile_id(binding: &meerkat_core::mcp_config::McpConnectorBinding, admitted: &str,
    profiles: &[meerkat::ConnectorClientAuthProfile]) -> Result<String, &'static str> {
    let matching = profiles.iter().filter(|profile| profile.issuer == binding.issuer
        && profile.client == binding.client && profile.binding() == admitted).collect::<Vec<_>>();
    if matching.len() != 1 { return Err("historical_client_profile_changed_or_missing"); }
    Ok(matching[0].id.clone())
}

// This is a trusted, private metadata projection through the setup factory's
// original store. A Status refusal is not missing data. Token records and
// unrelated metadata never leave this function or enter a model response.
async fn native_defaults(owner: &TerminalConnectors, selected: &ExistingConnector)
    -> Result<ReconnectInputs, &'static str> {
    use meerkat_auth_core::{auth_store::TokenKey, connector_oauth::ConnectorCredentialMetadata};
    let persistence = owner.factory.resolution_provider_auth_persistence()
        .map_err(|_| "original_persistence_unavailable")?.ok_or("original_persistence_unavailable")?;
    let key = TokenKey::from_credential_identity(&meerkat_core::AuthCredentialIdentity::Account(selected.slot().clone()));
    let metadata = {
        let tokens = persistence.token_store().load_for(&key, meerkat_core::auth::TokenStoreAccess::Status).await
            .map_err(|_| "reconnect_metadata_not_observed")?.ok_or("reconnect_metadata_missing")?;
        ConnectorCredentialMetadata::from_tokens(&tokens).ok_or("reconnect_metadata_missing")?
    };
    let binding = selected.config().connector.as_ref().expect("validated connector");
    if metadata.issuer != binding.issuer || metadata.client != binding.client
        || metadata.resource != binding.resource || metadata.strategy_id != binding.strategy_id {
        return Err("reconnect_metadata_descriptor_changed");
    }
    let profile_id = if let Some(admitted) = metadata.client_auth_profile.as_deref() {
        let profiles = owner.profiles.current_profiles().await.map_err(|_| "client_profiles_not_observed")?;
        Some(matching_profile_id(binding, admitted, &profiles)?)
    } else { None };
    selected.check_current().await?;
    Ok(ReconnectInputs { scopes:metadata.requested_scopes, profile_id, profile_binding:metadata.client_auth_profile })
}

fn defaults_notice(defaults: &Result<ReconnectInputs, &'static str>) -> Value {
    match defaults {
        Ok(defaults) => json!({"state":"available","requested_scopes":defaults.scopes,
            "client_auth_profile":defaults.profile_id,"source":"native_credential_metadata_and_exact_current_profile"}),
        Err(reason) => json!({"state":"not_available","reason":reason,
            "instruction":"No historical inputs were inferred. Explicitly review new scopes and client profile if reconnecting."}),
    }
}

fn logout_error(error: &meerkat::HostAuthError) -> Value {
    // The public helper uses Display for some reasons. Only its documented
    // fixed-text branch can reach either presentation; no raw detail escapes.
    let message = if error.withholds_detail() { error.public_message() }
        else { "Native logout did not return success. Its effects remain unconfirmed.".into() };
    json!({"message":message,"native_detail_withheld":error.withholds_detail()})
}

async fn inventory(owner: &TerminalConnectors, query: &Inventory) -> Result<Value, ConnectError> {
    let (mut servers, generation) = crate::realm::read_own_mcp_config(&owner.storage).await?;
    servers.sort_by(|left, right| left.name.cmp(&right.name));
    if servers.windows(2).any(|pair| pair[0].name == pair[1].name) {
        return Err(unavailable("The selected realm has ambiguous service definitions."));
    }
    let page = servers.iter().filter(|server| server.connector.is_some()
        && query.after.as_ref().is_none_or(|after| server.name > *after)).take(PAGE_SIZE + 1).collect::<Vec<_>>();
    let profiles = owner.profiles.current_profiles().await;
    let mut rows = Vec::new();
    for server in page.iter().take(PAGE_SIZE) {
        let binding = server.connector.as_ref().expect("connector page");
        let selected = ExistingConnector::from_observation(&owner.storage, &servers, generation, &server.name);
        let (status, management, defaults) = match selected {
            Ok(selected) => {
                let status = observation(owner.auth.facade.connector_status(selected.slot()).await, selected.slot());
                let defaults = defaults_notice(&native_defaults(owner, &selected).await);
                selected.check_current().await.map_err(unavailable)?;
                (status, json!({"available":true}), defaults)
            },
            Err(reason) => (json!({"phase":"not_observed"}), json!({"available":false,"reason":reason}),
                json!({"state":"not_observed","reason":reason})),
        };
        let choices = profiles.as_ref().ok().map(|profiles| profiles.iter()
            .filter(|profile| profile.issuer == binding.issuer && profile.client == binding.client)
            .map(|profile| profile.id.clone()).collect::<Vec<_>>());
        rows.push(json!({"service":server.name,"credential_realm":binding.realm_id,"slot":binding.slot_id,
            "issuer":binding.issuer,"client":binding.client,"resource":binding.resource,
            "strategy":binding.strategy_id,"configured_account":binding.account,
            "operation_requirements":binding.operations,"matching_client_profiles":choices,
            "account_status":status,"management":management,"reconnect_defaults":defaults}));
    }
    // Rows describe one own-document observation, not a synthesized inventory.
    let (mut current, current_generation) = crate::realm::read_own_mcp_config(&owner.storage).await?;
    current.sort_by(|left, right| left.name.cmp(&right.name));
    if current != servers || current_generation != generation {
        return Err(unavailable("The account inventory changed while being read. Read it again."));
    }
    Ok(json!({"realm":owner.storage.locator.realm.as_str(),"services":rows,
        "next":(page.len() > PAGE_SIZE).then(|| page[PAGE_SIZE - 1].name.clone()),
        "configuration_generation":generation,"scope":"own_definitions_and_point_account_observations",
        "notice":"Reconnect inputs are reviewed before use; a refused metadata read is not absence. This inventory is not account-slot enumeration, viewer admission, grant inspection or recovery of lost attempts."}))
}

pub(super) async fn present<F: Future<Output = io::Result<()>> + Send, W: Write>(
    owner: &mut TerminalConnectors, mut request: Request, approvals: &LocalApprovals,
    presenter: &mut ApprovalPresenter, input: &mut TerminalInput, output: &mut W, interrupt: F,
) -> Result<bool, ConnectError> {
    if request.response.is_closed() { return Ok(true); }
    approvals.check_session(&request.session)?;
    StockPrincipals::for_local_operator(&owner.storage.locator.realm)
        .map_err(|_| unavailable("The selected realm operator is unavailable."))?;
    tokio::pin!(interrupt);
    match request.action {
        Requested::Inventory(query) => {
            let result = tokio::select! { biased;
                signal = interrupt.as_mut() => { signal.map_err(|e| ConnectError::native("Listen for account inventory cancellation", e))?; return Ok(false); },
                _ = request.response.closed() => return Ok(true),
                result = inventory(owner, &query) => match result {
                    Ok(result) => result,
                    Err(_) => return request_refused("account_inventory_unavailable", request.response, output),
                },
            };
            approvals.check_session(&request.session)?;
            let _ = request.response.send(result);
            Ok(true)
        },
        Requested::Change(change) => manage(owner, &request.session, change, request.response,
            approvals, presenter, input, output, interrupt.as_mut()).await,
    }
}

async fn manage<F: Future<Output = io::Result<()>> + Send, W: Write>(
    owner: &mut TerminalConnectors, session: &SessionId, change: Change, mut response: oneshot::Sender<Value>,
    approvals: &LocalApprovals, presenter: &mut ApprovalPresenter, input: &mut TerminalInput,
    output: &mut W, mut interrupt: Pin<&mut F>,
) -> Result<bool, ConnectError> {
    if owner.reviewed.is_some() { return request_refused("reviewed_finalization_waiting_for_setup_close", response, output); }
    let selected = match ExistingConnector::select(&owner.storage, change.service()).await {
        Ok(selected) => selected, Err(reason) => return request_refused(reason, response, output),
    };
    let before = match owner.auth.facade.connector_status(selected.slot()).await {
        Ok(status) if &status.slot == selected.slot() => status,
        _ => return request_refused("account_observation_unavailable", response, output),
    };
    if let Err(reason) = selected.check_current().await { return request_refused(reason, response, output); }
    approvals.check_session(session)?;
    if response.is_closed() { return Ok(true); }
    match change {
        Change::Reconnect { scopes, client_auth_profile, .. } => {
            if let Err(reason) = selected.check_account(&before) { return request_refused(reason, response, output); }
            let recovered = if scopes.is_none() {
                match native_defaults(owner, &selected).await {
                    Ok(defaults) => Some(defaults),
                    Err(reason) => return request_refused(reason, response, output),
                }
            } else { None };
            let target = match &recovered {
                Some(defaults) => selected.target(defaults.scopes.clone(), defaults.profile_id.clone()),
                None => selected.target(scopes.expect("explicit choices").into_iter().collect(), client_auth_profile),
            };
            let profile = match profiles::selected(&owner.profiles, &target).await {
                Ok(profile) => profile,
                Err(error) => return error.respond(response, output),
            };
            if target.client_auth_profile.is_some() && profile.is_none() {
                return request_refused("existing_client_profile_required", response, output);
            }
            let body = json!({"phase":"same_account_reconnect","realm":owner.storage.locator.realm.as_str(),
                "service":selected.config().name,"slot":selected.slot().account.as_str(),
                "account":target.account,"issuer":target.issuer,"client":target.client,"resource":target.resource,
                "requested_scopes":target.scopes,"client_profile":profiles::display(profile.as_ref()),
                "input_source":if recovered.is_some() { "native_credential_metadata_and_exact_current_profile" } else { "explicit_new_choices" },
                "expected_configuration_generation":selected.generation(),
                "notice":"Sign in again as this exact configured account using the scopes/profile and source shown. This changes only native authentication; it does not change service configuration, issue or renew access, switch account, or prove service readiness. Existing revoked or expired grants remain revoked or expired."});
            let decision = match review(approvals, presenter, input, output,
                review_request(session, "Reconnect this same account?", body)?, &mut response, interrupt.as_mut()).await? {
                ReviewOutcome::Decided(decision) => decision,
                ReviewOutcome::CallerGone => return Ok(true),
                ReviewOutcome::Stopped => return Ok(false),
            };
            if decision.status != ApprovalStatus::Approved {
                let _ = response.send(json!({"state":"denied","phase":"same_account_reconnect","login_started":false}));
                return Ok(true);
            }
            if let Err(reason) = selected.check_current().await { return request_refused(reason, response, output); }
            if let Some(recovered) = recovered {
                if native_defaults(owner, &selected).await.as_ref() != Ok(&recovered) {
                    return request_refused("reconnect_defaults_changed_or_unavailable", response, output);
                }
            }
            if let Err(error) = profiles::recheck(&owner.profiles, &target, profile.as_ref()).await { return error.respond(response, output); }
            let current = match owner.auth.facade.connector_status(selected.slot()).await {
                Ok(status) => status, Err(_) => return request_refused("account_observation_unavailable", response, output),
            };
            if let Err(reason) = selected.check_account(&current) { return request_refused(reason, response, output); }
            if let Err(reason) = selected.check_current().await { return request_refused(reason, response, output); }
            approvals.check_session(session)?;
            if response.is_closed() { return Ok(true); }
            let result = login::run(&mut owner.control, target.clone(), LoginSelection::Existing(Arc::new(selected.clone())),
                &mut response, input, output, interrupt.as_mut()).await?;
            if let Some(reply) = result.stop_reply() {
                writeln!(output, "{}", result.stop_message())
                    .map_err(|e| ConnectError::native("Display reconnect settlement", e))?;
                let _ = response.send(reply);
                return Ok(matches!(result, login::End::Stopped { reason:login::Stop::Cancelled | login::Stop::CallerGone, .. }));
            }
            let completed = match result {
                login::End::Completed(completed) => completed,
                login::End::Refused(reason) => {
                    let _ = response.send(json!({"state":"unavailable","phase":"same_account_reconnect","reason":reason,
                        "configuration_written":false,"grants_written":false}));
                    return Ok(true);
                },
                login::End::Stopped { .. } => unreachable!("stop settled above"),
            };
            // Native completion is historical. Readback cannot renew this
            // review or erase a completed login when another owner changed.
            let definition_current = selected.check_current().await.is_ok();
            let profile_current = profiles::recheck(&owner.profiles, &target, profile.as_ref()).await.is_ok();
            let status = observation(owner.auth.facade.connector_status(selected.slot()).await, selected.slot());
            let result = json!({"state":"login_completed","phase":"same_account_reconnect",
                "service":selected.config().name,"verified_account":completed.native.verified_account.subject,
                "definition_current":definition_current,"client_profile_current":profile_current,"account_status":status,
                "configuration_written":false,"grants_written":false,"readiness_verified":false});
            writeln!(output, "Native sign-in completed for {}. Configuration and grants were not written. Current account status: {}. Definition current: {}; client profile current: {}. Service access is checked when used.",
                review_text(&completed.native.verified_account.subject), review_text(result["account_status"]["phase"].as_str().unwrap_or("unavailable")),
                definition_current, profile_current).map_err(|e| ConnectError::native("Display completed reconnect", e))?;
            let _ = response.send(result);
            Ok(true)
        },
        Change::Disconnect { .. } => disconnect(owner, session, selected, before, response,
            approvals, presenter, input, output, interrupt).await,
    }
}

async fn disconnect<F: Future<Output = io::Result<()>> + Send, W: Write>(
    owner: &mut TerminalConnectors, session: &SessionId, selected: ExistingConnector,
    before: meerkat::ConnectorAuthStatus, mut response: oneshot::Sender<Value>,
    approvals: &LocalApprovals, presenter: &mut ApprovalPresenter, input: &mut TerminalInput,
    output: &mut W, mut interrupt: Pin<&mut F>,
) -> Result<bool, ConnectError> {
    let (definitions, _) = match crate::realm::read_own_mcp_config(&owner.storage).await {
        Ok(observed) => observed, Err(_) => return request_refused("configuration_unavailable", response, output),
    };
    let known_bindings = definitions.iter().filter(|server| server.connector.as_ref().is_some_and(|binding|
        binding.realm_id == selected.slot().realm.as_str() && binding.slot_id == selected.slot().account.as_str()))
        .map(|server| server.name.clone()).collect::<Vec<_>>();
    let body = json!({"phase":"disconnect_credential_slot","realm":selected.slot().realm.as_str(),
        "slot":selected.slot().account.as_str(),"current_account":observation(Ok(before), selected.slot()),
        "known_services_in_this_realm":known_bindings,
        "notice":"Disconnect this native credential slot, including whichever account it holds when native logout runs. This is a slot-wide action, not an account-revision conditional update. Other services using the slot can lose access; the listed services cover this realm only. This CLI joins only its exact retained attempt. Native logout also drains current native flow membership and payloads before credential release; this does not join a lost host callback task or prevent future login. Independent agent grants, service definitions and frontend roles are not revoked or removed. Provider-side credential revocation is not promised. A concurrent new login can authorize the slot again."});
    let decision = match review(approvals, presenter, input, output,
        review_request(session, "Disconnect this credential slot?", body)?, &mut response, interrupt.as_mut()).await? {
        ReviewOutcome::Decided(decision) => decision,
        ReviewOutcome::CallerGone => return Ok(true),
        ReviewOutcome::Stopped => return Ok(false),
    };
    if decision.status != ApprovalStatus::Approved {
        let _ = response.send(json!({"state":"denied","phase":"disconnect_credential_slot","logout_started":false}));
        return Ok(true);
    }
    if let Err(reason) = selected.check_current().await { return request_refused(reason, response, output); }
    approvals.check_session(session)?;
    if response.is_closed() { return Ok(true); }
    if let Some(slot) = owner.control.pending_slot() {
        if slot != selected.slot() { return request_refused("another_live_attempt_pending", response, output); }
        owner.control.retire().map_err(unavailable)?;
    }
    let mut caller_gone = false;
    let mut stop = None;
    let result = {
        let stopping = async {
            tokio::select! { biased;
                signal = interrupt.as_mut() => signal,
                _ = response.closed() => { caller_gone = true; Ok(()) },
                end = input.wait_for_end() => end,
            }
        };
        tokio::pin!(stopping);
        secrets::joined_effect(stopping.as_mut(), &mut stop,
            || owner.auth.facade.connector_logout(selected.slot())).await
    };
    input.check_cleanup().map_err(|e| ConnectError::native("Restore disconnect input", e))?;
    let state = match &result { None => "not_started", Some(Ok(())) => "logout_completed", Some(Err(_)) => "logout_unconfirmed" };
    let status = observation(owner.auth.facade.connector_status(selected.slot()).await, selected.slot());
    let facts = json!({"state":state,"phase":"disconnect_credential_slot","slot":selected.slot().account.as_str(),
        "account_status":status,"configuration_written":false,"grants_written":false,
        "error":result.as_ref().and_then(|result| result.as_ref().err()).map(logout_error),
        "notice":"Logout outcome and current slot status are separate facts. Current status cannot settle an uncertain earlier effect. Native logout drains current native flow membership and payloads; lost host callback tasks are not joined and future logins remain possible. Independent grants were not revoked. Do not retry automatically."});
    writeln!(output, "Credential slot {}: {}. Current account status: {}. Independent grants and service configuration were not changed by this action.",
        review_text(selected.slot().account.as_str()), state,
        review_text(facts["account_status"]["phase"].as_str().unwrap_or("unavailable")))
        .map_err(|e| ConnectError::native("Display native logout outcome", e))?;
    let _ = response.send(facts);
    match stop {
        Some(secrets::Stop::ListenerFailed(error)) => Err(ConnectError::native("Listen for disconnect cancellation", error)),
        Some(secrets::Stop::Cancelled) => Ok(caller_gone),
        None => Ok(true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn management_proposals_cannot_supply_identity_credentials_or_installation_authority() {
        let base = json!({"action":"reconnect","service":"mail","scopes":["openid"],"client_auth_profile":null});
        assert!(decode(OAUTH_MANAGE_TOOL, &base.to_string()).is_ok());
        assert!(decode(OAUTH_MANAGE_TOOL, r#"{"action":"reconnect","service":"mail"}"#).is_ok());
        assert!(decode(OAUTH_MANAGE_TOOL, r#"{"action":"disconnect","service":"mail"}"#).is_ok());
        for extra in [json!({"scopes":["openid"]}), json!({"client_auth_profile":null}),
            json!({"scopes":["openid"],"client_auth_profile":null})] {
            let mut disconnect = json!({"action":"disconnect","service":"mail"});
            disconnect.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            assert!(decode(OAUTH_MANAGE_TOOL, &disconnect.to_string()).is_err());
        }
        for field in ["account","slot","realm","issuer","client","resource","strategy","job_id","configuration","grant","code","state","secret"] {
            let mut extra = base.clone(); extra[field] = json!("not-accepted");
            assert!(decode(OAUTH_MANAGE_TOOL, &extra.to_string()).is_err(), "{field}");
        }
        let mut missing = base.clone(); missing.as_object_mut().unwrap().remove("client_auth_profile");
        assert!(decode(OAUTH_MANAGE_TOOL, &missing.to_string()).is_err());
        for scopes in [json!(["openid","openid"]), Value::Null, json!([""]), json!(["read\nwrite"])] {
            let mut changed = base.clone(); changed["scopes"] = scopes;
            assert!(decode(OAUTH_MANAGE_TOOL, &changed.to_string()).is_err());
        }
        assert!(decode(OAUTH_MANAGE_TOOL, r#"{"action":"disconnect","service":"mail","service":"other"}"#).is_err());
        assert!(decode(OAUTH_STATUS_TOOL, r#"{"after":"mail","secret":"not-accepted"}"#).is_err());
    }

    #[test]
    fn logout_error_keeps_private_detail_and_effect_certainty_separate() {
        for error in [meerkat::HostAuthError::ConnectorTarget("private-error-canary".into()),
            meerkat::HostAuthError::Connector(meerkat::ConnectorLoginError::AuthLifecycle("private-error-canary".into()))] {
            let result = logout_error(&error).to_string();
            assert!(!result.contains("private-error-canary"));
            assert!(!result.contains("not_changed"));
        }
    }
}

#[cfg(test)]
#[path = "oauth_management/controls.rs"]
mod controls;
