//! Public remote exports observed through the same selected setup dispatcher.
//! Recipe declarations are research evidence, not observed tools or credentials.

use super::*;
use std::collections::BTreeSet;
use meerkat_core::McpServerConfig;

const EXPORT_TOOL: &str = "setup_export_recipe";
const EXPORT_SCOPE: &str = "current_portable_export_files_only";

/// A current canonical export and its recovery anchor, never a local install.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteSelection {
    pub(crate) realm: RealmLocator,
    pub(crate) session: SessionId,
    pub(crate) job_id: String,
    pub(crate) request_sha256: String,
    pub(crate) recovery_request_sha256: String,
    pub(crate) configuration: Arc<Value>,
    pub(crate) export: Value,
}

impl RemoteSelection {
    pub(crate) fn from_review_source(value: &Value, realm: &RealmLocator, session: SessionId)
        -> Result<Self, &'static str>
    {
        if !exact_keys(value,&["job_id","request_sha256","recovery_request_sha256","export"]) {
            return Err("remote_export_unconfirmed");
        }
        let job_id=hex(value,"job_id",32).ok_or("remote_export_unconfirmed")?.to_owned();
        let request_sha256=hex(value,"request_sha256",64).ok_or("remote_export_unconfirmed")?.to_owned();
        let recovery_request_sha256=hex(value,"recovery_request_sha256",64).ok_or("remote_export_unconfirmed")?.to_owned();
        let export=value["export"].clone();
        if serde_json::to_vec(&export).map_err(|_|"remote_export_unconfirmed")?.len()>MAX_DOCUMENT {
            return Err("remote_export_unconfirmed");
        }
        let selected=Self { realm:realm.clone(),session,job_id,request_sha256,recovery_request_sha256,
            configuration:Arc::new(export["mcp_configuration"].clone()),export };
        let operations=remote_operations(&selected.export)?;
        selected.public_configuration(&operations)?;
        Ok(selected)
    }

    pub(crate) fn server_name(&self) -> &str {
        self.export["declared_remote_service"]["mcp_name"].as_str().unwrap_or("")
    }

    pub(crate) fn operation_basis(&self) -> Value {
        json!({"kind":"remote_recipe_declarations", "recipe":self.export["recipe"],
            "selection_digest":self.export["selection_digest"],
            "declared_remote_service":self.export["declared_remote_service"]})
    }

    pub(crate) fn review_source(&self) -> Value {
        json!({"job_id":self.job_id,"request_sha256":self.request_sha256,
            "recovery_request_sha256":self.recovery_request_sha256,"export":self.export})
    }

    pub(crate) fn readback_request(&self, operations: &BTreeSet<String>) -> Value {
        json!({"realm":self.realm.realm.as_str(),"realm_state_root":self.realm.state_root,
            "remote_export":self.review_source(),
            "public_service":{"operation_basis":self.operation_basis(),"operations":operations}})
    }

    pub(crate) fn public_configuration(&self, operations: &BTreeSet<String>)
        -> Result<McpServerConfig, &'static str>
    {
        remote_configuration(&self.export, &self.configuration, operations)
    }
}

#[derive(Clone)]
pub(crate) enum PublicServiceSelection {
    Installed(InstalledConnectorSelection),
    Remote { selected: RemoteSelection, owner: Arc<CheckedInstalls> },
}

impl From<InstalledConnectorSelection> for PublicServiceSelection {
    fn from(value: InstalledConnectorSelection) -> Self { Self::Installed(value) }
}

