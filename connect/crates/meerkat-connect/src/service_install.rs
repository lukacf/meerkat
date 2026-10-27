//! Exact checked-install custody around the existing native MCP dispatcher.
//!
//! The package owner validates configurations and owns jobs, receipts and
//! reconciliation. These records only correlate a live native decision with
//! the check response and one delegated call in the same session/binding.

use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use meerkat_core::{
    AgentToolDispatcher, ContentBlock, SessionId, ToolCallView, ToolDispatchContext, ToolError, ToolResult,
    ops::ToolDispatchOutcome, runtime_bootstrap::RealmLocator,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

mod remote;
mod retained;
pub(crate) use remote::{PublicServiceSelection, RemoteSelection};

pub(crate) const INSTALL_TOOLS: &[&str] = &["setup_install", "setup_selected_install"];
pub(crate) const SELECTED_CHECK: &str = "setup_selected_check";
// A native stock grant slot, distinct from this server's saved raw MCP access.
pub(crate) const CHECKED_INSTALL_ROUTE: &str = "meerkat-connect-checked-installs";

pub(crate) fn checked_install_operations() -> BTreeSet<String> {
    INSTALL_TOOLS.iter().copied().chain(std::iter::once(SELECTED_CHECK))
        .map(str::to_owned).collect()
}
const MAX_DOCUMENT: usize = 1024 * 1024;
const MAX_RETAINED: usize = 8 * 1024 * 1024;
const MAX_PROPOSALS: usize = 128;
const READBACK_SCOPE: &str = "retained_payload_and_launch_binding_only";

fn refused(name: &str) -> ToolError {
    ToolError::access_denied(name)
}
fn unavailable() -> ToolError {
    ToolError::execution_failed(
        "Checked installation custody is unavailable; do not repeat installation",
    )
}
fn unconfirmed() -> ToolError {
    ToolError::execution_failed(
        "The exact setup owner response was not retained; do not repeat installation",
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Library,
    Selected,
}
impl Kind {
    fn check(name: &str) -> Option<Self> {
        match name {
            "setup_check" => Some(Self::Library),
            SELECTED_CHECK => Some(Self::Selected),
            _ => None,
        }
    }
    fn install(self) -> &'static str {
        match self {
            Self::Library => "setup_install",
            Self::Selected => "setup_selected_install",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum OperationEvidence {
    Publisher { names: BTreeMap<String, String>, no_auth: bool, credential_fields: Option<std::collections::BTreeSet<String>> },
    Selected {
        names: BTreeMap<String, String>,
        authentication: toolkit_package::AuthenticationRequirement,
        inputs: SelectedServiceInputs,
    },
}

/// Exact selected software inputs already checked by the package owner.
/// These are declarations and source evidence, not observed MCP tools.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SelectedServiceInputs {
    pub(crate) package_sha256: String,
    pub(crate) document: SelectedDocument,
    pub(crate) environment_secret_slots: Option<toolkit_package::EnvironmentSecretSlots>,
    effect_sha256: String,
    provenance: Value,
}

/// Mutually exclusive source-document identity. This projects the package
/// owner's checked documents and cannot validate or authorize an installation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SelectedDocument {
    Manifest { selection_sha256: String, manifest_digest: String },
    Software { selection_sha256: String, declaration_digest: String },
}
impl SelectedDocument {
    pub(crate) fn from_value(value: &Value) -> Result<Self, ToolError> {
        let hash = |key| hex(value, key, 64).map(str::to_owned).ok_or_else(unconfirmed);
        let selection_sha256 = hash("selection_sha256")?;
        match value.get("document_kind") {
            None if value.get("declaration_digest").is_none() => Ok(Self::Manifest {
                selection_sha256, manifest_digest: hash("manifest_digest")?,
            }),
            Some(kind) if kind == "local_software_declaration" && value.get("manifest_digest").is_none() => Ok(Self::Software {
                selection_sha256, declaration_digest: hash("declaration_digest")?,
            }),
            _ => Err(unconfirmed()),
        }
    }
    pub(crate) fn filename(&self) -> &'static str {
        match self { Self::Manifest { .. } => "activation.json", Self::Software { .. } => "declaration.json" }
    }
    pub(crate) fn sha256(&self) -> &str {
        match self { Self::Manifest { selection_sha256, .. } | Self::Software { selection_sha256, .. } => selection_sha256 }
    }
    pub(crate) fn basis_kind(&self) -> &'static str {
        match self { Self::Manifest { .. } => "selected_manifest_declarations", Self::Software { .. } => "selected_software_declarations" }
    }
    fn add_basis(&self, basis: &mut Value) {
        basis["kind"] = json!(self.basis_kind());
        basis["selection_sha256"] = json!(self.sha256());
        match self {
            Self::Manifest { manifest_digest, .. } => basis["manifest_digest"] = json!(manifest_digest),
            Self::Software { declaration_digest, .. } => {
                basis["document_kind"] = json!("local_software_declaration");
                basis["declaration_digest"] = json!(declaration_digest);
            }
        }
    }
}

impl OperationEvidence {
    fn names(&self) -> &BTreeMap<String, String> {
        match self { Self::Publisher { names, .. } | Self::Selected { names, .. } => names }
    }
    fn no_auth(&self) -> bool {
        match self {
            Self::Publisher { no_auth, .. } => *no_auth,
            Self::Selected { authentication, .. } => matches!(authentication, toolkit_package::AuthenticationRequirement::None),
        }
    }
}

#[derive(Default)]
struct OperationMetadata {
    publisher: Option<Arc<()>>,
    observed: Option<OperationEvidence>,
}

struct Proposal {
    session: SessionId,
    epoch: u64,
    kind: Kind,
    install: Value,
    review: Value,
    used: bool,
    readback: Option<Readback>,
    operation_names: Arc<Mutex<OperationMetadata>>,
}

/// Historical software evidence, never account access or native attachment.
/// No MCP command, environment, configuration body or credential is retained.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InstalledSelection {
    pub(crate) realm: RealmLocator,
    pub(crate) session: SessionId,
    pub(crate) job_id: String,
    pub(crate) validation_id: String,
    pub(crate) family: String,
    pub(crate) profile: String,
    pub(crate) mcp_path: PathBuf,
    pub(crate) receipt_path: PathBuf,
    pub(crate) receipt_sha256: Option<String>,
    pub(crate) configuration_sha256: Option<String>,
    pub(crate) server_name: String,
    // Hash of the returned JSON projection, not a hash of the mcp.json file.
    pub(crate) mcp_document_sha256: String,
    pub(crate) recovery_request_sha256: Option<String>,
}

// Optional software capability from the canonical installed-wheel verifier.
// This is not a credential, operation grant or successful native execution.
#[derive(Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeConnectorEntry {
    schema_version: u32,
    pub(crate) module: String,
    pub(crate) mode: String,
    pub(crate) configuration_argument: String,
    profiles: Option<Vec<String>>,
    member: String,
    member_sha256: String,
    wheel: NativeConnectorWheel,
    package_sources_sha256: String,
}
#[derive(Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeConnectorWheel { filename: String, sha256: String }
impl NativeConnectorEntry {
    pub(crate) fn observed(value: &Value, family: &str, profile: &str) -> Result<Self, ToolError> {
        let entry: Self = serde_json::from_value(value.clone()).map_err(|_| unconfirmed())?;
        let digest = |s: &str| s.len() == 64 && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        let (module, member, wheel_prefix) = match family {
            "services" if value.get("profiles").is_none() => (
                "meerkat_toolkit_services.adapters.services_mcp",
                "meerkat_toolkit_services/adapters/services_mcp.py",
                "meerkat_toolkit_services-",
            ),
            "agno" if profile == "salesforce" && entry.profiles.as_ref()
                .is_some_and(|profiles| profiles.len() == 1 && profiles[0] == "salesforce") => (
                "meerkat_toolkit_agno.agno_mcp",
                "meerkat_toolkit_agno/agno_mcp.py",
                "meerkat_toolkit_agno-",
            ),
            _ => return Err(unconfirmed()),
        };
        if entry.schema_version != 1 || entry.module != module
            || entry.mode != "serve-native" || entry.configuration_argument != "--config"
            || entry.member != member
            || !entry.wheel.filename.starts_with(wheel_prefix)
            || !entry.wheel.filename.ends_with(".whl") || entry.wheel.filename.len() > 255
            || !entry.wheel.filename.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.+!-".contains(&b))
            || !digest(&entry.member_sha256) || !digest(&entry.wheel.sha256)
            || !digest(&entry.package_sources_sha256) { return Err(unconfirmed()); }
        Ok(entry)
    }
}

#[derive(Clone, Default)]
pub(crate) struct ReceiptCapture {
    pub(crate) installed: Option<InstalledSelection>,
    pub(crate) reconciliation_revision: Option<u64>,
    /// The last owner read matched. This is not current service callability.
    pub(crate) matched: bool,
    // Exact owner-returned transport document. Never serialized or logged.
    configuration: Option<Arc<Value>>,
    native_connector: Option<NativeConnectorEntry>,
    recovery_request_sha256: Option<String>,
}

