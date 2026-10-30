//! Fresh access actions for an existing own service definition. The stock
//! owners remain authoritative; configured ceilings are not selected grants.

use super::*;
use meerkat::governed_realm::stock::{
    StockGovernanceError,
    issuance::{
        DEFAULT_STOCK_AGENT, ReviewedMcpInstallation, StockGrantOutcome, StockGrantSlot,
        StockRevokeOutcome, grant_stock_agent_for_reviewed_installation, revoke_stock_agent,
    },
};
use serde::Serialize;
use std::collections::BTreeSet;

const PAGE_SIZE: usize = 16;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Inventory {
    after: Option<String>,
    agent: Option<String>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Change {
    Grant {
        service: String,
        agent: String,
        operations: Vec<String>,
        expires_in_ms: Option<u64>,
    },
    Revoke {
        service: String,
        agent: String,
    },
}
impl Change {
    fn service(&self) -> &str {
        match self {
            Self::Grant { service, .. } | Self::Revoke { service, .. } => service,
        }
    }
    fn agent(&self) -> &str {
        match self {
            Self::Grant { agent, .. } | Self::Revoke { agent, .. } => agent,
        }
    }
}

pub struct Request {
    session: SessionId,
    action: Requested,
    response: oneshot::Sender<Value>,
}
enum Requested {
    Inventory(Inventory),
    Change(Change),
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 256 && !name.chars().any(char::is_control)
}
fn refused() -> ToolError {
    ToolError::execution_failed(
        "Select an exact configured service, native agent target and access action. A grant requires explicit declared raw operations and lifetime. No installation job, credentials, config or existing grant reference is accepted.",
    )
}
fn request_refused(
    reason: &'static str,
    response: oneshot::Sender<Value>,
    output: &mut impl Write,
) -> Result<bool, ConnectError> {
    let message = "This access request could not proceed; it started no grant change. Check current configured services and choose a new action. An earlier uncertain action remains unresolved.";
    let _ = response.send(json!({"state":"unavailable","phase":"service_access_management","reason":reason,"instruction":message}));
    writeln!(output, "{message}")
        .map_err(|error| ConnectError::native("Display access request refusal", error))?;
    Ok(true)
}

pub(super) fn tools() -> [Arc<ToolDef>; 2] {
    [
        Arc::new(ToolDef::new(
            ACCESS_INVENTORY_TOOL,
            "List one page of the selected standalone realm's own configured service names and declared raw-operation ceilings. Use this when the person wants to finish missing access, grant access or stop one agent using a service; the person need not know internal names. Returns the default stock agent and validates an optional explicit alternate native target. This is neither an agent catalog nor selected-grant inspection: it cannot report current permissions, grant status or expiry. It reads no credentials or internal grant files. Pass next as after for another page.",
            json!({"type":"object","additionalProperties":false,"properties":{
            "after":{"type":"string","minLength":1,"maxLength":256},
            "agent":{"type":"string","minLength":1,"maxLength":256}}}),
        )),
        Arc::new(ToolDef::new(
            ACCESS_MANAGE_TOOL,
            "Propose fresh access management for an existing service returned by toolkit_service_access_inventory. grant issues and selects NEW access for this exact native agent, an explicit nonempty subset of declared raw operations and an explicit lifetime (null means until revoked). It can finish access after interrupted installation without reinstalling or changing credentials. revoke targets whichever grant is selected for this agent/service WHEN EXECUTED, not a historical grant reference. Protected terminal review precedes native administration after setup closes. No automatic retry: earlier uncertain actions remain unresolved. Native revocation, unselection and superseded cleanup are separate outcomes. Service configuration, shared credentials and frontend roles are unchanged. Later-opened realms use new selections; currently open owners can retain existing selections.",
            // The model-facing root is one object; parse_change still enforces each action's fields.
            json!({"type":"object","additionalProperties":false,"required":["action","service","agent"],
            "properties":{"action":{"type":"string","enum":["grant","revoke"]},
                "service":{"type":"string","minLength":1,"maxLength":256},
                "agent":{"type":"string","minLength":1,"maxLength":256},
                "operations":{"type":"array","minItems":1,"maxItems":256,"uniqueItems":true,"items":{"type":"string","minLength":1,"maxLength":256}},
                "expires_in_ms":{"type":["integer","null"],"minimum":1}}}),
        )),
    ]
}

fn parse_change(raw: Value) -> Result<Change, ToolError> {
    if raw.get("action").and_then(Value::as_str) == Some("grant")
        && raw.get("expires_in_ms").is_none()
    {
        return Err(refused());
    }
    let change: Change = serde_json::from_value(raw).map_err(|_| refused())?;
    if !valid_name(change.service()) || !valid_name(change.agent()) {
        return Err(refused());
    }
    if let Change::Grant {
        operations,
        expires_in_ms,
        ..
    } = &change
        && (operations.is_empty()
            || operations.len() > 256
            || *expires_in_ms == Some(0)
            || operations.iter().any(|name| !valid_name(name))
            || operations.iter().collect::<BTreeSet<_>>().len() != operations.len())
    {
        return Err(refused());
    }
    Ok(change)
}

pub(super) async fn dispatch(
    owner: &RequestTool,
    call: ToolCallView<'_>,
    context: &ToolDispatchContext,
) -> Result<ToolDispatchOutcome, ToolError> {
    if call.args.get().len() > 32 * 1024 {
        return Err(refused());
    }
    serde_json::from_str::<crate::forge::UniqueJson>(call.args.get()).map_err(|_| refused())?;
    let session = context.origin_session_id().ok_or_else(refused)?.clone();
    let action = match call.name {
        ACCESS_INVENTORY_TOOL => {
            let query: Inventory = call.parse_args().map_err(|_| refused())?;
            if query.after.as_ref().is_some_and(|name| !valid_name(name))
                || query.agent.as_ref().is_some_and(|name| !valid_name(name))
            {
                return Err(refused());
            }
            Requested::Inventory(query)
        }
        ACCESS_MANAGE_TOOL => {
            Requested::Change(parse_change(call.parse_args().map_err(|_| refused())?)?)
        }
        _ => return Err(refused()),
    };
    let (response, receive) = oneshot::channel();
    owner
        .requests
        .try_send(ConnectorRequest::AccessManagement(Request {
            session,
            action,
            response,
        }))
        .map_err(|_| refused())?;
    let result = receive.await.map_err(|_| ToolError::execution_failed(
        "Access management returned no result. This is not evidence that an earlier or entered native action failed. Do not retry it automatically."))?;
    Ok(ToolResult::new(call.id.into(), result.to_string(), false).into())
}

async fn owned(storage: &ResolvedStorage) -> Result<(Vec<McpServerConfig>, u64), &'static str> {
    let (mut servers, generation) = crate::realm::read_own_mcp_config(storage)
        .await
        .map_err(|_| "owned_definitions_unavailable")?;
    servers.sort_by(|a, b| a.name.cmp(&b.name));
    if servers.windows(2).any(|pair| pair[0].name == pair[1].name) {
        return Err("owned_definitions_ambiguous");
    }
    Ok((servers, generation))
}
fn validate_selection(
    storage: &ResolvedStorage,
    config: &McpServerConfig,
    change: &Change,
) -> Result<(), &'static str> {
    if config.name != change.service()
        || !valid_name(change.service())
        || !valid_name(change.agent())
    {
        return Err("select_exact_service_and_agent");
    }
    stock_agent_principal(&storage.locator.realm, change.agent())
        .map_err(|_| "native_agent_target_invalid")?;
    if let Change::Grant {
        operations,
        expires_in_ms,
        ..
    } = change
    {
        let ceiling = config
            .access
            .as_ref()
            .ok_or("declared_operation_ceiling_unavailable")?;
        if operations.is_empty()
            || operations.len() > 256
            || *expires_in_ms == Some(0)
            || operations.iter().collect::<BTreeSet<_>>().len() != operations.len()
            || operations
                .iter()
                .any(|name| !valid_name(name) || !ceiling.allowed_operations.contains(name))
        {
            return Err("select_nonempty_exact_declared_operation_subset");
        }
    }
    Ok(())
}
async fn inventory(storage: &ResolvedStorage, query: &Inventory) -> Result<Value, &'static str> {
    let explicit_agent = query.agent.as_deref();
    if let Some(agent) = explicit_agent {
        stock_agent_principal(&storage.locator.realm, agent)
            .map_err(|_| "native_agent_target_invalid")?;
    }
    let (servers, _) = owned(storage).await?;
    let page = servers
        .iter()
        .filter(|server| {
            query
                .after
                .as_ref()
                .is_none_or(|after| server.name > *after)
        })
        .take(PAGE_SIZE + 1)
        .collect::<Vec<_>>();
    let rows = page.iter().take(PAGE_SIZE).map(|server| json!({"service":server.name,
        "declared_raw_operation_ceiling":server.access.as_ref().map(|access| &access.allowed_operations),
        "grant_requires_explicit_declared_subset":true})).collect::<Vec<_>>();
    let next = (page.len() > PAGE_SIZE).then(|| page[PAGE_SIZE - 1].name.clone());
    Ok(
        json!({"realm":storage.locator.realm.as_str(),"services":rows,"next":next,
        "default_stock_agent":DEFAULT_STOCK_AGENT,"validated_explicit_target":explicit_agent,
        "scope":"own_configured_services_only","current_grants":"not_inspected",
        "notice":"Declared ceilings are not effective permissions or proof of live tools. The default is a native stock target; validation of another explicit target is not evidence that a running agent or current grant exists. No current grant status, operation selection or expiry is claimed."}),
    )
}