impl PublicServiceSelection {
    pub(crate) fn realm(&self) -> &RealmLocator {
        match self { Self::Installed(value) => &value.installed.realm, Self::Remote { selected, .. } => &selected.realm }
    }
    pub(crate) fn session(&self) -> &SessionId {
        match self { Self::Installed(value) => &value.installed.session, Self::Remote { selected, .. } => &selected.session }
    }
    pub(crate) fn job_id(&self) -> &str {
        match self { Self::Installed(value) => &value.installed.job_id, Self::Remote { selected, .. } => &selected.job_id }
    }
    pub(crate) fn public_configuration(&self, operations: &BTreeSet<String>)
        -> Result<McpServerConfig, &'static str>
    {
        match self {
            Self::Installed(value) => value.public_configuration(operations),
            Self::Remote { selected, owner } => {
                let current = owner.remote_selection(&selected.realm, &selected.session, &selected.job_id)
                    .map_err(|_| "remote_export_unconfirmed")?;
                if &current != selected { return Err("remote_export_changed"); }
                selected.public_configuration(operations)
            }
        }
    }
    /// Exact declared raw-name ceiling for model-visible request correction.
    /// Reuse current public-route validation before disclosing any names; this
    /// projection neither translates declaration IDs nor authorizes access.
    pub(crate) fn public_operations(&self) -> Result<BTreeSet<String>, &'static str> {
        let operations = match self {
            Self::Installed(value) => value.raw_operations.clone(),
            Self::Remote { selected, .. } => remote_operations(&selected.export)?,
        };
        self.public_configuration(&operations)?;
        Ok(operations)
    }
    pub(crate) fn operation_basis(&self) -> Value {
        match self { Self::Installed(value) => value.operation_basis(), Self::Remote { selected, .. } => selected.operation_basis() }
    }
    pub(crate) fn review_source(&self) -> Value {
        match self {
            Self::Installed(value) => {
                let software = &value.installed;
                json!({"software":{"job_id":software.job_id,"validation_id":software.validation_id,
                    "receipt_sha256":software.receipt_sha256,"configuration_sha256":software.configuration_sha256,
                    "document_sha256":software.mcp_document_sha256,"mcp_path":software.mcp_path,"receipt_path":software.receipt_path,
                    "family":software.family,"profile":software.profile,"server_name":software.server_name,
                    "recovery_request_sha256":software.recovery_request_sha256}})
            }
            Self::Remote { selected, .. } => json!({"remote_export":selected.review_source()}),
        }
    }
}

pub(super) struct RemoteCapture {
    session: SessionId,
    epoch: u64,
    request_sha256: String,
    recovery_request_sha256: String,
    selection: Option<RemoteSelection>,
    revision: u64,
    publisher: Option<Arc<()>>,
    matched: bool,
    bytes: usize,
}

impl CheckedInstalls {
    pub(crate) fn public_selection(self: &Arc<Self>, realm: &RealmLocator,
        session: &SessionId, job_id: &str) -> Result<PublicServiceSelection, ConnectorSelectionError>
    {
        // A known remote job never falls back to an installation interpretation.
        let remote = self.state.lock().map_err(|_| ConnectorSelectionError::Unconfirmed)?
            .remote.contains_key(job_id);
        if remote {
            Ok(PublicServiceSelection::Remote { selected:self.remote_selection(realm, session, job_id)?, owner:self.clone() })
        } else { self.connector_selection(realm, session, job_id).map(Into::into) }
    }

    fn remote_selection(&self, realm: &RealmLocator, session: &SessionId, job_id: &str)
        -> Result<RemoteSelection, ConnectorSelectionError>
    {
        let state = self.state.lock().map_err(|_| ConnectorSelectionError::Unconfirmed)?;
        let target = state.remote.get(job_id).ok_or(ConnectorSelectionError::Unconfirmed)?;
        if state.retired || state.realm.as_ref() != Some(realm) || &target.session != session
            || !target.matched || target.epoch != self.inner.execution_binding_epoch(EXPORT_TOOL)
            || target.epoch != self.inner.execution_binding_epoch("setup_job")
            || target.epoch != self.inner.execution_binding_epoch("setup_reconcile")
        { return Err(ConnectorSelectionError::Unconfirmed); }
        target.selection.clone().ok_or(ConnectorSelectionError::Unconfirmed)
    }

