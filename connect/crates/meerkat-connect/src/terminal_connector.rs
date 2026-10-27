//! Terminal transport for the existing checked-install and native login owners.
//! Returned facts are historical review evidence, never a live grant or account.

use crate::{
    ConnectError, SessionStartup,
    approval::{ApprovalPresenter, LocalApprovals, present_approval},
    connector_auth::{Control, Reply},
    connector_install::InstallationLogin,
    service_install::{CheckedInstalls, InstalledSelection},
    terminal::{TerminalInput, review_text},
};
use async_trait::async_trait;
use futures_util::FutureExt;
use meerkat::governed_realm::stock::{
    StockPrincipals,
    finalize::{
        ReviewedConnector as NativeReviewedConnector, ReviewedConnectorFinalization,
        finalize_reviewed_connector,
    },
    issuance::{
        McpOperations, RevocationOutcome, StockGrantError, StockGrantRequest, StockGrantTarget,
        stock_agent_principal,
    },
};
use meerkat_auth_core::auth_oauth::{
    LoopbackWaitEnd, bind_loopback_callback_with_redirect,
};
use meerkat_core::{
    AgentToolDispatcher, ApprovalActionKind, ApprovalDecision, ApprovalOwnerRef,
    ApprovalPrincipalId, ApprovalProposedAction, ApprovalRecord, ApprovalRequest,
    ApprovalResourceId, ApprovalResourceKind, ApprovalResourceRef, ApprovalRisk, ApprovalStatus,
    McpServerConfig, ResolvedStorage, SessionId, ToolDef, ToolDispatchContext, ToolError,
    ToolMutationClass, ToolResult, mcp_config::McpConnectorOperation, ops::ToolDispatchOutcome,
    types::ToolCallView,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    future::Future,
    io::{self, Write},
    pin::Pin,
    sync::Arc,
    time::{Duration, SystemTime},
};
use tokio::sync::{mpsc, oneshot, watch};

mod access;
mod installed;
mod profiles;
mod login;
mod oauth_management;
mod plain;
mod secrets;
#[cfg(test)]
#[path = "terminal_connector/exit_tests.rs"]
mod exit_tests;

pub use secrets::management::{ServiceCredentialCommand, run_service_credential_command};

pub(crate) const TOOL: &str = "toolkit_connect_service_account";
pub(crate) const PLAIN_TOOL: &str = "toolkit_enable_public_service";
pub(crate) const SECRET_TOOL: &str = "toolkit_enable_secret_service";
pub(crate) const OAUTH_STATUS_TOOL: &str = "toolkit_service_account_status";
pub(crate) const OAUTH_MANAGE_TOOL: &str = "toolkit_manage_service_account";
pub(crate) const SECRET_STATUS_TOOL: &str = "toolkit_service_credential_status";
pub(crate) const SECRET_MANAGE_TOOL: &str = "toolkit_manage_service_credentials";
pub(crate) const ACCESS_INVENTORY_TOOL: &str = "toolkit_service_access_inventory";
pub(crate) const INSTALLED_LIST_TOOL: &str = "toolkit_installed_service_candidates";
pub(crate) const INSTALLED_SELECT_TOOL: &str = "toolkit_select_installed_service";
pub(crate) const ACCESS_MANAGE_TOOL: &str = "toolkit_manage_service_access";
pub(crate) const TOOLS: &[&str] = &[TOOL, PLAIN_TOOL, SECRET_TOOL, SECRET_STATUS_TOOL, SECRET_MANAGE_TOOL, OAUTH_STATUS_TOOL, OAUTH_MANAGE_TOOL, ACCESS_INVENTORY_TOOL, ACCESS_MANAGE_TOOL, INSTALLED_LIST_TOOL, INSTALLED_SELECT_TOOL];
const WINDOW: Duration = Duration::from_secs(120);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Proposal {
    job_id: String,
    connector: crate::stdio::wire::ConnectorTarget,
    operations: BTreeMap<String, McpConnectorOperation>,
    agent: String,
    // Explicit null means until revoked. Missing must not select that lifetime.
    expires_in_ms: Option<u64>,
}

/// Owned handoff from a still-current setup with its original factory.
/// The factory retains credential persistence, not the setup runtime custody.
/// Native administration must re-read config, account and authority after close.
pub struct ReviewedConnector {
    storage: ResolvedStorage,
    factory: meerkat::AgentFactory,
    installed: InstalledSelection,
    document: Arc<Value>,
    target: meerkat::ConnectorOAuthTarget,
    account: meerkat::ConnectorVerifiedAccount,
    config: McpServerConfig,
    expected_generation: u64,
    client_profile_binding: Option<String>,
    request: StockGrantRequest,
    decision: ApprovalRecord,
}

impl ReviewedConnector {
    /// Call the native finalizer only after the setup and its input owners close.
    /// The native result, not login/review completion, determines the message.
    pub async fn finalize_after_close<W: Write>(self, output: &mut W) -> Result<(), ConnectError> {
        self.finalize_outcome_after_close().await.display(output)
    }

    async fn finalize_outcome_after_close(self) -> FinalizedInstallation {
        let interrupt = std::future::pending::<io::Result<()>>();
        tokio::pin!(interrupt);
        self.finalize_outcome_with_interrupt(interrupt.as_mut()).await
    }

    async fn finalize_outcome_with_interrupt<I: Future<Output = io::Result<()>>>(self,
        mut interrupt: Pin<&mut I>) -> FinalizedInstallation {
        let target = FinalizationTarget::new(&self.storage, &self.config, &self.request,
            &self.installed.job_id, &self.installed.session);
        let mut stopped = None;
        let outcome = self.finalize_native_after_close(interrupt.as_mut(), &mut stopped).await
            .unwrap_or_else(FinalizationOutcome::BeforeAdministrationRefused);
        FinalizedInstallation::new_with_stop(target, outcome, stopped)
    }

    async fn finalize_native_after_close<I: Future<Output = io::Result<()>>>(self,
        interrupt: Pin<&mut I>, stopped: &mut Option<secrets::Stop>) -> Result<FinalizationOutcome, ConnectError> {
        if self.decision.status != ApprovalStatus::Approved
            || self.installed.server_name != self.config.name
        {
            return Err(unavailable(
                "The exact approved installation is required. Nothing was finalized.",
            ));
        }
        plain::recheck_retained(&self.installed, &self.document, None).await?;
        let review = native_finalization_review(
            &self.storage,
            &self.target,
            &self.account,
            &self.config,
            &self.request,
            self.expected_generation,
            self.client_profile_binding.as_deref(),
        )?;
        let principals = StockPrincipals::for_local_operator(&self.storage.locator.realm)
            .map_err(|error| ConnectError::native("Identify the native realm operator", error))?;
        // Reuse the setup factory's exact credential persistence. Native code
        // takes administrative custody before constructing its auth service;
        // no setup runtime or prebuilt auth service is reopened here.
        Ok(settle_connector_effect(interrupt, stopped, || finalize_reviewed_connector(
            &self.storage,
            &principals,
            &self.factory,
            &review,
        )).await)
    }
}

/// Core owns the entire connector finalizer once entered. Only gate its entry;
/// cancellation does not split or abandon that native composite operation.
async fn settle_connector_effect<I, F>(interrupt: Pin<&mut I>, stopped: &mut Option<secrets::Stop>,
    effect: impl FnOnce() -> F) -> FinalizationOutcome
where I: Future<Output = io::Result<()>>, F: Future<Output = ReviewedConnectorFinalization> {
    match secrets::joined_effect(interrupt, stopped, effect).await {
        Some(outcome) => FinalizationOutcome::Connector(outcome),
        None => FinalizationOutcome::BeforeAdministrationStopped,
    }
}

fn native_finalization_review(
    storage: &ResolvedStorage,
    target: &meerkat::ConnectorOAuthTarget,
    account: &meerkat::ConnectorVerifiedAccount,
    config: &McpServerConfig,
    request: &StockGrantRequest,
    expected_generation: u64,
    client_profile_binding: Option<&str>,
) -> Result<NativeReviewedConnector, ConnectError> {
    let StockGrantTarget::Mcp {
        server,
        operations: McpOperations::Exact(operations),
    } = &request.target
    else {
        return Err(unavailable(
            "Finalization requires the one reviewed MCP server and exact operations.",
        ));
    };
    let binding = config
        .connector
        .as_ref()
        .ok_or_else(|| unavailable("The reviewed server has no connector binding."))?;
    if target.client_auth_profile.is_some() != client_profile_binding.is_some()
        || server != &config.name
        || operations.is_empty()
        || operations
            != &binding
                .operations
                .keys()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>()
        || target.slot.realm != storage.locator.realm
        || binding.realm_id != target.slot.realm.as_str()
        || binding.slot_id != target.slot.account.as_str()
        || binding.issuer != target.issuer
        || binding.client != target.client
        || binding.resource != target.resource
        || binding.strategy_id != target.strategy_id
        || account.issuer != target.issuer
        || account.strategy_id != target.strategy_id
        || binding.account != account.subject
        || target
            .account
            .known()
            .is_some_and(|selected| selected != account.subject)
        || target.account.is_unverified()
    {
        return Err(unavailable(
            "The server, realm or operations differ from the exact access review. Nothing was finalized.",
        ));
    }
    Ok(NativeReviewedConnector {
        agent: request.agent.clone(),
        slot: target.slot.clone(),
        account: account.clone(),
        client_profile_binding: client_profile_binding.map(str::to_owned),
        server: config.clone(),
        expected_generation,
        operations: McpOperations::Exact(operations.clone()),
        expires_in_ms: request.expires_in_ms,
    })
}

fn finalization_message(
    server: &str,
    realm: &str,
    agent: &str,
    outcome: ReviewedConnectorFinalization,
) -> Result<String, ConnectError> {
    finalization_message_ref(server, realm, agent, &outcome)
}

fn finalization_message_ref(server: &str, realm: &str, agent: &str,
    outcome: &ReviewedConnectorFinalization,
) -> Result<String, ConnectError> {
    match outcome {
        ReviewedConnectorFinalization::Refused(_) => Err(unavailable(
            "Native finalization was refused. No server configuration or grant was written by finalization."
        )),
        ReviewedConnectorFinalization::PersistUncertain { .. } => Err(unavailable(
            "The server configuration write has an uncertain outcome. No grant was issued. Do not repeat this finalization automatically."
        )),
        ReviewedConnectorFinalization::Persisted { generation, grant } =>
            persisted_grant_message_ref(server, realm, agent, *generation, grant.as_ref()),
    }
}

fn persisted_grant_message(
    server: &str, realm: &str, agent: &str, generation: u64,
    grant: Result<meerkat::governed_realm::stock::issuance::StockGrantOutcome, StockGrantError>,
) -> Result<String, ConnectError> {
    persisted_grant_message_ref(server, realm, agent, generation, grant.as_ref())
}

fn persisted_grant_message_ref(server: &str, realm: &str, agent: &str, generation: u64,
    grant: Result<&meerkat::governed_realm::stock::issuance::StockGrantOutcome, &StockGrantError>,
) -> Result<String, ConnectError> {
    let saved = format!("Saved {server} in realm {realm} at configuration generation {generation}");
    configured_grant_message_ref(&saved, agent, grant)
}

fn configured_grant_message_ref(saved: &str, agent: &str,
    grant: Result<&meerkat::governed_realm::stock::issuance::StockGrantOutcome, &StockGrantError>,
) -> Result<String, ConnectError> {
    match grant {
        Ok(outcome) => {
            let mut message = format!(
                "{saved}; native access is selected for agent {agent}. Open a fresh session in this realm to use the reviewed operations."
            );
            if outcome.cleanup.iter().any(|attempt| attempt.outcome != RevocationOutcome::Revoked) {
                message.push_str(" Previous grant cleanup is incomplete; bounded native references and outcomes were retained.");
            }
            Ok(message)
        }
        Err(StockGrantError::Refused(reason)) => Err(unavailable(&format!(
            "{saved}, but native grant issuance was refused ({}). No new grant was issued or selected.",
            grant_refusal_tag(reason),
        ))),
        Err(StockGrantError::IssuanceUncertain { id }) => Err(unavailable(&format!(
            "{saved}, but issuance of grant {} is uncertain. The selection is unchanged. Do not repeat this finalization automatically.",
            id.as_str()
        ))),
        Err(StockGrantError::NotSelected { issued, .. }) => Err(unavailable(&format!(
            "{saved}; grant {} was issued but not selected. The previous selection is unchanged. Do not repeat this finalization automatically.",
            issued.grant_id.as_str()
        ))),
        Err(StockGrantError::SelectionUncertain {
            issued,
            ..
        }) => Err(unavailable(&format!(
            "{saved}; grant {} was issued and selected, but selection durability is uncertain. A restart may recover either selection. Do not repeat this finalization automatically.",
            issued.grant_id.as_str()
        ))),
        Err(StockGrantError::CommandUncertain) => Err(unavailable(&format!(
            "{saved}, but the grant command did not complete. Whether it issued, selected or revoked a grant is unknown. Do not repeat this finalization automatically."
        ))),
    }
}

/// A protected access review, consumed only after the setup owner closes.
pub enum ReviewedInstallation {
    Account(ReviewedConnector),
    Plain(plain::ReviewedPlainInstallation),
    Secret(secrets::ReviewedSecretInstallation),
}
impl ReviewedInstallation {
    /// Obtain the actual settled result before any terminal rendering. This
    /// value carries observations only, never authority for another attempt.
    pub async fn finalize_outcome_after_close(self) -> FinalizedInstallation {
        match self {
            Self::Account(reviewed) => reviewed.finalize_outcome_after_close().await,
            Self::Plain(reviewed) => reviewed.finalize_outcome_after_close().await,
            Self::Secret(reviewed) => {
                let interrupt = std::future::pending::<io::Result<()>>();
                tokio::pin!(interrupt);
                reviewed.finalize_outcome_after_close(interrupt.as_mut()).await
            },
        }
    }
    /// Use the calling conversation's retained listener after setup has closed.
    /// The caller must retain this future until settlement. A secret installer
    /// joins each entered effect before stopping, without repolling a consumed
    /// listener. The returned result retains cancellation and native outcomes.
    pub async fn finalize_outcome_unless_cancelled<I: Future<Output = io::Result<()>>>(self,
        mut interrupt: Pin<&mut I>) -> Result<Option<FinalizedInstallation>, ConnectError> {
        if let Some(signal) = interrupt.as_mut().now_or_never() {
            signal.map_err(|error| ConnectError::native("Listen for finalization cancellation", error))?;
            return Ok(None);
        }
        Ok(Some(match self {
            Self::Secret(reviewed) => reviewed.finalize_outcome_after_close(interrupt.as_mut()).await,
            Self::Plain(reviewed) => reviewed.finalize_outcome_with_interrupt(interrupt.as_mut()).await,
            Self::Account(reviewed) => reviewed.finalize_outcome_with_interrupt(interrupt.as_mut()).await,
        }))
    }
    pub async fn finalize_after_close<W: Write>(self, output: &mut W) -> Result<(), ConnectError> {
        self.finalize_outcome_after_close().await.display(output)
    }
}