impl std::fmt::Debug for ReceiptCapture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReceiptCapture")
            .field("installed", &self.installed)
            .field("reconciliation_revision", &self.reconciliation_revision)
            .field("matched", &self.matched)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub(crate) struct Readback {
    epoch: u64,
    session: SessionId,
    job: Arc<Mutex<Option<String>>>,
    family: String,
    binding: Arc<dyn AgentToolDispatcher>,
    realm: RealmLocator,
    validation_id: String,
    proposal: Arc<Value>,
    receipt: Arc<Mutex<ReceiptCapture>>,
    operation_names: Arc<Mutex<OperationMetadata>>,
}

/// Host-only selection from the same checked job. Its document is private;
/// possession does not establish account, native attachment or permission.
#[derive(Clone)]
pub(crate) struct InstalledConnectorSelection {
    pub(crate) installed: InstalledSelection,
    pub(crate) configuration: Arc<Value>,
    pub(crate) raw_operations: std::collections::BTreeSet<String>,
    pub(crate) native_connector: Option<NativeConnectorEntry>,
    owner: Arc<CheckedInstalls>,
    metadata: OperationEvidence,
    pub(crate) no_credential_references: bool,
    pub(crate) credential_references: Option<Vec<ServiceCredentialReference>>,
}
pub(crate) enum ConnectorSelectionError {
    Unconfirmed,
    MetadataRequired { family: String, profile: String },
}
/// Names-only projection returned by the checked package owner. Values are
/// environment variable names, never credentials or native account identities.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ServiceCredentialReference {
    pub(crate) field_path: Vec<String>,
    pub(crate) environment_variable: String,
}

/// Exact source of a protected input label. Selected process requirements do
/// not fabricate the configuration-field paths used by library adapters.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "source", content = "declaration", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ServiceCredentialSlot {
    LibraryField(ServiceCredentialReference),
    SelectedEnvironment(String),
}
impl ServiceCredentialSlot {
    pub(crate) fn environment_variable(&self) -> &str {
        match self { Self::LibraryField(value) => &value.environment_variable, Self::SelectedEnvironment(value) => value }
    }
    pub(crate) fn label(&self) -> String {
        match self { Self::LibraryField(value) => value.field_path.join("."), Self::SelectedEnvironment(value) => value.clone() }
    }
}

fn environment_name(value: &str) -> bool {
    let mut bytes = value.bytes();
    (1..=128).contains(&value.len())
        && bytes.next().is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn credential_fields(response: &Value) -> Result<Option<std::collections::BTreeSet<String>>, ToolError> {
    let Some(fields) = response.get("fields") else { return Ok(None); };
    let fields = fields.as_array().filter(|rows| rows.len() <= 256).ok_or_else(unconfirmed)?;
    let mut names = std::collections::BTreeSet::new();
    let mut credentials = std::collections::BTreeSet::new();
    for field in fields {
        let name = field["name"].as_str().filter(|name| environment_name(name)).ok_or_else(unconfirmed)?;
        if !names.insert(name.to_owned()) { return Err(unconfirmed()); }
        let Some(is_credential) = field["credential_reference"].as_bool() else { return Err(unconfirmed()); };
        if is_credential {
            if field["kind"] != "string" { return Err(unconfirmed()); }
            credentials.insert(name.to_owned());
        }
    }
    Ok(Some(credentials))
}

fn service_credential_references(
    proposal: &Value, metadata: &OperationEvidence,
) -> Result<Option<Vec<ServiceCredentialReference>>, ToolError> {
    credential_references(&proposal["selection"], proposal["owner_proposal"].get("credential_environment_references"), metadata)
}

fn credential_references(
    selected: &Value, value: Option<&Value>, metadata: &OperationEvidence,
) -> Result<Option<Vec<ServiceCredentialReference>>, ToolError> {
    let Some(value) = value else { return Ok(None); };
    let rows = value.as_array().filter(|rows| rows.len() <= 256).ok_or_else(unconfirmed)?;
    // Legacy public/OAuth selections retain their existing independent checks.
    // Only an exact current library-help field projection can enable this path.
    let OperationEvidence::Publisher { credential_fields: Some(fields), .. } = metadata else { return Ok(None); };
    let mut expected = BTreeMap::new();
    for field in fields {
        if let Some(value) = selected["values"].get(field) {
            let value = value.as_str().filter(|value| environment_name(value)).ok_or_else(unconfirmed)?;
            let path = if selected["family"] == "activepieces" { vec![field.clone()] }
                else { vec!["config".to_owned(), field.clone()] };
            expected.insert(path, value.to_owned());
        }
    }
    let mut observed = BTreeMap::new();
    let mut slots = std::collections::BTreeSet::new();
    let mut references = Vec::new();
    for row in rows {
        let reference: ServiceCredentialReference = serde_json::from_value(row.clone()).map_err(|_| unconfirmed())?;
        if !(1..=2).contains(&reference.field_path.len())
            || reference.field_path.iter().any(|key| !environment_name(key))
            || !environment_name(&reference.environment_variable)
            || !slots.insert(reference.environment_variable.clone())
            || observed.insert(reference.field_path.clone(), reference.environment_variable.clone()).is_some()
        { return Err(unconfirmed()); }
        references.push(reference);
    }
    if observed != expected { return Err(unconfirmed()); }
    references.sort_by(|a, b| a.field_path.cmp(&b.field_path));
    Ok(Some(references))
}

impl InstalledConnectorSelection {
    /// Reuse the exact checked package launch for either protected setup surface.
    /// Explicit no-auth metadata is required independently of native connector absence.
    pub(crate) fn public_configuration(&self, operations: &std::collections::BTreeSet<String>)
        -> Result<meerkat_core::McpServerConfig, &'static str>
    {
        self.check_current().map_err(|_| "installation_unconfirmed")?;
        if !self.no_auth() || (self.selected_inputs().is_none() && !self.no_credential_references)
            || operations.is_empty() || operations.len() > 128
            || !operations.is_subset(&self.raw_operations)
        { return Err("explicit_public_profile_required"); }
        public_launch_configuration(&self.installed, &self.configuration, operations)
    }

    pub(crate) fn no_auth(&self) -> bool { self.metadata.no_auth() }

    pub(crate) fn secret_configuration(&self, operations: &std::collections::BTreeSet<String>)
        -> Result<meerkat_core::McpServerConfig, &'static str>
    {
        self.check_current().map_err(|_| "installation_unconfirmed")?;
        let references = self.secret_slots()?;
        if self.no_auth() || !operations.is_subset(&self.raw_operations)
        { return Err("checked_credential_references_required"); }
        // An optional serve-native capability does not replace the checked
        // normal launch or select a native connector/account for this route.
        secret_launch_configuration(&self.installed, &self.configuration, operations, &references)
    }

    /// Disclose the declared raw-name ceiling only for a current, checked
    /// credential launch. This neither translates IDs nor authorizes access.
    pub(crate) fn secret_operations(&self) -> Result<std::collections::BTreeSet<String>, &'static str> {
        self.secret_configuration(&self.raw_operations)?;
        Ok(self.raw_operations.clone())
    }

    pub(crate) fn secret_slots(&self) -> Result<Vec<ServiceCredentialSlot>, &'static str> {
        if let Some(inputs) = self.selected_inputs() {
            let names = inputs.environment_secret_slots.as_ref().ok_or("checked_credential_references_required")?;
            Ok(names.names().iter().cloned().map(ServiceCredentialSlot::SelectedEnvironment).collect())
        } else {
            let fields = self.credential_references.as_ref().filter(|rows| !rows.is_empty())
                .ok_or("checked_credential_references_required")?;
            Ok(fields.iter().cloned().map(ServiceCredentialSlot::LibraryField).collect())
        }
    }

    pub(crate) fn selected_inputs(&self) -> Option<&SelectedServiceInputs> {
        match &self.metadata { OperationEvidence::Selected { inputs, .. } => Some(inputs), _ => None }
    }

    pub(crate) fn operation_basis(&self) -> Value {
        match &self.metadata {
            OperationEvidence::Publisher { .. } => json!({"kind":"package_published_metadata"}),
            OperationEvidence::Selected { authentication, inputs, .. } => {
                let mut basis = json!({"authentication":authentication,
                    "package_sha256":inputs.package_sha256,"effect_sha256":inputs.effect_sha256,
                    "source":inputs.provenance,
                    "notice":"These exact raw tool names and authentication requirements are retained declarations. No tools/list, service invocation or readiness is asserted."});
                inputs.document.add_basis(&mut basis);
                basis
            },
        }
    }

    pub(crate) fn check_current(&self) -> Result<(), ToolError> {
        let selected = self.owner.connector_selection(
            &self.installed.realm, &self.installed.session, &self.installed.job_id,
        ).map_err(|_| unconfirmed())?;
        if selected.installed != self.installed || selected.configuration != self.configuration
            || selected.raw_operations != self.raw_operations
            || selected.metadata != self.metadata || selected.native_connector != self.native_connector
            || selected.no_credential_references != self.no_credential_references
            || selected.credential_references != self.credential_references {
            return Err(unconfirmed());
        }
        Ok(())
    }
}
/// Project the one observed package stdio launch without inventing credentials
/// or native connector identity. Callers separately own the no-auth declaration
/// and exact allowed operations; this pure function carries no authority.
pub(crate) fn public_launch_configuration(
    installed: &InstalledSelection, configuration: &Value,
    operations: &std::collections::BTreeSet<String>,
) -> Result<meerkat_core::McpServerConfig, &'static str> {
    if operations.is_empty() || operations.len() > 128 { return Err("invalid_operation_binding"); }
    let document = configuration.as_object().ok_or("unsupported_public_launch")?;
    let servers = document.get("mcpServers").and_then(Value::as_object)
        .ok_or("unsupported_public_launch")?;
    if document.len() != 1 || servers.len() != 1 { return Err("unsupported_public_launch"); }
    let launch: meerkat_core::mcp_config::McpStdioConfig = serde_json::from_value(servers.get(&installed.server_name)
        .ok_or("unsupported_public_launch")?.clone()).map_err(|_| "unsupported_public_launch")?;
    if !std::path::Path::new(&launch.command).is_absolute() || launch.command.contains('\0')
        || launch.args.iter().any(|argument| argument.contains('\0')) || !launch.env.is_empty()
        || !launch.secret_env.is_empty()
    { return Err("unsupported_public_launch"); }
    let mut config = meerkat_core::McpServerConfig::stdio(&installed.server_name,
        launch.command, launch.args, launch.env);
    config.access = Some(meerkat_core::McpServerAccess::allowing(operations.iter().cloned()));
    meerkat_core::mcp_config::reject_realm_server_env_references(std::slice::from_ref(&config))
        .map_err(|_| "unsupported_public_launch")?;
    config.validate_access().map_err(|_| "invalid_operation_binding")?;
    Ok(config)
}