pub(super) async fn present<F: Future<Output = io::Result<()>> + Send, W: Write>(
    owner: &mut TerminalConnectors,
    mut request: Request,
    approvals: &LocalApprovals,
    presenter: &mut ApprovalPresenter,
    input: &mut TerminalInput,
    output: &mut W,
    interrupt: F,
) -> Result<bool, ConnectError> {
    if request.response.is_closed() {
        return Ok(true);
    }
    if approvals.check_session(&request.session).is_err() {
        return request_refused("review_owner_closed", request.response, output);
    }
    if StockPrincipals::for_local_operator(&owner.storage.locator.realm).is_err() {
        return request_refused(
            "selected_realm_operator_unavailable",
            request.response,
            output,
        );
    }
    tokio::pin!(interrupt);
    match request.action {
        Requested::Inventory(query) => {
            let result = tokio::select! { biased;
                signal = interrupt.as_mut() => { signal.map_err(|e| ConnectError::native("Listen for access inventory cancellation", e))?; return Ok(false); },
                _ = request.response.closed() => return Ok(true),
                result = inventory(&owner.storage, &query) => result,
            };
            if approvals.check_session(&request.session).is_err() {
                return request_refused("review_owner_closed", request.response, output);
            }
            match result {
                Ok(result) => {
                    let _ = request.response.send(result);
                    Ok(true)
                }
                Err(reason) => request_refused(reason, request.response, output),
            }
        }
        Requested::Change(change) => {
            if owner.reviewed.is_some() {
                return request_refused(
                    "reviewed_finalization_waiting_for_setup_close",
                    request.response,
                    output,
                );
            }
            let snapshot = tokio::select! { biased;
                signal = interrupt.as_mut() => { signal.map_err(|e| ConnectError::native("Listen for access selection cancellation", e))?; return Ok(false); },
                _ = request.response.closed() => return Ok(true),
                result = owned(&owner.storage) => result,
            };
            let (servers, generation) = match snapshot {
                Ok(value) => value,
                Err(reason) => return request_refused(reason, request.response, output),
            };
            let Some(config) = servers
                .into_iter()
                .find(|server| server.name == change.service())
            else {
                return request_refused(
                    "select_current_owned_service_from_inventory",
                    request.response,
                    output,
                );
            };
            if let Err(reason) = validate_selection(&owner.storage, &config, &change) {
                return request_refused(reason, request.response, output);
            }
            if approvals.check_session(&request.session).is_err() {
                return request_refused("review_owner_closed", request.response, output);
            }
            if input.require_terminal().is_err() {
                return request_refused("protected_terminal_required", request.response, output);
            }
            let body = review_body(&owner.storage, &config, &change);
            let decision = match review(
                approvals,
                presenter,
                input,
                output,
                review_request(
                    &request.session,
                    "Change this agent's service access?",
                    body,
                )?,
                &mut request.response,
                interrupt.as_mut(),
            )
            .await?
            {
                ReviewOutcome::Decided(value) => value,
                ReviewOutcome::CallerGone => return Ok(true),
                ReviewOutcome::Stopped => return Ok(false),
            };
            if decision.status != ApprovalStatus::Approved {
                let _ = request.response.send(json!({"state":"denied","phase":"service_access_management","native_action_entered":false}));
                return Ok(true);
            }
            if approvals.check_session(&request.session).is_err() {
                return request_refused("review_owner_closed", request.response, output);
            }
            if request.response.is_closed() {
                return Ok(true);
            }
            // The native reviewed issuer repeats its exact config/generation
            // check under custody. Revoke instead retains the reviewed exact
            // realm/agent/service target, selected by native custody at execution.
            if matches!(&change, Change::Grant { .. }) {
                let current = tokio::select! { biased;
                    signal = interrupt.as_mut() => { signal.map_err(|e| ConnectError::native("Listen after access review", e))?; return Ok(false); },
                    _ = request.response.closed() => return Ok(true),
                    result = owned(&owner.storage) => result,
                };
                if !current.as_ref().is_ok_and(|(servers, current_generation)| {
                    *current_generation == generation
                        && servers
                            .iter()
                            .filter(|server| server.name == config.name)
                            .eq(std::iter::once(&config))
                }) {
                    return request_refused(
                        "owned_definition_changed_after_review",
                        request.response,
                        output,
                    );
                }
            }
            // Revoke no longer awaits a configuration read here. Observe any
            // ready stop or lost caller before retaining either approved action.
            tokio::select! { biased;
                signal = interrupt.as_mut() => { signal.map_err(|e| ConnectError::native("Listen before retaining access review", e))?; return Ok(false); },
                _ = request.response.closed() => return Ok(true),
                _ = std::future::ready(()) => {},
            }
            if approvals.check_session(&request.session).is_err() {
                return request_refused("review_owner_closed", request.response, output);
            }
            if request.response.is_closed() {
                return Ok(true);
            }
            let reviewed = ReviewedAccess {
                storage: owner.storage.clone(),
                session: request.session,
                config,
                generation,
                change,
                decision: *decision,
            };
            if request.response.send(json!({"state":"pending","phase":"service_access_management",
                "reason":"reviewed_access_waiting_for_setup_close","native_action_entered":false,
                "instruction":"Finish this turn. The host must close setup and finish cleanup before this fresh native access action. This review is not a grant or a replay of an earlier action."})).is_ok() {
                owner.reviewed = Some(ReviewedAction::Access(Box::new(reviewed)));
            }
            Ok(true)
        }
    }
}

fn review_body(storage: &ResolvedStorage, config: &McpServerConfig, change: &Change) -> Value {
    // Project only the retained definition's nonsecret binding and the exact
    // proposed operations. These declarations are not a fresh account read.
    let configured_connection = config.connector.as_ref().map(|binding| json!({
        "realm":binding.realm_id,"slot":binding.slot_id,"client":binding.client,"resource":binding.resource,
        "account":{"issuer":binding.issuer,"strategy":binding.strategy_id,"subject":binding.account},
    }));
    let selected_operation_requirements = match change {
        Change::Grant { operations, .. } => Some(
            operations
                .iter()
                .map(|name| {
                    (
                        name,
                        config
                            .connector
                            .as_ref()
                            .and_then(|binding| binding.operations.get(name)),
                    )
                })
                .collect::<BTreeMap<_, _>>(),
        ),
        Change::Revoke { .. } => None,
    };
    json!({"phase":"service_access_management","realm":storage.locator.realm.as_str(),"selection":change,
        "configured_connection":configured_connection,"selected_operation_requirements":selected_operation_requirements,
        "connection_context":match change {
            Change::Grant {..} => "Configured binding and selected raw-operation requirements only, not a freshly verified account or observed scopes. A null binding or operation requirement means no connector declaration is available in this definition; no account, destination or scope is inferred or broadened. Native issuance rechecks the exact reviewed definition and generation.",
            Change::Revoke {..} => "The configured binding is context at review, not a freshly verified account. It is not a restriction on this revoke. Current grant operations were not inspected. Revocation targets the agent/service selection when executed, even if its account or operation meaning has changed since review.",
        },
        "current_grants":"not_inspected",
        "consequences":match change {
            Change::Grant {..} => "Issue and select NEW access for this agent/service and exact operations, replacing that target's prior selection if any. Other targets remain. Superseded cleanup may be deferred or uncertain. New selections affect later-opened realms; open owners can retain existing selections.",
            Change::Revoke {..} => "Revoke whichever grant is selected for this agent/service when native administration executes. A concurrent change can change that selected grant. This does not target a previously displayed reference. Native revocation, unselection and cleanup are reported separately; only Revoked confirms revocation.",
        },
        "notice":"This action does not reinstall, change the service definition, rotate shared credentials or change frontend roles. It does not prove a usable account or successful service operation. Earlier uncertain actions remain unresolved. There is no automatic retry. Cancellation after native entry waits for its actual outcome."})
}