    pub(super) async fn observe_export(&self, call: ToolCallView<'_>, context: &ToolDispatchContext,
        operation: impl Future<Output=Result<ToolDispatchOutcome,ToolError>>)
        -> Result<ToolDispatchOutcome, ToolError>
    {
        let session = context.origin_session_id().ok_or_else(|| refused(call.name))?.clone();
        let epoch = self.inner.execution_binding_epoch(call.name);
        {
            let state = self.state.lock().map_err(|_| unavailable())?;
            if state.retired || state.realm.is_none() || state.remote.len() >= MAX_PROPOSALS {
                return Err(refused(call.name));
            }
        }
        let outcome = operation.await?;
        let value = result_document(call, &outcome)?;
        if value.get("error").is_some() { return Ok(outcome); }
        let job = &value["job"];
        let id = hex(job,"id",32).ok_or_else(unconfirmed)?;
        let request_sha256 = hex(job,"request_sha256",64).ok_or_else(unconfirmed)?.to_owned();
        let recovery_request_sha256 = hex(&job["recovery_request"],"sha256",64).ok_or_else(unconfirmed)?.to_owned();
        if job["kind"] != "export" || job["family"] != "remote-recipe" || job["recovery_request"]["id"] != id {
            return Err(unconfirmed());
        }
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        if state.retired || epoch != self.inner.execution_binding_epoch(call.name) { return Err(unconfirmed()); }
        if let Some(prior) = state.remote.get(id) {
            if prior.session != session || prior.epoch != epoch || prior.request_sha256 != request_sha256
                || prior.recovery_request_sha256 != recovery_request_sha256 { return Err(unconfirmed()); }
        } else {
            if state.remote.len() >= MAX_PROPOSALS || state.proposals.values().filter_map(|p|p.readback.as_ref())
                .any(|p|p.job.lock().is_ok_and(|v|v.as_deref()==Some(id))) { return Err(unconfirmed()); }
            state.remote.insert(id.into(),RemoteCapture { session,epoch,request_sha256,recovery_request_sha256,
                selection:None,revision:0,publisher:None,matched:false,bytes:0 });
        }
        Ok(outcome)
    }

    pub(super) async fn observe_remote_readback(&self, call: ToolCallView<'_>, context: &ToolDispatchContext,
        operation: impl Future<Output=Result<ToolDispatchOutcome,ToolError>>)
        -> Result<ToolDispatchOutcome, ToolError>
    {
        let args = document(call.args.get())?;
        let id = args["job_id"].as_str().ok_or_else(|| refused(call.name))?.to_owned();
        let token = Arc::new(());
        let captured = {
            let mut state = self.state.lock().map_err(|_| unavailable())?;
            let retired = state.retired;
            let realm = state.realm.clone();
            state.remote.get_mut(&id).map(|target| {
                if retired || context.origin_session_id() != Some(&target.session)
                    || target.epoch != self.inner.execution_binding_epoch(call.name) { return Err(refused(call.name)); }
                target.matched = false;
                target.publisher = Some(token.clone());
                Ok((realm.ok_or_else(unconfirmed)?,target.session.clone(),target.epoch))
            }).transpose()?
        };
        let Some((realm,session,epoch)) = captured else { return operation.await; };
        let outcome = operation.await?;
        let value = result_document(call,&outcome)?;
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        if state.retired || epoch != self.inner.execution_binding_epoch(call.name) { return Err(unconfirmed()); }
        let target = state.remote.get_mut(&id).ok_or_else(unconfirmed)?;
        if !target.publisher.as_ref().is_some_and(|v|Arc::ptr_eq(v,&token)) { return Err(unconfirmed()); }
        if value.get("error").is_some() { return Ok(outcome); }
        if call.name == "setup_job" {
            let job = &value["job"];
            if job["id"] != id || job["kind"] != "export" || job["family"] != "remote-recipe"
                || job["request_sha256"] != target.request_sha256
                || job["recovery_request"] != json!({"id":id,"sha256":target.recovery_request_sha256}) {
                return Err(unconfirmed());
            }
        }
        let observation = if call.name == "setup_job" { &value["reconciliation"]["observation"] } else { &value["observation"] };
        if observation.is_null() { return Ok(outcome); }
        let revision = observation["revision"].as_u64().filter(|v|*v>0 && *v>=target.revision).ok_or_else(unconfirmed)?;
        if observation["job_id"] != id || observation["scope"] != EXPORT_SCOPE { return Err(unconfirmed()); }
        target.revision = revision;
        if observation["status"] != "matched" { return Ok(outcome); }
        let result = &observation["result"];
        if result["scope"] != EXPORT_SCOPE || result["export_check"]["state"] != "matches" { return Err(unconfirmed()); }
        let export = result["configuration"].clone();
        let configuration = Arc::new(export["mcp_configuration"].clone());
        let selected = RemoteSelection { realm,session,job_id:id,request_sha256:target.request_sha256.clone(),
            recovery_request_sha256:target.recovery_request_sha256.clone(),configuration,export };
        let Ok(operations) = remote_operations(&selected.export) else { return Ok(outcome); };
        if selected.public_configuration(&operations).is_err() { return Ok(outcome); }
        let bytes = serde_json::to_vec(&selected.export).map_err(|_|unconfirmed())?.len();
        let previous_bytes = target.bytes;
        let retained = state.remote_bytes.checked_sub(previous_bytes).and_then(|v|v.checked_add(bytes))
            .filter(|v|*v<=MAX_RETAINED).ok_or_else(unconfirmed)?;
        let target = state.remote.get_mut(&selected.job_id).ok_or_else(unconfirmed)?;
        target.bytes = bytes;
        // Removal or changed metadata invalidates a retained clone. The current
        // owner may later supply a fresh declaration, requiring a new review.
        target.selection = Some(selected);
        target.matched = true;
        state.remote_bytes = retained;
        Ok(outcome)
    }
}