/// Preserve the checked normal stdio adapter. Only the complete package-owned
/// credential declarations supply names for the native realm secret resolver.
pub(crate) fn secret_launch_configuration(
    installed: &InstalledSelection, configuration: &Value,
    operations: &std::collections::BTreeSet<String>, references: &[ServiceCredentialSlot],
) -> Result<meerkat_core::McpServerConfig, &'static str> {
    let slots: std::collections::BTreeSet<_> = references.iter()
        .map(|reference| reference.environment_variable().to_owned()).collect();
    if slots.is_empty() || slots.len() != references.len() || slots.len() > 256
        || slots.iter().any(|name| !environment_name(name))
    { return Err("checked_credential_references_required"); }
    // This projector enforces one exact launch, no inline environment values
    // and no package-supplied native secret_env. Its caller owns authentication
    // classification; calling it does not claim this is a public service.
    let mut config = public_launch_configuration(installed, configuration, operations)?;
    let meerkat_core::mcp_config::McpTransportConfig::Stdio(launch) = &mut config.transport else {
        return Err("unsupported_secret_launch");
    };
    launch.secret_env = slots;
    meerkat_core::mcp_config::reject_realm_server_env_references(std::slice::from_ref(&config))
        .map_err(|_| "unsupported_secret_launch")?;
    config.validate_access().map_err(|_| "invalid_operation_binding")?;
    Ok(config)
}

impl Readback {
    pub(crate) fn available(&self) -> bool {
        self.epoch == self.binding.execution_binding_epoch("setup_job")
            && self.job.lock().is_ok_and(|job| job.is_some())
    }
}

pub(crate) struct HeldInstall {
    key: String,
    call_id: String,
    name: String,
    arguments: String,
    review: Value,
    readback: Readback,
}
impl HeldInstall {
    pub(crate) fn realm(&self) -> &meerkat_core::RealmId { &self.readback.realm.realm }
    pub(crate) fn review(&self) -> Value {
        self.review.clone()
    }
    pub(crate) fn readback(&self) -> Readback {
        self.readback.clone()
    }
}

#[derive(Default)]
struct State {
    proposals: BTreeMap<String, Proposal>,
    remote: BTreeMap<String, remote::RemoteCapture>,
    remote_bytes: usize,
    returning: retained::Returning,
    retained_bytes: usize,
    armed: Option<HeldInstall>,
    retired: bool,
    realm: Option<RealmLocator>,
}