/// One protected terminal action waiting for the existing setup-close boundary.
/// Access-only work carries no installation job or installation outcome.
pub enum ReviewedAction {
    Installation(ReviewedInstallation),
    Access(access::ReviewedAccess),
}
impl From<ReviewedInstallation> for ReviewedAction {
    fn from(value: ReviewedInstallation) -> Self { Self::Installation(value) }
}
impl ReviewedAction {
    pub async fn finalize_outcome_unless_cancelled<I: Future<Output = io::Result<()>>>(self,
        mut interrupt: Pin<&mut I>) -> Result<Option<FinalizedAction>, ConnectError> {
        let (outcome, continuation) = match self {
            Self::Installation(reviewed) => {
                let Some(mut result) = reviewed.finalize_outcome_unless_cancelled(interrupt.as_mut()).await? else { return Ok(None); };
                let notice = result.take_continuation_notice();
                (ActionOutcome::Installation(Arc::new(result)), notice)
            },
            Self::Access(reviewed) => {
                if let Some(signal) = interrupt.as_mut().now_or_never() {
                    signal.map_err(|error| ConnectError::native("Listen before access administration", error))?;
                    return Ok(None);
                }
                let mut result = reviewed.finalize(interrupt.as_mut()).await;
                let notice = result.take_continuation_notice();
                (ActionOutcome::Access(result), notice)
            },
        };
        Ok(Some(FinalizedAction {outcome, continuation}))
    }
}

/// Transport composition only. Native installation and access outcomes remain
/// separate; only actual installation results enter installation replay guards.
pub struct FinalizedAction {
    outcome: ActionOutcome,
    continuation: Option<FinalizationNotice>,
}
enum ActionOutcome {
    Installation(Arc<FinalizedInstallation>),
    Access(access::FinalizedAccess),
}
impl FinalizedAction {
    /// Finish only the CLI invocation. Historical results grant no retry or
    /// current-state authority, and a later success cannot settle an earlier
    /// uncertain effect.
    pub fn finish_invocation(completed: &[Self], result: Result<(), ConnectError>) -> Result<(), ConnectError> {
        // A failure already reports a nonzero exit. Preserve that primary
        // error; a finalization result is not a cleanup failure.
        result?;
        if !completed.iter().any(Self::has_unconfirmed_effect) { return Ok(()); }
        Err(unavailable(
            "A finalized setup action has an unresolved or uncertain outcome. Review the displayed result and any retained setup artifacts. Later successful actions do not settle it; do not repeat it automatically."))
    }
    fn has_unconfirmed_effect(&self) -> bool {
        match &self.outcome {
            ActionOutcome::Installation(value) => value.has_unconfirmed_effect(),
            ActionOutcome::Access(value) => value.has_unconfirmed_effect(),
        }
    }
    pub fn take_continuation_notice(&mut self) -> Option<FinalizationNotice> { self.continuation.take() }
    pub fn installation(&self) -> Option<Arc<FinalizedInstallation>> {
        match &self.outcome { ActionOutcome::Installation(value) => Some(value.clone()), ActionOutcome::Access(_) => None }
    }
    pub fn stopped_during_finalization(&self) -> bool {
        match &self.outcome { ActionOutcome::Installation(value) => value.stopped_during_finalization(), ActionOutcome::Access(value) => value.stopped() }
    }
    pub fn display_for_continuation<W: Write>(&self, output: &mut W) -> Result<(), ConnectError> {
        match &self.outcome { ActionOutcome::Installation(value) => value.display_for_continuation(output), ActionOutcome::Access(value) => value.display(output) }
    }
    pub fn retain(&self, directory: &std::path::Path, ordinal: usize) -> Result<(), ConnectError> {
        match &self.outcome { ActionOutcome::Installation(value) => value.retain(directory, ordinal), ActionOutcome::Access(value) => value.retain(directory, ordinal) }
    }
}

/// This invocation's observed finalizer results. Fields are private so a
/// caller cannot turn a model statement into a native finalization notice.
#[derive(Debug)]
pub struct FinalizedInstallation {
    target: FinalizationTarget,
    outcome: FinalizationOutcome,
    continuation: Option<FinalizationNotice>,
    stopped: Option<secrets::Stop>,
}

/// One process-owned occurrence, constructed at finalizer settlement and moved
/// into native binding at most once. Retaining observation facts cannot mint
/// another header. There is deliberately no Clone or public constructor.
#[derive(Debug)]
pub struct FinalizationNotice {
    realm: meerkat_core::RealmLocator,
    session: SessionId,
    input: meerkat_runtime::Input,
}
impl FinalizationNotice {
    pub(crate) fn belongs_to(&self, realm: &meerkat_core::RealmLocator, session: &SessionId) -> bool {
        &self.realm == realm && &self.session == session
    }
    #[cfg(test)]
    pub(crate) fn input_for_test(&self) -> &meerkat_runtime::Input { &self.input }
    pub(crate) fn into_input(self) -> meerkat_runtime::Input { self.input }
}

#[derive(Debug)]
struct FinalizationTarget {
    realm: meerkat_core::RealmLocator,
    server: String,
    agent: String,
    job_id: String,
    session: SessionId,
}
impl FinalizationTarget {
    fn new(storage: &ResolvedStorage, server: &McpServerConfig, request: &StockGrantRequest,
        job_id: &str, session: &SessionId) -> Self {
        Self { realm: storage.locator.clone(), server: server.name.clone(),
            agent: request.agent.clone(), job_id: job_id.to_owned(), session: session.clone() }
    }
}

/// Preserve each native owner's result. The generic MCP composition does not
/// manufacture the connector finalizer's enum or an observed provider account.
#[derive(Debug)]
enum FinalizationOutcome {
    BeforeAdministrationRefused(ConnectError),
    BeforeAdministrationStopped,
    Connector(ReviewedConnectorFinalization),
    PublicPersistenceError(meerkat_core::mcp_config::McpRealmPersistError),
    PublicPersisted { generation: u64,
        grant: Result<meerkat::governed_realm::stock::issuance::StockGrantOutcome, StockGrantError> },
    PublicReused { generation: u64,
        grant: Result<meerkat::governed_realm::stock::issuance::StockGrantOutcome, StockGrantError> },
    PublicStoppedBeforeGrant { generation: u64, reused: bool },
    Secret(secrets::Outcome),
}

fn uncertain_grant(result: Result<&meerkat::governed_realm::stock::issuance::StockGrantOutcome, &StockGrantError>) -> bool {
    matches!(result, Err(StockGrantError::IssuanceUncertain { .. } | StockGrantError::NotSelected { .. }
        | StockGrantError::SelectionUncertain { .. } | StockGrantError::CommandUncertain))
}

fn unconfirmed_grant_effect(result: Result<&meerkat::governed_realm::stock::issuance::StockGrantOutcome, &StockGrantError>) -> bool {
    match result {
        Ok(outcome) => outcome.cleanup.iter().any(|attempt| attempt.outcome == RevocationOutcome::Uncertain),
        Err(_) => uncertain_grant(result),
    }
}

fn public_persistence_may_have_changed(error: &meerkat_core::mcp_config::McpRealmPersistError) -> bool {
    use meerkat_core::{mcp_config::McpRealmPersistError as Persist, config_runtime::ConfigRuntimeError as Config};
    // Legacy add uses Config errors even after document replacement, so keep
    // that conservative split. The entered API distinguishes a refused entry
    // from an unknown write outcome; neither exposes its diagnostic payload.
    !matches!(error, Persist::ServerExists(_) | Persist::ServerNotFound(_)
        | Persist::EnvReference(_) | Persist::Access(_) | Persist::NotAdmitted(_)
        | Persist::Config(Config::GenerationConflict { .. }))
}

fn grant_refusal_tag(error: &meerkat::governed_realm::stock::StockGovernanceError) -> &'static str {
    use meerkat::governed_realm::stock::StockGovernanceError as Error;
    // Preserve the typed reason category only. Payloads can hold paths,
    // principals, configuration or native diagnostics and stay in memory.
    match error {
        Error::OperatorUnavailable => "OperatorUnavailable",
        Error::Principal(_) => "Principal",
        Error::GrantsUnavailable => "GrantsUnavailable",
        Error::GrantOwner(_) => "GrantOwner",
        Error::ForeignGrantOwner => "ForeignGrantOwner",
        Error::Unsupported(_) => "Unsupported",
        Error::Destination(_) => "Destination",
        Error::Binding(_) => "Binding",
        Error::NotGoverned => "NotGoverned",
        Error::RequesterUnsupported(_) => "RequesterUnsupported",
        Error::CallerNotAdmitted(_) => "CallerNotAdmitted",
        Error::ScopeNotRepresentable(_) => "ScopeNotRepresentable",
        Error::ConfigurationUnavailable => "ConfigurationUnavailable",
        Error::InstallationChanged(_) => "InstallationChanged",
        Error::CommandUncertain => "CommandUncertain",
        Error::Review(_) => "Review",
        Error::Store(_) => "Store",
        Error::Persistence(_) => "Persistence",
        Error::Io(_) => "Io",
    }
}

fn public_persistence_details(error: &meerkat_core::mcp_config::McpRealmPersistError) -> Value {
    use meerkat_core::{mcp_config::McpRealmPersistError as Persist, config_runtime::ConfigRuntimeError as Config};
    // Only variant tags and generation numbers. Error text may contain a
    // credential-bearing configuration line or a local path.
    match error {
        Persist::ServerExists(_) => json!({"kind":"ServerExists"}),
        Persist::ServerNotFound(_) => json!({"kind":"ServerNotFound"}),
        Persist::EnvReference(_) => json!({"kind":"EnvReference"}),
        Persist::Access(_) => json!({"kind":"Access"}),
        Persist::NotAdmitted(_) => json!({"kind":"NotAdmitted"}),
        Persist::OutcomeUnknown(_) => json!({"kind":"OutcomeUnknown"}),
        Persist::Config(error) => json!({"kind":"Config", "config":match error {
            Config::GenerationConflict { expected, current } => json!({"kind":"GenerationConflict","expected":expected,"current":current}),
            Config::LockTimeout => json!({"kind":"LockTimeout"}),
            Config::Io(_) => json!({"kind":"Io"}),
            Config::Json(_) => json!({"kind":"Json"}),
            Config::Config(_) => json!({"kind":"Config"}),
        }}),
    }
}

impl FinalizedInstallation {
    fn new(target: FinalizationTarget, outcome: FinalizationOutcome) -> Self {
        Self::new_with_stop(target, outcome, None)
    }

    fn new_with_stop(target: FinalizationTarget, outcome: FinalizationOutcome,
        stopped: Option<secrets::Stop>) -> Self {
        let mut observed = Self { target, outcome, continuation: None, stopped };
        observed.continuation = Some(FinalizationNotice {
            realm: observed.target.realm.clone(), session: observed.target.session.clone(), input: observed.make_input(),
        });
        observed
    }

    /// Move the one retained occurrence out before sharing the observation.
    /// Calling again returns None, never a fresh native input identity.
    pub fn take_continuation_notice(&mut self) -> Option<FinalizationNotice> {
        self.continuation.take()
    }

    /// The finalizer consumed the original listener and joined its entered
    /// effect. Do not poll that completed listener or start a continuation.
    pub fn stopped_during_finalization(&self) -> bool {
        self.stopped.is_some()
            || matches!(&self.outcome, FinalizationOutcome::Secret(outcome) if outcome.stopped())
    }

    fn has_unconfirmed_effect(&self) -> bool {
        match &self.outcome {
            FinalizationOutcome::BeforeAdministrationRefused(_)
                | FinalizationOutcome::BeforeAdministrationStopped
                | FinalizationOutcome::Connector(ReviewedConnectorFinalization::Refused(_)) => false,
            FinalizationOutcome::Connector(ReviewedConnectorFinalization::PersistUncertain { .. }) => true,
            FinalizationOutcome::Connector(ReviewedConnectorFinalization::Persisted { grant, .. })
                | FinalizationOutcome::PublicPersisted { grant, .. }
                | FinalizationOutcome::PublicReused { grant, .. } => unconfirmed_grant_effect(grant.as_ref()),
            FinalizationOutcome::PublicPersistenceError(error) => public_persistence_may_have_changed(error),
            FinalizationOutcome::PublicStoppedBeforeGrant { .. } => false,
            FinalizationOutcome::Secret(outcome) => outcome.has_unconfirmed_effect(),
        }
    }

    pub(crate) fn belongs_to(&self, realm: &meerkat_core::RealmLocator, session: &SessionId) -> bool {
        &self.target.realm == realm && &self.target.session == session
    }

    fn make_input(&self) -> meerkat_runtime::Input {
        // Native supplies the fresh durable header; the actual input variant
        // and origin identify a process event, never literal human text.
        let mut header = meerkat_runtime::PromptInput::new(String::new(), None).header;
        header.source = meerkat_runtime::InputOrigin::External { source_name: "meerkat-connect-finalization".into() };
        meerkat_runtime::Input::ExternalEvent(meerkat_runtime::ExternalEventInput {
            header, event_type: "connect_finalization_observation".into(),
            // Native's SystemNotice projection reads body. Keep exactly one
            // bounded result body; arbitrary structured fields are not text.
            payload: json!({"body":self.notice().to_string()}), blocks: None,
            handling_mode: meerkat_runtime::HandlingMode::default(), render_metadata: None, objective_id: None,
        })
    }