pub struct ReviewedAccess {
    storage: ResolvedStorage,
    session: SessionId,
    config: McpServerConfig,
    generation: u64,
    change: Change,
    decision: ApprovalRecord,
}
enum Outcome {
    NotEntered(&'static str),
    Grant(Result<StockGrantOutcome, StockGrantError>),
    Revoke(Result<StockRevokeOutcome, StockGovernanceError>),
}
pub struct FinalizedAccess {
    realm: meerkat_core::RealmLocator,
    session: SessionId,
    change: Change,
    outcome: Outcome,
    stopped: Option<secrets::Stop>,
    continuation: Option<FinalizationNotice>,
}
impl ReviewedAccess {
    pub(super) async fn finalize<I: Future<Output = io::Result<()>>>(
        self,
        mut interrupt: Pin<&mut I>,
    ) -> FinalizedAccess {
        let mut stopped = None;
        let outcome = if self.decision.status != ApprovalStatus::Approved
            || validate_selection(&self.storage, &self.config, &self.change).is_err()
        {
            Outcome::NotEntered("exact_review_unavailable")
        } else if let Ok(principals) =
            StockPrincipals::for_local_operator(&self.storage.locator.realm)
        {
            match &self.change {
                Change::Grant {
                    service,
                    agent,
                    operations,
                    expires_in_ms,
                } => {
                    let request = StockGrantRequest {
                        agent: agent.clone(),
                        expires_in_ms: *expires_in_ms,
                        target: StockGrantTarget::Mcp {
                            server: service.clone(),
                            operations: McpOperations::Exact(operations.iter().cloned().collect()),
                        },
                    };
                    let installation = ReviewedMcpInstallation {
                        server: self.config.clone(),
                        generation: self.generation,
                    };
                    match secrets::joined_effect(interrupt.as_mut(), &mut stopped, || {
                        grant_stock_agent_for_reviewed_installation(
                            &self.storage,
                            &principals,
                            &request,
                            &installation,
                        )
                    })
                    .await
                    {
                        Some(result) => Outcome::Grant(result),
                        None => Outcome::NotEntered("stopped_before_native_entry"),
                    }
                }
                Change::Revoke { service, agent } => {
                    let slot = StockGrantSlot::Mcp(service.clone());
                    match secrets::joined_effect(interrupt.as_mut(), &mut stopped, || {
                        revoke_stock_agent(
                            &self.storage.locator.state_root,
                            self.storage.locator.realm.as_str(),
                            &principals,
                            agent,
                            &slot,
                        )
                    })
                    .await
                    {
                        Some(result) => Outcome::Revoke(result),
                        None => Outcome::NotEntered("stopped_before_native_entry"),
                    }
                }
            }
        } else {
            Outcome::NotEntered("selected_realm_operator_unavailable")
        };
        FinalizedAccess::new(
            self.storage.locator,
            self.session,
            self.change,
            outcome,
            stopped,
        )
    }
}

fn reference(value: &meerkat_authorization_contracts::work_association::GrantLineageRef) -> Value {
    json!({"grant_id":value.grant_id,"issued_revision":value.issued_revision,"authority_generation":value.authority_generation})
}
fn cleanup(value: &[meerkat::governed_realm::stock::issuance::CleanupAttempt]) -> Value {
    json!(
        value
            .iter()
            .map(
                |attempt| json!({"agent":attempt.agent,"grant":reference(&attempt.grant),
        "outcome":format!("{:?}",attempt.outcome)})
            )
            .collect::<Vec<_>>()
    )
}
fn outcome_facts(outcome: &Outcome) -> Value {
    match outcome {
        Outcome::NotEntered(reason) => json!({"state":"not_entered","reason":reason}),
        Outcome::Grant(Ok(result)) => json!({"state":"selected","issued":reference(&result.issued),
            "superseded":result.superseded.as_ref().map(reference),"cleanup":cleanup(&result.cleanup)}),
        Outcome::Grant(Err(StockGrantError::Refused(reason))) => {
            json!({"state":"refused","reason_kind":grant_refusal_tag(reason),"issued":false,"selection_changed":false})
        }
        Outcome::Grant(Err(StockGrantError::IssuanceUncertain { id })) => {
            json!({"state":"issuance_uncertain","possibly_issued_id":id,"selection_changed":false})
        }
        Outcome::Grant(Err(StockGrantError::NotSelected { issued, .. })) => {
            json!({"state":"issued_not_selected","issued":reference(issued),"selection_changed":false})
        }
        Outcome::Grant(Err(StockGrantError::SelectionUncertain {
            issued, previous, ..
        })) => {
            json!({"state":"selection_durability_uncertain","issued":reference(issued),"previous":previous.as_ref().map(reference)})
        }
        Outcome::Grant(Err(StockGrantError::CommandUncertain)) => {
            json!({"state":"command_uncertain"})
        }
        Outcome::Revoke(Ok(result)) => {
            json!({"state":"returned","selected_at_execution":reference(&result.revoked),
            "revocation":format!("{:?}",result.outcome),"revocation_confirmed":result.outcome == RevocationOutcome::Revoked,
            "unselection":if result.unselected.is_ok() {"returned_success"} else {"returned_error"},"cleanup":cleanup(&result.cleanup)})
        }
        // This error family is not universally pre-effect. Keep its exact
        // category without turning a failed command into a no-change receipt.
        Outcome::Revoke(Err(reason)) => {
            json!({"state":"command_did_not_return_outcome","reason_kind":grant_refusal_tag(reason),"effects":"not_confirmed"})
        }
    }
}
impl FinalizedAccess {
    fn new(
        realm: meerkat_core::RealmLocator,
        session: SessionId,
        change: Change,
        outcome: Outcome,
        stopped: Option<secrets::Stop>,
    ) -> Self {
        let mut observed = Self {
            realm,
            session,
            change,
            outcome,
            stopped,
            continuation: None,
        };
        let mut header = meerkat_runtime::PromptInput::new(String::new(), None).header;
        header.source = meerkat_runtime::InputOrigin::External {
            source_name: "meerkat-connect-access".into(),
        };
        observed.continuation = Some(FinalizationNotice {
            realm: observed.realm.clone(),
            session: observed.session.clone(),
            input: meerkat_runtime::Input::ExternalEvent(meerkat_runtime::ExternalEventInput {
                header,
                event_type: "connect_access_observation".into(),
                payload: json!({"body":observed.notice().to_string()}),
                blocks: None,
                handling_mode: meerkat_runtime::HandlingMode::default(),
                render_metadata: None,
                objective_id: None,
            }),
        });
        observed
    }
    fn notice(&self) -> Value {
        json!({"kind":"connect_access_observation","realm":self.realm.realm.as_str(),"selection":self.change,
            "facts":outcome_facts(&self.outcome),"stop":self.stopped.as_ref().map(|stop| match stop {
                secrets::Stop::Cancelled => "cancelled", secrets::Stop::ListenerFailed(_) => "listener_failed",
            }),"configuration_changed":false,"credentials_changed":false,"service_readiness":"not_observed",
            "prior_uncertain_actions":"not_resolved","current_grants":"not_inspected",
            "instruction":"This is this invocation's actual fresh access result, not human consent or a current grant inspection. Keep issuance/selection, revocation, unselection and cleanup outcomes distinct. Continue the requested setup using these facts; no automatic retry, installation replay or claim of successful service use. New selections affect later-opened realms; open owners may retain prior selections. A later action cannot settle an older uncertain one."})
    }
    pub(super) fn take_continuation_notice(&mut self) -> Option<FinalizationNotice> {
        self.continuation.take()
    }
    pub(super) fn stopped(&self) -> bool {
        self.stopped.is_some()
    }
    pub(super) fn has_unconfirmed_effect(&self) -> bool {
        match &self.outcome {
            Outcome::NotEntered(_) => false,
            Outcome::Grant(result) => unconfirmed_grant_effect(result.as_ref()),
            Outcome::Revoke(Ok(result)) => {
                result.outcome == RevocationOutcome::Uncertain
                    || result.unselected.is_err()
                    || result
                        .cleanup
                        .iter()
                        .any(|attempt| attempt.outcome == RevocationOutcome::Uncertain)
            }
            // This API returns entered revocation/selection effects in Ok;
            // losing its owned command is the error with an unknown effect.
            Outcome::Revoke(Err(error)) => matches!(error, StockGovernanceError::CommandUncertain),
        }
    }
    pub(super) fn display<W: Write>(&self, output: &mut W) -> Result<(), ConnectError> {
        writeln!(output, "{}", self.notice())
            .map_err(|error| ConnectError::native("Display native access result", error))
    }
    pub(super) fn retain(
        &self,
        directory: &std::path::Path,
        ordinal: usize,
    ) -> Result<(), ConnectError> {
        use std::os::unix::fs::OpenOptionsExt;
        let observed = serde_json::to_vec_pretty(&self.notice()).map_err(|_| {
            unavailable("The access observation could not be encoded; no retry was started.")
        })?;
        let result = (|| -> io::Result<()> {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(directory.join(format!("standalone-access-{ordinal}.txt")))?;
            file.write_all(&observed)?;
            file.sync_all()
        })();
        result.map_err(|_| unavailable("The access observation could not be fully retained. The actual typed native result is held by this invocation and displayed separately; no retry was started."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_auth_core::auth_store::{
        FileLockCoordinator, FileTokenStore, ProviderAuthPersistence,
    };
    use meerkat_core::{ApprovalService, ConfigDelta, ConfigStore, FileConfigStore};
    use std::path::Path;

    #[test]
    fn access_exit_keeps_unknown_native_effects_but_not_definite_no_entry() {
        let root = tempfile::tempdir().unwrap();
        let storage = storage(root.path(), "exit-fixture");
        for (outcome, expected) in [
            (Outcome::NotEntered("stopped_before_native_entry"), false),
            (
                Outcome::Grant(Err(StockGrantError::Refused(
                    StockGovernanceError::GrantsUnavailable,
                ))),
                false,
            ),
            (Outcome::Grant(Err(StockGrantError::CommandUncertain)), true),
            (
                Outcome::Revoke(Err(StockGovernanceError::GrantsUnavailable)),
                false,
            ),
            (
                Outcome::Revoke(Err(StockGovernanceError::CommandUncertain)),
                true,
            ),
        ] {
            let result = FinalizedAccess::new(
                storage.locator.clone(),
                SessionId::new(),
                change("mail", "fixture"),
                outcome,
                Some(secrets::Stop::Cancelled),
            );
            let action = FinalizedAction {
                outcome: ActionOutcome::Access(Box::new(result)),
                continuation: None,
            };
            assert_eq!(
                FinalizedAction::finish_invocation(&[action], Ok(())).is_err(),
                expected
            );
        }
    }

    #[test]
    fn access_exit_preserves_unknown_revocation_unselection_and_cleanup() {
        let root = tempfile::tempdir().unwrap();
        let storage = storage(root.path(), "exit-fixture");
        let principal = stock_agent_principal(&storage.locator.realm, "fixture").unwrap();
        // Identifier shape only, never passed to a native operation.
        let reference: meerkat_authorization_contracts::work_association::GrantLineageRef =
            serde_json::from_value(json!({"root_authority":principal,
                "authority_namespace":"fixture-authority","authority_generation":1,
                "authority_incarnation":"6f75f457-4232-4e12-8426-508d49ece197",
                "grant_id":"current","issued_revision":1}))
            .unwrap();
        for (revocation, unselected, cleanup, expected) in [
            (
                RevocationOutcome::Revoked,
                Ok(()),
                RevocationOutcome::Revoked,
                false,
            ),
            (
                RevocationOutcome::Deferred,
                Ok(()),
                RevocationOutcome::Deferred,
                false,
            ),
            (
                RevocationOutcome::Uncertain,
                Ok(()),
                RevocationOutcome::Revoked,
                true,
            ),
            (
                RevocationOutcome::Revoked,
                Err("PRIVATE_UNSELECTION".into()),
                RevocationOutcome::Revoked,
                true,
            ),
            (
                RevocationOutcome::Revoked,
                Ok(()),
                RevocationOutcome::Uncertain,
                true,
            ),
        ] {
            let result = FinalizedAccess::new(
                storage.locator.clone(),
                SessionId::new(),
                Change::Revoke {
                    service: "mail".into(),
                    agent: "fixture".into(),
                },
                Outcome::Revoke(Ok(StockRevokeOutcome {
                    revoked: reference.clone(),
                    outcome: revocation,
                    unselected,
                    cleanup: vec![meerkat::governed_realm::stock::issuance::CleanupAttempt {
                        agent: "fixture".into(),
                        grant: reference.clone(),
                        outcome: cleanup,
                    }],
                })),
                None,
            );
            let action = FinalizedAction {
                outcome: ActionOutcome::Access(Box::new(result)),
                continuation: None,
            };
            let result = FinalizedAction::finish_invocation(&[action], Ok(()));
            assert_eq!(result.is_err(), expected);
            if let Err(error) = result {
                assert!(!error.to_string().contains("PRIVATE_UNSELECTION"));
            }
        }
    }

    fn storage(root: &Path, realm: &str) -> ResolvedStorage {
        crate::RealmOptions {
            context_root: root.to_owned(),
            state_root: Some(root.join("state")),
            user_config_root: Some(root.join("home")),
        }
        .resolve(realm)
        .unwrap()
    }
    fn config(name: &str) -> McpServerConfig {
        let mut config = McpServerConfig::stdio(
            name,
            "/bin/true",
            vec!["private-launch-argument".into()],
            [("PRIVATE_INLINE".into(), "private-inline-value".into())]
                .into_iter()
                .collect(),
        );
        config.access = Some(meerkat_core::McpServerAccess::allowing([
            "read_messages",
            "send_message",
        ]));
        config
    }
    fn configured_connection(storage: &ResolvedStorage) -> McpServerConfig {
        let mut config = config("mail");
        config.connector = Some(meerkat_core::mcp_config::McpConnectorBinding {
            realm_id: storage.locator.realm.as_str().into(),
            slot_id: "configured-mail-slot".into(),
            issuer: "https://identity.example".into(),
            client: "configured-mail-client".into(),
            resource: "https://mail.example".into(),
            strategy_id: "oidc-userinfo-v1".into(),
            account: "configured-mail-subject".into(),
            operations: [
                (
                    "read_messages".into(),
                    McpConnectorOperation {
                        required_scopes: ["mail.read".into()].into(),
                        destination_origin: "https://read.mail.example".into(),
                        read_only: true,
                    },
                ),
                (
                    "send_message".into(),
                    McpConnectorOperation {
                        required_scopes: ["mail.send".into()].into(),
                        destination_origin: "https://send.mail.example".into(),
                        read_only: false,
                    },
                ),
            ]
            .into(),
        });
        config
    }
    async fn save(storage: &ResolvedStorage, configs: &[McpServerConfig]) {
        let path = crate::realm::config_source(storage)
            .unwrap()
            .config_doc_path(&storage.locator.realm);
        if path.exists() {
            let runtime = crate::realm::own_config_runtime(storage).unwrap();
            runtime
                .patch(ConfigDelta(json!({"tools":{"mcp_servers":configs}})), None)
                .await
                .unwrap();
        } else {
            FileConfigStore::new(path, meerkat_models::canonical())
                .patch(ConfigDelta(json!({"tools":{"mcp_servers":configs}})))
                .await
                .unwrap();
        }
    }
    async fn govern(storage: &ResolvedStorage) {
        let (_, bundle) = meerkat::storage_provider::open_realm_persistence_with_layout(
            storage.layout.clone(),
            storage.locator.realm.as_str(),
            None,
            Some(meerkat_store::RealmOrigin::Explicit),
        )
        .await
        .unwrap();
        drop(bundle);
        let principals = StockPrincipals::for_local_operator(&storage.locator.realm).unwrap();
        meerkat::governed_realm::stock::govern_stock_realm(
            &storage.locator.state_root,
            storage.locator.realm.as_str(),
            &principals,
        )
        .await
        .unwrap();
    }
    fn change(service: &str, agent: &str) -> Change {
        Change::Grant {
            service: service.into(),
            agent: agent.into(),
            operations: vec!["read_messages".into()],
            expires_in_ms: Some(60000),
        }
    }
    fn decision(session: &SessionId) -> ApprovalRecord {
        let service = ApprovalService::new();
        let requested = service.request(review_request(session,"Controlled fresh access review",
            json!({"scope":"native access boundary control, not an installed service or live account"})).unwrap()).unwrap();
        service
            .decide(
                &requested.approval_id,
                ApprovalDecision::Approve,
                ApprovalPrincipalId::new("fixture-operator").unwrap(),
                None,
                None,
            )
            .unwrap()
    }
    async fn reviewed(storage: &ResolvedStorage, change: Change) -> ReviewedAccess {
        let (servers, generation) = owned(storage).await.unwrap();
        let config = servers
            .into_iter()
            .find(|server| server.name == change.service())
            .unwrap();
        let session = SessionId::new();
        ReviewedAccess {
            storage: storage.clone(),
            config,
            generation,
            change,
            decision: decision(&session),
            session,
        }
    }
    async fn finish(reviewed: ReviewedAccess) -> FinalizedAccess {
        let interrupt = std::future::pending::<io::Result<()>>();
        tokio::pin!(interrupt);
        reviewed.finalize(interrupt.as_mut()).await
    }
    async fn owner(root: &Path, storage: ResolvedStorage) -> TerminalConnectors {
        let persistence = ProviderAuthPersistence::new(
            Arc::new(FileTokenStore::new(root.join("credentials"))),
            Arc::new(FileLockCoordinator::new(root.join("credential-locks"))),
        );
        let mut startup = SessionStartup {
            storage,
            config: meerkat::Config::default(),
            factory: meerkat::AgentFactory::new(root.join("factory"))
                .with_provider_auth_persistence(persistence),
            persistence: None,
            connector_auth: None,
            standalone: None,
        };
        Box::pin(TerminalConnectors::open(&mut startup))
            .await
            .unwrap()
    }
    // A governed realm's terminal owner takes the governed native owner, as the
    // standalone continuation reopens it; `owner` opens ordinarily, which Core
    // refuses for a governed realm (`RealmGoverned`).
    async fn governed_owner(root: &Path, storage: ResolvedStorage) -> TerminalConnectors {
        let native = crate::stdio::tests::realm_native_owner(&storage).await;
        let persistence = ProviderAuthPersistence::new(
            Arc::new(FileTokenStore::new(root.join("credentials"))),
            Arc::new(FileLockCoordinator::new(root.join("credential-locks"))),
        );
        let mut startup = SessionStartup {
            storage,
            config: meerkat::Config::default(),
            factory: meerkat::AgentFactory::new(root.join("factory"))
                .with_provider_auth_persistence(persistence),
            persistence: Some(native),
            connector_auth: None,
            standalone: None,
        };
        Box::pin(TerminalConnectors::open(&mut startup))
            .await
            .unwrap()
    }
    #[derive(Clone, Default)]
    struct Seen(Arc<std::sync::Mutex<Vec<u8>>>);
    impl Seen {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }
    impl Write for Seen {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    // Change review requires a protected terminal (`require_terminal`); scripted
    // input is a non-terminal by design. The human types on the PTY master.
    fn protected_terminal() -> (TerminalInput, std::os::fd::OwnedFd) {
        let pair = nix::pty::openpty(None, None).unwrap();
        (TerminalInput::from_owned_fd(pair.slave), pair.master)
    }
    async fn answer(seen: &Seen, lines: &std::os::fd::OwnedFd, approve: bool) {
        while !seen.text().contains("Type 'approve ") {
            tokio::task::yield_now().await;
        }
        let text = seen.text();
        let id = text
            .split("Type 'approve ")
            .last()
            .unwrap()
            .split('\'')
            .next()
            .unwrap();
        let line = format!("{} {id}\n", if approve { "approve" } else { "deny" });
        nix::unistd::write(lines, line.as_bytes()).unwrap();
    }
    fn request(session: &SessionId, change: Change) -> (Request, oneshot::Receiver<Value>) {
        let (response, receive) = oneshot::channel();
        (
            Request {
                session: session.clone(),
                action: Requested::Change(change),
                response,
            },
            receive,
        )
    }

    #[test]
    fn registered_management_schemas_have_closed_object_discriminators() {
        let root = tempfile::tempdir().unwrap();
        let (requests, _receive) = mpsc::channel(1);
        let dispatcher = RequestTool {
            installs: CheckedInstalls::fixture(Arc::new(
                meerkat_core::gateway::DynamicToolComposite::new(vec![]),
            )),
            storage: storage(root.path(), "registered-management-schemas"),
            requests,
        };
        let mut definitions = dispatcher.tools().to_vec();
        definitions.extend(
            crate::paired_host_tool::PairedHostFrontend::new(
                crate::paired_host::PairedHostClient::new("http://127.0.0.1:9").unwrap(),
            )
            .dispatcher()
            .tools()
            .iter()
            .cloned(),
        );
        definitions.push(Arc::new(
            crate::installed_service_review::installed_account_request_definition(),
        ));
        for name in [
            crate::paired_host_tool::PAIRED_HOST_SECRET_CREATE,
            crate::paired_host_tool::PAIRED_HOST_FRONTEND_PREPARE,
            meerkat_toolkit::setup::SETUP_ACCOUNT_REQUEST,
        ] {
            assert_eq!(
                definitions
                    .iter()
                    .filter(|definition| definition.name == name)
                    .count(),
                1,
                "{name}"
            );
        }
        for definition in definitions.iter() {
            assert_eq!(
                definition.input_schema["type"], "object",
                "{}",
                definition.name
            );
            assert_eq!(
                definition.input_schema["additionalProperties"], false,
                "{}",
                definition.name
            );
        }
        for definition in definitions.iter() {
            for keyword in ["oneOf", "anyOf", "allOf", "enum", "not"] {
                assert!(
                    definition.input_schema.get(keyword).is_none(),
                    "{}: {keyword}",
                    definition.name
                );
            }
        }
        for (name, actions, required, property_names) in [
            (
                OAUTH_MANAGE_TOOL,
                json!(["reconnect", "disconnect"]),
                json!(["action", "service"]),
                vec!["action", "service", "scopes", "client_auth_profile"],
            ),
            (
                ACCESS_MANAGE_TOOL,
                json!(["grant", "revoke"]),
                json!(["action", "service", "agent"]),
                vec!["action", "service", "agent", "operations", "expires_in_ms"],
            ),
        ] {
            let registered = definitions
                .iter()
                .filter(|definition| definition.name == name)
                .collect::<Vec<_>>();
            assert_eq!(registered.len(), 1, "{name}");
            let schema = &registered[0].input_schema;
            assert_eq!(schema["additionalProperties"], false, "{name}");
            assert_eq!(schema["required"], required, "{name}");
            let properties = schema["properties"].as_object().unwrap();
            assert_eq!(
                properties
                    .keys()
                    .map(String::as_str)
                    .collect::<BTreeSet<_>>(),
                property_names.into_iter().collect::<BTreeSet<_>>(),
                "{name}"
            );
            assert_eq!(
                properties["action"],
                json!({"type":"string","enum":actions}),
                "{name}"
            );
        }
    }

    #[test]
    fn access_proposals_require_explicit_lifetime_and_exact_operations() {
        let base = json!({"action":"grant","service":"mail","agent":"reader","operations":["read_messages"],"expires_in_ms":null});
        assert!(parse_change(base.clone()).is_ok());
        for field in ["action", "service", "agent", "operations", "expires_in_ms"] {
            let mut raw = base.clone();
            raw.as_object_mut().unwrap().remove(field);
            assert!(parse_change(raw).is_err());
        }
        for operations in [
            json!([]),
            json!(["read_messages", "read_messages"]),
            json!("all"),
        ] {
            let mut raw = base.clone();
            raw["operations"] = operations;
            assert!(parse_change(raw).is_err());
        }
        let mut raw = base.clone();
        raw["expires_in_ms"] = json!(0);
        assert!(parse_change(raw).is_err());
        let mut raw = base;
        raw["job_id"] = json!("historical-job");
        assert!(parse_change(raw).is_err());
        assert!(
            parse_change(
                json!({"action":"revoke","service":"mail","agent":"reader","grant_id":"old"})
            )
            .is_err()
        );
        assert!(parse_change(json!({"action":"revoke","service":"mail","agent":"reader"})).is_ok());
        for extra in [
            json!({"operations":["read_messages"]}),
            json!({"expires_in_ms":null}),
            json!({"operations":["read_messages"],"expires_in_ms":null}),
        ] {
            let mut revoke = json!({"action":"revoke","service":"mail","agent":"reader"});
            revoke
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            assert!(parse_change(revoke).is_err());
        }
    }

    #[tokio::test]
    async fn access_inventory_is_paginated_config_projection_not_grant_or_credential_inspection() {
        let root = tempfile::tempdir().unwrap();
        let selected = storage(root.path(), "access-inventory");
        let mut configs = (0..18)
            .map(|index| config(&format!("mail-{index:02}")))
            .collect::<Vec<_>>();
        configs[0].access = None;
        save(&selected, &configs).await;
        let other = storage(root.path(), "other-realm");
        save(&other, &[config("foreign-mail")]).await;
        let first = inventory(
            &selected,
            &Inventory {
                after: None,
                agent: Some("reader".into()),
            },
        )
        .await
        .unwrap();
        assert_eq!(first["services"].as_array().unwrap().len(), 16);
        assert_eq!(first["default_stock_agent"], DEFAULT_STOCK_AGENT);
        assert_eq!(first["validated_explicit_target"], "reader");
        assert_eq!(first["current_grants"], "not_inspected");
        assert!(first["services"][0]["declared_raw_operation_ceiling"].is_null());
        let second = inventory(
            &selected,
            &Inventory {
                after: Some(first["next"].as_str().unwrap().into()),
                agent: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(second["services"].as_array().unwrap().len(), 2);
        assert!(second["next"].is_null());
        let text = format!("{first}{second}");
        for private in [
            "private-launch-argument",
            "private-inline-value",
            "PRIVATE_INLINE",
            "/bin/true",
            "foreign-mail",
        ] {
            assert!(!text.contains(private));
        }
        assert!(validate_selection(&selected, &configs[0], &change("mail-00", "reader")).is_err());
        assert!(
            validate_selection(
                &selected,
                &configs[0],
                &Change::Revoke {
                    service: "mail-00".into(),
                    agent: "reader".into()
                }
            )
            .is_ok()
        );
        let mut wider = change("mail-01", "reader");
        if let Change::Grant { operations, .. } = &mut wider {
            operations.push("undocumented_write".into());
        }
        assert!(validate_selection(&selected, &configs[1], &wider).is_err());
    }

    #[tokio::test]
    async fn access_presenter_refuses_missing_stale_or_wrong_session_without_ending_conversation() {
        tokio::time::timeout(
            Duration::from_secs(30),
            Box::pin(async {
                let root = tempfile::tempdir().unwrap();
                let selected = storage(root.path(), "access-review");
                save(&selected, &[configured_connection(&selected)]).await;
                let mut owner = owner(root.path(), selected.clone()).await;
                let session = SessionId::new();
                let (approvals, mut presenter) =
                    LocalApprovals::create_paired_host(root.path()).unwrap();
                approvals.bind_session(&session).unwrap();
                let (mut input, lines) = protected_terminal();
                let seen = Seen::default();
                let mut output = seen.clone();
                for (request_session, proposal) in [
                    (SessionId::new(), change("mail", "reader")),
                    (session.clone(), change("missing", "reader")),
                ] {
                    let (request, receive) = request(&request_session, proposal);
                    assert!(
                        present(
                            &mut owner,
                            request,
                            &approvals,
                            &mut presenter,
                            &mut input,
                            &mut output,
                            std::future::pending::<io::Result<()>>()
                        )
                        .await
                        .unwrap()
                    );
                    assert_eq!(receive.await.unwrap()["state"], "unavailable");
                    assert!(!owner.has_reviewed());
                }
                for kind in [
                    "connector_grant",
                    "missing_operation",
                    "stdio_grant",
                    "connector_revoke",
                ] {
                    let mut saved = configured_connection(&selected);
                    if kind == "missing_operation" {
                        saved
                            .connector
                            .as_mut()
                            .unwrap()
                            .operations
                            .remove("read_messages");
                    }
                    if kind == "stdio_grant" {
                        saved.connector = None;
                        if let meerkat_core::mcp_config::McpTransportConfig::Stdio(stdio) =
                            &mut saved.transport
                        {
                            stdio.secret_env.insert("PRIVATE_SECRET_ENV".into());
                        }
                    }
                    save(&selected, &[saved]).await;
                    let change = if kind == "connector_revoke" {
                        Change::Revoke {
                            service: "mail".into(),
                            agent: "reader".into(),
                        }
                    } else {
                        change("mail", "reader")
                    };
                    let (request, receive) = request(&session, change);
                    let seen = Seen::default();
                    let mut output = seen.clone();
                    let (result, ()) = tokio::join!(
                        present(
                            &mut owner,
                            request,
                            &approvals,
                            &mut presenter,
                            &mut input,
                            &mut output,
                            std::future::pending::<io::Result<()>>()
                        ),
                        answer(&seen, &lines, false)
                    );
                    assert!(result.unwrap());
                    assert_eq!(receive.await.unwrap()["state"], "denied");
                    assert!(!owner.has_reviewed());
                    let text = seen.text();
                    assert!(text.contains("configured_connection"));
                    assert!(text.contains("not a freshly verified account"));
                    if kind == "stdio_grant" {
                        assert!(text.contains("\"configured_connection\": null"));
                        assert!(!text.contains("configured-mail-subject"));
                    } else {
                        for field in [
                            "configured-mail-subject",
                            "https://identity.example",
                            "oidc-userinfo-v1",
                            "configured-mail-slot",
                            "configured-mail-client",
                            "https://mail.example",
                        ] {
                            assert!(text.contains(field));
                        }
                    }
                    if kind == "connector_grant" {
                        for detail in [
                            "read_messages",
                            "mail.read",
                            "https://read.mail.example",
                            "\"read_only\": true",
                        ] {
                            assert!(text.contains(detail));
                        }
                    } else if kind == "connector_revoke" {
                        assert!(text.contains("\"selected_operation_requirements\": null"));
                        assert!(text.contains("not a restriction on this revoke"));
                        assert!(text.contains(
                            "when executed, even if its account or operation meaning has changed"
                        ));
                        assert!(!text.contains("read_messages"));
                        assert!(!text.contains("mail.read"));
                    } else {
                        assert!(text.contains("\"read_messages\": null"));
                        assert!(
                            text.contains(
                                "no connector declaration is available in this definition"
                            )
                        );
                        assert!(!text.contains("mail.read"));
                        assert!(!text.contains("https://read.mail.example"));
                    }
                    for private_or_unselected in [
                        "private-launch-argument",
                        "private-inline-value",
                        "PRIVATE_INLINE",
                        "PRIVATE_SECRET_ENV",
                        "/bin/true",
                        "send_message",
                        "mail.send",
                        "https://send.mail.example",
                        "verified_account",
                    ] {
                        assert!(!text.contains(private_or_unselected));
                    }
                }
            }),
        )
        .await
        .expect("local refusals and a later review must settle");
    }

    #[tokio::test]
    async fn access_review_rechecks_definition_and_generation_before_queueing() {
        tokio::time::timeout(
            Duration::from_secs(30),
            Box::pin(async {
                for substitution in [
                    "ceiling",
                    "account",
                    "resource",
                    "operation_destination",
                    "operation_scopes",
                    "generation",
                ] {
                    let root = tempfile::tempdir().unwrap();
                    let selected = storage(root.path(), "access-current");
                    save(&selected, &[configured_connection(&selected)]).await;
                    let mut owner = owner(root.path(), selected.clone()).await;
                    let session = SessionId::new();
                    let (approvals, mut presenter) =
                        LocalApprovals::create_paired_host(root.path()).unwrap();
                    approvals.bind_session(&session).unwrap();
                    let (mut input, lines) = protected_terminal();
                    let seen = Seen::default();
                    let mut output = seen.clone();
                    let (request, receive) = request(&session, change("mail", "reader"));
                    let human = async {
                        while !seen.text().contains("Type 'approve ") {
                            tokio::task::yield_now().await;
                        }
                        let text = seen.text();
                        assert!(text.contains("configured-mail-subject"));
                        assert!(text.contains("https://mail.example"));
                        assert!(text.contains("https://read.mail.example"));
                        assert!(text.contains("mail.read"));
                        let mut changed = configured_connection(&selected);
                        match substitution {
                            "ceiling" => {
                                changed
                                    .access
                                    .as_mut()
                                    .unwrap()
                                    .allowed_operations
                                    .remove("read_messages");
                            }
                            "account" => {
                                changed.connector.as_mut().unwrap().account =
                                    "substituted-subject".into()
                            }
                            "resource" => {
                                changed.connector.as_mut().unwrap().resource =
                                    "https://other.example".into()
                            }
                            "operation_destination" => {
                                changed
                                    .connector
                                    .as_mut()
                                    .unwrap()
                                    .operations
                                    .get_mut("read_messages")
                                    .unwrap()
                                    .destination_origin = "https://other-api.example".into()
                            }
                            "operation_scopes" => {
                                changed
                                    .connector
                                    .as_mut()
                                    .unwrap()
                                    .operations
                                    .get_mut("read_messages")
                                    .unwrap()
                                    .required_scopes = ["broader.scope".into()].into()
                            }
                            "generation" => {}
                            _ => unreachable!(),
                        }
                        let mut configs = vec![changed];
                        if substitution == "generation" {
                            configs.push(config("unrelated-calendar"));
                        }
                        save(&selected, &configs).await;
                        answer(&seen, &lines, true).await;
                    };
                    let (result, ()) = tokio::join!(
                        present(
                            &mut owner,
                            request,
                            &approvals,
                            &mut presenter,
                            &mut input,
                            &mut output,
                            std::future::pending::<io::Result<()>>()
                        ),
                        human
                    );
                    assert!(result.unwrap());
                    assert_eq!(
                        receive.await.unwrap()["reason"],
                        "owned_definition_changed_after_review"
                    );
                    assert!(!owner.has_reviewed());
                }
            }),
        )
        .await
        .expect("stale protected review must settle locally");
    }

    // Compare only the controlled fixture's credential artifact bytes. There
    // is no native grant-file inspection, account login or credential decoding.
    fn credential_artifact(path: &Path) -> BTreeMap<std::path::PathBuf, Vec<u8>> {
        fn capture(path: &Path, result: &mut BTreeMap<std::path::PathBuf, Vec<u8>>) {
            let metadata = std::fs::symlink_metadata(path).unwrap();
            assert!(!metadata.file_type().is_symlink());
            if metadata.is_dir() {
                for entry in std::fs::read_dir(path).unwrap() {
                    capture(&entry.unwrap().path(), result);
                }
            } else {
                assert!(metadata.is_file());
                result.insert(path.to_owned(), std::fs::read(path).unwrap());
            }
        }
        let mut result = BTreeMap::new();
        capture(path, &mut result);
        result
    }

    #[tokio::test]
    async fn revoke_review_keeps_exact_target_after_account_or_unrelated_generation_change_without_effects()
     {
        for substitution in ["account", "unrelated_generation"] {
            // Native fixture effects are awaited outside observer timeouts.
            let root = tempfile::tempdir().unwrap();
            let selected = storage(root.path(), "access-revoke-current");
            save(&selected, &[config("mail")]).await;
            govern(&selected).await;
            let existing = finish(reviewed(&selected, change("mail", "reader")).await).await;
            let Outcome::Grant(Ok(existing)) = existing.outcome else {
                panic!("fixture selected grant required")
            };
            let mut saved = configured_connection(&selected);
            if let meerkat_core::mcp_config::McpTransportConfig::Stdio(stdio) = &mut saved.transport
            {
                stdio.secret_env.insert("FIXTURE_KEY".into());
            }
            save(&selected, &[saved.clone()]).await;
            let persistence = ProviderAuthPersistence::new(
                Arc::new(FileTokenStore::new(root.path().join("credentials"))),
                Arc::new(FileLockCoordinator::new(
                    root.path().join("credential-locks"),
                )),
            );
            meerkat::set_mcp_service_secret(
                &selected,
                &persistence,
                &saved,
                "FIXTURE_KEY",
                "controlled-revoke-key",
            )
            .await
            .unwrap();
            let (_, reviewed_generation) = owned(&selected).await.unwrap();
            let mut owner = governed_owner(root.path(), selected.clone()).await;
            let session = SessionId::new();
            let (approvals, mut presenter) =
                LocalApprovals::create_paired_host(root.path()).unwrap();
            approvals.bind_session(&session).unwrap();
            let (mut input, lines) = protected_terminal();
            let seen = Seen::default();
            let mut output = seen.clone();
            let target = Change::Revoke {
                service: "mail".into(),
                agent: "reader".into(),
            };
            let (request, receive) = request(&session, target.clone());
            let (stop, stopped) = oneshot::channel();
            let mut stop = Some(stop);
            let interrupt = async {
                stopped
                    .await
                    .map_err(|_| io::Error::other("fixture stop owner dropped"))?;
                Ok(())
            };
            // The observer borrows this retained completion. It never owns a
            // native fixture mutation; timeout cleanup drives this same review.
            let mut presenting = Box::pin(present(
                &mut owner,
                request,
                &approvals,
                &mut presenter,
                &mut input,
                &mut output,
                interrupt,
            ));
            let opened=tokio::time::timeout(Duration::from_secs(30),async {
                tokio::select! {biased;
                    result=presenting.as_mut()=>Some(result),
                    _=async {while !seen.text().contains("Type 'approve ") {tokio::task::yield_now().await;}}=>None,
                }
            }).await;
            match opened {
                Ok(None) => {}
                Ok(Some(_)) => panic!("access presenter ended before protected review"),
                Err(_) => {
                    let _ = stop.take().unwrap().send(());
                    let _settled = presenting.as_mut().await;
                    panic!("protected review did not open before its observation deadline");
                }
            }
            assert!(seen.text().contains("configured-mail-subject"));
            assert!(seen.text().contains("not a restriction on this revoke"));
            let mut changed = saved.clone();
            if substitution == "account" {
                changed.connector.as_mut().unwrap().account = "replacement-mail-subject".into();
            }
            let mut configs = vec![changed];
            if substitution == "unrelated_generation" {
                configs.push(config("unrelated-calendar"));
            }
            // The presenter is suspended while this native mutation settles;
            // neither a presenter timeout nor its panic can drop this write.
            save(&selected, &configs).await;
            let before = owned(&selected).await.unwrap();
            assert!(before.1 > reviewed_generation);
            if substitution == "unrelated_generation" {
                assert_eq!(
                    before.0.iter().find(|server| server.name == "mail"),
                    Some(&saved)
                );
            }
            let credentials = credential_artifact(&root.path().join("credentials"));
            assert!(!credentials.is_empty());
            answer(&seen, &lines, true).await;
            let result =
                match tokio::time::timeout(Duration::from_secs(30), presenting.as_mut()).await {
                    Ok(result) => result,
                    Err(_) => {
                        let _ = stop.take().unwrap().send(());
                        let _settled = presenting.as_mut().await;
                        panic!("approved review did not settle before its observation deadline");
                    }
                };
            drop(presenting);
            assert!(result.unwrap());
            let response = receive.await.unwrap();
            assert_eq!(response["state"], "pending");
            assert_eq!(response["native_action_entered"], false);
            assert_eq!(
                response["reason"],
                "reviewed_access_waiting_for_setup_close"
            );
            let Some(ReviewedAction::Access(pending)) = owner.take_reviewed_action() else {
                panic!("fresh target review required")
            };
            assert_eq!(pending.storage.locator, selected.locator);
            assert_eq!(pending.session, session);
            assert_eq!(
                serde_json::to_value(&pending.change).unwrap(),
                serde_json::to_value(&target).unwrap()
            );
            assert_eq!(pending.config, saved);
            assert_eq!(pending.generation, reviewed_generation);
            assert_eq!(pending.decision.status, ApprovalStatus::Approved);
            assert!(approvals.check_session(&session).is_ok());
            assert_eq!(owned(&selected).await.unwrap(), before);
            assert!(credential_artifact(&root.path().join("credentials")) == credentials);
            // As main does after setup closes, drop the terminal owner and its
            // realm execution custody before native administration.
            drop(owner);
            // The queued action has not been finalized. A separate public
            // native revoke, awaited outside an observer timeout, proves its
            // original target selection survived review unchanged.
            let probe = finish(reviewed(&selected, target).await).await;
            let Outcome::Revoke(Ok(probe)) = probe.outcome else {
                panic!("existing native selection must remain")
            };
            assert_eq!(reference(&probe.revoked), reference(&existing.issued));
            assert_eq!(probe.outcome, RevocationOutcome::Revoked);
            assert_eq!(owned(&selected).await.unwrap(), before);
            assert!(credential_artifact(&root.path().join("credentials")) == credentials);
            drop(pending);
        }
    }

    #[tokio::test]
    async fn revoke_post_review_boundary_observes_ready_stop_caller_loss_and_session_retirement() {
        for boundary in ["interrupt", "listener_error", "caller", "session"] {
            let root = tempfile::tempdir().unwrap();
            let selected = storage(root.path(), "access-revoke-boundary");
            save(&selected, &[configured_connection(&selected)]).await;
            let mut owner = owner(root.path(), selected.clone()).await;
            let session = SessionId::new();
            let (approvals, mut presenter) =
                LocalApprovals::create_paired_host(root.path()).unwrap();
            approvals.bind_session(&session).unwrap();
            let (mut input, lines) = protected_terminal();
            let seen = Seen::default();
            let mut output = seen.clone();
            let (request, receive) = request(
                &session,
                Change::Revoke {
                    service: "mail".into(),
                    agent: "reader".into(),
                },
            );
            let before = owned(&selected).await.unwrap();
            let mut receive = Some(receive);
            let mut post_decision_polls = 0;
            let interrupt = std::future::poll_fn(|_| {
                if !seen
                    .text()
                    .contains("Decision recorded. Waiting for the native outcome.")
                {
                    return std::task::Poll::Pending;
                }
                post_decision_polls += 1;
                // Let review consume the approved decision on its first
                // biased poll, then act at the presenter's queue boundary.
                if post_decision_polls == 1 {
                    return std::task::Poll::Pending;
                }
                match boundary {
                    "interrupt" => std::task::Poll::Ready(Ok(())),
                    "listener_error" => std::task::Poll::Ready(Err(io::Error::other(
                        "controlled stop listener failure",
                    ))),
                    "caller" => {
                        drop(receive.take());
                        std::task::Poll::Pending
                    }
                    "session" => {
                        approvals.retire();
                        std::task::Poll::Pending
                    }
                    _ => unreachable!(),
                }
            });
            // Native fixture setup is complete. This timeout observes only
            // the presenter and scripted answer, never a native finalizer.
            let (result, ()) = tokio::time::timeout(Duration::from_secs(30), async {
                tokio::join!(
                    present(
                        &mut owner,
                        request,
                        &approvals,
                        &mut presenter,
                        &mut input,
                        &mut output,
                        interrupt
                    ),
                    answer(&seen, &lines, true)
                )
            })
            .await
            .expect("post-review boundary observation must settle");
            assert_eq!(post_decision_polls, 2);
            assert!(!owner.has_reviewed());
            assert_eq!(owned(&selected).await.unwrap(), before);
            match boundary {
                "interrupt" => {
                    assert!(!result.unwrap());
                    assert!(receive.unwrap().await.is_err());
                }
                "listener_error" => {
                    assert!(result.is_err());
                    assert!(receive.unwrap().await.is_err());
                }
                "caller" => {
                    assert!(result.unwrap());
                    assert!(receive.is_none());
                }
                "session" => {
                    assert!(result.unwrap());
                    assert_eq!(
                        receive.unwrap().await.unwrap()["reason"],
                        "review_owner_closed"
                    );
                }
                _ => unreachable!(),
            }
        }
    }

    #[tokio::test]
    async fn pending_access_uses_single_review_slot_and_never_replays_prior_installation() {
        tokio::time::timeout(
            Duration::from_secs(30),
            Box::pin(async {
                let root = tempfile::tempdir().unwrap();
                let selected = storage(root.path(), "access-pending");
                save(&selected, &[config("mail")]).await;
                let mut owner = owner(root.path(), selected.clone()).await;
                let session = SessionId::new();
                let old = Arc::new(FinalizedInstallation::new(
                    FinalizationTarget {
                        realm: selected.locator.clone(),
                        server: "mail".into(),
                        agent: "reader".into(),
                        job_id: "older-installation".into(),
                        session: session.clone(),
                    },
                    FinalizationOutcome::PublicReused {
                        generation: 7,
                        grant: Err(StockGrantError::CommandUncertain),
                    },
                ));
                owner.prior_finalizations = vec![old.clone()].into();
                assert!(owner.target_unresolved("mail"));
                let (approvals, mut presenter) =
                    LocalApprovals::create_paired_host(root.path()).unwrap();
                approvals.bind_session(&session).unwrap();
                let (mut input, lines) = protected_terminal();
                let seen = Seen::default();
                let mut output = seen.clone();
                let (first, receive) = request(&session, change("mail", "reader"));
                let (result, ()) = tokio::join!(
                    present(
                        &mut owner,
                        first,
                        &approvals,
                        &mut presenter,
                        &mut input,
                        &mut output,
                        std::future::pending::<io::Result<()>>()
                    ),
                    answer(&seen, &lines, true)
                );
                assert!(result.unwrap());
                assert_eq!(
                    receive.await.unwrap()["reason"],
                    "reviewed_access_waiting_for_setup_close"
                );
                assert!(owner.has_reviewed());
                assert!(owner.take_reviewed().is_none());
                assert!(owner.has_reviewed());
                let (second, receive) = request(
                    &session,
                    Change::Revoke {
                        service: "mail".into(),
                        agent: "reader".into(),
                    },
                );
                assert!(
                    present(
                        &mut owner,
                        second,
                        &approvals,
                        &mut presenter,
                        &mut input,
                        &mut output,
                        std::future::pending::<io::Result<()>>()
                    )
                    .await
                    .unwrap()
                );
                assert_eq!(
                    receive.await.unwrap()["reason"],
                    "reviewed_finalization_waiting_for_setup_close"
                );
                assert!(matches!(
                    owner.take_reviewed_action(),
                    Some(ReviewedAction::Access(_))
                ));
                assert!(Arc::ptr_eq(&owner.prior_finalizations[0], &old));
                assert!(owner.target_unresolved("mail"));
            }),
        )
        .await
        .expect("one current action and old independent uncertainty must coexist");
    }

    #[tokio::test]
    async fn lost_access_requester_never_retains_a_reviewed_action() {
        let root = tempfile::tempdir().unwrap();
        let selected = storage(root.path(), "access-lost");
        save(&selected, &[config("mail")]).await;
        let mut owner = owner(root.path(), selected).await;
        let session = SessionId::new();
        let (approvals, mut presenter) = LocalApprovals::create_paired_host(root.path()).unwrap();
        approvals.bind_session(&session).unwrap();
        let (_lines, receive_lines) = mpsc::channel(1);
        let mut input = TerminalInput::scripted(receive_lines);
        let mut output = Vec::new();
        let (request, receive) = request(&session, change("mail", "reader"));
        drop(receive);
        assert!(
            present(
                &mut owner,
                request,
                &approvals,
                &mut presenter,
                &mut input,
                &mut output,
                std::future::pending::<io::Result<()>>()
            )
            .await
            .unwrap()
        );
        assert!(!owner.has_reviewed());
        assert!(output.is_empty());
    }

    #[tokio::test]
    async fn fresh_native_access_finishes_saved_secret_service_without_install_or_credential_change()
     {
        tokio::time::timeout(
            Duration::from_secs(30),
            Box::pin(async {
                let root = tempfile::tempdir().unwrap();
                let selected = storage(root.path(), "access-partial");
                let mut saved = config("mail");
                if let meerkat_core::mcp_config::McpTransportConfig::Stdio(stdio) =
                    &mut saved.transport
                {
                    stdio.secret_env.insert("FIXTURE_KEY".into());
                }
                save(&selected, &[saved.clone()]).await;
                govern(&selected).await;
                let persistence = ProviderAuthPersistence::new(
                    Arc::new(FileTokenStore::new(root.path().join("credentials"))),
                    Arc::new(FileLockCoordinator::new(
                        root.path().join("credential-locks"),
                    )),
                );
                meerkat::set_mcp_service_secret(
                    &selected,
                    &persistence,
                    &saved,
                    "FIXTURE_KEY",
                    "controlled-key-value",
                )
                .await
                .unwrap();
                let source = meerkat::RealmMcpServiceSecrets::from_realm_source(
                    crate::realm::config_source(&selected).unwrap(),
                    selected.locator.realm.clone(),
                    persistence.token_store().clone(),
                );
                let before = crate::realm::read_own_mcp_config(&selected).await.unwrap();
                let result = finish(reviewed(&selected, change("mail", "reader")).await).await;
                assert!(matches!(&result.outcome, Outcome::Grant(Ok(_))));
                assert_eq!(
                    crate::realm::read_own_mcp_config(&selected).await.unwrap(),
                    before
                );
                let observed = source.observe(&saved).await.unwrap();
                assert_eq!(
                    observed.slots.get("FIXTURE_KEY"),
                    Some(&meerkat_core::auth::McpServiceSecretPresence::Stored)
                );
                assert_eq!(result.notice()["credentials_changed"], false);
                assert_eq!(result.notice()["configuration_changed"], false);
                assert!(result.notice().get("source_job").is_none());
            }),
        )
        .await
        .expect("real exact reviewed issuance on existing saved config must settle");
    }

    #[tokio::test]
    async fn native_access_refuses_changed_definition_or_generation_after_close_review() {
        tokio::time::timeout(
            Duration::from_secs(30),
            Box::pin(async {
                for different_definition in [false, true] {
                    let root = tempfile::tempdir().unwrap();
                    let selected = storage(root.path(), "access-conflict");
                    save(&selected, &[config("mail")]).await;
                    govern(&selected).await;
                    let reviewed = reviewed(&selected, change("mail", "reader")).await;
                    let mut saved = config("mail");
                    if different_definition {
                        saved
                            .access
                            .as_mut()
                            .unwrap()
                            .allowed_operations
                            .remove("read_messages");
                    }
                    save(&selected, &[saved]).await;
                    let before = crate::realm::read_own_mcp_config(&selected).await.unwrap();
                    let result = finish(reviewed).await;
                    assert!(matches!(
                        &result.outcome,
                        Outcome::Grant(Err(StockGrantError::Refused(
                            StockGovernanceError::InstallationChanged(_)
                        )))
                    ));
                    assert_eq!(
                        crate::realm::read_own_mcp_config(&selected).await.unwrap(),
                        before
                    );
                }
            }),
        )
        .await
        .expect("native definition and generation checks must settle");
    }

    #[tokio::test]
    async fn native_revoke_targets_selection_at_execution_and_preserves_other_targets() {
        tokio::time::timeout(
            Duration::from_secs(60),
            Box::pin(async {
                let root = tempfile::tempdir().unwrap();
                let selected = storage(root.path(), "access-revoke");
                save(&selected, &[config("mail"), config("calendar")]).await;
                govern(&selected).await;
                let first = finish(reviewed(&selected, change("mail", "reader")).await).await;
                let Outcome::Grant(Ok(first)) = first.outcome else {
                    panic!("first native grant required")
                };
                let captured = reviewed(
                    &selected,
                    Change::Revoke {
                        service: "mail".into(),
                        agent: "reader".into(),
                    },
                )
                .await;
                let other_agent =
                    finish(reviewed(&selected, change("mail", "other-agent")).await).await;
                let other_service =
                    finish(reviewed(&selected, change("calendar", "reader")).await).await;
                let Outcome::Grant(Ok(other_agent)) = other_agent.outcome else {
                    panic!("other agent grant required")
                };
                let Outcome::Grant(Ok(other_service)) = other_service.outcome else {
                    panic!("other service grant required")
                };
                let replacement = finish(reviewed(&selected, change("mail", "reader")).await).await;
                let Outcome::Grant(Ok(replacement)) = replacement.outcome else {
                    panic!("replacement grant required")
                };
                assert_ne!(reference(&replacement.issued), reference(&first.issued));
                let other_realm = storage(root.path(), "isolated-access-revoke");
                save(&other_realm, &[config("mail")]).await;
                govern(&other_realm).await;
                let isolated = finish(reviewed(&other_realm, change("mail", "reader")).await).await;
                let Outcome::Grant(Ok(isolated)) = isolated.outcome else {
                    panic!("isolated realm grant required")
                };
                let before = crate::realm::read_own_mcp_config(&selected).await.unwrap();
                let revoked = finish(captured).await;
                let Outcome::Revoke(Ok(revoked)) = revoked.outcome else {
                    panic!("target revoke outcome required")
                };
                assert_eq!(reference(&revoked.revoked), reference(&replacement.issued));
                assert_eq!(revoked.outcome, RevocationOutcome::Revoked);
                // Check independent selections through public mutations and their
                // actual returned references, never by reading internal grant files.
                for (service, agent, expected) in [
                    ("mail", "other-agent", other_agent.issued),
                    ("calendar", "reader", other_service.issued),
                ] {
                    let result = finish(
                        reviewed(
                            &selected,
                            Change::Revoke {
                                service: service.into(),
                                agent: agent.into(),
                            },
                        )
                        .await,
                    )
                    .await;
                    let Outcome::Revoke(Ok(result)) = result.outcome else {
                        panic!("independent target remained selectable")
                    };
                    assert_eq!(reference(&result.revoked), reference(&expected));
                    assert_eq!(result.outcome, RevocationOutcome::Revoked);
                }
                let result = finish(
                    reviewed(
                        &other_realm,
                        Change::Revoke {
                            service: "mail".into(),
                            agent: "reader".into(),
                        },
                    )
                    .await,
                )
                .await;
                let Outcome::Revoke(Ok(result)) = result.outcome else {
                    panic!("other realm selection remained available")
                };
                assert_eq!(reference(&result.revoked), reference(&isolated.issued));
                assert_eq!(result.outcome, RevocationOutcome::Revoked);
                assert_eq!(
                    crate::realm::read_own_mcp_config(&selected).await.unwrap(),
                    before
                );
            }),
        )
        .await
        .expect("native exact target issuance and independent revocations must settle");
    }

    #[tokio::test]
    async fn access_wrapper_preentry_stop_and_notice_are_not_installation_history() {
        let root = tempfile::tempdir().unwrap();
        let selected = storage(root.path(), "access-stop");
        save(&selected, &[config("mail")]).await;
        let pending = ReviewedAction::Access(Box::new(
            reviewed(&selected, change("mail", "reader")).await,
        ));
        let interrupt = std::future::ready(Ok(()));
        tokio::pin!(interrupt);
        assert!(
            pending
                .finalize_outcome_unless_cancelled(interrupt.as_mut())
                .await
                .unwrap()
                .is_none()
        );
        let session = SessionId::new();
        let mut access = FinalizedAccess::new(
            selected.locator.clone(),
            session.clone(),
            change("mail", "reader"),
            Outcome::Grant(Err(StockGrantError::CommandUncertain)),
            None,
        );
        let notice = access.take_continuation_notice().unwrap();
        assert!(access.take_continuation_notice().is_none());
        assert!(notice.belongs_to(&selected.locator, &session));
        assert!(!notice.belongs_to(&selected.locator, &SessionId::new()));
        let mut action = FinalizedAction {
            outcome: ActionOutcome::Access(Box::new(access)),
            continuation: Some(notice),
        };
        assert!(action.installation().is_none());
        assert!(action.take_continuation_notice().is_some());
        assert!(action.take_continuation_notice().is_none());
    }

    #[tokio::test]
    async fn entered_access_effect_is_joined_before_stop_and_output_failure() {
        // Transport control for the same joined_effect used by both real
        // native calls. Its controlled native-shaped result is not runtime proof.
        let (entered_tx, entered_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let (stop_tx, stop_rx) = oneshot::channel();
        let settled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let settled_effect = settled.clone();
        let mut stopped = None;
        let interrupt = async {
            stop_rx.await.unwrap();
            Ok(())
        };
        tokio::pin!(interrupt);
        let result = {
            let mut effect = Box::pin(secrets::joined_effect(
                interrupt.as_mut(),
                &mut stopped,
                || async move {
                    entered_tx.send(()).unwrap();
                    release_rx.await.unwrap();
                    settled_effect.store(true, std::sync::atomic::Ordering::SeqCst);
                    Err::<StockGrantOutcome, _>(StockGrantError::CommandUncertain)
                },
            ));
            let driver = async {
                entered_rx.await.unwrap();
                stop_tx.send(()).unwrap();
                tokio::task::yield_now().await;
                assert!(!settled.load(std::sync::atomic::Ordering::SeqCst));
                release_tx.send(()).unwrap();
            };
            let (result, ()) = tokio::join!(effect.as_mut(), driver);
            result
        };
        assert!(settled.load(std::sync::atomic::Ordering::SeqCst));
        let result = result.unwrap();
        assert!(matches!(&result, Err(StockGrantError::CommandUncertain)));
        assert!(matches!(&stopped, Some(secrets::Stop::Cancelled)));
        struct Fail;
        impl Write for Fail {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("closed output"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let root = tempfile::tempdir().unwrap();
        let selected = storage(root.path(), "access-settled");
        let outcome = FinalizedAccess::new(
            selected.locator,
            SessionId::new(),
            change("mail", "reader"),
            Outcome::Grant(result),
            stopped,
        );
        assert!(outcome.display(&mut Fail).is_err());
        assert_eq!(outcome.notice()["facts"]["state"], "command_uncertain");
        assert!(outcome.stopped());
        outcome.retain(root.path(), 1).unwrap();
        let retained =
            std::fs::read_to_string(root.path().join("standalone-access-1.txt")).unwrap();
        assert!(retained.contains("command_uncertain"));
        assert!(outcome.retain(root.path(), 1).is_err());
        assert_eq!(outcome.notice()["facts"]["state"], "command_uncertain");
    }

    #[tokio::test]
    async fn access_outcomes_preserve_uncertain_revocation_unselection_and_cleanup_without_error_text()
     {
        let root = tempfile::tempdir().unwrap();
        let selected = storage(root.path(), "access-outcomes");
        save(&selected, &[config("mail")]).await;
        govern(&selected).await;
        let issued = finish(reviewed(&selected, change("mail", "reader")).await).await;
        let Outcome::Grant(Ok(issued)) = issued.outcome else {
            panic!("a real native reference is required")
        };
        let reference = issued.issued;
        // Controlled native result variants with an actual identifier exercise
        // projection only, not the runtime production of those failure modes.
        let private = "private-native-diagnostic";
        let grant_cases = [
            (
                StockGrantError::Refused(StockGovernanceError::Destination(private.into())),
                "refused",
            ),
            (
                StockGrantError::NotSelected {
                    issued: reference.clone(),
                    reason: private.into(),
                },
                "issued_not_selected",
            ),
            (
                StockGrantError::SelectionUncertain {
                    issued: reference.clone(),
                    previous: Some(reference.clone()),
                    reason: private.into(),
                },
                "selection_durability_uncertain",
            ),
            (StockGrantError::CommandUncertain, "command_uncertain"),
        ];
        for (error, state) in grant_cases {
            let facts = outcome_facts(&Outcome::Grant(Err(error)));
            assert_eq!(facts["state"], state);
            assert!(!facts.to_string().contains(private));
        }
        for revocation in [
            RevocationOutcome::Revoked,
            RevocationOutcome::ControllerInUse,
            RevocationOutcome::Deferred,
            RevocationOutcome::ReferencesUnknown,
            RevocationOutcome::CustodyUnavailable,
            RevocationOutcome::Refused,
            RevocationOutcome::Uncertain,
        ] {
            let confirmed = revocation == RevocationOutcome::Revoked;
            let result = StockRevokeOutcome {
                revoked: reference.clone(),
                outcome: revocation,
                unselected: Err(private.into()),
                cleanup: vec![meerkat::governed_realm::stock::issuance::CleanupAttempt {
                    agent: "reader".into(),
                    grant: reference.clone(),
                    outcome: RevocationOutcome::Deferred,
                }],
            };
            let facts = outcome_facts(&Outcome::Revoke(Ok(result)));
            assert_eq!(facts["revocation_confirmed"], confirmed);
            assert_eq!(facts["unselection"], "returned_error");
            assert_eq!(facts["cleanup"][0]["outcome"], "Deferred");
            assert!(!facts.to_string().contains(private));
        }
        let facts = outcome_facts(&Outcome::Revoke(Err(
            StockGovernanceError::CommandUncertain,
        )));
        assert_eq!(facts["effects"], "not_confirmed");
        assert!(facts.get("revocation_confirmed").is_none());
    }
}