pub(crate) struct CheckedInstalls {
    // Production constructs this only from its exact retained McpRouterAdapter.
    inner: Arc<dyn AgentToolDispatcher>,
    state: Mutex<State>,
    // Installed compositions reuse this one approval owner locally. Shared
    // approval custody retains only the inner binding, never this wrapper.
    software_review: Option<Arc<crate::approval::LocalApprovals>>,
    // Only the standalone composition owns a checked native route. The route
    // never escapes this wrapper into the session's dispatch surface.
    checked_route: Option<Arc<meerkat::CheckedMcpRoute>>,
}
impl CheckedInstalls {
    pub(crate) fn new(inner: Arc<meerkat::McpRouterAdapter>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            state: Mutex::new(State::default()),
            software_review: None,
            checked_route: None,
        })
    }

    pub(crate) fn with_checked_route(
        adapter: Arc<meerkat::McpRouterAdapter>,
        session: SessionId,
    ) -> Result<Arc<Self>, meerkat::CheckedMcpRouteError> {
        let route = adapter.checked_route(session, meerkat::CheckedMcpRouteSpec {
            route: CHECKED_INSTALL_ROUTE.to_owned(),
            server: crate::setup::SERVER_NAME.to_owned(),
            operations: checked_install_operations(),
        })?;
        let inner = Arc::new(meerkat_core::gateway::DynamicToolComposite::new(vec![
            adapter, route.clone(),
        ]));
        Ok(Arc::new(Self {
            inner,
            state: Mutex::new(State::default()),
            software_review: None,
            checked_route: Some(route),
        }))
    }

    pub(crate) fn with_software_review(
        inner: Arc<meerkat::McpRouterAdapter>,
        approvals: Arc<crate::approval::LocalApprovals>,
    ) -> Arc<Self> {
        Arc::new(Self {
            inner,
            state: Mutex::new(State::default()),
            software_review: Some(approvals),
            checked_route: None,
        })
    }



    #[cfg(test)]
    pub(crate) fn fixture_with_review(
        inner: Arc<dyn AgentToolDispatcher>,
        approvals: Arc<crate::approval::LocalApprovals>,
    ) -> Arc<Self> {
        Arc::new(Self {
            inner,
            state: Mutex::new(State::default()),
            software_review: Some(approvals),
            checked_route: None,
        })
    }

    #[cfg(test)]
    pub(crate) fn fixture(inner: Arc<dyn AgentToolDispatcher>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            state: Mutex::new(State::default()),
            software_review: None,
            checked_route: None,
        })
    }

    #[cfg(test)]
    pub(crate) fn poison_fixture_state(&self) {
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _state = self.state.lock().unwrap();
            panic!("synthetic checked-install custody poison");
        }));
        assert!(poisoned.is_err());
        assert!(self.state.is_poisoned());
    }

    pub(crate) fn retire(&self) {
        if let Some(route) = &self.checked_route {
            route.close();
        }
        if let Ok(mut state) = self.state.lock() {
            state.retired = true;
            state.armed = None;
            state.remote.clear();
            state.remote_bytes = 0;
            state.returning = retained::Returning::default();
            for proposal in state.proposals.values() {
                if let Ok(mut slot) = proposal.operation_names.lock() {
                    slot.publisher = None;
                    slot.observed = None;
                }
            }
        }
    }

    /// Bind the host's selected native storage before opening its setup session.
    /// Equal realm names at different roots remain different selections.
    pub(crate) fn bind_realm(&self, realm: &RealmLocator) -> Result<(), ToolError> {
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        if state.retired || !state.proposals.is_empty() || !state.remote.is_empty() || !state.returning.is_empty() {
            return Err(refused("setup_install"));
        }
        match &state.realm {
            Some(existing) if existing != realm => Err(refused("setup_install")),
            Some(_) => Ok(()),
            None => {
                state.realm = Some(realm.clone());
                Ok(())
            }
        }
    }

    /// Host-only access to evidence from one already selected job. The caller
    /// must still join native account facts and obtain actual access authority.
    pub(crate) fn installed_selection(
        &self,
        realm: &RealmLocator,
        session: &SessionId,
        job_id: &str,
    ) -> Result<ReceiptCapture, ToolError> {
        let state = self.state.lock().map_err(|_| unavailable())?;
        if state.realm.as_ref() != Some(realm) {
            return Err(refused("setup_job"));
        }
        let target = state
            .proposals
            .values()
            .filter_map(|p| p.readback.as_ref())
            .find(|r| {
                r.session == *session && r.job.lock().is_ok_and(|j| j.as_deref() == Some(job_id))
            })
            .ok_or_else(|| refused("setup_job"))?;
        let mut receipt = target.receipt.lock().map_err(|_| unavailable())?.clone();
        if let Some(installed) = &mut receipt.installed {
            installed.recovery_request_sha256 = receipt.recovery_request_sha256.clone();
        }
        receipt.matched &=
            !state.retired && target.epoch == self.inner.execution_binding_epoch("setup_job");
        Ok(receipt)
    }

    pub(crate) fn connector_selection(
        self: &Arc<Self>, realm: &RealmLocator, session: &SessionId, job_id: &str,
    ) -> Result<InstalledConnectorSelection, ConnectorSelectionError> {
        use ConnectorSelectionError::Unconfirmed;
        let state = self.state.lock().map_err(|_| Unconfirmed)?;
        if state.retired || state.realm.as_ref() != Some(realm) {
            return Err(Unconfirmed);
        }
        let readback = state.proposals.values().filter_map(|p| p.readback.as_ref())
            .find(|r| r.session == *session && r.job.lock()
                .is_ok_and(|j| j.as_deref() == Some(job_id)));
        let Some(readback) = readback else {
            return state.returning.selection(self, session, job_id).ok_or(Unconfirmed);
        };
        let selected_package = readback.proposal["check_tool"] == SELECTED_CHECK;
        let metadata_tool = if selected_package { SELECTED_CHECK } else { "setup_help" };
        if readback.epoch != self.inner.execution_binding_epoch("setup_job")
            || readback.epoch != self.inner.execution_binding_epoch(metadata_tool) {
            return Err(Unconfirmed);
        }
        let capture = readback.receipt.lock().map_err(|_| Unconfirmed)?;
        let installed = capture.installed.as_ref().filter(|_| capture.matched)
            .filter(|i| i.receipt_sha256.is_some()).ok_or(Unconfirmed)?;
        let configuration = capture.configuration.clone().ok_or(Unconfirmed)?;
        let metadata = readback.operation_names.lock().map_err(|_| Unconfirmed)?
            .observed.clone().ok_or_else(|| ConnectorSelectionError::MetadataRequired {
                family: installed.family.clone(), profile: installed.profile.clone(),
            })?;
        let raw_operations = if selected_package {
            if !matches!(&metadata, OperationEvidence::Selected { .. }) { return Err(Unconfirmed); }
            metadata.names().values().cloned().collect()
        } else {
            let selected = readback.proposal["selection"]["operations"].as_array()
                .filter(|v| !v.is_empty()).ok_or(Unconfirmed)?;
            let raw = selected.iter().map(|id| {
                metadata.names().get(id.as_str().ok_or(Unconfirmed)?)
                    .cloned().ok_or(Unconfirmed)
            }).collect::<Result<std::collections::BTreeSet<_>, _>>()?;
            if raw.len() != selected.len() { return Err(Unconfirmed); }
            raw
        };
        let mut installed = installed.clone();
        installed.recovery_request_sha256 = capture.recovery_request_sha256.clone();
        let no_credential_references = readback.proposal["owner_proposal"]
            .get("credential_environment_references").and_then(Value::as_array)
            .is_some_and(Vec::is_empty);
        let credential_references = service_credential_references(&readback.proposal, &metadata).map_err(|_| Unconfirmed)?;
        Ok(InstalledConnectorSelection { installed, configuration,
            raw_operations, native_connector: capture.native_connector.clone(), owner: self.clone(),
            no_credential_references, credential_references, metadata })
    }

    pub(crate) fn retain_action(
        &self,
        call: ToolCallView<'_>,
        session: &SessionId,
    ) -> Result<HeldInstall, ToolError> {
        let arguments = document(call.args.get())?;
        let key = hex(&arguments, "validation_id", 32).ok_or_else(|| refused(call.name))?;
        let state = self.state.lock().map_err(|_| unavailable())?;
        let proposal = state.proposals.get(key).ok_or_else(|| refused(call.name))?;
        let realm = state.realm.clone().ok_or_else(|| refused(call.name))?;
        if state.retired
            || state.armed.is_some()
            || proposal.used
            || proposal.session != *session
            || proposal.kind.install() != call.name
            || proposal.epoch != self.inner.execution_binding_epoch(call.name)
            || arguments != proposal.install
        {
            return Err(refused(call.name));
        }
        let family = match proposal.kind {
            Kind::Library => proposal.install["family"]
                .as_str()
                .ok_or_else(|| refused(call.name))?
                .into(),
            Kind::Selected => format!(
                "selected-{}",
                proposal.review["owner_proposal"]["profile"]
                    .as_str()
                    .ok_or_else(|| refused(call.name))?
            ),
        };
        Ok(HeldInstall {
            key: key.into(),
            call_id: call.id.into(),
            name: call.name.into(),
            arguments: call.args.get().into(),
            review: proposal.review.clone(),
            readback: Readback {
                epoch: proposal.epoch,
                session: session.clone(),
                job: Arc::new(Mutex::new(None)),
                family,
                binding: self.inner.clone(),
                realm,
                validation_id: key.into(),
                proposal: Arc::new(proposal.review.clone()),
                receipt: Arc::new(Mutex::new(ReceiptCapture::default())),
                operation_names: proposal.operation_names.clone(),
            },
        })
    }

    pub(crate) fn arm_action(&self, held: HeldInstall) -> Result<(), ToolError> {
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        let proposal = state
            .proposals
            .get(&held.key)
            .ok_or_else(|| refused("setup_install"))?;
        if state.retired
            || state.armed.is_some()
            || proposal.used
            || proposal.session != held.readback.session
            || proposal.review != held.review
            || proposal.epoch != held.readback.epoch
            || proposal.epoch != self.inner.execution_binding_epoch(proposal.kind.install())
        {
            return Err(refused("setup_install"));
        }
        state.armed = Some(held);
        Ok(())
    }

    pub(crate) fn discard_unused_arm(&self, call: ToolCallView<'_>) -> Result<(), ToolError> {
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        if state.armed.as_ref().is_some_and(|held| {
            held.call_id == call.id && held.name == call.name && held.arguments == call.args.get()
        }) {
            state.armed = None;
        }
        Ok(())
    }

    /// Join a retained review to this exact checked owner and proposal. Only
    /// nonsecret correlation and a retained job locator leave this owner.
    pub(crate) fn software_review_binding(
        &self, target: &Readback, call: ToolCallView<'_>, session: &SessionId,
        native: &meerkat_core::ApprovalRecord,
    ) -> Result<Value, ToolError> {
        let arguments = document(call.args.get())?;
        let state = self.state.lock().map_err(|_| unavailable())?;
        let proposal = state.proposals.get(&target.validation_id).ok_or_else(|| refused("setup_state"))?;
        let mut reviewed = proposal.review.clone();
        reviewed["tool"] = call.name.into();
        reviewed["call_id"] = call.id.into();
        if state.retired || state.realm.as_ref() != Some(&target.realm)
            || target.session != *session || proposal.session != *session
            || !Arc::ptr_eq(&target.binding, &self.inner)
            || !Arc::ptr_eq(&target.operation_names, &proposal.operation_names)
            || target.epoch != proposal.epoch
            || target.epoch != self.inner.execution_binding_epoch(call.name)
            || proposal.kind.install() != call.name || proposal.install != arguments
            || target.proposal.as_ref() != &proposal.review
            || !matches!(&native.owner, meerkat_core::ApprovalOwnerRef::Session { session_id } if session_id == session)
            || native.resource.kind != meerkat_core::ApprovalResourceKind::ToolCall
            || native.resource.id != meerkat_core::ApprovalResourceId::new(call.name)
            || native.proposed_action.kind != meerkat_core::ApprovalActionKind::ToolCall
            || native.proposed_action.body.as_ref() != Some(&reviewed)
        { return Err(refused("setup_state")); }
        let job = target.job.lock().map_err(|_| unavailable())?.clone();
        Ok(json!({"call_id":call.id, "tool":call.name,
            "arguments_sha256":format!("{:x}", Sha256::digest(call.args.get().as_bytes())),
            "validation_id":target.validation_id, "retained_install_job_id":job}))
    }

    async fn observe_software_review(
        &self, call: ToolCallView<'_>, context: &ToolDispatchContext,
        plan: Option<&meerkat_core::ResolvedToolExecutionPlan>,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        let approvals = self.software_review.as_ref().ok_or_else(|| refused(call.name))?;
        if document(call.args.get())? != json!({}) { return Err(refused(call.name)); }
        let refreshed = context.check_tool_authorization(call, plan)?;
        let current = refreshed.as_ref().unwrap_or(context);
        approvals.check_session(current.origin_session_id().ok_or_else(|| refused(call.name))?)
            .map_err(|_| refused(call.name))?;
        let mut outcome = self.inner_call(call, current, plan).await?;
        let refreshed = current.check_tool_authorization(call, plan)?;
        let current = refreshed.as_ref().unwrap_or(current);
        let mut value = response_document(call, &outcome)?;
        if outcome.result.is_error || value.get("error").is_some() { return Ok(outcome); }
        // An inner server cannot substitute its own review observation.
        if value.get("software_review_observation").is_some() { return Err(unconfirmed()); }
        value["software_review_observation"] = approvals.software_review_observation(self, call, current, plan).await?;
        current.check_tool_authorization(call, plan)?;
        let text = value.to_string();
        if text.len() > MAX_DOCUMENT { return Err(unconfirmed()); }
        outcome.result.content = vec![ContentBlock::Text { text }];
        Ok(outcome)
    }

    pub(crate) fn permits_readback(&self, target: &Readback, call: ToolCallView<'_>) -> bool {
        if !["setup_job", "setup_reconcile", "setup_stop"].contains(&call.name)
            || target.epoch != self.inner.execution_binding_epoch(call.name)
        {
            return false;
        }
        let Ok(job) = target.job.lock() else {
            return false;
        };
        job.as_ref().is_some_and(|id| {
            document(call.args.get()).is_ok_and(|mut args| {
                if call.name == "setup_job"
                    && let Some(object) = args.as_object_mut()
                    && let Some(wait) = object.remove("wait_ms")
                    && !wait.as_u64().is_some_and(|millis| millis <= 10_000)
                {
                    return false;
                }
                args == json!({"job_id":id})
            })
        })
    }

    fn capture(
        &self,
        call: ToolCallView<'_>,
        session: &SessionId,
        epoch: u64,
        kind: Kind,
        outcome: &ToolDispatchOutcome,
    ) -> Result<(), ToolError> {
        let request = document(call.args.get())?;
        if outcome.result.is_error {
            return Ok(());
        }
        let response = result_document(call, outcome)?;
        // Preserve the owner's correction guidance, but never retain a failed
        // configuration check as an approvable proposal.
        if response.get("error").is_some() {
            return Ok(());
        }
        let key = hex(&response, "validation_id", 32)
            .ok_or_else(unconfirmed)?
            .to_owned();
        let install = match kind {
            Kind::Library => {
                if !exact_keys(&request, &["family", "profile", "operations", "values"])
                    || request["family"].as_str().is_none()
                    || request["profile"].as_str().is_none()
                    || response["scope"] != "configuration_only"
                    || response["family"] != request["family"]
                    || response["profile"] != request["profile"]
                {
                    return Err(unconfirmed());
                }
                let digest = hex(&response, "configuration_sha256", 64).ok_or_else(unconfirmed)?;
                let mut install = request.clone();
                install["validation_id"] = key.clone().into();
                install["configuration_sha256"] = digest.into();
                install
            }
            Kind::Selected => {
                if response["scope"] != "selected_software_inputs_only"
                    || request["profile"].as_str().is_none()
                    || response["profile"] != request["profile"]
                {
                    return Err(unconfirmed());
                }
                let digest = hex(&response, "effect_sha256", 64).ok_or_else(unconfirmed)?;
                json!({"validation_id":key,"effect_sha256":digest})
            }
        };
        let metadata = if kind == Kind::Selected { selected_service_metadata(&response)? } else { None };
        // This is the actual owner's response, not a local reconstruction of
        // its package plan, validation, expected account, or install receipt.
        let review = json!({"kind":"service_software_install","check_tool":call.name,
            "selection":request,"owner_proposal":response,
            "scope":"software_installation_only"});
        let charge =
            (call.args.get().len() + outcome.result.text_content().len()).saturating_mul(3);
        let mut state = self.state.lock().map_err(|_| unconfirmed())?;
        if state.retired || epoch != self.inner.execution_binding_epoch(call.name) {
            return Err(unconfirmed());
        }
        if let Some(old) = state.proposals.get(&key) {
            return if old.session == *session
                && old.epoch == epoch
                && old.kind == kind
                && old.install == install
                && old.review == review
            {
                Ok(())
            } else {
                *old.operation_names.lock().map_err(|_| unavailable())? = OperationMetadata::default();
                if let Some(readback) = &old.readback {
                    readback.receipt.lock().map_err(|_| unavailable())?.matched = false;
                }
                Err(unconfirmed())
            };
        }
        if state.proposals.len() >= MAX_PROPOSALS
            || state.retained_bytes.saturating_add(charge) > MAX_RETAINED
        {
            return Err(unconfirmed());
        }
        state.retained_bytes += charge;
        state.proposals.insert(
            key,
            Proposal {
                session: session.clone(),
                epoch,
                kind,
                install,
                review,
                used: false,
                readback: None,
                operation_names: Arc::new(Mutex::new(OperationMetadata { publisher: None, observed: metadata })),
            },
        );
        Ok(())
    }

    fn consume(&self, call: ToolCallView<'_>, session: &SessionId) -> Result<Readback, ToolError> {
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        let held = state.armed.as_ref().ok_or_else(|| refused(call.name))?;
        let proposal = state
            .proposals
            .get(&held.key)
            .ok_or_else(|| refused(call.name))?;
        if state.retired
            || proposal.used
            || proposal.kind.install() != call.name
            || held.call_id != call.id
            || held.arguments != call.args.get()
            || held.readback.session != *session
            || proposal.epoch != self.inner.execution_binding_epoch(call.name)
        {
            return Err(refused(call.name));
        }
        let held = state.armed.take().ok_or_else(|| refused(call.name))?;
        let proposal = state
            .proposals
            .get_mut(&held.key)
            .ok_or_else(|| refused(call.name))?;
        proposal.used = true;
        proposal.readback = Some(held.readback.clone());
        Ok(held.readback)
    }

    async fn observe_readback(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        operation: impl Future<Output = Result<ToolDispatchOutcome, ToolError>>,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        let args = document(call.args.get())?;
        let target = {
            let state = self.state.lock().map_err(|_| unavailable())?;
            state
                .proposals
                .values()
                .filter_map(|p| p.readback.as_ref())
                .find(|r| {
                    r.job
                        .lock()
                        .is_ok_and(|j| j.as_deref().is_some_and(|id| args["job_id"] == id))
                })
                .cloned()
        };
        // Acquisition and other jobs remain owned by the existing dispatcher.
        // An unknown job cannot create a checked-install receipt here.
        let Some(target) = target else {
            return self.observe_remote_readback(call, context, operation).await;
        };
        if context.origin_session_id() != Some(&target.session)
            || !self.permits_readback(&target, call)
        {
            return Err(refused(call.name));
        }
        // A dropped/lost read keeps the last historical record, but cannot
        // advertise a fresh match. A later exact owner read can reconcile it.
        target.receipt.lock().map_err(|_| unavailable())?.matched = false;
        let outcome = operation.await?;
        if target.epoch != self.inner.execution_binding_epoch(call.name) {
            return Err(unconfirmed());
        }
        let value = response_document(call, &outcome)?;
        // A correlated owner error (for example, a busy readback) is not a
        // lost response. Preserve it without publishing a receipt match.
        if outcome.result.is_error || value.get("error").is_some() {
            return Ok(outcome);
        }
        self.capture_readback(&target, &value, call.name == "setup_job")?;
        Ok(outcome)
    }

    async fn observe_help(
        &self, call: ToolCallView<'_>, context: &ToolDispatchContext,
        operation: impl Future<Output = Result<ToolDispatchOutcome, ToolError>>,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        let request = document(call.args.get())?;
        let session = context.origin_session_id().ok_or_else(|| refused(call.name))?;
        let epoch = self.inner.execution_binding_epoch(call.name);
        let targets = {
            let state = self.state.lock().map_err(|_| unavailable())?;
            if state.retired { return Err(refused(call.name)); }
            let targets = state.proposals.values().filter(|p| p.kind == Kind::Library
                && p.session == *session && p.epoch == epoch
                && p.install["family"] == request["family"]
                && p.install["profile"] == request["profile"])
                .map(|p| p.operation_names.clone()).collect::<Vec<_>>();
            let mut owned = Vec::new();
            for target in targets {
                let token = Arc::new(());
                *target.lock().map_err(|_| unavailable())? = OperationMetadata {
                    publisher: Some(token.clone()), observed: None,
                };
                owned.push((target, token));
            }
            owned
        };
        // Reserve each existing slot before awaiting. A newer help, conflict
        // or retirement displaces this publisher; a late reply cannot restore it.
        let outcome = operation.await?;
        if targets.is_empty() { return Ok(outcome); }
        let response = result_document(call, &outcome)?;
        if response.get("error").is_some() { return Ok(outcome); }
        if response["family"] != request["family"] || response["profile"] != request["profile"] {
            return Err(unconfirmed());
        }
        let Some(metadata) = publisher_metadata(&response)? else { return Ok(outcome); };
        let state = self.state.lock().map_err(|_| unavailable())?;
        if state.retired || epoch != self.inner.execution_binding_epoch(call.name) {
            return Err(unconfirmed());
        }
        for (target, token) in &targets {
            if !target.lock().map_err(|_| unavailable())?.publisher.as_ref()
                .is_some_and(|owner| Arc::ptr_eq(owner, token)) {
                return Err(unconfirmed());
            }
        }
        for (target, _) in targets {
            *target.lock().map_err(|_| unavailable())? = OperationMetadata {
                publisher: None, observed: Some(metadata.clone()),
            };
        }
        Ok(outcome)
    }

    fn capture_readback(
        &self,
        target: &Readback,
        value: &Value,
        includes_job: bool,
    ) -> Result<(), ToolError> {
        let expected = target
            .job
            .lock()
            .map_err(|_| unavailable())?
            .clone()
            .ok_or_else(unconfirmed)?;
        // job_detail is one owner snapshot. Keep the receipt lock until its
        // job and observation have both been checked; never publish the job
        // as matched before reading that same snapshot's reconciliation.
        let mut capture = target.receipt.lock().map_err(|_| unavailable())?;
        capture.matched = false;
        let observation = if includes_job {
            match value.get("reconciliation") {
                None => None,
                Some(reconciliation) => {
                    let observation = reconciliation
                        .as_object()
                        .and_then(|v| v.get("observation"))
                        .ok_or_else(unconfirmed)?;
                    (!observation.is_null()).then_some(observation)
                }
            }
        } else {
            Some(&value["observation"])
        };
        if let Some(observation) = observation {
            let revision = observation["revision"]
                .as_u64()
                .filter(|v| *v > 0)
                .ok_or_else(unconfirmed)?;
            if observation["job_id"] != expected || observation["scope"] != READBACK_SCOPE {
                return Err(unconfirmed());
            }
            if capture
                .reconciliation_revision
                .is_some_and(|old| revision < old)
            {
                return Err(unconfirmed());
            }
            // Preserve the highest structurally bound owner revision even
            // when its status, result or installed identity is rejected.
            capture.reconciliation_revision = Some(revision);
        }
        let mut next = capture.clone();
        if includes_job {
            let job = &value["job"];
            if job["id"] != expected || job["kind"] != "install" || job["family"] != target.family {
                return Err(unconfirmed());
            }
            if let Some(anchor) = job.get("recovery_request") {
                let request_sha = hex(anchor, "sha256", 64).ok_or_else(unconfirmed)?;
                if !exact_keys(anchor, &["id", "sha256"]) || anchor["id"] != expected
                    || next.recovery_request_sha256.as_ref().is_some_and(|old| old != request_sha)
                { return Err(unconfirmed()); }
                next.recovery_request_sha256 = Some(request_sha.to_owned());
            } else if next.recovery_request_sha256.is_some() {
                return Err(unconfirmed());
            }
            match job["status"].as_str() {
                Some("succeeded") => {
                    let installed = installed_projection(target, &expected, &job["result"], false)?;
                    retain_receipt(&mut next, installed, false)?;
                }
                Some("running" | "unconfirmed") => {}
                _ => return Err(unconfirmed()),
            }
        }
        if let Some(observation) = observation {
            match observation["status"].as_str() {
                Some("matched") => {
                    let installed =
                        installed_projection(target, &expected, &observation["result"], true)?;
                    retain_receipt(&mut next, installed, true)?;
                }
                Some("checking" | "unconfirmed") => next.matched = false,
                _ => return Err(unconfirmed()),
            }
        }
        *capture = next;
        Ok(())
    }

    async fn dispatch_checked(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: Option<&meerkat_core::ResolvedToolExecutionPlan>,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        if call.name == "setup_state" && self.software_review.is_some() {
            return self.observe_software_review(call, context, plan).await;
        }
        let Some(approvals) = self.software_review.as_ref()
            .filter(|_| INSTALL_TOOLS.contains(&call.name)) else {
            return self.delegate(call, context, plan, self.inner_call(call, context, plan)).await;
        };
        if matches!(approvals.await_service_dispatch(self, call, context, plan).await?,
            crate::approval::SoftwareReviewOutcome::RequiresHuman)
        {
            // This is the application's next action, not a native task state,
            // approval, installation receipt or authority to retry automatically.
            context.check_tool_authorization(call, plan)?;
            return Ok(ToolResult::new(call.id.into(),
                serde_json::json!({"state":"requires_action","action":"human_installation_review",
                    "installed":false}).to_string(), false).into());
        }
        // This exact future owns loss/uncertainty through physical settlement.
        // No await separates the arm from installing its cancellation guard.
        let _custody = approvals.service_dispatch_guard(call, context)?;
        // Human review can outlast the originating permission. Recheck this
        // exact borrowed call and actual plan without extracting/rebuilding the
        // native prepared check. The outer native gate remains its own owner.
        let refreshed = match context.check_tool_authorization(call, plan) {
            Ok(current) => current,
            Err(error) => return approvals.settle_service_result(
                self, call, context, meerkat_core::LiveBridgeEffectOutcome::Failed, Err(error)),
        };
        let current = refreshed.as_ref().unwrap_or(context);
        if approvals.check_session(current.origin_session_id()
            .ok_or_else(|| refused(call.name))?).is_err() {
            return approvals.settle_service_result(self, call, current,
                meerkat_core::LiveBridgeEffectOutcome::Failed, Err(refused(call.name)));
        }
        if let Err(error) = approvals.check_service_entry(call, current) {
            return approvals.settle_service_result(self, call, current,
                meerkat_core::LiveBridgeEffectOutcome::Failed, Err(error));
        }
        let mut physical = None;
        let result = self.delegate(call, current, plan, async {
            let result = self.inner_call(call, current, plan).await;
            // The package owner's return is distinct from later local capture
            // failures. A malformed software receipt does not undo entry.
            physical = Some(if result.is_ok() {
                meerkat_core::LiveBridgeEffectOutcome::Committed
            } else {
                meerkat_core::LiveBridgeEffectOutcome::Unknown
            });
            result
        }).await;
        // No later await separates the actual result from local settlement.
        // Dropping an entered future leaves the existing SpentUncertain state.
        approvals.settle_service_result(
            self, call, current,
            physical.unwrap_or(meerkat_core::LiveBridgeEffectOutcome::Failed), result,
        )
    }

    async fn inner_call(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: Option<&meerkat_core::ResolvedToolExecutionPlan>,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        let outcome = match plan {
            Some(plan) => self.inner.dispatch_resolved_with_context(call, context, plan).await,
            None => self.inner.dispatch_with_context(call, context).await,
        };
        outcome
    }

    async fn delegate(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: Option<&meerkat_core::ResolvedToolExecutionPlan>,
        operation: impl Future<Output = Result<ToolDispatchOutcome, ToolError>>,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        if call.name == "setup_export_recipe" {
            return self.observe_export(call, context, operation).await;
        }
        let check = Kind::check(call.name);
        if check.is_none() && !INSTALL_TOOLS.contains(&call.name) {
            if call.name == "setup_help" {
                return self.observe_help(call, context, operation).await;
            }
            if ["setup_job", "setup_reconcile"].contains(&call.name) {
                return self.observe_readback(call, context, operation).await;
            }
            return operation.await;
        }
        let session = context
            .origin_session_id()
            .ok_or_else(|| refused(call.name))?;
        let epoch = self.inner.execution_binding_epoch(call.name);
        if self.state.lock().map_err(|_| unavailable())?.retired {
            return Err(refused(call.name));
        }
        // No await separates consuming the exact arm from first polling the
        // retained native dispatcher. Cancellation cannot rearm this proposal.
        let readback = if check.is_none() {
            Some(self.consume(call, session)?)
        } else {
            None
        };
        let outcome = operation.await?;
        let _ = plan;
        if let Some(kind) = check {
            self.capture(call, session, epoch, kind, &outcome)?;
        } else if let Some(readback) = readback {
            let value = result_document(call, &outcome)?;
            if value.get("error").is_some() {
                return Err(unconfirmed());
            }
            let job = hex(&value["job"], "id", 32).ok_or_else(unconfirmed)?;
            if value["job"]["kind"] != "install" || value["job"]["family"] != readback.family {
                return Err(unconfirmed());
            }
            {
                // Consume and job binding use the same state owner. Hold its
                // lock across duplicate detection and binding so concurrent
                // install replies cannot both claim one returned job ID.
                let state = self.state.lock().map_err(|_| unconfirmed())?;
                if state
                    .proposals
                    .values()
                    .filter_map(|p| p.readback.as_ref())
                    .any(|other| {
                        !Arc::ptr_eq(&other.receipt, &readback.receipt)
                            && other
                                .job
                                .lock()
                                .is_ok_and(|known| known.as_deref() == Some(job))
                    })
                {
                    return Err(unconfirmed());
                }
                // A job locator is not job state, completion, or permission.
                // Keep it even if a later local publication check fails.
                *readback.job.lock().map_err(|_| unconfirmed())? = Some(job.into());
            }
            self.capture_readback(&readback, &value, true)?;
            if epoch != self.inner.execution_binding_epoch(call.name)
                || self.state.lock().map_err(|_| unconfirmed())?.retired
            {
                return Err(unconfirmed());
            }
        }
        Ok(outcome)
    }
}