    fn blocks_reentry(&self, realm: &meerkat_core::RealmLocator, server: &str) -> bool {
        if &self.target.realm != realm || self.target.server != server { return false; }
        match &self.outcome {
            FinalizationOutcome::BeforeAdministrationRefused(_) => false,
            FinalizationOutcome::BeforeAdministrationStopped => false,
            FinalizationOutcome::Connector(ReviewedConnectorFinalization::Refused(_)) => false,
            FinalizationOutcome::Connector(ReviewedConnectorFinalization::PersistUncertain { .. }) => true,
            FinalizationOutcome::Connector(ReviewedConnectorFinalization::Persisted { grant, .. })
                | FinalizationOutcome::PublicPersisted { grant, .. }
                | FinalizationOutcome::PublicReused { grant, .. } => uncertain_grant(grant.as_ref()),
            FinalizationOutcome::PublicPersistenceError(error) => public_persistence_may_have_changed(error),
            FinalizationOutcome::PublicStoppedBeforeGrant { .. } => false,
            FinalizationOutcome::Secret(outcome) => outcome.blocks_reentry(),
        }
    }

    fn message(&self) -> Result<String, ConnectError> {
        let target = &self.target;
        match &self.outcome {
            FinalizationOutcome::BeforeAdministrationRefused(_) => Err(unavailable(
                "Finalization stopped before native administration. No service configuration or grant was written by this finalizer.")),
            FinalizationOutcome::BeforeAdministrationStopped => Err(unavailable(
                "Setup stopped before native administration. No configuration or grant operation was started.")),
            FinalizationOutcome::Connector(outcome) => finalization_message_ref(
                &target.server, target.realm.realm.as_str(), &target.agent, outcome),
            FinalizationOutcome::PublicPersistenceError(error) => Err(unavailable(&format!(
                "The public-service configuration write did not complete ({}). No grant was requested. {}",
                public_persistence_details(error), if public_persistence_may_have_changed(error) { "The write may be partial. Do not repeat this target automatically." }
                else { "The native writer refused before changing the configuration." }))),
            FinalizationOutcome::PublicPersisted { generation, grant } => persisted_grant_message_ref(
                &target.server, target.realm.realm.as_str(), &target.agent, *generation, grant.as_ref()),
            FinalizationOutcome::PublicReused { generation, grant } => configured_grant_message_ref(
                &format!("The exact reviewed public-service configuration for {} in realm {} at generation {} was already saved and was reused. This is a new access request; any prior uncertain grant outcome remains unresolved",
                    target.server, target.realm.realm.as_str(), generation), &target.agent, grant.as_ref()),
            FinalizationOutcome::PublicStoppedBeforeGrant { reused, .. } => Err(unavailable(if *reused {
                "Public-service setup stopped before requesting new access. The existing configuration was not changed; no new grant was requested."
            } else {
                "Public-service setup stopped after the configuration was saved. No new grant was requested. The saved configuration was not rolled back."
            })),
            FinalizationOutcome::Secret(outcome) => outcome.message(target),
        }
    }

    /// Legacy terminal behavior, with the typed result still held by the caller.
    pub fn display<W: Write>(&self, output: &mut W) -> Result<(), ConnectError> {
        let message = self.message()?;
        writeln!(output, "{message}").map_err(|error| ConnectError::native("Display native finalization result", error))
    }

    /// Continuing setup also displays refusal/partial outcomes and lets the
    /// agent address independent work. Failure to print does not consume facts.
    pub fn display_for_continuation<W: Write>(&self, output: &mut W) -> Result<(), ConnectError> {
        let message = self.message().unwrap_or_else(|error| error.to_string());
        writeln!(output, "{message}").map_err(|error| ConnectError::native("Display native finalization result", error))?;
        if let Some(stop) = &self.stopped {
            let message = match stop {
                secrets::Stop::Cancelled => "Cancellation was observed. Any entered operation was joined; no later operation was started.",
                secrets::Stop::ListenerFailed(_) => "Cancellation monitoring failed. Any entered operation was joined; no later operation was started.",
            };
            writeln!(output, "{message}").map_err(|error| ConnectError::native("Display finalization stop", error))?;
            if matches!(stop, secrets::Stop::ListenerFailed(_)) {
                return Err(unavailable("Cancellation monitoring failed during installation finalization. Review the displayed result and any retained setup artifacts."));
            }
        }
        Ok(())
    }

    /// Private invocation evidence, using the existing setup artifact owner.
    /// An interrupted write is not a durable receipt and cannot authorize retry.
    pub fn retain(&self, directory: &std::path::Path, ordinal: usize) -> Result<(), ConnectError> {
        use std::os::unix::fs::OpenOptionsExt;
        let path = directory.join(format!("standalone-finalization-{ordinal}.txt"));
        // Only typed status and native identifier references enter this
        // observation file. Provider errors, account documents, credentials
        // and arbitrary native reason strings are not serialized here.
        let observed = serde_json::to_vec_pretty(&self.private_observation())
            .map_err(|_| unavailable("The finalization observation could not be encoded. The typed result remains held; no retry was started."))?;
        let result = (|| -> io::Result<()> {
            let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)?;
            file.write_all(&observed)?;
            file.sync_all()
        })();
        result.map_err(|_| unavailable(
            "The finalization observation could not be fully retained in the setup artifacts. The typed native result remains held by this invocation; its bounded status is displayed separately. No retry was started."))
    }

    fn private_observation(&self) -> Value {
        let reference = |value: &meerkat_authorization_contracts::work_association::GrantLineageRef| json!({
            "grant_id":value.grant_id, "issued_revision":value.issued_revision,
            "authority_generation":value.authority_generation,
        });
        let references = |result: Result<&meerkat::governed_realm::stock::issuance::StockGrantOutcome, &StockGrantError>| {
            match result {
                Ok(outcome) => json!({"issued":reference(&outcome.issued),
                    "superseded":outcome.superseded.as_ref().map(&reference),
                    "cleanup":outcome.cleanup.iter().map(|attempt| json!({"agent":attempt.agent,
                        "grant_id":attempt.grant.grant_id,"outcome":format!("{:?}",attempt.outcome)})).collect::<Vec<_>>()}),
                Err(StockGrantError::IssuanceUncertain { id }) => json!({"possibly_issued_id":id}),
                Err(StockGrantError::NotSelected { issued, .. }) => json!({"issued":reference(issued)}),
                Err(StockGrantError::SelectionUncertain { issued, previous, .. }) => json!({"issued":reference(issued),
                    "previous":previous.as_ref().map(&reference)}),
                Err(StockGrantError::Refused(_) | StockGrantError::CommandUncertain) => Value::Null,
            }
        };
        let native_references = match &self.outcome {
            FinalizationOutcome::Connector(ReviewedConnectorFinalization::Persisted { grant, .. })
                | FinalizationOutcome::PublicPersisted { grant, .. }
                | FinalizationOutcome::PublicReused { grant, .. } => references(grant.as_ref()),
            FinalizationOutcome::Secret(outcome) => outcome.grant().map_or(Value::Null, |result| references(result.as_ref())),
            _ => Value::Null,
        };
        json!({"scope":"invocation_finalization_observation_only", "observation":self.notice(),
            "native_grant_references":native_references,
            "authority":"This file cannot restore an owner, authorize a call, select a grant or justify retry. Native current state remains authoritative."})
    }

    pub(crate) fn notice(&self) -> Value {
        let grant = |result: Result<&meerkat::governed_realm::stock::issuance::StockGrantOutcome, &StockGrantError>| {
            match result {
                Ok(outcome) => json!({"state":"selected", "prior_cleanup_complete":outcome.cleanup.iter()
                    .all(|attempt| attempt.outcome == RevocationOutcome::Revoked)}),
                Err(StockGrantError::Refused(reason)) => json!({"state":"refused", "reason_kind":grant_refusal_tag(reason), "issued":false, "selection_changed":false}),
                Err(StockGrantError::IssuanceUncertain { .. }) => json!({"state":"issuance_uncertain", "selection_changed":false}),
                Err(StockGrantError::NotSelected { .. }) => json!({"state":"issued_not_selected", "issued":true, "selection_changed":false}),
                Err(StockGrantError::SelectionUncertain { .. }) => json!({"state":"selection_durability_uncertain", "issued":true}),
                Err(StockGrantError::CommandUncertain) => json!({"state":"command_uncertain"}),
            }
        };
        let facts = match &self.outcome {
            FinalizationOutcome::BeforeAdministrationRefused(_) => json!({"persistence":"not_entered", "grant":{"state":"not_requested"}}),
            FinalizationOutcome::BeforeAdministrationStopped => json!({"persistence":"not_entered", "grant":{"state":"not_requested"}}),
            FinalizationOutcome::Connector(ReviewedConnectorFinalization::Refused(
                meerkat::governed_realm::stock::finalize::ReviewedConnectorRefusal::ConfigConflict { expected,current })) =>
                json!({"persistence":"refused", "error":{"kind":"GenerationConflict","expected":expected,"current":current}, "grant":{"state":"not_requested"}}),
            FinalizationOutcome::Connector(ReviewedConnectorFinalization::Refused(_)) => json!({"persistence":"refused", "grant":{"state":"not_requested"}}),
            FinalizationOutcome::Connector(ReviewedConnectorFinalization::PersistUncertain { .. }) => json!({"persistence":"uncertain", "grant":{"state":"not_requested"}}),
            FinalizationOutcome::Connector(ReviewedConnectorFinalization::Persisted { generation, grant: result })
                | FinalizationOutcome::PublicPersisted { generation, grant: result } => json!({"persistence":"saved", "generation":generation, "grant":grant(result.as_ref())}),
            FinalizationOutcome::PublicReused { generation, grant: result } => json!({"persistence":"already_saved",
                "generation":generation, "new_access_request":true, "prior_grant_outcome":"not_resolved", "grant":grant(result.as_ref())}),
            FinalizationOutcome::PublicPersistenceError(error) => json!({"persistence":if public_persistence_may_have_changed(error) {"uncertain"} else {"refused"}, "error":public_persistence_details(error), "grant":{"state":"not_requested"}}),
            FinalizationOutcome::PublicStoppedBeforeGrant { generation, reused } => json!({
                "persistence":if *reused {"already_saved"} else {"saved"},
                "generation":generation, "grant":{"state":"not_requested"}}),
            FinalizationOutcome::Secret(outcome) => outcome.facts(outcome.grant()
                .map_or_else(|| json!({"state":"not_requested"}), |result| grant(result.as_ref()))),
        };
        let mut notice = json!({"kind":"connect_finalization_observation", "realm":self.target.realm.realm.as_str(),
            "server":self.target.server, "consumer_agent":self.target.agent, "source_job":self.target.job_id,
            "facts":facts, "service_readiness":"not_observed",
            "automatic_reentry_blocked":self.blocks_reentry(&self.target.realm,&self.target.server),
            "instruction":"This is the process-owned result of the completed finalizer, not human consent. Continue the original setup task using these exact facts. Do not repeat this finalizer or claim service use. If this target is unresolved, explain that limit and continue independent requested services; a new package/job does not permit replay for the same realm and server."});
        if let Some(stop) = &self.stopped {
            notice["stop"] = json!(match stop {
                secrets::Stop::Cancelled => "cancelled", secrets::Stop::ListenerFailed(_) => "listener_failed",
            });
        }
        notice
    }
}

struct RequestTool {
    installs: Arc<CheckedInstalls>,
    storage: ResolvedStorage,
    requests: mpsc::Sender<ConnectorRequest>,
}

pub enum ConnectorRequest {
    Account(AccountRequest),
    Plain(plain::PlainRequest),
    Secret(secrets::SecretRequest),
    CredentialManagement(secrets::management::Request),
    AccountManagement(oauth_management::Request),
    AccessManagement(access::Request),
    Installed(installed::Request),
}

pub struct AccountRequest {
    installed: InstalledSelection,
    document: Arc<Value>,
    target: meerkat::ConnectorOAuthTarget,
    selection: InstallationLogin,
    grant: StockGrantRequest,
    response: oneshot::Sender<Value>,
}

/// Local review input, not an authentication ceremony or invocation authority.
struct AccountReview {
    installed: InstalledSelection,
    document: Arc<Value>,
    target: meerkat::ConnectorOAuthTarget,
    account: meerkat::ConnectorVerifiedAccount,
    config: McpServerConfig,
    grant: StockGrantRequest,
    response: oneshot::Sender<Value>,
    login_completed: bool,
    client_profile: Option<meerkat::ConnectorClientAuthProfile>,
}

pub struct TerminalConnectors {
    storage: ResolvedStorage,
    factory: meerkat::AgentFactory,
    control: Control,
    auth: Arc<crate::connector_auth::ConnectorAuth>,
    profiles: Arc<meerkat::RealmConnectorClients>,
    requests: mpsc::Receiver<ConnectorRequest>,
    send: mpsc::Sender<ConnectorRequest>,
    reviewed: Option<ReviewedAction>,
    prior_finalizations: Arc<[Arc<FinalizedInstallation>]>,
}

#[derive(Debug, thiserror::Error)]
enum CallbackWaitFailure {
    #[error("native callback retirement did not return its joined receipt")]
    RetirementFailed,
    #[error("native callback returned an unsupported terminal outcome")]
    UnsupportedOutcome,
}

fn unavailable(message: &str) -> ConnectError {
    ConnectError::InvalidInput(message.into())
}
fn refused() -> ToolError {
    ToolError::execution_failed(
        "The exact current installed job and explicit connector/access proposal are required. No sign-in or grant was started.",
    )
}

impl TerminalConnectors {
    pub async fn open(startup: &mut SessionStartup) -> Result<Self, ConnectError> {
        Self::open_with_finalizations(startup, Arc::from([])).await
    }