fn remote_operations(export: &Value) -> Result<BTreeSet<String>, &'static str> {
    let declaration = &export["declared_remote_service"];
    if declaration["authentication"] != json!({"kind":"none"}) { return Err("remote_sign_in_required_or_unknown"); }
    let names = declaration["operation_tools"].as_object().filter(|m|!m.is_empty() && m.len()<=128)
        .ok_or("remote_operations_unconfirmed")?;
    let raw = names.iter().map(|(id,value)| {
        let value = value.as_str().filter(|v|!v.is_empty() && v.len()<=128
            && v.bytes().all(|b|b.is_ascii_alphanumeric() || b"_.-".contains(&b)))
            .ok_or("remote_operations_unconfirmed")?;
        toolkit_package::Id::new(id).map_err(|_|"remote_operations_unconfirmed")?;
        Ok(value.to_owned())
    }).collect::<Result<BTreeSet<_>, &'static str>>()?;
    if raw.len()!=names.len() { return Err("remote_operations_unconfirmed"); }
    Ok(raw)
}

fn remote_configuration(export: &Value, configuration: &Value, operations: &BTreeSet<String>)
    -> Result<McpServerConfig, &'static str>
{
    let raw = remote_operations(export)?;
    if operations.is_empty() || !operations.is_subset(&raw) { return Err("invalid_operation_binding"); }
    let declaration = &export["declared_remote_service"];
    let digest_key = match export.get("selection_kind") {
        None if exact_keys(declaration,&["manifest_digest","component","export","mcp_name","authentication","operation_tools"]) => "manifest_digest",
        Some(kind) if kind == "remote_declaration" && declaration["selection_kind"] == "remote_declaration"
            && exact_keys(declaration,&["selection_kind","declaration_digest","component","export","mcp_name","authentication","operation_tools"]) => "declaration_digest",
        _ => return Err("remote_export_unconfirmed"),
    };
    if export["scope"] != "portable_mcp_configuration_only" || export["mcp_configuration"] != *configuration
        || declaration[digest_key] != export["selection_digest"]
        || hex(declaration,digest_key,64).is_none()
    { return Err("remote_export_unconfirmed"); }
    for key in ["component","export","mcp_name"] {
        toolkit_package::Id::new(declaration[key].as_str().ok_or("remote_export_unconfirmed")?)
            .map_err(|_|"remote_export_unconfirmed")?;
    }
    if !exact_keys(configuration,&["$schema","mcpServers"])
        || configuration["$schema"] != "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json" {
        return Err("unsupported_public_remote_launch");
    }
    let name = declaration["mcp_name"].as_str().ok_or("remote_export_unconfirmed")?;
    let servers = configuration["mcpServers"].as_object().filter(|v|v.len()==1)
        .ok_or("unsupported_public_remote_launch")?;
    let launch = servers.get(name).ok_or("unsupported_public_remote_launch")?;
    // The canonical portable owner already validates the URL. Strict typed
    // projection carries only its selected transport, URL and empty headers.
    let portable: toolkit_package::PortableMcpServer = serde_json::from_value(launch.clone())
        .map_err(|_|"unsupported_public_remote_launch")?;
    let toolkit_package::PortableMcpServer::StreamableHttp { url,headers } = portable else {
        return Err("unsupported_public_remote_launch");
    };
    if !headers.is_empty() || url.scheme() != "https" || url.host_str().is_none()
        || !url.username().is_empty() || url.password().is_some() || url.query().is_some() || url.fragment().is_some()
    { return Err("unsupported_public_remote_launch"); }
    let mut server = McpServerConfig::streamable_http(name,url.to_string(),Default::default());
    server.access=Some(meerkat_core::McpServerAccess::allowing(operations.iter().cloned()));
    server.validate_access().map_err(|_|"invalid_operation_binding")?;
    Ok(server)
}