fn installed_projection(
    target: &Readback,
    job_id: &str,
    result: &Value,
    reconciliation: bool,
) -> Result<(InstalledSelection, Arc<Value>, Option<NativeConnectorEntry>), ToolError> {
    if result["scope"]
        != if reconciliation {
            READBACK_SCOPE
        } else {
            "observed_software_installation_only"
        }
    {
        return Err(unconfirmed());
    }
    let proposal = &target.proposal["owner_proposal"];
    let selection = &target.proposal["selection"];
    let profile = proposal["profile"].as_str().ok_or_else(unconfirmed)?;
    if target.proposal["check_tool"] == "setup_check" {
        if result["family"] != target.family
            || result["profile"] != profile
            || result["configuration_sha256"] != proposal["configuration_sha256"]
            || (!reconciliation && result["contents_sha256"] != proposal["contents_sha256"])
        {
            return Err(unconfirmed());
        }
    } else {
        if result["selection_kind"] != profile
            || result["component"] != proposal["component"]
            || SelectedDocument::from_value(result)? != SelectedDocument::from_value(proposal)?
            || result["effect_sha256"] != proposal["effect_sha256"]
            || result.get("declared_service") != proposal.get("declared_service")
            || (profile == "mcpb"
                && result["configuration_sha256"] != selection["configuration_sha256"])
        {
            return Err(unconfirmed());
        }
    }
    installed_launch(InstalledIdentity {
        realm: &target.realm, session: &target.session, job_id,
        validation_id: &target.validation_id, family: &target.family, profile, mcp_name: proposal.get("mcp_name"),
    }, result, reconciliation, proposal["destination"].as_str())
}