    pub async fn open_with_finalizations(startup: &mut SessionStartup,
        prior_finalizations: Arc<[Arc<FinalizedInstallation>]>,
    ) -> Result<Self, ConnectError> {
        let storage = startup.storage.clone();
        let factory = startup.factory.clone();
        // One live native source serves both native login/refresh and terminal
        // review. It composes the selected realm on every read.
        let profiles = Arc::new(meerkat::RealmConnectorClients::from_realm_source(
            crate::realm::config_source(&storage)?, storage.locator.realm.clone(),
        ));
        let auth = startup.configure_realm_connectors(profiles.clone()).await?;
        let control = Control::new(auth.clone());
        let (send, requests) = mpsc::channel(1);
        Ok(Self {
            storage,
            factory,
            control,
            auth,
            profiles,
            requests,
            send,
            reviewed: None,
            prior_finalizations,
        })
    }
    fn target_unresolved(&self, server: &str) -> bool {
        self.prior_finalizations.iter().any(|outcome| outcome.blocks_reentry(&self.storage.locator, server))
    }
    pub(crate) fn router(&self) -> meerkat::McpRouter {
        self.control.router()
    }
    pub(crate) fn dispatcher(
        &self,
        installs: Arc<CheckedInstalls>,
    ) -> Arc<dyn AgentToolDispatcher> {
        Arc::new(RequestTool {
            installs,
            storage: self.storage.clone(),
            requests: self.send.clone(),
        })
    }
    pub(crate) fn dispatcher_factory(&self) -> crate::service_preparation::TerminalToolFactory {
        let storage = self.storage.clone();
        let requests = self.send.clone();
        Arc::new(move |installs| Arc::new(RequestTool {
            installs, storage: storage.clone(), requests: requests.clone(),
        }))
    }
    pub async fn next(&mut self) -> Option<ConnectorRequest> {
        self.requests.recv().await
    }
    pub fn has_reviewed(&self) -> bool { self.reviewed.is_some() }
    /// Existing installation-only callers never discard a pending access action.
    pub fn take_reviewed(&mut self) -> Option<ReviewedInstallation> {
        match self.reviewed.take() {
            Some(ReviewedAction::Installation(value)) => Some(value),
            other => { self.reviewed = other; None },
        }
    }
    pub fn take_reviewed_action(&mut self) -> Option<ReviewedAction> { self.reviewed.take() }
    pub fn retire(&mut self) -> Result<(), ConnectError> {
        self.control.retire().map_err(unavailable)
    }

    pub async fn present<F: Future<Output = io::Result<()>> + Send, W: Write>(
        &mut self, request: ConnectorRequest, approvals: &LocalApprovals,
        presenter: &mut ApprovalPresenter, input: &mut TerminalInput,
        output: &mut W, interrupt: F,
    ) -> Result<bool, ConnectError> {
        match request {
            ConnectorRequest::Installed(request) => installed::present(self, request, approvals, interrupt).await,
            ConnectorRequest::Account(request) => self.present_account(
                request, approvals, presenter, input, output, interrupt,
            ).await,
            ConnectorRequest::Plain(request) => plain::present(
                self, request, approvals, presenter, input, output, interrupt,
            ).await,
            ConnectorRequest::Secret(request) => secrets::present(
                self, request, approvals, presenter, input, output, interrupt,
            ).await,
            ConnectorRequest::CredentialManagement(request) => secrets::management::present(
                self, request, approvals, presenter, input, output, interrupt,
            ).await,
            ConnectorRequest::AccountManagement(request) => oauth_management::present(
                self, request, approvals, presenter, input, output, interrupt,
            ).await,
            ConnectorRequest::AccessManagement(request) => access::present(
                self, request, approvals, presenter, input, output, interrupt,
            ).await,
        }
    }

    /// The caller owns this future until native callback/exchange cleanup ends.
    /// Tool disappearance and Ctrl-C retire the exact ceremony, never another slot.
    async fn present_account<F: Future<Output = io::Result<()>> + Send, W: Write>(
        &mut self,
        mut request: AccountRequest,
        approvals: &LocalApprovals,
        presenter: &mut ApprovalPresenter,
        input: &mut TerminalInput,
        output: &mut W,
        interrupt: F,
    ) -> Result<bool, ConnectError> {
        if request.response.is_closed() {
            return Ok(true);
        }
        if self.reviewed.is_some() {
            let _ = request.response.send(json!({"state":"pending", "reason":"reviewed_finalization_waiting_for_setup_close", "grant_issued":false}));
            return Ok(true);
        }
        if self.target_unresolved(&request.installed.server_name) {
            return selection_refused("prior_finalization_unresolved", request.response, output);
        }
        if let Err(reason) = request.selection.check_current() {
            return selection_refused(reason, request.response, output);
        }
        if let Err(reason) = request_documents(plain::recheck_retained(
            &request.installed, &request.document, None,
        )).await {
            return selection_refused(reason, request.response, output);
        }
        if let Err(reason) = request.selection.check_current() {
            return selection_refused(reason, request.response, output);
        }
        if request.response.is_closed() { return Ok(true); }
        tokio::pin!(interrupt);
        let prepared = profiles::prepare(
            &self.storage, &self.factory, &self.profiles, &mut request.target,
            &request.installed.session, approvals, presenter, input, output,
            &mut request.response, || request.selection.check_current(), interrupt.as_mut(),
        ).await;
        let client_profile = match profiles::presentation(prepared, request.response, output)? {
            profiles::Presentation::Current(profile, response) => { request.response = response; profile }
            profiles::Presentation::Finished(keep_turn) => return Ok(keep_turn),
        };
        if let Err(reason) = request.selection.check_target(&request.target) {
            return selection_refused(reason, request.response, output);
        }
        let status = self
            .auth
            .facade
            .connector_status(&request.target.slot)
            .await
            .map_err(|error| ConnectError::provider_setup("Observe connector sign-in", error))?;
        if status.phase == meerkat::ConnectorAuthPhase::Authorized {
            // Reuse the native observation without starting an OAuth ceremony.
            // The same account-access review and native finalizer remain required.
            let installation = match request.selection.join_authorized(&request.target, &status) {
                Ok(installation) => installation,
                Err(reason) => return selection_refused(reason, request.response, output),
            };
            let account = match crate::connector_install::authorized_account(&request.target, &status) {
                Ok(account) => account.clone(),
                Err(reason) => return selection_refused(reason, request.response, output),
            };
            let config = match reviewed_installation_configuration(&installation) {
                Ok(config) => config,
                Err(reason) => return selection_refused(reason, request.response, output),
            };
            let selection = request.selection;
            return self
                .review_account(
                    AccountReview {
                        installed: request.installed,
                        document: request.document,
                        target: request.target,
                        account,
                        config,
                        grant: request.grant,
                        response: request.response,
                        login_completed: false,
                        client_profile,
                    },
                    approvals,
                    presenter,
                    input,
                    output,
                    || selection.check_current(),
                    interrupt.as_mut(),
                )
                .await;
        }
        if request.response.is_closed() {
            return Ok(true);
        }
        // The terminal owns the target proposal. Its exact descriptor is shown
        // before discovery/sign-in; authorization material never goes to tools.
        let login_review = review_request(
            &request.installed.session,
            "Sign in for this installed integration",
            json!({"phase":"sign_in_only", "realm":self.storage.locator.realm.as_str(),
                "job_id":request.installed.job_id, "server":request.installed.server_name,
                "issuer":request.target.issuer, "client":request.target.client,
                "resource":request.target.resource, "scopes":request.target.scopes,
                "account_selection":request.target.account,
                "client_profile":profiles::display(client_profile.as_ref()),
                "notice":"This permits native sign-in only. Account access is reviewed after the native account is verified."}),
        )?;
        let decision = match review(
            approvals,
            presenter,
            input,
            output,
            login_review,
            &mut request.response,
            interrupt.as_mut(),
        )
        .await? {
            ReviewOutcome::Decided(decision) => decision,
            ReviewOutcome::CallerGone => return Ok(true),
            ReviewOutcome::Stopped => return Ok(false),
        };
        if decision.status != ApprovalStatus::Approved {
            let _ = request
                .response
                .send(json!({"state":"denied", "phase":"sign_in", "grant_issued":false}));
            return Ok(true);
        }
        if request.response.is_closed() {
            return Ok(true);
        }
        if let Err(error) = profiles::recheck(&self.profiles, &request.target, client_profile.as_ref()).await {
            return error.respond(request.response, output);
        }
        if let Err(reason) = request.selection.check_current() {
            return selection_refused(reason, request.response, output);
        }
        if request.response.is_closed() { return Ok(true) }
        let login = login::run(&mut self.control, request.target.clone(),
            crate::connector_auth::LoginSelection::Installed(Arc::new(request.selection)),
            &mut request.response, input, output, interrupt.as_mut()).await?;
        if let Some(reply) = login.stop_reply() {
            writeln!(output, "{}", login.stop_message())
                .map_err(|e| ConnectError::native("Display sign-in settlement", e))?;
            let _ = request.response.send(reply);
            return Ok(matches!(login, login::End::Stopped { reason:login::Stop::Cancelled | login::Stop::CallerGone, .. }));
        }
        let completed = match login {
            login::End::Completed(completed) => completed,
            login::End::Refused(code) => {
                let _ = request.response.send(json!({"state":"unavailable", "reason":code, "grant_issued":false}));
                return Ok(true);
            },
            login::End::Stopped { .. } => unreachable!("stopped ceremony was settled above"),
        };
        let config = match reviewed_configuration(&completed) {
            Ok(config) => config,
            Err(reason) => return selection_refused(reason, request.response, output),
        };
        self.review_account(
            AccountReview {
                installed: request.installed,
                document: request.document,
                target: completed.target.clone(),
                account: completed.native.verified_account.clone(),
                config,
                grant: request.grant,
                response: request.response,
                login_completed: true,
                client_profile,
            },
            approvals,
            presenter,
            input,
            output,
            || completed.prepared_installation().map(|_| ()),
            interrupt.as_mut(),
        )
        .await
    }

    async fn review_account<W: Write>(
        &mut self,
        mut request: AccountReview,
        approvals: &LocalApprovals,
        presenter: &mut ApprovalPresenter,
        input: &mut TerminalInput,
        output: &mut W,
        check_selected: impl Fn() -> Result<(), &'static str>,
        mut interrupt: Pin<&mut impl Future<Output = io::Result<()>>>,
    ) -> Result<bool, ConnectError> {
        if let Err(reason) = check_selected() {
            return selection_refused(reason, request.response, output);
        }
        if request.response.is_closed() {
            return Ok(true);
        }
        if let Err(reason) = request_documents(plain::recheck_retained(
            &request.installed, &request.document, None,
        )).await {
            return selection_refused(reason, request.response, output);
        }
        if let Err(reason) = check_selected() {
            return selection_refused(reason, request.response, output);
        }
        if request.response.is_closed() { return Ok(true); }
        if let Err(error) = profiles::recheck(&self.profiles, &request.target, request.client_profile.as_ref()).await {
            return error.respond(request.response, output);
        }
        if let Err(reason) = check_selected() {
            return selection_refused(reason, request.response, output);
        }
        let config = request.config;
        let (own_servers, expected_generation) =
            crate::realm::read_own_mcp_config(&self.storage).await?;
        if own_servers.iter().any(|server| server.name == config.name) {
            return selection_refused("server_name_already_selected", request.response, output);
        }
        let body = json!({"phase":"account_access", "realm":self.storage.locator.realm.as_str(),
            "job_id":request.installed.job_id, "server":config.name,
            "account":{"issuer":request.account.issuer,
                "strategy":request.account.strategy_id,
                "subject":request.account.subject},
            "operations":config.connector.as_ref().map(|c| &c.operations),
            "agent":request.grant.agent, "expires_in_ms":request.grant.expires_in_ms,
            "expected_configuration_generation":expected_generation,
            "client_profile":profiles::display(request.client_profile.as_ref()),
            "notice":"After setup closes, Connect rechecks retained installation documents and native finalization rechecks this account and configuration generation, saves this exact server, and issues and selects only this access. This review does not itself make the service ready."});
        let decision = match review(
            approvals,
            presenter,
            input,
            output,
            review_request(
                &request.installed.session,
                "Review installed account access",
                body,
            )?,
            &mut request.response,
            interrupt.as_mut(),
        )
        .await? {
            ReviewOutcome::Decided(decision) => decision,
            ReviewOutcome::CallerGone => return Ok(true),
            ReviewOutcome::Stopped => return Ok(false),
        };
        // Recheck selected software immediately before the immutable handoff.
        // Account evidence remains historical; native finalization re-observes it.
        if let Err(reason) = check_selected() {
            return selection_refused(reason, request.response, output);
        }
        if decision.status != ApprovalStatus::Approved {
            let _ = request.response.send(json!({"state":"denied", "phase":"account_access", "login_completed":request.login_completed, "grant_issued":false}));
            return Ok(true);
        }
        if request.response.is_closed() {
            return Ok(true);
        }
        // A decision cannot renew changed source documents. Refuse before
        // passive account work, then retain the later fence across its awaits.
        if let Err(reason) = request_documents(plain::recheck_retained(
            &request.installed, &request.document, None,
        )).await {
            return selection_refused(reason, request.response, output);
        }
        let current = self
            .auth
            .facade
            .connector_status(&request.target.slot)
            .await
            .map_err(|error| {
                ConnectError::provider_setup("Recheck reviewed connector account", error)
            })?;
        if let Err(reason) = check_reviewed_account(&request.target, &request.account, &current) {
            return selection_refused(reason, request.response, output);
        }
        if let Err(error) = profiles::recheck(&self.profiles, &request.target, request.client_profile.as_ref()).await {
            return error.respond(request.response, output);
        }
        if let Err(reason) = check_selected() {
            return selection_refused(reason, request.response, output);
        }
        if let Err(reason) = request_documents(plain::recheck_retained(
            &request.installed, &request.document, None,
        )).await {
            return selection_refused(reason, request.response, output);
        }
        if let Err(reason) = check_selected() {
            return selection_refused(reason, request.response, output);
        }
        if request.response.is_closed() {
            return Ok(true);
        }
        self.reviewed = Some(ReviewedInstallation::Account(ReviewedConnector {
            storage: self.storage.clone(),
            factory: self.factory.clone(),
            installed: request.installed,
            document: request.document,
            target: request.target,
            account: request.account,
            config,
            expected_generation,
            client_profile_binding: profiles::binding(request.client_profile.as_ref()),
            request: request.grant,
            decision,
        }).into());
        let _ = request.response.send(json!({"state":"pending", "reason":"reviewed_finalization_waiting_for_setup_close",
            "login_completed":request.login_completed, "access_reviewed":true, "grant_issued":false,
            "instruction":"The exact review is retained. Finish this turn so native finalization can run after the setup session closes. The host will report its actual outcome before the conversation continues. Do not invoke the integration or claim the service is ready before the terminal reports the native finalization result."}));
        Ok(true)
    }
}

/// Convert only a selected document/readback failure into request-local feedback.
/// Terminal I/O, provider and native cleanup errors do not enter this adapter.
async fn request_documents(
    check: impl Future<Output = Result<(), ConnectError>>,
) -> Result<(), &'static str> {
    check.await.map_err(|_| "selected_documents_unconfirmed")
}

