//! Checked public-service setup through the existing terminal and native owners.
//!
//! Explicit no-auth metadata describes a selected package profile. It grants
//! nothing. The protected review, realm configuration and agent grant remain
//! separate facts; this path never constructs an account or credential owner.

use super::*;
use crate::service_install::{PublicServiceSelection, RemoteSelection, SelectedServiceInputs};
use meerkat::governed_realm::stock::issuance::{
    ReviewedMcpInstallation, grant_stock_agent_for_reviewed_installation,
};
use meerkat_core::{ConfigDelta, McpRealmPersistTarget};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, path::Path};
use tokio::io::AsyncReadExt;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Proposal {
    job_id: String,
    operations: Vec<String>,
    agent: String,
    expires_in_ms: Option<u64>,
}

pub struct PlainRequest {
    selection: PublicServiceSelection,
    config: McpServerConfig,
    grant: StockGrantRequest,
    response: oneshot::Sender<Value>,
}

/// Immutable protected review with no live setup, auth or runtime handle.
pub struct ReviewedPlainInstallation {
    storage: ResolvedStorage,
    source: ReviewedPublicSource,
    config: McpServerConfig,
    own_servers: Vec<McpServerConfig>,
    expected_generation: u64,
    request: StockGrantRequest,
    decision: ApprovalRecord,
}

/// Immutable source custody survives setup disposal without retaining a dispatcher.
enum ReviewedPublicSource {
    Installed {
        installed: InstalledSelection,
        document: Arc<Value>,
        inputs: Option<SelectedServiceInputs>,
    },
    Remote(RemoteSelection),
}
impl ReviewedPublicSource {
    fn from_selection(value: &PublicServiceSelection) -> Self {
        match value {
            PublicServiceSelection::Installed(value) => Self::Installed {
                installed: value.installed.clone(),
                document: value.configuration.clone(),
                inputs: value.selected_inputs().cloned(),
            },
            PublicServiceSelection::Remote { selected, .. } => Self::Remote(selected.clone()),
        }
    }
    fn realm(&self) -> &meerkat_core::RealmLocator {
        match self {
            Self::Installed { installed, .. } => &installed.realm,
            Self::Remote(value) => &value.realm,
        }
    }
    fn session(&self) -> &SessionId {
        match self {
            Self::Installed { installed, .. } => &installed.session,
            Self::Remote(value) => &value.session,
        }
    }
    fn job_id(&self) -> &str {
        match self {
            Self::Installed { installed, .. } => &installed.job_id,
            Self::Remote(value) => &value.job_id,
        }
    }
    fn server_name(&self) -> &str {
        match self {
            Self::Installed { installed, .. } => &installed.server_name,
            Self::Remote(value) => value.server_name(),
        }
    }
    async fn recheck(
        &self,
        storage: &ResolvedStorage,
        config: &McpServerConfig,
    ) -> Result<(), ConnectError> {
        match self {
            Self::Installed {
                installed,
                document,
                inputs,
            } => recheck_retained(installed, document, inputs.as_ref()).await,
            Self::Remote(selected) => {
                if selected.realm != storage.locator {
                    return Err(unavailable("The selected remote realm changed"));
                }
                let operations = &config
                    .access
                    .as_ref()
                    .ok_or_else(|| unavailable("Remote operation ceiling is missing"))?
                    .allowed_operations;
                let request = selected.readback_request(operations);
                let resources = crate::resources::InstalledResources::locate()?;
                let current = resources.readback(storage, &request).await?;
                if current != selected.export
                    || selected.public_configuration(operations).ok().as_ref() != Some(config)
                {
                    return Err(unavailable(
                        "The reviewed remote export changed. No service finalization started.",
                    ));
                }
                Ok(())
            }
        }
    }
}

fn refused() -> ToolError {
    ToolError::execution_failed(
        "A current reconciled public source, explicit no-sign-in declaration and exact access proposal are required. No configuration or grant was written.",
    )
}

pub(super) fn tool() -> Arc<ToolDef> {
    Arc::new(ToolDef::new(
        PLAIN_TOOL,
        "Ask the protected terminal to save one reconciled public service and grant its chosen agent exact raw operations. Requires explicit authentication kind none and exact raw names from either current library publisher metadata or the selected package's canonical manifest declarations. Library profiles need explicit empty credential references and setup_help after setup_check. Outside-catalog Python/Node selections need the selected portable_mcp_entries operation_tools mapping. For a remote HTTP MCP, retain its researched recipe, export the exact selection and reconcile that export job. Local packages still require installation and reconciliation. Supply only job id, raw operations, consumer agent and explicit expires_in_ms (null means until revoked). No commands, config, credentials, accounts or grants are accepted. Finalization runs after setup closes. Pending review never means ready; this tool does not invoke the service.",
        json!({"type":"object","additionalProperties":false,
            "required":["job_id","operations","agent","expires_in_ms"],
            "properties":{"job_id":{"type":"string"},
                "operations":{"type":"array","minItems":1,"maxItems":128,"uniqueItems":true,
                    "items":{"type":"string","minLength":1,"maxLength":128}},
                "agent":{"type":"string"},
                "expires_in_ms":{"type":["integer","null"],"minimum":1}}}),
    ))
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
    let raw: Value = call.parse_args().map_err(|_| refused())?;
    if raw.get("expires_in_ms").is_none() {
        return Err(refused());
    }
    let proposal: Proposal = serde_json::from_value(raw).map_err(|_| refused())?;
    if proposal.expires_in_ms == Some(0)
        || proposal.operations.is_empty()
        || proposal.operations.len() > 128
        || proposal.operations.iter().any(|name| {
            name.is_empty()
                || name.len() > 128
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
        })
    {
        return Err(refused());
    }
    let operations: BTreeSet<_> = proposal.operations.iter().cloned().collect();
    if operations.len() != proposal.operations.len() {
        return Err(refused());
    }
    let session = context.origin_session_id().ok_or_else(refused)?;
    stock_agent_principal(&owner.storage.locator.realm, &proposal.agent).map_err(|_| refused())?;
    let selection = owner
        .installs
        .public_selection(&owner.storage.locator, session, &proposal.job_id)
        .map_err(|_| refused())?;
    let config = configuration(&selection, &operations).map_err(|_| refused())?;
    let grant = StockGrantRequest {
        agent: proposal.agent,
        expires_in_ms: proposal.expires_in_ms,
        target: StockGrantTarget::Mcp {
            server: config.name.clone(),
            operations: McpOperations::Exact(operations),
        },
    };
    let (response, receive) = oneshot::channel();
    owner
        .requests
        .try_send(ConnectorRequest::Plain(PlainRequest {
            selection,
            config,
            grant,
            response,
        }))
        .map_err(|_| refused())?;
    let result = receive.await.map_err(|_| ToolError::execution_failed(
        "Protected public-service review ended without a completed handoff. No readiness is established.",
    ))?;
    Ok(ToolResult::new(call.id.into(), result.to_string(), false).into())
}

/// Project only the package owner's observed stdio launch. The host-selected
/// ceiling uses the retained publisher or selected-manifest names, never aliases.
fn configuration(
    selected: &PublicServiceSelection,
    operations: &BTreeSet<String>,
) -> Result<McpServerConfig, &'static str> {
    selected.public_configuration(operations)
}