struct InstalledIdentity<'a> {
    realm: &'a RealmLocator,
    session: &'a SessionId,
    job_id: &'a str,
    validation_id: &'a str,
    family: &'a str,
    profile: &'a str,
    mcp_name: Option<&'a Value>,
}

fn installed_launch(
    identity: InstalledIdentity<'_>, result: &Value, reconciliation: bool, destination: Option<&str>,
) -> Result<(InstalledSelection, Arc<Value>, Option<NativeConnectorEntry>), ToolError> {
    let path = |key: &str| -> Result<PathBuf, ToolError> {
        let text = result[key]
            .as_str()
            .filter(|p| p.len() <= 4096 && !p.contains('\0'))
            .ok_or_else(unconfirmed)?;
        let path = PathBuf::from(text);
        if !path.is_absolute()
            || path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(unconfirmed());
        }
        Ok(path)
    };
    let mcp_path = path("mcp_path")?;
    let receipt_path = path("receipt_path")?;
    if mcp_path.file_name().is_none_or(|n| n != "mcp.json")
        || receipt_path
            .file_name()
            .is_none_or(|n| n != "installed.json")
        || mcp_path.parent() != receipt_path.parent()
    {
        return Err(unconfirmed());
    }
    if let Some(destination) = destination
        && mcp_path != PathBuf::from(destination).join("ready/mcp.json")
    {
        return Err(unconfirmed());
    }
    let mcp = &result["mcp_configuration"];
    if !exact_keys(mcp, &["mcpServers"]) {
        return Err(unconfirmed());
    }
    let servers = mcp["mcpServers"]
        .as_object()
        .filter(|m| m.len() == 1)
        .ok_or_else(unconfirmed)?;
    let (server_name, server) = servers.iter().next().ok_or_else(unconfirmed)?;
    if server_name.is_empty()
        || server_name.len() > 256
        || server_name.chars().any(char::is_control)
        || !server.is_object()
        || identity.mcp_name
            .is_some_and(|name| name.as_str() != Some(server_name.as_str()))
    {
        return Err(unconfirmed());
    }
    let receipt_sha256 = result
        .get("receipt_sha256")
        .map(|_| {
            hex(result, "receipt_sha256", 64)
                .map(str::to_owned)
                .ok_or_else(unconfirmed)
        })
        .transpose()?;
    if reconciliation && receipt_sha256.is_none() {
        return Err(unconfirmed());
    }
    let configuration_sha256 = result
        .get("configuration_sha256")
        .map(|_| {
            hex(result, "configuration_sha256", 64)
                .map(str::to_owned)
                .ok_or_else(unconfirmed)
        })
        .transpose()?;
    let native_connector = if reconciliation {
        result.get("native_connector")
            .map(|v| NativeConnectorEntry::observed(v, identity.family, identity.profile)).transpose()?
    } else { None };
    Ok((InstalledSelection {
        realm: identity.realm.clone(),
        session: identity.session.clone(),
        job_id: identity.job_id.into(),
        validation_id: identity.validation_id.into(),
        family: identity.family.into(),
        profile: identity.profile.into(),
        mcp_path,
        receipt_path,
        receipt_sha256,
        configuration_sha256,
        recovery_request_sha256: None,
        server_name: server_name.clone(),
        mcp_document_sha256: format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(mcp).map_err(|_| unconfirmed())?)
        ),
    }, Arc::new(mcp.clone()), native_connector))
}