/// Only the static refusal codes from the selected installation/account joins
/// use this path. I/O, provider and native cleanup failures remain errors.
fn selection_refused(
    reason: &'static str, response: oneshot::Sender<Value>, output: &mut impl Write,
) -> Result<bool, ConnectError> {
    let message = "The selected integration or account no longer matches this request. No access was granted by this request; check its current selection before trying again.";
    writeln!(output, "{message}")
        .map_err(|error| ConnectError::native("Display connector selection refusal", error))?;
    let _ = response.send(json!({"state":"unavailable", "phase":"installed_selection",
        "reason":reason, "instruction":message, "grant_issued":false}));
    Ok(true)
}

/// Refuse a changed native observation rather than silently reviewing a new
/// account. A later change is still the post-close native finalizer's decision.
fn check_reviewed_account(
    target: &meerkat::ConnectorOAuthTarget,
    reviewed: &meerkat::ConnectorVerifiedAccount,
    current: &meerkat::ConnectorAuthStatus,
) -> Result<(), &'static str> {
    if crate::connector_install::authorized_account(target, current)? != reviewed {
        return Err("reviewed_account_changed");
    }
    Ok(())
}

pub(crate) fn review_request(
    session: &SessionId,
    summary: &str,
    body: Value,
) -> Result<ApprovalRequest, ConnectError> {
    Ok(ApprovalRequest {
        requester: ApprovalPrincipalId::new("connect-setup-agent")
            .map_err(|e| ConnectError::native("Name review requester", e))?,
        owner: ApprovalOwnerRef::Session {
            session_id: session.clone(),
        },
        resource: ApprovalResourceRef {
            kind: ApprovalResourceKind::Runtime,
            id: ApprovalResourceId::new("selected-connector"),
        },
        proposed_action: ApprovalProposedAction {
            kind: ApprovalActionKind::Other,
            summary: summary.into(),
            body: Some(body.clone()),
        },
        risk: ApprovalRisk::Medium,
        request_body: Some(body),
        allowed_decisions: [ApprovalDecision::Approve, ApprovalDecision::Deny]
            .into_iter()
            .collect(),
        expires_at: Some((SystemTime::now() + Duration::from_secs(300)).into()),
        metadata: Default::default(),
        request_provenance: None,
    })
}

enum ReviewOutcome {
    Decided(ApprovalRecord),
    CallerGone,
    Stopped,
}

async fn review<W: Write>(
    approvals: &LocalApprovals,
    presenter: &mut ApprovalPresenter,
    input: &mut TerminalInput,
    output: &mut W,
    request: ApprovalRequest,
    caller: &mut oneshot::Sender<Value>,
    mut interrupt: Pin<&mut impl Future<Output = io::Result<()>>>,
) -> Result<ReviewOutcome, ConnectError> {
    let decision = approvals.review_access(request);
    tokio::pin!(decision);
    loop {
        tokio::select! {
            biased;
            _=caller.closed()=>return Ok(ReviewOutcome::CallerGone),
            signal=interrupt.as_mut()=> {
                signal.map_err(|error| ConnectError::native("listen for cancellation", error))?;
                return Ok(ReviewOutcome::Stopped);
            },
            result=decision.as_mut()=>return result.map(ReviewOutcome::Decided),
            prompt=presenter.next()=> {
                let prompt=prompt.ok_or_else(||unavailable("The access presenter ended"))?;
                let mut caller_gone = false;
                let (shown, completed) = {
                    let shown = present_approval(prompt,input,output,async {
                        tokio::select! { biased;
                            _=caller.closed()=> { caller_gone = true; Ok(()) },
                            result=interrupt.as_mut()=>result,
                        }
                    });
                    tokio::pin!(shown);
                    // The existing review owns its deadline while displayed.
                    // Ready caller loss or interruption keeps precedence.
                    tokio::select! { biased;
                        shown = shown.as_mut() => (Some(shown), None),
                        result = decision.as_mut() => (None, Some(result)),
                    }
                };
                if let Some(result) = completed {
                    input.check_cleanup().map_err(|error| ConnectError::native("restore credential review input", error))?;
                    return result.map(ReviewOutcome::Decided);
                }
                if !shown.expect("presentation completed without the review")? {
                    return Ok(if caller_gone { ReviewOutcome::CallerGone } else { ReviewOutcome::Stopped });
                }
            }
        }
    }
}

#[async_trait]
impl AgentToolDispatcher for RequestTool {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        let mut definitions = vec![Arc::new(ToolDef::new(TOOL,
            "Ask the protected terminal to reuse an already signed-in account or sign in for an exact reconciled installed job, then review native account access. Supply the selected publisher's explicit OAuth descriptor and per-raw-operation requirements. No commands, config, credential values or native grants are accepted. Agent and lifetime are proposals shown to the human; expires_in_ms must be explicit (null means until revoked). A named native client profile may be selected by ID. The protected terminal guides missing-profile setup; never request profile settings or secrets in chat. The result never means the service is ready. Authorization URLs and callback values stay outside the model.",
            json!({"type":"object","additionalProperties":false,"required":["job_id","connector","operations","agent","expires_in_ms"],
                "properties":{"job_id":{"type":"string"},"connector":{
                    "type":"object","additionalProperties":false,
                    "required":["slot_id","issuer","client","resource","scopes","strategy_id","account_selection"],
                    "properties":{"slot_id":{"type":"string"},"issuer":{"type":"string"},
                        "client":{"type":"string"},"resource":{"type":"string"},
                        "scopes":{"type":"array","items":{"type":"string"},"uniqueItems":true},
                        "strategy_id":{"type":"string"},
                        "client_auth_profile":{"type":["string","null"],"description":"Native realm profile ID only. Omit or null to choose in the terminal (Enter keeps the public-client default); the terminal handles missing profiles and secrets."},
                        "account_selection":{"type":["string","null"],"description":"Exact known native account subject, or null to discover and verify it."}}},
                    "operations":{"type":"object","minProperties":1,"maxProperties":128,
                        "additionalProperties":{"type":"object","additionalProperties":false,
                            "required":["required_scopes","destination_origin"],
                            "properties":{"required_scopes":{"type":"array","items":{"type":"string"},"uniqueItems":true},
                                "destination_origin":{"type":"string","description":"Explicit scheme://host[:port] from the selected integration's documented API."}}}},
                    "agent":{"type":"string"},
                    "expires_in_ms":{"type":["integer","null"],"minimum":1}}})))];
        definitions.push(plain::tool());
        definitions.push(secrets::tool());
        definitions.extend(secrets::management::tools());
        definitions.extend(oauth_management::tools());
        definitions.extend(access::tools());
        definitions.extend(installed::tools());
        definitions.into()
    }
    fn tool_catalog_capabilities(&self) -> meerkat_core::ToolCatalogCapabilities {
        meerkat_core::ToolCatalogCapabilities {
            exact_catalog: true,
            may_require_catalog_control_plane: false,
        }
    }
    fn tool_mutation_class(&self, name: &str) -> ToolMutationClass {
        if matches!(name, SECRET_STATUS_TOOL | OAUTH_STATUS_TOOL | ACCESS_INVENTORY_TOOL | INSTALLED_LIST_TOOL | INSTALLED_SELECT_TOOL) {
            ToolMutationClass::ReadOnly
        } else if TOOLS.contains(&name) {
            ToolMutationClass::Mutating
        } else {
            ToolMutationClass::Unknown
        }
    }
    async fn dispatch(&self, _call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        Err(refused())
    }
    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        if matches!(call.name, INSTALLED_LIST_TOOL | INSTALLED_SELECT_TOOL) {
            return installed::dispatch(self, call, context).await;
        }
        if matches!(call.name, ACCESS_INVENTORY_TOOL | ACCESS_MANAGE_TOOL) {
            return access::dispatch(self, call, context).await;
        }
        if matches!(call.name, SECRET_STATUS_TOOL | SECRET_MANAGE_TOOL) {
            return secrets::management::dispatch(self, call, context).await;
        }
        if matches!(call.name, OAUTH_STATUS_TOOL | OAUTH_MANAGE_TOOL) {
            return oauth_management::dispatch(self, call, context).await;
        }
        if call.name == PLAIN_TOOL {
            return plain::dispatch(self, call, context).await;
        }
        if call.name == SECRET_TOOL {
            return secrets::dispatch(self, call, context).await;
        }
        if call.name != TOOL || call.args.get().len() > 32 * 1024 {
            return Err(refused());
        }
        let proposal = proposal(call.parse_args().map_err(|_| refused())?)?;
        let session = context.origin_session_id().ok_or_else(refused)?;
        stock_agent_principal(&self.storage.locator.realm, &proposal.agent)
            .map_err(|_| refused())?;
        let selected = self
            .installs
            .connector_selection(&self.storage.locator, session, &proposal.job_id)
            .map_err(|_| refused())?;
        let installed = selected.installed.clone();
        let document = selected.configuration.clone();
        let grant = StockGrantRequest {
            agent: proposal.agent,
            expires_in_ms: proposal.expires_in_ms,
            target: StockGrantTarget::Mcp {
                server: installed.server_name.clone(),
                operations: McpOperations::Exact(proposal.operations.keys().cloned().collect()),
            },
        };
        let target = proposal
            .connector
            .into_native(self.storage.locator.realm.clone())
            .map_err(|_| refused())?;
        if target.account.is_unverified() {
            return Err(refused());
        }
        let selection =
            InstallationLogin::prepare(selected, proposal.operations).map_err(|_| refused())?;
        selection.check_target(&target).map_err(|_| refused())?;
        let (response, receive) = oneshot::channel();
        self.requests
            .try_send(ConnectorRequest::Account(AccountRequest {
                installed,
                document,
                target,
                selection,
                grant,
                response,
            }))
            .map_err(|_| refused())?;
        let response=receive.await.map_err(|_|ToolError::execution_failed("Protected connector review ended without a completed handoff. No service readiness is established."))?;
        Ok(ToolResult::new(call.id.into(), response.to_string(), false).into())
    }
}

/// The terminal has consumed its one callback, so a failed completion ends
/// that attempt. The shared private-pipe control still permits callback retries.
/// Run this after joining completion and before returning for a stopped caller.
/// A reported cleanup failure is terminal and must not trigger another cancel.
pub(crate) fn settle_terminal_completion(
    control: &mut Control, reply: Result<Reply, &'static str>, cleanup_failed: bool,
) -> Result<Result<Arc<crate::connector_auth::CompletedLogin>, &'static str>, ConnectError> {
    if cleanup_failed {
        return Err(unavailable("Native connector cancellation did not confirm cleanup"));
    }
    match reply {
        Ok(Reply::Completed(completed)) => Ok(Ok(completed)),
        other => {
            // Native Missing is already terminal; Control::retire handles it
            // without logging out or erasing a completed account. Any other
            // retirement failure stays fatal rather than reporting retryability.
            control.retire().map_err(unavailable)?;
            match other {
                Err(code) => Ok(Err(code)),
                Ok(_) => Err(unavailable("Native connector completion did not return a completed login")),
            }
        }
    }
}

fn join_results(
    first: Result<(), ConnectError>,
    second: Result<(), ConnectError>,
) -> Result<(), ConnectError> {
    match (first, second) {
        (Err(primary), Err(cleanup)) => Err(ConnectError::Cleanup {
            primary: Box::new(primary),
            cleanup: Box::new(cleanup),
        }),
        (Err(error), _) | (_, Err(error)) => Err(error),
        _ => Ok(()),
    }
}

/// Project the live selected installation into the exact reviewed ceiling.
/// This does not persist it or grant permission to invoke its operations.
pub(crate) fn reviewed_configuration(
    completed: &crate::connector_auth::CompletedLogin,
) -> Result<McpServerConfig, &'static str> {
    reviewed_installation_configuration(completed.prepared_installation()?)
}

fn reviewed_installation_configuration(
    installation: &meerkat::ConnectorMcpInstallation,
) -> Result<McpServerConfig, &'static str> {
    crate::connector_install::reviewed_configuration(installation)
}