pub(super) async fn present<F: Future<Output = io::Result<()>> + Send, W: Write>(
    owner: &mut TerminalConnectors,
    mut request: PlainRequest,
    approvals: &LocalApprovals,
    presenter: &mut ApprovalPresenter,
    input: &mut TerminalInput,
    output: &mut W,
    interrupt: F,
) -> Result<bool, ConnectError> {
    if request.response.is_closed() {
        return Ok(true);
    }
    if owner.reviewed.is_some() {
        let _ = request.response.send(json!({"state":"pending",
            "reason":"reviewed_finalization_waiting_for_setup_close","grant_issued":false}));
        return Ok(true);
    }
    if owner.target_unresolved(&request.config.name) {
        return selection_refused("prior_finalization_unresolved", request.response, output);
    }
    if let Err(reason) = check_request(&request) {
        return selection_refused(reason, request.response, output);
    }
    if let Err(reason) = request_documents(
        ReviewedPublicSource::from_selection(&request.selection)
            .recheck(&owner.storage, &request.config),
    )
    .await
    {
        return selection_refused(reason, request.response, output);
    }
    let (own_servers, expected_generation) =
        crate::realm::read_own_mcp_config(&owner.storage).await?;
    let already_saved = match exact_saved_configuration(&own_servers, &request.config) {
        Ok(value) => value,
        Err(reason) => return selection_refused(reason, request.response, output),
    };
    if !already_saved && effective_name_present(&owner.storage, &request.config.name).await? {
        return selection_refused("server_name_already_selected", request.response, output);
    }
    let runtime = crate::realm::own_config_runtime(&owner.storage)?;
    preview(&runtime, &own_servers, &request.config).await?;
    let notice = if already_saved {
        "After setup closes, Connect rechecks the retained installation and realm configuration, reuses this exact saved server and asks native governance for a new grant of only these operations. This may replace selected access, and an earlier uncertain grant may remain. This review does not settle that earlier outcome. The grant is operator-managed server policy: a later authorized operator can reconfigure the server. The retained source declares no sign-in requirement; it does not prove anonymous execution or service readiness. Saved configuration and issued grants remain if a later step fails."
    } else {
        "After setup closes, Connect rechecks retained installation documents and realm generation, saves this exact server and asks native governance to grant only these server operations. This is operator-managed server policy: a later authorized operator reconfiguration can change the server. It does not promise continuing permission for one fixed endpoint or account. The retained source declares no sign-in is required; this is researched metadata and does not prove anonymous execution or an observed account. Saved configuration and issued grants remain if a later step fails. This review is not service readiness."
    };
    let body = json!({"phase":"public_service_access", "realm":owner.storage.locator.realm.as_str(),
        "job_id":request.selection.job_id(), "source":request.selection.review_source(), "authentication":{"kind":"none"},
        "server":request.config, "operation_basis":request.selection.operation_basis(), "grant":{
            "agent":request.grant.agent,"operations":request.config.access.as_ref().map(|access| &access.allowed_operations),
            "expires_in_ms":request.grant.expires_in_ms},
        "expected_configuration_generation":expected_generation,
        "configuration_action":if already_saved { "reuse_exact_saved_configuration" } else { "save_configuration" },
        "grant_scope":"operator_managed_server_policy",
        "notice":notice});
    tokio::pin!(interrupt);
    let decision = match review(
        approvals,
        presenter,
        input,
        output,
        review_request(
            request.selection.session(),
            if already_saved {
                "Review new access to this saved public service?"
            } else {
                "Enable this public service?"
            },
            body,
        )?,
        &mut request.response,
        interrupt.as_mut(),
    )
    .await?
    {
        ReviewOutcome::Decided(decision) => decision,
        ReviewOutcome::CallerGone => return Ok(true),
        ReviewOutcome::Stopped => return Ok(false),
    };
    if decision.status != ApprovalStatus::Approved {
        let _ = request
            .response
            .send(json!({"state":"denied","phase":"public_service_access","grant_issued":false}));
        return Ok(true);
    }
    if request.response.is_closed() {
        return Ok(true);
    }
    if let Err(reason) = check_request(&request) {
        return selection_refused(reason, request.response, output);
    }
    if let Err(reason) = request_documents(
        ReviewedPublicSource::from_selection(&request.selection)
            .recheck(&owner.storage, &request.config),
    )
    .await
    {
        return selection_refused(reason, request.response, output);
    }
    if let Err(reason) = check_request(&request) {
        return selection_refused(reason, request.response, output);
    }
    if request.response.is_closed() {
        return Ok(true);
    }
    let source = ReviewedPublicSource::from_selection(&request.selection);
    owner.reviewed = Some(
        ReviewedInstallation::Plain(ReviewedPlainInstallation {
            storage: owner.storage.clone(),
            source,
            config: request.config,
            own_servers,
            expected_generation,
            request: request.grant,
            decision,
        })
        .into(),
    );
    let _ = request.response.send(json!({"state":"pending",
        "reason":"reviewed_finalization_waiting_for_setup_close","access_reviewed":true,"grant_issued":false,
        "instruction":"Finish this turn. The terminal will close setup and report native persistence and grant results before continuing the conversation. Do not invoke the service, repeat this request or claim readiness before that result."}));
    Ok(true)
}

fn check_request(request: &PlainRequest) -> Result<(), &'static str> {
    let StockGrantTarget::Mcp {
        server,
        operations: McpOperations::Exact(operations),
    } = &request.grant.target
    else {
        return Err("invalid_operation_binding");
    };
    if server != &request.config.name
        || configuration(&request.selection, operations)? != request.config
    {
        return Err("installation_changed");
    }
    Ok(())
}

// Reuse only one exact descriptor from this realm's own reviewed snapshot.
// A matching name alone never selects a replacement or establishes prior access.
fn exact_saved_configuration(
    own_servers: &[McpServerConfig],
    server: &McpServerConfig,
) -> Result<bool, &'static str> {
    let mut named = own_servers.iter().filter(|value| value.name == server.name);
    match (named.next(), named.next()) {
        (None, None) => Ok(false),
        (Some(value), None) if value == server => Ok(true),
        _ => Err("server_name_already_selected"),
    }
}

// Called only for an absent own definition. This protects initial setup from a
// known inherited-name collision; it is a preflight, not atomic exclusion of
// concurrent parent changes. The native own-document generation still admits add.
async fn effective_name_present(
    storage: &ResolvedStorage,
    name: &str,
) -> Result<bool, ConnectError> {
    Ok(crate::realm::read_config(storage)
        .await?
        .tools
        .mcp_servers
        .iter()
        .any(|server| server.name == name))
}

async fn preview(
    runtime: &meerkat_core::ConfigRuntime,
    own_servers: &[McpServerConfig],
    server: &McpServerConfig,
) -> Result<(), ConnectError> {
    let mut candidate = own_servers.to_vec();
    if !exact_saved_configuration(own_servers, server).map_err(unavailable)? {
        candidate.push(server.clone());
    }
    runtime
        .patch_preview(&ConfigDelta(json!({"tools":{"mcp_servers":candidate}})))
        .await
        .map_err(|error| ConnectError::native("Preview public-service configuration", error))?
        .validate_for_write(meerkat_models::canonical(), own_servers)
        .map_err(|error| ConnectError::native("Validate public-service configuration", error))?;
    Ok(())
}