fn retain_receipt(
    capture: &mut ReceiptCapture,
    observed: (InstalledSelection, Arc<Value>, Option<NativeConnectorEntry>),
    matched_observation: bool,
) -> Result<(), ToolError> {
    let (mut installed, configuration, native_connector) = observed;
    if let Some(previous) = &capture.installed {
        let mut compare = installed.clone();
        // An initial library job omits the receipt digest. A later verified
        // owner read may add it; historical job reads cannot remove it.
        if compare.receipt_sha256.is_none() {
            compare.receipt_sha256 = previous.receipt_sha256.clone();
        }
        let mut expected = previous.clone();
        if expected.receipt_sha256.is_none() {
            expected.receipt_sha256 = compare.receipt_sha256.clone();
        }
        if compare != expected {
            return Err(unconfirmed());
        }
        installed = compare;
    }
    if matched_observation || capture.reconciliation_revision.is_none() {
        capture.matched = true;
    }
    capture.installed = Some(installed);
    capture.configuration = Some(configuration);
    // Only a current canonical reconciliation can add or clear this optional
    // observation. Reading the historical install job cannot erase it.
    if matched_observation { capture.native_connector = native_connector; }
    Ok(())
}

fn publisher_metadata(response: &Value) -> Result<Option<OperationEvidence>, ToolError> {
    let rows = response["operations"].as_array()
        .filter(|rows| !rows.is_empty() && rows.len() <= 128).ok_or_else(unconfirmed)?;
    let mut names = BTreeMap::new();
    let mut raw_names = std::collections::BTreeSet::new();
    for row in rows {
        let id = operation_name(&row["id"])?;
        // A publisher without actual raw-name metadata remains usable for
        // ordinary setup; this native join does not infer a name from id.
        let Some(raw) = row.get("tool") else { return Ok(None); };
        let raw = operation_name(raw)?;
        if names.insert(id.to_owned(), raw.to_owned()).is_some()
            || !raw_names.insert(raw.to_owned()) { return Err(unconfirmed()); }
    }
    let no_auth = match response.get("authentication") {
        None => false,
        Some(value) if value == &json!({"kind":"none"}) => {
            let fields = response["fields"].as_array().ok_or_else(unconfirmed)?;
            if fields.iter().any(|field| field.get("credential_reference") != Some(&Value::Bool(false))) {
                return Err(unconfirmed());
            }
            true
        },
        Some(_) => return Err(unconfirmed()),
    };
    let credential_fields = credential_fields(response)?;
    Ok(Some(OperationEvidence::Publisher { names, no_auth, credential_fields }))
}