fn proposal(mut raw: Value) -> Result<Proposal, ToolError> {
    if raw.get("expires_in_ms").is_none() {
        return Err(refused());
    }
    // The terminal tool accepts null as the same profile choice as omission.
    // Keep the shared private-pipe decoder's stricter optional-field contract.
    if let Some(connector) = raw.get_mut("connector").and_then(Value::as_object_mut) {
        if connector.get("client_auth_profile").is_some_and(Value::is_null) {
            connector.remove("client_auth_profile");
        }
    }
    let proposal: Proposal = serde_json::from_value(raw).map_err(|_| refused())?;
    if proposal.expires_in_ms == Some(0) {
        return Err(refused());
    }
    Ok(proposal)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn exact_proposal() -> Value {
        json!({"job_id":"a".repeat(32), "agent":"rkat-agent", "expires_in_ms":60000,
            "connector":{"slot_id":"selected-account", "issuer":"https://issuer.test",
                "client":"public-client", "resource":"https://api.test", "scopes":["read"],
                "strategy_id":"oidc-userinfo-v1", "account_selection":null},
            "operations":{"list":{"required_scopes":["read"],"destination_origin":"https://api.test"}}})
    }
    fn finalizer_storage(root: &std::path::Path, realm: &str) -> ResolvedStorage {
        crate::RealmOptions {
            context_root: root.to_owned(),
            state_root: Some(root.join("state")),
            user_config_root: Some(root.join("home")),
        }
        .resolve(realm)
        .unwrap()
    }

    #[tokio::test]
    async fn post_close_account_finalizer_rechecks_retained_documents_before_native_effects() {
        tokio::time::timeout(Duration::from_secs(30), Box::pin(retained_document_finalization_scenario()))
            .await.expect("the document boundary and passive native account check must settle");
    }

    async fn retained_document_finalization_scenario() {
        use meerkat::governed_realm::stock::govern_stock_realm;
        use meerkat_auth_core::auth_store::{FileLockCoordinator, FileTokenStore, ProviderAuthPersistence};
        use meerkat_core::{ApprovalService, ConfigDelta, ConfigStore, FileConfigStore};
        use meerkat_core::connection::{CredentialAccountId, CredentialAccountRef};
        use meerkat_core::mcp_config::McpConnectorBinding;
        use sha2::{Digest, Sha256};

        // This enters the actual post-close consumer and real native realm
        // owners. The retained package/review is a fixture; no setup helper,
        // installed package, sign-in or live service use is claimed.
        for changed in [None, Some("ready/installed.json"), Some("configuration/selected.json"),
            Some("ready/mcp.json"), Some("missing-receipt")] {
            let directory = tempfile::tempdir().unwrap();
            let storage = finalizer_storage(directory.path(), "work");
            let config_path = crate::realm::config_source(&storage).unwrap()
                .config_doc_path(&storage.locator.realm);
            let state_path = config_path.with_file_name("config_state.json");
            let config_store = FileConfigStore::new(config_path.clone(), meerkat_models::canonical());
            config_store.patch(ConfigDelta(json!({"max_tokens":1234,"tools":{"shell_enabled":false,
                "mcp_servers":[McpServerConfig::stdio("unrelated", "/bin/true", vec![], Default::default())]}})))
                .await.unwrap();
            let (_, bundle) = meerkat::storage_provider::open_realm_persistence_with_layout(
                storage.layout.clone(), storage.locator.realm.as_str(), None,
                Some(meerkat_store::RealmOrigin::Explicit),
            ).await.unwrap();
            drop(bundle);
            let principals = StockPrincipals::for_local_operator(&storage.locator.realm).unwrap();
            govern_stock_realm(&storage.locator.state_root, storage.locator.realm.as_str(), &principals)
                .await.unwrap();
            let paths = meerkat_store::realm_paths_in(&storage.locator.state_root, storage.locator.realm.as_str());
            let owner_path = meerkat_store::grant_owner_store_path(&paths.root);
            let before_owner = std::fs::read(&owner_path).unwrap();
            let before_config = std::fs::read(&config_path).unwrap();
            assert!(!before_owner.is_empty());
            assert!(!state_path.exists());

            let root = directory.path().join("installed");
            std::fs::create_dir_all(root.join("ready")).unwrap();
            std::fs::create_dir(root.join("configuration")).unwrap();
            let receipt = br#"{"scope":"inert retained fixture"}"#;
            let configuration = br#"{"profile":"slack","operations":["auth_test"],"config":{"token_env":"UNRESOLVED_REFERENCE"}}"#;
            let command = root.join("runtime/bin/python").to_str().unwrap().to_owned();
            let args = vec!["-I".to_owned(), "-m".to_owned(),
                "meerkat_toolkit_services.adapters.services_mcp".to_owned(), "serve".to_owned(),
                "--config".to_owned(), root.join("configuration/selected.json").to_str().unwrap().to_owned()];
            let document = Arc::new(json!({"mcpServers":{"mail":{"command":command,"args":args}}}));
            let document_bytes = serde_json::to_vec(document.as_ref()).unwrap();
            std::fs::write(root.join("ready/installed.json"), receipt).unwrap();
            std::fs::write(root.join("configuration/selected.json"), configuration).unwrap();
            std::fs::write(root.join("ready/mcp.json"), &document_bytes).unwrap();
            let session = SessionId::new();
            let installed = InstalledSelection {
                realm: storage.locator.clone(), session:session.clone(), job_id:"1".repeat(32), validation_id:"2".repeat(32),
                family:"services".into(), profile:"slack".into(), server_name:"mail".into(),
                mcp_path:root.join("ready/mcp.json"), receipt_path:root.join("ready/installed.json"),
                receipt_sha256:Some(format!("{:x}",Sha256::digest(receipt))),
                configuration_sha256:Some(format!("{:x}",Sha256::digest(configuration))),
                mcp_document_sha256:format!("{:x}",Sha256::digest(&document_bytes)),
                recovery_request_sha256:Some("3".repeat(64)),
            };
            plain::recheck_retained(&installed, &document, None).await.unwrap();
            let target = meerkat::ConnectorOAuthTarget {
                slot: CredentialAccountRef { realm:storage.locator.realm.clone(),
                    account:CredentialAccountId::parse("mail").unwrap() },
                issuer:"https://issuer.invalid".into(), strategy_id:"oidc-userinfo-v1".into(),
                client:"public-client".into(), resource:"https://api.invalid".into(),
                scopes:["read".into()].into(),
                account:meerkat_auth_core::connector_oauth::AccountSelection::Discover,
                client_auth_profile:None,
            };
            let account = meerkat::ConnectorVerifiedAccount { issuer:target.issuer.clone(),
                strategy_id:target.strategy_id.clone(), subject:"historical-fixture-account".into() };
            let mut native_args = args;
            native_args[3] = "serve-native".into();
            let mut config = McpServerConfig::stdio("mail", command, native_args, Default::default());
            config.access = Some(meerkat_core::McpServerAccess::allowing(["slack_auth_test"]));
            config.connector = Some(McpConnectorBinding {
                realm_id:storage.locator.realm.as_str().into(), slot_id:"mail".into(),
                issuer:target.issuer.clone(), client:target.client.clone(), resource:target.resource.clone(),
                strategy_id:target.strategy_id.clone(), account:account.subject.clone(),
                operations:[("slack_auth_test".into(), McpConnectorOperation {
                    required_scopes:["read".into()].into(), destination_origin:target.resource.clone(), read_only:true,
                })].into(),
            });
            let grant = StockGrantRequest { agent:"rkat-agent".into(), expires_in_ms:None,
                target:StockGrantTarget::Mcp { server:"mail".into(),
                    operations:McpOperations::Exact(["slack_auth_test".into()].into()) } };
            let approvals = ApprovalService::new();
            let pending = approvals.request(review_request(&session, "Fixture account-access review",
                json!({"scope":"post-close document boundary only"})).unwrap()).unwrap();
            let decision = approvals.decide(&pending.approval_id, ApprovalDecision::Approve,
                ApprovalPrincipalId::new("fixture-operator").unwrap(), None, None).unwrap();
            let factory = meerkat::AgentFactory::new(paths.root.clone()).with_provider_auth_persistence(
                ProviderAuthPersistence::new(Arc::new(FileTokenStore::new(directory.path().join("credentials"))),
                    Arc::new(FileLockCoordinator::new(directory.path().join("credential-locks")))),
            );
            let reviewed = ReviewedConnector { storage:storage.clone(), factory, installed, document,
                target, account, config, expected_generation:0, client_profile_binding:None,
                request:grant, decision };
            if let Some(path) = changed {
                if path == "missing-receipt" {
                    std::fs::remove_file(root.join("ready/installed.json")).unwrap();
                } else {
                    std::fs::write(root.join(path), b"changed after protected review").unwrap();
                }
            }
            let mut output = Vec::new();
            let finalized = reviewed.finalize_outcome_after_close().await;
            assert!(finalized.belongs_to(&storage.locator, &session));
            let facts = finalized.notice();
            assert_eq!(facts["facts"]["persistence"], if changed.is_some() { "not_entered" } else { "refused" });
            assert_eq!(facts["facts"]["grant"]["state"], "not_requested");
            let error = finalized.display(&mut output).unwrap_err().to_string();
            let artifacts = directory.path().join("observations");
            std::fs::create_dir(&artifacts).unwrap();
            finalized.retain(&artifacts, 1).unwrap();
            let retained: Value = serde_json::from_slice(&std::fs::read(artifacts.join("standalone-finalization-1.txt")).unwrap()).unwrap();
            assert_eq!(retained["observation"], facts, "display failure did not replace the actual native outcome");
            if changed.is_some() {
                let FinalizationOutcome::BeforeAdministrationRefused(reason) = &finalized.outcome else {
                    panic!("changed documents reached native administration");
                };
                let detail = reason.to_string();
                assert!(detail.contains("retained integration document") || detail.contains("Retained integration documents"), "{detail}");
                assert!(detail.contains("Service finalization did not start"), "{detail}");
                assert!(error.contains("Finalization stopped before native administration"), "{error}");
                assert!(!error.contains("Native finalization was refused"), "{error}");
            } else {
                // Matching documents reach the existing native finalizer. Its
                // real empty credential store refuses the historical account;
                // the document check cannot fabricate account authorization.
                assert!(matches!(&finalized.outcome, FinalizationOutcome::Connector(
                    ReviewedConnectorFinalization::Refused(
                        meerkat::governed_realm::stock::finalize::ReviewedConnectorRefusal::AccountChanged))));
                assert!(error.contains("Native finalization was refused"), "{error}");
            }
            assert!(output.is_empty());
            assert_eq!(std::fs::read(&config_path).unwrap(), before_config);
            assert_eq!(std::fs::read(&owner_path).unwrap(), before_owner);
            assert!(!state_path.exists(), "no service configuration generation was published");
            assert_eq!(meerkat_store::read_realm_posture_in(&storage.locator.state_root,
                storage.locator.realm.as_str()).await.unwrap(), meerkat_store::RealmPosture::Governed);
        }
    }

    #[test]
    fn finalization_notice_uses_native_projection_and_private_evidence_is_not_a_retry_authority() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let storage = finalizer_storage(root.path(), "work");
        let session = SessionId::new();
        let mut outcome = FinalizedInstallation::new(
            FinalizationTarget { realm:storage.locator.clone(), server:"mail".into(),
                agent:"rkat-agent".into(), job_id:"original-job".into(), session:session.clone() },
            FinalizationOutcome::Connector(ReviewedConnectorFinalization::PersistUncertain {
                reason:"PRIVATE_NATIVE_DIAGNOSTIC_MUST_NOT_BECOME_MODEL_INPUT".into() }),
        );
        let notice = outcome.take_continuation_notice().unwrap();
        assert!(outcome.take_continuation_notice().is_none(), "the retained native fact cannot mint another occurrence");
        assert!(notice.belongs_to(&storage.locator, &session));
        assert!(!notice.belongs_to(&storage.locator, &SessionId::new()));
        let input = notice.into_input();
        let meerkat_runtime::Input::ExternalEvent(event) = &input else { panic!("process result is not human input") };
        assert_eq!(event.event_type, "connect_finalization_observation");
        assert!(matches!(&event.header.source, meerkat_runtime::InputOrigin::External { source_name }
            if source_name == "meerkat-connect-finalization"));
        assert!(event.blocks.is_none(), "the finalizer supplies no additional model content");
        assert_eq!(event.payload.as_object().unwrap().len(), 1);
        assert_eq!(event.payload["body"], outcome.notice().to_string());
        assert!(event.header.authority_association.is_none(), "only actual native binding may supply authority");
        let content = meerkat_runtime::input::runtime_input_run_started_content(&input).unwrap();
        // Native owns the complete rendering, which may repeat a fact in its
        // summary and structured event block. Check the public facts and
        // privacy here; the resumed-context control counts whole projections.
        assert!(content.text_content().contains(&outcome.notice().to_string()));
        assert!(!content.text_content().contains("PRIVATE_NATIVE_DIAGNOSTIC"));
        assert!(outcome.belongs_to(&storage.locator, &session));
        assert!(!outcome.belongs_to(&storage.locator, &SessionId::new()));
        let other = finalizer_storage(root.path(), "other");
        assert!(!outcome.belongs_to(&other.locator, &session));
        outcome.retain(root.path(), 1).unwrap();
        let artifact = root.path().join("standalone-finalization-1.txt");
        assert_eq!(std::fs::metadata(&artifact).unwrap().permissions().mode() & 0o777, 0o600);
        let retained = std::fs::read_to_string(&artifact).unwrap();
        assert!(!retained.contains("PRIVATE_NATIVE_DIAGNOSTIC"));
        let error = outcome.retain(root.path(), 1).unwrap_err().to_string();
        assert!(!error.contains("PRIVATE_NATIVE_DIAGNOSTIC"));
        assert!(error.contains("No retry was started"));
        assert_eq!(std::fs::read_to_string(artifact).unwrap(), retained, "existing observation is never overwritten");
        assert!(matches!(&outcome.outcome, FinalizationOutcome::Connector(ReviewedConnectorFinalization::PersistUncertain { reason })
            if reason == "PRIVATE_NATIVE_DIAGNOSTIC_MUST_NOT_BECOME_MODEL_INPUT"), "actual typed result survives observation errors");
    }

    #[test]
    fn typed_partial_results_preserve_native_references_without_authorizing_reentry() {
        use meerkat::governed_realm::stock::{StockGovernanceError,
            issuance::{CleanupAttempt, StockGrantOutcome}};
        let root = tempfile::tempdir().unwrap();
        let storage = finalizer_storage(root.path(), "work");
        let principal = stock_agent_principal(&storage.locator.realm, "fixture").unwrap();
        // Exact native reference shapes are observation fixtures only. Neither
        // native issuance nor authority reconstruction is exercised here.
        let reference = |id: &str| serde_json::from_value(json!({
            "root_authority":&principal,"authority_namespace":"fixture-authority",
            "authority_generation":1,"authority_incarnation":"6f75f457-4232-4e12-8426-508d49ece197",
            "grant_id":id,"issued_revision":1})).unwrap();
        let private_reason = "PRIVATE_SELECTION_DIAGNOSTIC";
        let cases = [
            (Ok(StockGrantOutcome { issued:reference("current"), superseded:Some(reference("previous")),
                cleanup:vec![CleanupAttempt { agent:"fixture".into(), grant:reference("older"),
                    outcome:RevocationOutcome::Uncertain }] }), "selected", false),
            (Err(StockGrantError::Refused(StockGovernanceError::GrantsUnavailable)), "refused", false),
            (Err(StockGrantError::Refused(StockGovernanceError::Io(io::Error::other(private_reason)))), "refused", false),
            (Err(StockGrantError::Refused(StockGovernanceError::RequesterUnsupported(private_reason.into()))), "refused", false),
            (Err(StockGrantError::Refused(StockGovernanceError::CallerNotAdmitted(
                meerkat_core::OperationAuthorizationError::Unavailable))), "refused", false),
            (Err(StockGrantError::Refused(StockGovernanceError::Review(private_reason.into()))), "refused", false),
            (Err(StockGrantError::IssuanceUncertain { id:serde_json::from_value(json!("possibly-issued")).unwrap() }),
                "issuance_uncertain", true),
            (Err(StockGrantError::NotSelected { issued:reference("current"), reason:private_reason.into() }),
                "issued_not_selected", true),
            (Err(StockGrantError::SelectionUncertain { issued:reference("current"), previous:Some(reference("previous")),
                reason:private_reason.into() }), "selection_durability_uncertain", true),
            (Err(StockGrantError::CommandUncertain), "command_uncertain", true),
        ];
        for (index, (grant, state, blocked)) in cases.into_iter().enumerate() {
            let observed = FinalizedInstallation::new(FinalizationTarget { realm:storage.locator.clone(),
                server:"mail".into(),agent:"fixture".into(),job_id:"reviewed-job".into(),session:SessionId::new() },
                FinalizationOutcome::Connector(ReviewedConnectorFinalization::Persisted { generation:18, grant }));
            observed.retain(root.path(), index + 1).unwrap();
            let saved: Value = serde_json::from_slice(&std::fs::read(root.path()
                .join(format!("standalone-finalization-{}.txt",index + 1))).unwrap()).unwrap();
            assert_eq!(saved["scope"], "invocation_finalization_observation_only");
            assert_eq!(saved["observation"]["facts"]["persistence"], "saved");
            assert_eq!(saved["observation"]["facts"]["generation"], 18);
            assert_eq!(saved["observation"]["facts"]["grant"]["state"], state);
            assert_eq!(saved["observation"]["automatic_reentry_blocked"], blocked);
            assert_eq!(observed.blocks_reentry(&storage.locator,"mail"), blocked);
            assert!(!observed.blocks_reentry(&storage.locator,"other-service"));
            assert!(!saved.to_string().contains(private_reason));
            assert!(!observed.notice().to_string().contains(private_reason));
            let mut terminal = Vec::new();
            observed.display_for_continuation(&mut terminal).unwrap();
            assert!(!String::from_utf8(terminal).unwrap().contains(private_reason));
            for excluded in ["root_authority", "authority_namespace", "authority_incarnation", "fixture-authority"] {
                assert!(!saved.to_string().contains(excluded));
            }
            let references = &saved["native_grant_references"];
            match state {
                "selected" => {
                    assert_eq!(references["issued"], json!({"grant_id":"current","issued_revision":1,"authority_generation":1}));
                    assert_eq!(references["superseded"], json!({"grant_id":"previous","issued_revision":1,"authority_generation":1}));
                    assert_eq!(references["cleanup"][0]["grant_id"], "older");
                    assert_eq!(references["cleanup"][0]["agent"], "fixture");
                    assert_eq!(references["cleanup"][0]["outcome"], "Uncertain");
                    assert_eq!(saved["observation"]["facts"]["grant"]["prior_cleanup_complete"], false);
                }
                "issuance_uncertain" => assert_eq!(references["possibly_issued_id"], "possibly-issued"),
                "issued_not_selected" => assert_eq!(references["issued"]["grant_id"], "current"),
                "selection_durability_uncertain" => {
                    assert_eq!(references["issued"]["grant_id"], "current");
                    assert_eq!(references["previous"]["grant_id"], "previous");
                }
                "refused" => {
                    assert!(references.is_null());
                    let expected = match index {
                        1 => "GrantsUnavailable", 2 => "Io", 3 => "RequesterUnsupported",
                        4 => "CallerNotAdmitted", 5 => "Review", _ => unreachable!(),
                    };
                    assert_eq!(saved["observation"]["facts"]["grant"]["reason_kind"], expected);
                    assert_eq!(saved["observation"]["facts"]["grant"]["issued"], false);
                    assert_eq!(saved["observation"]["facts"]["grant"]["selection_changed"], false);
                }
                "command_uncertain" => assert!(references.is_null()),
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn plain_native_failures_preserve_uncertainty_and_generation_facts_without_error_text() {
        use meerkat_core::{mcp_config::McpRealmPersistError as Persist, config_runtime::ConfigRuntimeError as Config};
        let root = tempfile::tempdir().unwrap();
        let storage = finalizer_storage(root.path(), "work");
        let private = "PRIVATE_CONFIGURATION_OR_PATH";
        // Typed native error inputs, not a simulated successful write. The
        // separate native writer control covers real sidecar failure ordering.
        let cases = [
            (Persist::ServerExists(private.into()), "refused", "ServerExists", false),
            (Persist::Config(Config::GenerationConflict { expected:7,current:9 }), "refused", "GenerationConflict", false),
            (Persist::Config(Config::LockTimeout), "uncertain", "LockTimeout", true),
            (Persist::Config(Config::Io(io::Error::other(private))), "uncertain", "Io", true),
            (Persist::Config(Config::Json(serde_json::from_str::<Value>("not-json").unwrap_err())), "uncertain", "Json", true),
            (Persist::NotAdmitted(meerkat_core::AdministrationEntryRefusal::Unavailable(private.into())),
                "refused", "NotAdmitted", false),
            (Persist::OutcomeUnknown(Config::Io(io::Error::other(private))), "uncertain", "OutcomeUnknown", true),
            (Persist::OutcomeUnknown(Config::GenerationConflict { expected:7,current:9 }),
                "uncertain", "OutcomeUnknown", true),
        ];
        for (index,(error,persistence,kind,blocked)) in cases.into_iter().enumerate() {
            let observed = FinalizedInstallation::new(FinalizationTarget { realm:storage.locator.clone(),
                server:"reviewed-service".into(),agent:"reader".into(),job_id:"prior-job".into(),session:SessionId::new() },
                FinalizationOutcome::PublicPersistenceError(error));
            let notice = observed.notice();
            assert_eq!(notice["facts"]["persistence"], persistence);
            assert_eq!(notice["facts"]["grant"]["state"], "not_requested");
            assert_eq!(notice["automatic_reentry_blocked"], blocked);
            let detail = &notice["facts"]["error"];
            if matches!(kind, "ServerExists" | "NotAdmitted" | "OutcomeUnknown") {
                assert_eq!(detail["kind"], kind);
            }
            else { assert_eq!(detail["kind"], "Config"); assert_eq!(detail["config"]["kind"], kind); }
            if kind == "GenerationConflict" {
                assert_eq!(detail["config"], json!({"kind":"GenerationConflict","expected":7,"current":9}));
            }
            observed.retain(root.path(), index+1).unwrap();
            let artifact = std::fs::read_to_string(root.path().join(format!("standalone-finalization-{}.txt",index+1))).unwrap();
            let mut output=Vec::new(); observed.display_for_continuation(&mut output).unwrap();
            assert!(!artifact.contains(private));
            let output = String::from_utf8(output).unwrap();
            assert!(!output.contains(private));
            if kind == "NotAdmitted" {
                assert!(output.contains("refused before changing the configuration"));
            } else if kind == "OutcomeUnknown" {
                assert!(output.contains("The write may be partial. Do not repeat this target automatically."));
            }
            assert!(!observed.blocks_reentry(&storage.locator,"different-service"));
            assert!(matches!(&observed.outcome,FinalizationOutcome::PublicPersistenceError(_)),
                "Plain errors remain separate from the native connector disposition");
        }
        let observed=FinalizedInstallation::new(FinalizationTarget { realm:storage.locator.clone(),
            server:"reviewed-service".into(),agent:"reader".into(),job_id:"prior-job".into(),session:SessionId::new() },
            FinalizationOutcome::Connector(ReviewedConnectorFinalization::Refused(
                meerkat::governed_realm::stock::finalize::ReviewedConnectorRefusal::ConfigConflict { expected:11,current:13 })));
        assert_eq!(observed.notice()["facts"]["error"], json!({"kind":"GenerationConflict","expected":11,"current":13}));
        assert!(!observed.blocks_reentry(&storage.locator,"reviewed-service"));
    }

    #[test]
    fn finalization_input_preserves_exact_lifetime_and_refuses_wider_requests() {
        use meerkat_core::connection::{CredentialAccountId, CredentialAccountRef};
        use meerkat_core::mcp_config::McpConnectorBinding;
        let directory = tempfile::tempdir().unwrap();
        let storage = finalizer_storage(directory.path(), "work");
        // Shape-only fixtures exercise mapping, never prove account/grant custody.
        let target = meerkat::ConnectorOAuthTarget {
            slot: CredentialAccountRef {
                realm: storage.locator.realm.clone(),
                account: CredentialAccountId::parse("mail").unwrap(),
            },
            issuer: "https://issuer.invalid".into(),
            strategy_id: "oidc-userinfo".into(),
            client: "public-client".into(),
            resource: "https://api.invalid".into(),
            scopes: ["read".into()].into(),
            account: meerkat_auth_core::connector_oauth::AccountSelection::Discover,
            client_auth_profile: None,
        };
        let account = meerkat::ConnectorVerifiedAccount {
            issuer: target.issuer.clone(),
            strategy_id: target.strategy_id.clone(),
            subject: "reviewed-account".into(),
        };
        let mut config = McpServerConfig::stdio("mail", "/bin/true", vec![], Default::default());
        config.access = Some(meerkat_core::McpServerAccess::allowing(["list"]));
        config.connector = Some(McpConnectorBinding {
            realm_id: "work".into(),
            slot_id: "mail".into(),
            issuer: account.issuer.clone(),
            client: "public-client".into(),
            resource: "https://api.invalid".into(),
            strategy_id: account.strategy_id.clone(),
            account: account.subject.clone(),
            operations: [(
                "list".into(),
                McpConnectorOperation {
                    required_scopes: ["read".into()].into(),
                    destination_origin: "https://api.invalid".into(),
                    read_only: true,
                },
            )]
            .into(),
        });
        let installation = meerkat::ConnectorMcpInstallation::from_config(&config)
            .unwrap()
            .unwrap();
        assert_eq!(
            reviewed_installation_configuration(&installation).unwrap(),
            config
        );
        let status = meerkat::ConnectorAuthStatus {
            slot: target.slot.clone(),
            phase: meerkat::ConnectorAuthPhase::Authorized,
            verified_account: Some(account.clone()),
            scopes: vec![],
            scope_evidence: None,
            expires_at: None,
            has_refresh_token: false,
        };
        assert!(check_reviewed_account(&target, &account, &status).is_ok());
        let mut changed_status = status.clone();
        changed_status.verified_account.as_mut().unwrap().subject = "replacement-account".into();
        assert_eq!(
            check_reviewed_account(&target, &account, &changed_status),
            Err("reviewed_account_changed")
        );
        changed_status = status.clone();
        changed_status.phase = meerkat::ConnectorAuthPhase::ReauthRequired;
        assert_eq!(
            check_reviewed_account(&target, &account, &changed_status),
            Err("account_not_authorized")
        );
        changed_status = status;
        changed_status.verified_account = None;
        assert_eq!(
            check_reviewed_account(&target, &account, &changed_status),
            Err("account_unverified")
        );
        let request = StockGrantRequest {
            agent: "mail-reader".into(),
            expires_in_ms: None,
            target: StockGrantTarget::Mcp {
                server: "mail".into(),
                operations: McpOperations::Exact(["list".into()].into()),
            },
        };
        let review =
            native_finalization_review(&storage, &target, &account, &config, &request, 17, None).unwrap();
        assert_eq!(review.agent, request.agent);
        assert_eq!(review.slot, target.slot);
        assert_eq!(review.account, account);
        assert_eq!(review.server, config);
        assert_eq!(review.expected_generation, 17);
        assert_eq!(
            review.operations,
            McpOperations::Exact(["list".into()].into())
        );
        assert!(review.client_profile_binding.is_none());
        assert_eq!(review.expires_in_ms, None);
        for grant_target in [
            StockGrantTarget::Controller {
                // A nonempty, parseable route keeps this a wrong-target test.
                routes: [meerkat_core::ControllerRouteKey::parse(&format!(
                    "{}{}",
                    meerkat_core::ControllerRouteKey::PREFIX,
                    "1".repeat(64)
                ))
                .unwrap()]
                .into(),
            },
            StockGrantTarget::Catalog,
            StockGrantTarget::Mcp {
                server: "other".into(),
                operations: McpOperations::Exact(["list".into()].into()),
            },
            StockGrantTarget::Mcp {
                server: "mail".into(),
                operations: McpOperations::All,
            },
            StockGrantTarget::Mcp {
                server: "mail".into(),
                operations: McpOperations::Exact(["list".into(), "write".into()].into()),
            },
        ] {
            let changed = StockGrantRequest {
                target: grant_target,
                ..request.clone()
            };
            assert!(
                native_finalization_review(&storage, &target, &account, &config, &changed, 17, None)
                    .is_err()
            );
        }
        for field in [
            "realm", "slot", "issuer", "client", "resource", "strategy", "account",
        ] {
            let mut changed = config.clone();
            let binding = changed.connector.as_mut().unwrap();
            match field {
                "realm" => binding.realm_id = "other".into(),
                "slot" => binding.slot_id = "other".into(),
                "issuer" => binding.issuer = "https://other.invalid".into(),
                "client" => binding.client = "other".into(),
                "resource" => binding.resource = "https://other.invalid".into(),
                "strategy" => binding.strategy_id = "other".into(),
                "account" => binding.account = "other".into(),
                _ => unreachable!(),
            }
            assert!(
                native_finalization_review(&storage, &target, &account, &changed, &request, 17, None)
                    .is_err()
            );
        }
        let timed = StockGrantRequest {
            expires_in_ms: Some(60_000),
            ..request.clone()
        };
        let timed_review =
            native_finalization_review(&storage, &target, &account, &config, &timed, 17, None).unwrap();
        assert_eq!(timed_review.expires_in_ms, Some(60_000));
        let foreign = finalizer_storage(directory.path(), "other");
        assert!(
            native_finalization_review(&foreign, &target, &account, &config, &request, 17, None).is_err()
        );
        let global = finalizer_storage(directory.path(), "global");
        let mut global_target = target.clone();
        global_target.slot.realm = global.locator.realm.clone();
        let mut global_config = config.clone();
        global_config.connector.as_mut().unwrap().realm_id = "global".into();
        let global_review = native_finalization_review(
            &global, &global_target, &account, &global_config, &timed, 17, None,
        ).unwrap();
        assert_eq!(global_review.slot.realm, global.locator.realm);
        assert_eq!(global_review.server, global_config);
        assert_eq!(global_review.expires_in_ms, Some(60_000));
        let named = meerkat::ConnectorOAuthTarget {
            client_auth_profile: Some("native-profile".into()), ..target.clone()
        };
        assert!(native_finalization_review(&storage, &named, &account, &config, &timed, 17, None).is_err());
        assert!(native_finalization_review(&storage, &target, &account, &config, &timed, 17, Some("native-profile@binding")).is_err());
        let named_review = native_finalization_review(&storage, &named, &account, &config, &timed, 17, Some("native-profile@binding")).unwrap();
        assert_eq!(named_review.client_profile_binding.as_deref(), Some("native-profile@binding"));
    }

    #[test]
    fn finalization_messages_distinguish_each_native_settlement_without_retry() {
        use meerkat::governed_realm::stock::{
            StockGovernanceError,
            finalize::ReviewedConnectorRefusal,
            issuance::{CleanupAttempt, StockGrantOutcome},
        };
        let realm = meerkat_core::connection::RealmId::parse("work").unwrap();
        let principal = stock_agent_principal(&realm, "fixture").unwrap();
        // Deserialized reference shapes are only formatter input. No native
        // owner receives them and this test makes no issuance/custody claim.
        let reference = || {
            serde_json::from_value(json!({
                "root_authority":&principal,
                "authority_namespace":"fixture-authority",
                "authority_generation":1,
                "authority_incarnation":"6f75f457-4232-4e12-8426-508d49ece197",
                "grant_id":"fixture-grant", "issued_revision":1
            }))
            .unwrap()
        };
        let error = |outcome| {
            finalization_message("mail", "work", "reader", outcome)
                .unwrap_err()
                .to_string()
        };
        let refused = error(ReviewedConnectorFinalization::Refused(
            ReviewedConnectorRefusal::AccountChanged,
        ));
        assert!(refused.contains("No server configuration or grant was written"));
        let persistence = error(ReviewedConnectorFinalization::PersistUncertain {
            reason: "append".into(),
        });
        assert!(persistence.contains("configuration write has an uncertain outcome"));
        assert!(persistence.contains("No grant was issued"));
        assert!(persistence.contains("Do not repeat"));
        let refused_grant = error(ReviewedConnectorFinalization::Persisted {
            generation: 18,
            grant: Err(StockGrantError::Refused(
                StockGovernanceError::GrantsUnavailable,
            )),
        });
        assert!(refused_grant.contains("Saved mail"));
        assert!(refused_grant.contains("No new grant was issued or selected"));
        let issuance = error(ReviewedConnectorFinalization::Persisted {
            generation: 18,
            grant: Err(StockGrantError::IssuanceUncertain {
                id: serde_json::from_value(json!("fixture-grant")).unwrap(),
            }),
        });
        assert!(issuance.contains("issuance of grant fixture-grant is uncertain"));
        assert!(issuance.contains("selection is unchanged"));
        let unselected = error(ReviewedConnectorFinalization::Persisted {
            generation: 18,
            grant: Err(StockGrantError::NotSelected {
                issued: reference(),
                reason: "compare".into(),
            }),
        });
        assert!(unselected.contains("issued but not selected"));
        let selection = error(ReviewedConnectorFinalization::Persisted {
            generation: 18,
            grant: Err(StockGrantError::SelectionUncertain {
                issued: reference(),
                previous: None,
                reason: "fsync".into(),
            }),
        });
        assert!(selection.contains("issued and selected, but selection durability is uncertain"));
        assert!(!selection.contains("not selected"));
        assert!(!selection.contains("No grant was issued"));
        let command = error(ReviewedConnectorFinalization::Persisted {
            generation: 18,
            grant: Err(StockGrantError::CommandUncertain),
        });
        assert!(command.contains("Saved mail"));
        assert!(command.contains("Whether it issued, selected or revoked a grant is unknown"));
        assert!(!command.contains("No grant was issued"));
        assert!(!command.contains("No new grant was issued"));
        assert!(!command.contains("selection is unchanged"));
        assert!(!command.contains("native access is selected"));
        for message in [issuance, unselected, selection, command] {
            assert!(message.contains("Do not repeat"));
        }
        let selected = finalization_message(
            "mail",
            "work",
            "reader",
            ReviewedConnectorFinalization::Persisted {
                generation: 18,
                grant: Ok(StockGrantOutcome {
                    issued: reference(),
                    superseded: None,
                    cleanup: vec![CleanupAttempt {
                        agent: "reader".into(),
                        grant: reference(),
                        outcome: RevocationOutcome::Uncertain,
                    }],
                }),
            },
        )
        .unwrap();
        assert!(selected.contains("native access is selected for agent reader"));
        assert!(selected.contains("Previous grant cleanup is incomplete"));
        assert!(!selected.contains("service is ready"));
    }

    #[tokio::test]
    async fn terminal_review_retires_when_its_exact_tool_waiter_disappears() {
        use std::os::unix::fs::PermissionsExt;
        for after_presentation in [false, true] {
            let root = tempfile::Builder::new()
                .permissions(std::fs::Permissions::from_mode(0o700))
                .tempdir()
                .unwrap();
            let (approvals, mut presenter) = LocalApprovals::create_paired_host(root.path()).unwrap();
            let session = SessionId::new();
            approvals.bind_session(&session).unwrap();
            let (mut response, receive) = oneshot::channel::<Value>();
            let (_input_send, input_receive) = mpsc::channel(1);
            let mut input = TerminalInput::scripted(input_receive);
            let mut output = Vec::new();
            let request = review_request(
                &session,
                "Review exact connector",
                json!({"account":"exact-account"}),
            )
            .unwrap();
            let interrupt = std::future::pending::<io::Result<()>>();
            tokio::pin!(interrupt);
            {
                let reviewing = review(
                    &approvals,
                    &mut presenter,
                    &mut input,
                    &mut output,
                    request,
                    &mut response,
                    interrupt.as_mut(),
                );
                tokio::pin!(reviewing);
                if after_presentation {
                    assert!(reviewing.as_mut().now_or_never().is_none());
                }
                drop(receive);
                assert!(matches!(reviewing.await.unwrap(), ReviewOutcome::CallerGone));
            }
            input.check_cleanup().unwrap();
            let service = meerkat_core::ApprovalService::with_store(Arc::new(
                meerkat_store::FileApprovalStore::open(approvals.store_path()).unwrap(),
            ))
            .unwrap();
            let records = service.list(Default::default()).unwrap();
            assert_eq!(records.len(), usize::from(after_presentation));
            assert!(records.iter().all(|record| record.decision.is_none()));
            assert!(!approvals.has_uncertain_dispatch());
            assert_eq!(!output.is_empty(), after_presentation);
        }
    }

    #[tokio::test]
    async fn terminal_management_review_expires_while_shown_without_a_model_timeout() {
        tokio::time::timeout(Duration::from_secs(2), async {
            let root = tempfile::tempdir().unwrap();
            let (approvals, mut presenter) = LocalApprovals::create(root.path()).unwrap();
            let session = SessionId::new();
            approvals.bind_session(&session).unwrap();
            let mut request = review_request(&session, "Review exact credential removal",
                json!({"phase":"service_credential_management","service":"cards","slots":["KEY"]})).unwrap();
            request.expires_at = Some((SystemTime::now() + Duration::from_millis(500)).into());
            let (mut caller, _receive) = oneshot::channel();
            let (lines, receive_lines) = mpsc::channel(1);
            let mut input = TerminalInput::scripted(receive_lines);
            let mut output = Vec::new();
            let interrupt = std::future::pending::<io::Result<()>>(); tokio::pin!(interrupt);
            let result = review(&approvals, &mut presenter, &mut input, &mut output,
                request, &mut caller, interrupt.as_mut()).await;
            assert!(matches!(result, Err(ConnectError::InvalidInput(ref message))
                if message == "The access review expired without a returned decision"));
            assert_eq!(String::from_utf8(output).unwrap().matches("Type 'approve ").count(), 1);
            input.check_cleanup().unwrap();
            lines.send(Ok("input after expiry".into())).await.unwrap();
            assert_eq!(input.recv().await.unwrap().unwrap(), "input after expiry");
            approvals.check_session(&session).unwrap();
            assert!(!approvals.has_uncertain_dispatch());
        }).await.expect("displayed credential review must retain its actual deadline");
    }

    #[tokio::test]
    async fn registered_interrupt_stops_review_before_entry_or_ready_approval() {
        use std::{os::unix::fs::PermissionsExt, sync::atomic::{AtomicUsize, Ordering}};
        for after_presentation in [false, true] {
            let root = tempfile::Builder::new()
                .permissions(std::fs::Permissions::from_mode(0o700))
                .tempdir().unwrap();
            let (approvals, mut presenter) = LocalApprovals::create_paired_host(root.path()).unwrap();
            let session = SessionId::new();
            approvals.bind_session(&session).unwrap();
            let (mut response, _receive) = oneshot::channel::<Value>();
            let (input_send, input_receive) = mpsc::channel(1);
            let mut input = TerminalInput::scripted(input_receive);
            let mut output = Vec::new();
            let request = review_request(
                &session, "Review exact connector", json!({"account":"exact-account"}),
            ).unwrap();
            let (cancel, cancelled) = oneshot::channel();
            let completed = Arc::new(AtomicUsize::new(0));
            let observed = completed.clone();
            let interrupt = async move {
                cancelled.await.unwrap();
                observed.fetch_add(1, Ordering::SeqCst);
                Ok(())
            };
            tokio::pin!(interrupt);
            assert!(interrupt.as_mut().now_or_never().is_none());
            let mut ready_approval = None;
            {
                let reviewing = review(
                    &approvals, &mut presenter, &mut input, &mut output,
                    request, &mut response, interrupt.as_mut(),
                );
                tokio::pin!(reviewing);
                if after_presentation {
                    assert!(reviewing.as_mut().now_or_never().is_none());
                    let service = meerkat_core::ApprovalService::with_store(Arc::new(
                        meerkat_store::FileApprovalStore::open(approvals.store_path()).unwrap(),
                    )).unwrap();
                    let records = service.list(Default::default()).unwrap();
                    assert_eq!(records.len(), 1);
                    let line = format!("approve {}", records[0].approval_id);
                    input_send.try_send(Ok(line.clone())).unwrap();
                    ready_approval = Some(line);
                }
                cancel.send(()).unwrap();
                assert!(matches!(reviewing.await.unwrap(), ReviewOutcome::Stopped));
            }
            // Returning without panic also proves review did not poll the
            // completed async interrupt again after its presenter consumed it.
            assert_eq!(completed.load(Ordering::SeqCst), 1);
            if let Some(line) = ready_approval {
                assert_eq!(input.recv().await.unwrap().unwrap(), line);
            }
            input.check_cleanup().unwrap();
            let service = meerkat_core::ApprovalService::with_store(Arc::new(
                meerkat_store::FileApprovalStore::open(approvals.store_path()).unwrap(),
            )).unwrap();
            let records = service.list(Default::default()).unwrap();
            assert_eq!(records.len(), usize::from(after_presentation));
            assert!(records.iter().all(|record| record.decision.is_none()));
            assert!(!approvals.has_uncertain_dispatch());
        }
    }

    #[tokio::test]
    async fn ordinary_review_approval_preserves_pending_interrupt() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir().unwrap();
        let (approvals, mut presenter) = LocalApprovals::create_paired_host(root.path()).unwrap();
        let session = SessionId::new();
        approvals.bind_session(&session).unwrap();
        let (mut response, _receive) = oneshot::channel::<Value>();
        let (input_send, input_receive) = mpsc::channel(1);
        let mut input = TerminalInput::scripted(input_receive);
        let mut output = Vec::new();
        let request = review_request(
            &session, "Review exact connector", json!({"account":"exact-account"}),
        ).unwrap();
        let (_cancel, cancelled) = oneshot::channel::<()>();
        let interrupt = async move {
            cancelled.await.unwrap();
            Ok(())
        };
        tokio::pin!(interrupt);
        assert!(interrupt.as_mut().now_or_never().is_none());
        {
            let reviewing = review(
                &approvals, &mut presenter, &mut input, &mut output,
                request, &mut response, interrupt.as_mut(),
            );
            tokio::pin!(reviewing);
            assert!(reviewing.as_mut().now_or_never().is_none());
            let service = meerkat_core::ApprovalService::with_store(Arc::new(
                meerkat_store::FileApprovalStore::open(approvals.store_path()).unwrap(),
            )).unwrap();
            let records = service.list(Default::default()).unwrap();
            assert_eq!(records.len(), 1);
            input_send.try_send(Ok(format!("approve {}", records[0].approval_id))).unwrap();
            let ReviewOutcome::Decided(decision) = reviewing.await.unwrap() else {
                panic!("the exact ordinary approval must be observed");
            };
            assert_eq!(decision.approval_id, records[0].approval_id);
            assert_eq!(decision.status, ApprovalStatus::Approved);
        }
        assert!(interrupt.as_mut().now_or_never().is_none());
        input.check_cleanup().unwrap();
        assert!(!approvals.has_uncertain_dispatch());
    }

    #[test]
    fn terminal_proposal_requires_explicit_lifetime_and_rejects_private_material() {
        let exact = exact_proposal();
        assert_eq!(proposal(exact.clone()).unwrap().expires_in_ms, Some(60000));
        let mut missing = exact.clone();
        missing.as_object_mut().unwrap().remove("expires_in_ms");
        assert!(proposal(missing).is_err());
        let mut until_revoked = exact.clone();
        until_revoked["expires_in_ms"] = Value::Null;
        assert_eq!(proposal(until_revoked).unwrap().expires_in_ms, None);
        let mut zero = exact.clone();
        zero["expires_in_ms"] = json!(0);
        assert!(proposal(zero).is_err());
        for (label, profile, expected) in [
            ("omitted", None, None),
            ("named", Some(json!("existing-profile")), Some("existing-profile")),
            ("null", Some(Value::Null), None),
        ] {
            let mut selected = exact.clone();
            if let Some(profile) = profile {
                selected["connector"]["client_auth_profile"] = profile;
            }
            let parsed = proposal(selected)
                .unwrap_or_else(|_| panic!("{label} profile selection must parse"));
            let target = parsed.connector.into_native(meerkat_core::RealmId::parse("work").unwrap())
                .unwrap_or_else(|_| panic!("{label} profile selection must retain its native target"));
            assert_eq!(target.client_auth_profile.as_deref(), expected, "{label} profile selection");
        }
        let mut private_wire = exact["connector"].clone();
        private_wire["client_auth_profile"] = Value::Null;
        assert!(serde_json::from_value::<crate::stdio::wire::ConnectorTarget>(private_wire).is_err(),
            "terminal null normalization must not relax the private-pipe contract");
        let mut private_profile = exact.clone();
        private_profile["connector"]["client_auth_profile"] = json!({"id":"mail","client_secret":"private"});
        assert!(proposal(private_profile).is_err());
        for key in [
            "client_secret",
            "access_token",
            "refresh_token",
            "authorize_url",
            "redirect_uri",
        ] {
            let mut injected = exact.clone();
            injected["connector"][key] = json!("private");
            assert!(
                proposal(injected).is_err(),
                "{key} must not enter the model tool contract"
            );
        }
        for key in [
            "command",
            "env",
            "grant",
            "approval_id",
            "native_association",
        ] {
            let mut injected = exact.clone();
            injected[key] = json!("untrusted");
            assert!(proposal(injected).is_err(), "{key} must remain host-owned");
        }
    }
}