impl ReviewedPlainInstallation {
    /// Main consumes this after the exact setup session and dispatchers close.
    /// Its existing finalization wrapper joins every effect once entered.
    pub async fn finalize_after_close<W: Write>(self, output: &mut W) -> Result<(), ConnectError> {
        self.finalize_outcome_after_close().await.display(output)
    }

    pub(super) async fn finalize_outcome_after_close(self) -> FinalizedInstallation {
        let interrupt = std::future::pending::<io::Result<()>>();
        tokio::pin!(interrupt);
        self.finalize_outcome_with_interrupt(interrupt.as_mut())
            .await
    }

    pub(super) async fn finalize_outcome_with_interrupt<I: Future<Output = io::Result<()>>>(
        self,
        mut interrupt: Pin<&mut I>,
    ) -> FinalizedInstallation {
        let target = FinalizationTarget::new(
            &self.storage,
            &self.config,
            &self.request,
            self.source.job_id(),
            self.source.session(),
        );
        let mut stopped = None;
        let outcome = self
            .finalize_effect_after_close(interrupt.as_mut(), &mut stopped)
            .await
            .unwrap_or_else(FinalizationOutcome::BeforeAdministrationRefused);
        FinalizedInstallation::new_with_stop(target, outcome, stopped)
    }

    async fn finalize_effect_after_close<I: Future<Output = io::Result<()>>>(
        self,
        interrupt: Pin<&mut I>,
        stopped: &mut Option<secrets::Stop>,
    ) -> Result<FinalizationOutcome, ConnectError> {
        let StockGrantTarget::Mcp {
            server,
            operations: McpOperations::Exact(operations),
        } = &self.request.target
        else {
            return Err(unavailable(
                "The exact public-service review is required. Nothing was finalized.",
            ));
        };
        if self.decision.status != ApprovalStatus::Approved
            || self.config.connector.is_some()
            || self.source.realm() != &self.storage.locator
            || server != &self.config.name
            || self.source.server_name() != self.config.name
            || operations.is_empty()
            || self
                .config
                .access
                .as_ref()
                .is_none_or(|access| &access.allowed_operations != operations)
        {
            return Err(unavailable(
                "The exact public-service review is required. Nothing was finalized.",
            ));
        }
        self.source.recheck(&self.storage, &self.config).await?;
        let runtime = crate::realm::own_config_runtime(&self.storage)?;
        let current = McpRealmPersistTarget::new(&runtime)
            .servers()
            .await
            .map_err(|error| {
                ConnectError::native("Recheck public-service realm configuration", error)
            })?;
        if current != (self.own_servers.clone(), self.expected_generation) {
            return Err(unavailable(
                "The reviewed realm configuration changed. No public-service configuration or grant was written.",
            ));
        }
        preview(&runtime, &self.own_servers, &self.config).await?;
        let principals = StockPrincipals::for_local_operator(&self.storage.locator.realm)
            .map_err(|error| ConnectError::native("Identify selected realm operator", error))?;
        let reused =
            exact_saved_configuration(&self.own_servers, &self.config).map_err(unavailable)?;
        if !reused && effective_name_present(&self.storage, &self.config.name).await? {
            return Err(unavailable(
                "This service name is already configured in the selected realm. No public-service configuration or grant was written.",
            ));
        }
        let (storage, config, request, principals) =
            (&self.storage, &self.config, &self.request, &principals);
        Ok(settle_public_effects(
            interrupt,
            stopped,
            reused.then_some(self.expected_generation),
            || async {
                McpRealmPersistTarget::new(&runtime)
                    .add(self.config.clone(), Some(self.expected_generation))
                    .await
                    .map(|saved| saved.generation)
            },
            |generation| async move {
                // Native rechecks this exact descriptor and generation under
                // its own custody. A stopped config write cannot start this call.
                let installation = ReviewedMcpInstallation {
                    server: config.clone(),
                    generation,
                };
                grant_stock_agent_for_reviewed_installation(
                    storage,
                    principals,
                    request,
                    &installation,
                )
                .await
            },
        )
        .await)
    }
}

/// One sequence for the production config and grant entry points. Cancellation
/// gates each effect; an entered effect always settles into its original type.
async fn settle_public_effects<I, P, PF, G, GF>(
    mut interrupt: Pin<&mut I>,
    stopped: &mut Option<secrets::Stop>,
    existing_generation: Option<u64>,
    persist: P,
    grant: G,
) -> FinalizationOutcome
where
    I: Future<Output = io::Result<()>>,
    P: FnOnce() -> PF,
    PF: Future<Output = Result<u64, meerkat_core::mcp_config::McpRealmPersistError>>,
    G: FnOnce(u64) -> GF,
    GF: Future<
        Output = Result<
            meerkat::governed_realm::stock::issuance::StockGrantOutcome,
            StockGrantError,
        >,
    >,
{
    let reused = existing_generation.is_some();
    let generation = match existing_generation {
        Some(generation) => generation,
        None => match secrets::joined_effect(interrupt.as_mut(), stopped, persist).await {
            None => return FinalizationOutcome::BeforeAdministrationStopped,
            Some(Ok(generation)) => generation,
            Some(Err(error)) => return FinalizationOutcome::PublicPersistenceError(error),
        },
    };
    let Some(grant) =
        secrets::joined_effect(interrupt.as_mut(), stopped, || grant(generation)).await
    else {
        return FinalizationOutcome::PublicStoppedBeforeGrant { generation, reused };
    };
    if reused {
        FinalizationOutcome::PublicReused { generation, grant }
    } else {
        FinalizationOutcome::PublicPersisted { generation, grant }
    }
}

#[cfg(test)]
#[path = "plain_cancellation_tests.rs"]
mod cancellation_tests;

/// Check documents already reconciled by the software owner. This is not a
/// second payload verifier or an atomic filesystem snapshot. Exclusive host
/// custody of the reconciled installation remains required.
pub(super) async fn recheck_retained(
    installed: &InstalledSelection,
    expected: &Value,
    selected_inputs: Option<&SelectedServiceInputs>,
) -> Result<(), ConnectError> {
    let changed = || {
        unavailable(
            "Retained integration documents changed or could not be read. Service finalization did not start.",
        )
    };
    let receipt = bounded_read(&installed.receipt_path).await?;
    if installed.receipt_sha256.as_deref()
        != Some(format!("{:x}", Sha256::digest(&receipt)).as_str())
    {
        return Err(changed());
    }
    let bytes = bounded_read(&installed.mcp_path).await?;
    serde_json::from_slice::<crate::forge::UniqueJson>(&bytes).map_err(|_| changed())?;
    let document: Value = serde_json::from_slice(&bytes).map_err(|_| changed())?;
    if &document != expected {
        return Err(changed());
    }
    let root = installed
        .mcp_path
        .parent()
        .and_then(Path::parent)
        .ok_or_else(changed)?;
    if let Some(inputs) = selected_inputs {
        // Python/Node installers retain these exact validated inputs. They do
        // not have the library's configuration/selected.json layout.
        for (name, expected) in [
            ("package.json", inputs.package_sha256.as_str()),
            (inputs.document.filename(), inputs.document.sha256()),
        ] {
            let bytes = bounded_read(&root.join("inputs").join(name)).await?;
            if format!("{:x}", Sha256::digest(&bytes)) != expected {
                return Err(changed());
            }
        }
    } else {
        let configuration = bounded_read(&root.join("configuration/selected.json")).await?;
        if installed.configuration_sha256.as_deref()
            != Some(format!("{:x}", Sha256::digest(&configuration)).as_str())
        {
            return Err(changed());
        }
    }
    Ok(())
}