fn selected_service_metadata(proposal: &Value) -> Result<Option<OperationEvidence>, ToolError> {
    let Some(declared) = proposal.get("declared_service") else { return Ok(None); };
    let has_slots = declared.get("environment_secret_slots").is_some();
    let keys: &[&str] = if has_slots { &["authentication", "operation_tools", "environment_secret_slots"] }
        else { &["authentication", "operation_tools"] };
    if !matches!(proposal["profile"].as_str(), Some("python" | "node")) || !exact_keys(declared, keys)
    { return Err(unconfirmed()); }
    let authentication: toolkit_package::AuthenticationRequirement =
        serde_json::from_value(declared["authentication"].clone()).map_err(|_| unconfirmed())?;
    let environment_secret_slots = if has_slots {
        if !matches!(authentication, toolkit_package::AuthenticationRequirement::Binding { .. }) { return Err(unconfirmed()); }
        let slots: toolkit_package::EnvironmentSecretSlots = serde_json::from_value(declared["environment_secret_slots"].clone())
            .map_err(|_| unconfirmed())?;
        if serde_json::to_value(&slots).map_err(|_| unconfirmed())? != declared["environment_secret_slots"] { return Err(unconfirmed()); }
        Some(slots)
    } else { None };
    let declared_names = declared["operation_tools"].as_object()
        .filter(|names| !names.is_empty() && names.len() <= 128).ok_or_else(unconfirmed)?;
    let mut names = BTreeMap::new();
    let mut raw_names = std::collections::BTreeSet::new();
    for (id, name) in declared_names {
        toolkit_package::Id::new(id).map_err(|_| unconfirmed())?;
        let name = operation_name(name)?.to_owned();
        if !raw_names.insert(name.clone()) { return Err(unconfirmed()); }
        names.insert(id.clone(), name);
    }
    let files = proposal["files"].as_object().ok_or_else(unconfirmed)?;
    let hash = |value: &Value, key| hex(value, key, 64).map(str::to_owned).ok_or_else(unconfirmed);
    let package_sha256 = hash(&proposal["files"], "package.json")?;
    let document = SelectedDocument::from_value(proposal)?;
    let other = if document.filename() == "activation.json" { "declaration.json" } else { "activation.json" };
    if files.contains_key(other) || hash(&proposal["files"], document.filename())? != document.sha256() {
        return Err(unconfirmed());
    }
    Ok(Some(OperationEvidence::Selected { names, authentication, inputs: SelectedServiceInputs {
        package_sha256, document, environment_secret_slots,
        effect_sha256: hash(proposal, "effect_sha256")?,
        provenance: json!({"plugin_source":proposal.get("plugin_source"), "acquired_source":proposal.get("acquired_source")}),
    }}))
}

#[cfg(test)]
pub(crate) fn selected_inputs_for_test(proposal: &Value) -> SelectedServiceInputs {
    match selected_service_metadata(proposal).expect("checked selected fixture").expect("selected metadata") {
        OperationEvidence::Selected { inputs, .. } => inputs,
        _ => panic!("selected fixture has selected evidence"),
    }
}

fn operation_name(value: &Value) -> Result<&str, ToolError> {
    value.as_str().filter(|v| !v.is_empty() && v.len() <= 128
        && v.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b)))
        .ok_or_else(unconfirmed)
}

fn document(raw: &str) -> Result<Value, ToolError> {
    if raw.len() > MAX_DOCUMENT {
        return Err(unconfirmed());
    }
    serde_json::from_str::<crate::forge::UniqueJson>(raw).map_err(|_| unconfirmed())?;
    let value: Value = serde_json::from_str(raw).map_err(|_| unconfirmed())?;
    if !value.is_object() {
        return Err(unconfirmed());
    }
    Ok(value)
}
fn result_document(
    call: ToolCallView<'_>,
    outcome: &ToolDispatchOutcome,
) -> Result<Value, ToolError> {
    if outcome.result.is_error {
        return Err(unconfirmed());
    }
    response_document(call, outcome)
}

fn response_document(
    call: ToolCallView<'_>,
    outcome: &ToolDispatchOutcome,
) -> Result<Value, ToolError> {
    if outcome.result.tool_use_id != call.id
        || !outcome.async_ops.is_empty()
        || !outcome.session_effects.is_empty()
    {
        return Err(unconfirmed());
    }
    match outcome.result.content.as_slice() {
        [ContentBlock::Text { text }] => document(text),
        // Native MCP preserves a server's structured JSON as one typed block.
        // Apply the same byte, duplicate-key and object checks to its raw JSON.
        [ContentBlock::Structured { data }] => document(data.get()),
        _ => Err(unconfirmed()),
    }
}
#[cfg(test)]
#[path = "service_install_document_tests.rs"]
mod document_tests;

fn hex<'a>(value: &'a Value, key: &str, length: usize) -> Option<&'a str> {
    value.get(key)?.as_str().filter(|text| {
        text.len() == length
            && text
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
fn exact_keys(value: &Value, keys: &[&str]) -> bool {
    value
        .as_object()
        .is_some_and(|map| map.len() == keys.len() && keys.iter().all(|key| map.contains_key(*key)))
}

impl Drop for CheckedInstalls {
    fn drop(&mut self) {
        // Arc handle clones do not run this destructor. Explicit retirement
        // closes earlier; last-owner drop also closes unpublished compositions.
        if let Some(route) = &self.checked_route {
            route.close();
        }
    }
}

#[async_trait]
impl AgentToolDispatcher for CheckedInstalls {
    fn tools(&self) -> Arc<[Arc<meerkat_core::ToolDef>]> {
        self.inner.tools()
    }
    fn tool_catalog_capabilities(&self) -> meerkat_core::ToolCatalogCapabilities {
        self.inner.tool_catalog_capabilities()
    }
    fn tool_catalog(&self) -> Arc<[meerkat_core::ToolCatalogEntry]> {
        self.inner.tool_catalog()
    }
    fn tool_mutation_class(&self, name: &str) -> meerkat_core::ToolMutationClass {
        self.inner.tool_mutation_class(name)
    }
    fn live_bridge_effect_kind(&self, name: &str) -> meerkat_core::LiveBridgeEffectKind {
        self.inner.live_bridge_effect_kind(name)
    }
    fn review_entry_support(&self, name: &str) -> meerkat_core::approval::review::ReviewEntrySupport {
        if self.checked_route.is_some() {
            self.inner.review_entry_support(name)
        } else {
            // Keep the existing installed composition's declaration unchanged.
            meerkat_core::approval::review::ReviewEntrySupport::Unsupported
        }
    }
    fn execution_binding_epoch(&self, name: &str) -> u64 {
        self.inner.execution_binding_epoch(name)
    }
    fn execution_binding_fingerprint(
        &self,
        name: &str,
    ) -> Result<
        meerkat_core::EphemeralToolBindingFingerprint,
        meerkat_core::ToolExecutionResolutionError,
    > {
        self.inner.execution_binding_fingerprint(name)
    }
    fn resolve_execution_plan(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        resolution: &meerkat_core::ToolExecutionResolutionContext,
    ) -> Result<meerkat_core::ResolvedToolExecutionPlan, meerkat_core::ToolExecutionResolutionError>
    {
        self.inner.resolve_execution_plan(call, context, resolution)
    }
    fn validate_resolved_execution_plan(
        &self,
        call: ToolCallView<'_>,
        resolution: &meerkat_core::ToolExecutionResolutionContext,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> Result<(), meerkat_core::ToolExecutionResolutionError> {
        self.inner
            .validate_resolved_execution_plan(call, resolution, plan)
    }
    fn pending_catalog_sources(&self) -> Arc<[String]> {
        self.inner.pending_catalog_sources()
    }
    async fn poll_external_updates(&self) -> meerkat_core::ExternalToolUpdate {
        self.inner.poll_external_updates().await
    }
    fn external_tool_surface_snapshot(&self) -> Option<meerkat_core::ExternalToolSurfaceSnapshot> {
        self.inner.external_tool_surface_snapshot()
    }
    fn capabilities(&self) -> meerkat_core::DispatcherCapabilities {
        self.inner.capabilities()
    }
    fn bind_mcp_server_lifecycle_handle(
        &self,
        handle: Arc<dyn meerkat_core::handles::McpServerLifecycleHandle>,
    ) {
        self.inner.bind_mcp_server_lifecycle_handle(handle);
    }
    fn bind_external_tool_surface_handle(
        &self,
        handle: Arc<dyn meerkat_core::handles::ExternalToolSurfaceHandle>,
    ) {
        self.inner.bind_external_tool_surface_handle(handle);
    }
    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        if Kind::check(call.name).is_some() || INSTALL_TOOLS.contains(&call.name) || call.name == "setup_export_recipe" {
            return Err(refused(call.name));
        }
        self.inner.dispatch(call).await
    }
    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        self.dispatch_checked(call, context, None).await
    }
    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        self.dispatch_checked(call, context, Some(plan)).await
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod checked_route_tests {
    use super::*;

    fn installs() -> Arc<CheckedInstalls> {
        CheckedInstalls::with_checked_route(
            Arc::new(meerkat::McpRouterAdapter::new(meerkat::McpRouter::new())),
            SessionId::new(),
        ).unwrap()
    }

    #[test]
    fn checked_route_closes_on_retire_or_last_owner_drop_not_handle_drop() {
        let owner = installs();
        let route = owner.checked_route.as_ref().unwrap().clone();
        let temporary = owner.clone();
        drop(temporary);
        assert!(route.is_open());
        let retained = owner.clone();
        owner.retire();
        assert!(!route.is_open());
        drop(owner);
        assert!(!route.is_open());
        drop(retained);

        let unpublished = installs();
        let route = unpublished.checked_route.as_ref().unwrap().clone();
        let retained = unpublished.clone();
        drop(unpublished);
        assert!(route.is_open());
        drop(retained);
        assert!(!route.is_open());

        let poisoned = installs();
        let route = poisoned.checked_route.as_ref().unwrap().clone();
        poisoned.poison_fixture_state();
        poisoned.retire();
        assert!(!route.is_open(), "poisoned proposal custody cannot keep a route open");
    }
}