async fn bounded_read(path: &Path) -> Result<Vec<u8>, ConnectError> {
    let failed = || {
        unavailable(
            "A retained integration document is unavailable. Service finalization did not start.",
        )
    };
    let file = tokio::fs::File::open(path).await.map_err(|_| failed())?;
    if !file.metadata().await.map_err(|_| failed())?.is_file() {
        return Err(failed());
    }
    let mut bytes = Vec::new();
    file.take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| failed())?;
    if bytes.len() > 1024 * 1024 {
        return Err(failed());
    }
    Ok(bytes)
}

#[cfg(test)]
mod selected_tests {
    use super::*;

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
    async fn answer_after_display(
        seen: &Seen,
        lines: &mpsc::Sender<io::Result<String>>,
        change: impl FnOnce(),
    ) {
        while !seen.text().contains("Type 'approve ") {
            tokio::task::yield_now().await;
        }
        change();
        let text = seen.text();
        let id = text
            .split("Type 'approve ")
            .last()
            .unwrap()
            .split('\'')
            .next()
            .unwrap();
        lines.send(Ok(format!("approve {id}"))).await.unwrap();
    }
    async fn terminal_owner(root: &Path, storage: ResolvedStorage) -> TerminalConnectors {
        use meerkat_auth_core::auth_store::{
            FileLockCoordinator, FileTokenStore, ProviderAuthPersistence,
        };
        let persistence = ProviderAuthPersistence::new(
            Arc::new(FileTokenStore::new(root.join("credentials"))),
            Arc::new(FileLockCoordinator::new(root.join("locks"))),
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

    #[tokio::test]
    async fn actual_public_and_account_reviews_keep_document_refusals_local_and_presenter_reusable()
    {
        tokio::time::timeout(Duration::from_secs(30),Box::pin(async {
            for account in [false,true] { for after_approval in [false,true] { for missing in [false,true] {
                let root=tempfile::tempdir().unwrap();
                let fixture=crate::stdio::tests::selected_public_fixture(root.path(),Some(declaration(json!({"kind":"none"})))).await;
                let selected=fixture.raw_selection().ok().unwrap();
                let storage=crate::RealmOptions { context_root:root.path().to_owned(),state_root:Some(root.path().join("state")),
                    user_config_root:Some(root.path().join("home")) }.resolve("selected-owner").unwrap();
                assert_eq!(storage.locator,selected.installed.realm);
                let mut owner=terminal_owner(root.path(),storage.clone()).await;
                let before_config = crate::realm::read_own_mcp_config(&storage).await.unwrap();
                let (approvals,mut presenter)=LocalApprovals::create_paired_host(root.path()).unwrap();
                approvals.bind_session(&selected.installed.session).unwrap();
                let (lines,receive_lines)=mpsc::channel(1);let mut input=TerminalInput::scripted(receive_lines);
                let seen=Seen::default();let mut output=seen.clone();
                let names=BTreeSet::from(["actual_read".to_owned()]);
                let config=selected.public_configuration(&names).unwrap();
                // Account review retains the library document layout. These
                // are controlled review inputs, not an installed account proof.
                let mut account_installed = selected.installed.clone();
                let configuration = b"controlled account configuration";
                let configuration_path = account_installed.mcp_path.parent().unwrap()
                    .parent().unwrap().join("configuration/selected.json");
                std::fs::create_dir_all(configuration_path.parent().unwrap()).unwrap();
                std::fs::write(&configuration_path, configuration).unwrap();
                account_installed.configuration_sha256 = Some(format!("{:x}", Sha256::digest(configuration)));
                let path=selected.installed.receipt_path.clone();let bytes=std::fs::read(&path).unwrap();
                let change=|| {if missing {std::fs::remove_file(&path).unwrap()} else {std::fs::write(&path,b"changed selected receipt").unwrap()}};
                if !after_approval {change();}
                let (response,receive)=oneshot::channel();
                let grant=StockGrantRequest {agent:"rkat-agent".into(),expires_in_ms:Some(60000),
                    target:StockGrantTarget::Mcp {server:config.name.clone(),operations:McpOperations::Exact(names.clone())}};
                let presenting=async {
                    if account {
                        // Controlled historical review input. Document refusal
                        // precedes native account observation; no signed-in
                        // account, installation or account authorization is claimed.
                        use meerkat_core::connection::{CredentialAccountId,CredentialAccountRef};
                        let target=meerkat::ConnectorOAuthTarget {slot:CredentialAccountRef {
                            realm:storage.locator.realm.clone(),account:CredentialAccountId::parse("mail").unwrap()},
                            issuer:"https://issuer.invalid".into(),client:"client".into(),resource:"https://service.invalid".into(),
                            scopes:["read".into()].into(),strategy_id:"oidc-userinfo-v1".into(),
                            account:meerkat_auth_core::connector_oauth::AccountSelection::Discover,client_auth_profile:None};
                        let account=meerkat::ConnectorVerifiedAccount {issuer:target.issuer.clone(),
                            strategy_id:target.strategy_id.clone(),subject:"historical-review-fixture".into()};
                        let interrupt=std::future::pending::<io::Result<()>>();tokio::pin!(interrupt);
                        owner.review_account(AccountReview {installed:account_installed,document:selected.configuration.clone(),
                            target,account,config:config.clone(),grant:grant.clone(),response,login_completed:false,client_profile:None},
                            &approvals,&mut presenter,&mut input,&mut output,||selected.check_current().map_err(|_| "selected_documents_unconfirmed"),interrupt.as_mut()).await
                    } else {
                        present(&mut owner,PlainRequest {selection:selected.clone().into(),config:config.clone(),grant:grant.clone(),response},
                            &approvals,&mut presenter,&mut input,&mut output,std::future::pending::<io::Result<()>>()).await
                    }
                };
                let human=async {if after_approval {answer_after_display(&seen,&lines,change).await;}};
                let (result,())=tokio::join!(presenting,human);
                assert!(result.unwrap());
                assert_eq!(seen.text().contains("Type 'approve "), after_approval);
                let refused=receive.await.unwrap();assert_eq!(refused["reason"],"selected_documents_unconfirmed");
                assert_eq!(refused["grant_issued"],false);assert!(owner.take_reviewed().is_none());
                assert!(!approvals.has_uncertain_dispatch());approvals.check_session(&selected.installed.session).unwrap();
                assert_eq!(crate::realm::read_own_mcp_config(&storage).await.unwrap(), before_config);
                input.check_cleanup().unwrap();
                std::fs::write(&path,&bytes).unwrap();seen.0.lock().unwrap().clear();
                let (response,receive)=oneshot::channel();
                let presenting=present(&mut owner,PlainRequest {selection:selected.clone().into(),config:config.clone(),grant,response},
                    &approvals,&mut presenter,&mut input,&mut output,std::future::pending::<io::Result<()>>());
                let (result,())=tokio::join!(presenting,answer_after_display(&seen,&lines,||{}));
                assert!(result.unwrap());assert_eq!(receive.await.unwrap()["reason"],"reviewed_finalization_waiting_for_setup_close");
                assert!(matches!(owner.take_reviewed(),Some(ReviewedInstallation::Plain(_))));
                assert!(!approvals.has_uncertain_dispatch());
                assert_eq!(crate::realm::read_own_mcp_config(&storage).await.unwrap(), before_config);
                owner.retire().unwrap();
            }}}
        })).await.expect("request-local document refusal and subsequent review must settle");
    }

    // Keep the original presenter, response wait and input driver through the
    // observation deadline. A deadline interrupts the presenter, then joins the
    // same work; it never drops an entered native operation or starts another.
    async fn joined_inherited_review<T>(
        presenting: impl Future<Output = Result<bool, ConnectError>>,
        response: impl Future<Output = T>,
        approve: bool,
        seen: &Seen,
        lines: &mpsc::Sender<io::Result<String>>,
        interrupt: oneshot::Sender<()>,
    ) -> (
        bool,
        std::thread::Result<Result<bool, ConnectError>>,
        std::thread::Result<T>,
        std::thread::Result<Result<(), &'static str>>,
    ) {
        let (done, mut finished) = oneshot::channel();
        let presenting = async {
            let result = std::panic::AssertUnwindSafe(presenting)
                .catch_unwind()
                .await;
            let _ = done.send(());
            result
        };
        let human = async {
            if !approve {
                return Ok(());
            }
            loop {
                let text = seen.text();
                if let Some((_, rest)) = text.rsplit_once("Type 'approve ") {
                    let id = rest.split('\'').next().ok_or("missing review id")?;
                    return tokio::select! { biased;
                        _ = &mut finished => Err("review ended before input"),
                        sent = lines.send(Ok(format!("approve {id}"))) => sent.map_err(|_| "review input closed"),
                    };
                }
                tokio::select! { biased;
                    _ = &mut finished => return Err("review ended without a prompt"),
                    _ = tokio::task::yield_now() => {},
                }
            }
        };
        let mut work = Box::pin(async {
            tokio::join!(
                presenting,
                std::panic::AssertUnwindSafe(response).catch_unwind(),
                std::panic::AssertUnwindSafe(human).catch_unwind()
            )
        });
        let (timed_out, result) =
            match tokio::time::timeout(Duration::from_secs(10), work.as_mut()).await {
                Ok(result) => (false, result),
                Err(_) => {
                    let _ = interrupt.send(());
                    (true, work.as_mut().await)
                }
            };
        drop(work);
        (timed_out, result.0, result.1, result.2)
    }

    // Extends the exact-owned-reuse oracle below. The package observation is
    // synthetic; these schedules use real local config, dispatcher, protected
    // presenter and finalization owners, without launching the selected server.
    async fn inherited_public_name_controls() {
        use meerkat::governed_realm::stock::govern_stock_realm;
        use meerkat_core::{ConfigStore, FileConfigStore};
        for mode in ["before_review", "after_review", "unrelated_parent"] {
            let root = tempfile::tempdir().unwrap();
            let fixture = crate::stdio::tests::selected_public_fixture(
                root.path(),
                Some(declaration(json!({"kind":"none"}))),
            )
            .await;
            let selected = fixture.raw_selection().ok().unwrap();
            let storage = crate::RealmOptions {
                context_root: root.path().to_owned(),
                state_root: Some(root.path().join("state")),
                user_config_root: Some(root.path().join("home")),
            }
            .resolve("selected-owner")
            .unwrap();
            let names = BTreeSet::from(["actual_read".to_owned()]);
            let config = selected.public_configuration(&names).unwrap();
            let grant = StockGrantRequest {
                agent: "rkat-agent".into(),
                expires_in_ms: Some(60000),
                target: StockGrantTarget::Mcp {
                    server: config.name.clone(),
                    operations: McpOperations::Exact(names),
                },
            };
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
            govern_stock_realm(
                &storage.locator.state_root,
                storage.locator.realm.as_str(),
                &principals,
            )
            .await
            .unwrap();
            let parent_path = storage.layout.user_rkat_root().unwrap().join("config.toml");
            let parent_file =
                FileConfigStore::new(parent_path.clone(), meerkat_models::canonical());
            let mut parent_server = config.clone();
            if mode != "before_review" {
                parent_server.name = "unrelated-inherited".into();
            }
            parent_file
                .patch(ConfigDelta(
                    json!({"tools":{"mcp_servers":[parent_server]}}),
                ))
                .await
                .unwrap();
            let config_path = crate::realm::config_source(&storage)
                .unwrap()
                .config_doc_path(&storage.locator.realm);
            let state_path = config_path.with_file_name("config_state.json");
            let paths = meerkat_store::realm_paths_in(
                &storage.locator.state_root,
                storage.locator.realm.as_str(),
            );
            let owner_path = meerkat_store::grant_owner_store_path(&paths.root);
            let mut owner = terminal_owner(root.path(), storage.clone()).await;
            let before = crate::realm::read_own_mcp_config(&storage).await.unwrap();
            assert!(before.0.is_empty());
            let before_config = std::fs::read(&config_path).ok();
            let before_state = std::fs::read(&state_path).ok();
            let before_owner = std::fs::read(&owner_path).ok();
            let before_parent = std::fs::read(&parent_path).unwrap();
            let (approvals, mut presenter) =
                LocalApprovals::create_paired_host(root.path()).unwrap();
            approvals.bind_session(&selected.installed.session).unwrap();
            let (lines, receive_lines) = mpsc::channel(1);
            let mut input = TerminalInput::scripted(receive_lines);
            let seen = Seen::default();
            let mut output = seen.clone();
            let (response, receive) = oneshot::channel();
            let (stop, stopped) = oneshot::channel();
            let request = ConnectorRequest::Plain(PlainRequest {
                selection: selected.clone().into(),
                config: config.clone(),
                grant,
                response,
            });
            let (timed_out, presented, reply, human) = joined_inherited_review(
                owner.present(
                    request,
                    &approvals,
                    &mut presenter,
                    &mut input,
                    &mut output,
                    async {
                        let _ = stopped.await;
                        Ok(())
                    },
                ),
                receive,
                mode != "before_review",
                &seen,
                &lines,
                stop,
            )
            .await;
            let cleanup = input.check_cleanup();
            if timed_out
                || !matches!(&presented, Ok(Ok(true)))
                || !matches!(&reply, Ok(Ok(_)))
                || !matches!(&human, Ok(Ok(())))
            {
                let _ = owner.retire();
                approvals.retire();
            }
            assert!(
                !timed_out,
                "{mode}: original public review joined after its deadline"
            );
            assert!(
                presented
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
                    .unwrap()
            );
            human
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
                .unwrap();
            let reply = reply
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
                .unwrap();
            cleanup.unwrap();
            assert_eq!(reply["grant_issued"], false);
            assert_eq!(
                crate::realm::read_own_mcp_config(&storage).await.unwrap(),
                before
            );
            assert_eq!(std::fs::read(&config_path).ok(), before_config);
            assert_eq!(std::fs::read(&state_path).ok(), before_state);
            assert_eq!(std::fs::read(&owner_path).ok(), before_owner);
            assert_eq!(std::fs::read(&parent_path).unwrap(), before_parent);
            approvals
                .check_session(&selected.installed.session)
                .unwrap();
            assert!(!approvals.has_uncertain_dispatch());
            if mode == "before_review" {
                assert_eq!(reply["reason"], "server_name_already_selected");
                assert!(!owner.has_reviewed());
                assert!(!seen.text().contains("Type 'approve "));
                // A permitted sibling uses the same dispatcher/presenter and
                // admission while the inherited name remains in its owner.
                let tools = owner.dispatcher(fixture.owner.clone());
                let context = ToolDispatchContext::default()
                    .with_runtime_identity(selected.installed.session.clone(), None);
                let args = serde_json::value::to_raw_value(&json!({})).unwrap();
                let mut waiting = Box::pin(tools.dispatch_with_context(
                    ToolCallView {
                        id: "sibling-inventory",
                        name: ACCESS_INVENTORY_TOOL,
                        args: &args,
                    },
                    &context,
                ));
                assert!(waiting.as_mut().now_or_never().is_none());
                let request = owner.requests.try_recv().unwrap();
                let (stop, stopped) = oneshot::channel();
                let (timed_out, presented, reply, human) = joined_inherited_review(
                    owner.present(
                        request,
                        &approvals,
                        &mut presenter,
                        &mut input,
                        &mut output,
                        async {
                            let _ = stopped.await;
                            Ok(())
                        },
                    ),
                    waiting.as_mut(),
                    false,
                    &seen,
                    &lines,
                    stop,
                )
                .await;
                drop(waiting);
                let cleanup = input.check_cleanup();
                if timed_out
                    || !matches!(&presented, Ok(Ok(true)))
                    || !matches!(&reply, Ok(Ok(_)))
                    || !matches!(&human, Ok(Ok(())))
                {
                    let _ = owner.retire();
                    approvals.retire();
                }
                assert!(!timed_out, "original sibling joined after its deadline");
                assert!(
                    presented
                        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
                        .unwrap()
                );
                human
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
                    .unwrap();
                let reply = reply
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
                    .unwrap();
                cleanup.unwrap();
                assert!(!reply.result.is_error);
                let inventory: Value = serde_json::from_str(&reply.result.text_content()).unwrap();
                assert_eq!(inventory["scope"], "own_configured_services_only");
                assert_eq!(inventory["services"], json!([]));
                assert_eq!(inventory["current_grants"], "not_inspected");
                assert!(!owner.has_reviewed());
                approvals
                    .check_session(&selected.installed.session)
                    .unwrap();
                assert!(!approvals.has_uncertain_dispatch());
                assert!(!seen.text().contains("Type 'approve "));
            } else {
                assert_eq!(
                    reply["reason"],
                    "reviewed_finalization_waiting_for_setup_close"
                );
                assert!(seen.text().contains("save_configuration"));
                let reviewed = owner.take_reviewed().unwrap();
                if mode == "after_review" {
                    // Parent content can change without this realm's own
                    // generation changing. Recheck just before the initial add.
                    parent_file
                        .patch(ConfigDelta(json!({"tools":{"mcp_servers":[config]}})))
                        .await
                        .unwrap();
                    assert_eq!(
                        crate::realm::read_own_mcp_config(&storage).await.unwrap(),
                        before
                    );
                    let current_parent = std::fs::read(&parent_path).unwrap();
                    owner.retire().unwrap();
                    let mut effect = Box::pin(
                        std::panic::AssertUnwindSafe(reviewed.finalize_outcome_after_close())
                            .catch_unwind(),
                    );
                    let (timed_out, result) = match tokio::time::timeout(
                        Duration::from_secs(10),
                        effect.as_mut(),
                    )
                    .await
                    {
                        Ok(result) => (false, result),
                        Err(_) => (true, effect.as_mut().await),
                    };
                    drop(effect);
                    let cleanup = input.check_cleanup();
                    approvals.retire();
                    assert!(!timed_out, "original finalizer joined after its deadline");
                    let finalized = result.unwrap_or_else(|panic| std::panic::resume_unwind(panic));
                    cleanup.unwrap();
                    assert!(
                        matches!(&finalized.outcome, FinalizationOutcome::BeforeAdministrationRefused(error)
                        if error.to_string().contains("service name is already configured in the selected realm"))
                    );
                    assert_eq!(finalized.notice()["facts"]["persistence"], "not_entered");
                    assert_eq!(std::fs::read(&parent_path).unwrap(), current_parent);
                }
                // The unrelated-parent branch proves review eligibility only;
                // it does not claim a configured service, grant or service use.
            }
            assert_eq!(
                crate::realm::read_own_mcp_config(&storage).await.unwrap(),
                before
            );
            assert_eq!(std::fs::read(&config_path).ok(), before_config);
            assert_eq!(std::fs::read(&state_path).ok(), before_state);
            assert_eq!(std::fs::read(&owner_path).ok(), before_owner);
            if mode != "after_review" {
                assert_eq!(std::fs::read(&parent_path).unwrap(), before_parent);
            }
            owner.retire().unwrap();
            approvals.retire();
            input.check_cleanup().unwrap();
        }
    }

    #[tokio::test]
    async fn exact_saved_public_configuration_requires_a_new_review_and_preserves_prior_access() {
        use meerkat::governed_realm::stock::govern_stock_realm;
        use meerkat_core::{ConfigStore, FileConfigStore};
        tokio::time::timeout(Duration::from_secs(60), Box::pin(async {
            for mode in ["approve", "deny", "changed", "conflict"] {
                let root = tempfile::tempdir().unwrap();
                // The existing selected-package fixture is inert. Only the
                // local review, config writer and grant owner below are real;
                // no server is launched or native setup-close proof claimed.
                let fixture = crate::stdio::tests::selected_public_fixture(root.path(),
                    Some(declaration(json!({"kind":"none"})))).await;
                let selected = fixture.raw_selection().ok().unwrap();
                let storage = crate::RealmOptions {
                    context_root: root.path().to_owned(), state_root: Some(root.path().join("state")),
                    user_config_root: Some(root.path().join("home")),
                }.resolve("selected-owner").unwrap();
                assert_eq!(storage.locator, selected.installed.realm);
                let names = BTreeSet::from(["actual_read".to_owned()]);
                let config = selected.public_configuration(&names).unwrap();
                let grant = StockGrantRequest { agent:"rkat-agent".into(), expires_in_ms:Some(60000),
                    target:StockGrantTarget::Mcp { server:config.name.clone(),
                        operations:McpOperations::Exact(names) } };
                let runtime = crate::realm::own_config_runtime(&storage).unwrap();
                let saved = McpRealmPersistTarget::new(&runtime).add(config.clone(), Some(0)).await.unwrap();
                let (_, bundle) = meerkat::storage_provider::open_realm_persistence_with_layout(
                    storage.layout.clone(), storage.locator.realm.as_str(), None,
                    Some(meerkat_store::RealmOrigin::Explicit),
                ).await.unwrap();
                drop(bundle);
                let principals = StockPrincipals::for_local_operator(&storage.locator.realm).unwrap();
                govern_stock_realm(&storage.locator.state_root, storage.locator.realm.as_str(), &principals).await.unwrap();
                // A prior real grant is deliberately present. Saved config is
                // never treated as proof that a prior request issued nothing.
                grant_stock_agent_for_reviewed_installation(&storage, &principals, &grant,
                    &ReviewedMcpInstallation { server:config.clone(), generation:saved.generation }).await.unwrap();
                let config_path = crate::realm::config_source(&storage).unwrap().config_doc_path(&storage.locator.realm);
                let state_path = config_path.with_file_name("config_state.json");
                let paths = meerkat_store::realm_paths_in(&storage.locator.state_root, storage.locator.realm.as_str());
                let owner_path = meerkat_store::grant_owner_store_path(&paths.root);
                let file = FileConfigStore::new(config_path.clone(), meerkat_models::canonical());
                let mut different = config.clone();
                different.access.as_mut().unwrap().allowed_operations.insert("other_operation".into());
                if mode == "conflict" {
                    file.patch(ConfigDelta(json!({"tools":{"mcp_servers":[different.clone()]}}))).await.unwrap();
                }
                let before_config = std::fs::read(&config_path).unwrap();
                let before_state = std::fs::read(&state_path).unwrap();
                let before_owner = std::fs::read(&owner_path).unwrap();
                let before = crate::realm::read_own_mcp_config(&storage).await.unwrap();
                let mut owner = terminal_owner(root.path(), storage.clone()).await;
                let (approvals, mut presenter) = LocalApprovals::create_paired_host(root.path()).unwrap();
                approvals.bind_session(&selected.installed.session).unwrap();
                let (lines, receive_lines) = mpsc::channel(1);
                let mut input = TerminalInput::scripted(receive_lines);
                let seen = Seen::default(); let mut output = seen.clone();
                let (response, receive) = oneshot::channel();
                let presenting = present(&mut owner, PlainRequest {
                    selection:selected.clone().into(), config:config.clone(), grant, response,
                }, &approvals, &mut presenter, &mut input, &mut output,
                    std::future::pending::<io::Result<()>>());
                let human = async {
                    if mode != "conflict" {
                        while !seen.text().contains("Type 'approve ") { tokio::task::yield_now().await; }
                        let text = seen.text();
                        let id = text.split("Type 'approve ").last().unwrap().split('\'').next().unwrap();
                        let decision = if mode == "deny" { "deny" } else { "approve" };
                        lines.send(Ok(format!("{decision} {id}"))).await.unwrap();
                    }
                };
                let (result, ()) = tokio::join!(presenting, human);
                assert!(result.unwrap(), "{mode}");
                let reply = receive.await.unwrap();
                assert_eq!(crate::realm::read_own_mcp_config(&storage).await.unwrap(), before);
                assert_eq!(std::fs::read(&config_path).unwrap(), before_config);
                assert_eq!(std::fs::read(&state_path).unwrap(), before_state);
                assert_eq!(std::fs::read(&owner_path).unwrap(), before_owner,
                    "protected review itself cannot issue or replace prior access");
                assert_eq!(reply["grant_issued"], false);
                if mode == "conflict" {
                    assert_eq!(reply["reason"], "server_name_already_selected");
                    assert!(!seen.text().contains("Type 'approve "));
                    assert!(owner.take_reviewed().is_none());
                } else {
                    assert!(seen.text().contains("reuse_exact_saved_configuration"));
                    assert!(seen.text().contains("an earlier uncertain grant may remain"));
                    if mode == "deny" {
                        assert_eq!(reply["state"], "denied");
                        assert!(owner.take_reviewed().is_none());
                    } else {
                        assert_eq!(reply["reason"], "reviewed_finalization_waiting_for_setup_close");
                        let reviewed = owner.take_reviewed().unwrap();
                        if mode == "changed" {
                            // Change the actual file behind the native runtime
                            // without publishing a generation. Content, not
                            // generation equality alone, must fence this grant.
                            file.patch(ConfigDelta(json!({"tools":{"mcp_servers":[different]}}))).await.unwrap();
                            assert_eq!(crate::realm::read_own_mcp_config(&storage).await.unwrap().1, before.1);
                        }
                        let current_config = std::fs::read(&config_path).unwrap();
                        owner.retire().unwrap();
                        let finalized = reviewed.finalize_outcome_after_close().await;
                        let result = finalized.display(&mut output);
                        assert_eq!(std::fs::read(&config_path).unwrap(), current_config,
                            "reuse and stale refusal must not rewrite the saved document");
                        assert_eq!(std::fs::read(&state_path).unwrap(), before_state);
                        if mode == "changed" {
                            assert!(result.is_err());
                            assert!(matches!(&finalized.outcome, FinalizationOutcome::BeforeAdministrationRefused(error)
                                if error.to_string().contains("reviewed realm configuration changed")));
                            assert_eq!(finalized.notice()["facts"]["persistence"], "not_entered");
                            assert_eq!(std::fs::read(&owner_path).unwrap(), before_owner);
                        } else {
                            result.unwrap();
                            assert_ne!(std::fs::read(&owner_path).unwrap(), before_owner,
                                "the fresh approval requests new access from the actual owner");
                            assert!(seen.text().contains("already saved and was reused"));
                            assert!(seen.text().contains("any prior uncertain grant outcome remains unresolved"));
                            assert!(matches!(&finalized.outcome, FinalizationOutcome::PublicReused { generation, grant:Ok(_) }
                                if *generation == saved.generation));
                            assert_eq!(finalized.notice()["facts"]["persistence"], "already_saved");
                            assert_eq!(finalized.notice()["facts"]["new_access_request"], true);
                            assert_eq!(finalized.notice()["facts"]["prior_grant_outcome"], "not_resolved");
                            assert!(seen.text().contains("native access is selected for agent rkat-agent"));
                        }
                    }
                }
                owner.retire().unwrap();
                approvals.retire(); input.check_cleanup().unwrap();
            }
        })).await.expect("saved-service review and actual owner finalization must settle");
        inherited_public_name_controls().await;
    }

    #[tokio::test]
    async fn fresh_job_cannot_bypass_uncertain_target_but_independent_review_remains_available() {
        tokio::time::timeout(Duration::from_secs(30), Box::pin(async {
            for prior in ["same-target", "reused-uncertain", "other-server", "other-realm", "known-refusal"] {
                let blocked = matches!(prior, "same-target" | "reused-uncertain");
                let root = tempfile::tempdir().unwrap();
                let fixture = crate::stdio::tests::selected_public_fixture(root.path(), Some(declaration(json!({"kind":"none"})))).await;
                let selected = fixture.raw_selection().ok().unwrap();
                let storage = crate::RealmOptions { context_root:root.path().to_owned(), state_root:Some(root.path().join("state")),
                    user_config_root:Some(root.path().join("home")) }.resolve("selected-owner").unwrap();
                let mut owner = terminal_owner(root.path(), storage.clone()).await;
                let before = crate::realm::read_own_mcp_config(&storage).await.unwrap();
                let names = BTreeSet::from(["actual_read".to_owned()]);
                let config = selected.public_configuration(&names).unwrap();
                let mut realm = storage.locator.clone();
                if prior == "other-realm" { realm.realm = meerkat_core::connection::RealmId::parse("other").unwrap(); }
                // Controlled actual native outcome variants exercise the
                // consumer guard. No issuance or service success is claimed.
                let outcome = FinalizedInstallation::new(FinalizationTarget { realm,
                    server:if prior == "other-server" { "unrelated".into() } else { config.name.clone() },
                    agent:"rkat-agent".into(), job_id:"different-prior-job".into(), session:selected.installed.session.clone() },
                    if prior == "known-refusal" { FinalizationOutcome::Connector(ReviewedConnectorFinalization::Refused(
                        meerkat::governed_realm::stock::finalize::ReviewedConnectorRefusal::AccountChanged)) }
                    else if prior == "reused-uncertain" { FinalizationOutcome::PublicReused {
                        generation: 7, grant: Err(StockGrantError::CommandUncertain) } }
                    else { FinalizationOutcome::Connector(ReviewedConnectorFinalization::PersistUncertain { reason:"controlled partial write".into() }) });
                assert_ne!(outcome.target.job_id, selected.installed.job_id);
                owner.prior_finalizations = vec![Arc::new(outcome)].into();
                let (approvals, mut presenter) = LocalApprovals::create_paired_host(root.path()).unwrap();
                approvals.bind_session(&selected.installed.session).unwrap();
                let (lines, receive_lines) = mpsc::channel(1); let mut input = TerminalInput::scripted(receive_lines);
                let seen = Seen::default(); let mut output = seen.clone();
                let (response, receive) = oneshot::channel();
                let grant = StockGrantRequest { agent:"rkat-agent".into(), expires_in_ms:Some(60000),
                    target:StockGrantTarget::Mcp { server:config.name.clone(), operations:McpOperations::Exact(names) } };
                let presenting = present(&mut owner, PlainRequest { selection:selected.clone().into(), config, grant, response },
                    &approvals, &mut presenter, &mut input, &mut output, std::future::pending::<io::Result<()>>());
                let human = async { if !blocked { answer_after_display(&seen, &lines, ||{}).await; } };
                let (result, ()) = tokio::join!(presenting, human);
                assert!(result.unwrap());
                let reply = receive.await.unwrap();
                assert_eq!(reply["reason"], if blocked { "prior_finalization_unresolved" } else { "reviewed_finalization_waiting_for_setup_close" });
                assert_eq!(seen.text().contains("Type 'approve "), !blocked);
                assert_eq!(owner.take_reviewed().is_some(), !blocked);
                assert_eq!(crate::realm::read_own_mcp_config(&storage).await.unwrap(), before);
                assert!(!approvals.has_uncertain_dispatch()); owner.retire().unwrap(); input.check_cleanup().unwrap();
            }
        })).await.expect("actual protected reviews and target refusal must settle");
    }

    fn declaration(authentication: Value) -> Value {
        json!({"authentication":authentication,"operation_tools":{"declared/read":"actual_read"}})
    }

    #[tokio::test]
    async fn selected_public_launch_uses_declared_raw_names_without_a_fake_account() {
        let root = tempfile::tempdir().unwrap();
        let fixture = crate::stdio::tests::selected_public_fixture(
            root.path(),
            Some(declaration(json!({"kind":"none"}))),
        )
        .await;
        let selected = fixture.raw_selection().ok().unwrap();
        let names = BTreeSet::from(["actual_read".to_owned()]);
        let config = configuration(&selected.clone().into(), &names).unwrap();
        assert!(config.connector.is_none());
        assert_eq!(config.access.unwrap().allowed_operations, names);
        assert!(
            configuration(
                &selected.clone().into(),
                &BTreeSet::from(["declared/read".to_owned()])
            )
            .is_err()
        );
        assert!(
            configuration(
                &selected.clone().into(),
                &BTreeSet::from(["unreviewed_write".to_owned()])
            )
            .is_err()
        );
        // There is deliberately no library configuration directory in this fixture.
        recheck_retained(
            &selected.installed,
            &selected.configuration,
            selected.selected_inputs(),
        )
        .await
        .unwrap();
        let other = tempfile::tempdir().unwrap();
        let binding = crate::stdio::tests::selected_public_fixture(other.path(),Some(declaration(json!({
            "kind":"binding","owner":"process","requirement":"key","credential_audience":"service"})))).await;
        let binding = binding.raw_selection().ok().unwrap();
        assert!(binding.native_connector.is_none());
        assert!(
            configuration(&binding.clone().into(), &names).is_err(),
            "NoConnector does not mean no provider authentication"
        );
    }

    #[tokio::test]
    async fn selected_retained_input_or_descriptor_change_refuses_before_persistence() {
        for name in [
            "inputs/package.json",
            "inputs/activation.json",
            "ready/mcp.json",
            "ready/installed.json",
        ] {
            let root = tempfile::tempdir().unwrap();
            let fixture = crate::stdio::tests::selected_public_fixture(
                root.path(),
                Some(declaration(json!({"kind":"none"}))),
            )
            .await;
            let selected = fixture.raw_selection().ok().unwrap();
            let destination = selected
                .installed
                .mcp_path
                .parent()
                .unwrap()
                .parent()
                .unwrap();
            std::fs::write(destination.join(name), b"changed retained document").unwrap();
            assert!(
                recheck_retained(
                    &selected.installed,
                    &selected.configuration,
                    selected.selected_inputs()
                )
                .await
                .is_err(),
                "{name}"
            );
        }
    }
    #[tokio::test]
    async fn software_retained_document_is_checked_before_the_native_finalizer_boundary() {
        for changed in [
            None,
            Some("inputs/declaration.json"),
            Some("inputs/package.json"),
            Some("ready/mcp.json"),
            Some("ready/installed.json"),
            Some("renamed"),
        ] {
            let root = tempfile::tempdir().unwrap();
            let fixture = crate::stdio::tests::selected_software_fixture(root.path()).await;
            let selected = fixture.raw_selection().ok().unwrap();
            let source = ReviewedPublicSource::from_selection(&selected.clone().into());
            let storage = crate::RealmOptions {
                context_root: root.path().to_owned(),
                state_root: Some(root.path().join("state")),
                user_config_root: Some(root.path().join("home")),
            }
            .resolve("selected-owner")
            .unwrap();
            let config = selected
                .public_configuration(&BTreeSet::from(["actual_read".to_owned()]))
                .unwrap();
            source.recheck(&storage, &config).await.unwrap();
            let destination = selected
                .installed
                .mcp_path
                .parent()
                .unwrap()
                .parent()
                .unwrap();
            assert!(!destination.join("inputs/activation.json").exists());
            if let Some(changed) = changed {
                if changed == "renamed" {
                    std::fs::rename(
                        destination.join("inputs/declaration.json"),
                        destination.join("inputs/activation.json"),
                    )
                    .unwrap();
                } else {
                    std::fs::write(destination.join(changed), b"changed").unwrap();
                }
                assert!(
                    source.recheck(&storage, &config).await.is_err(),
                    "{changed}"
                );
            } else {
                source.recheck(&storage, &config).await.unwrap();
            }
        }
    }
}
