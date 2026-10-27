//! Runtime subsystem types - routing, delivery, gating, memory, and session persistence.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::io::{BufRead, BufReader};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::auth::{
    JwtClaimsValidationConfig, build_jwt_verification_key, inspect_jwt_header, parse_jwks_json,
    parse_oidc_discovery_json, select_jwk_for_token, validate_jwt_with_verification_key,
};
use crate::baseline::{
    BaselineVerificationError, BaselineVerificationReport, verify_meerkat_baseline_symbols,
};
use crate::decisions::{
    AuthPolicy, AuthProvider, BigQueryNaming, ConsoleAccessRequest, ConsolePolicy,
    DecisionPolicyError, REQUIRED_RELEASE_TARGETS, ReleaseMetadata, RuntimeOpsPolicy,
    enforce_console_route_access, load_trusted_meta_modules_from_toml, parse_release_metadata_json,
    validate_bigquery_naming, validate_release_metadata, validate_runtime_ops_policy,
};
use crate::process::{ProcessBoundaryError, run_process_json_line};
use crate::protocol::parse_unified_event_line;
use crate::rpc::{RpcCapabilities, RpcCapabilitiesError, parse_rpc_capabilities};
use crate::types::{
    EventEnvelope, MetaConfig, ModuleConfig, ModuleEvent, PreSpawnData, RestartPolicy, UnifiedEvent,
};

mod bootstrap;
mod console_ingress;
pub mod cross_mob_control;
pub mod cross_mob_remote;
mod delivery;
mod event_transport;
mod gating;
mod memory;
pub mod metadata;
mod module_boundary;
mod module_process;
pub use module_process::{
    ModuleCleanupFailure, ModuleCleanupObservation, ModuleCleanupOperation, ModuleCleanupPhase,
    ModuleCleanupProcessObservation, ModuleCleanupReapedProcess,
};
pub mod keyed_inject;
#[cfg(all(test, unix))]
mod module_process_tests;
pub mod remote_host;
mod routing;
mod rpc;
mod session_store;
mod supervisor;

pub use bootstrap::{start_meta_runtime, start_meta_runtime_with_options};
/// Applies no session fence; use the routers a [`crate::UnifiedRuntime`] builds,
/// or the `_under_session_fences` variant
/// ([`validate_console_token_under_session_fences`]).
pub use console_ingress::handle_console_rest_json_route_with_snapshot_access_and_memory;
#[cfg(test)]
pub(crate) use console_ingress::resolve_authorized_console_caller_from_token_at;
pub use console_ingress::{
    AuthenticatedConsoleHuman, CONSOLE_HUMAN_TRUST_DOMAIN_PREFIX, ConsoleAgentLiveSnapshot,
    ConsoleHumanNotCurrent, ConsoleLiveSnapshot, ConsoleMember, ConsoleModelCapabilities,
    ConsoleRestJsonRequest, ConsoleRestJsonResponse, CurrentConsoleHumanEvidence,
    extract_bearer_token_from_header, handle_console_rest_json_route,
    handle_console_rest_json_route_with_snapshot,
    handle_console_rest_json_route_with_snapshot_access_memory_and_workgraph,
    handle_console_rest_json_route_with_snapshot_and_access, validate_console_token,
    validate_console_token_under_session_fences,
};
pub(crate) use console_ingress::{
    ConsoleCallerVerification, ResolvedConsoleCaller, console_member_access_attributes,
    fold_console_member_attributes, handle_console_rest_json_route_with_pending_rows,
    resolve_authorized_console_caller_from_token, session_fence_first_accepted_iat,
};
pub use event_transport::normalize_event_line;
pub(crate) use gating::is_legacy_sequential_pending_id;
pub(crate) use memory::staged_ledger_path;
pub use metadata::{
    InMemoryMetadataStore, LabelRpcResult, MemberIdleRetireOverrideRecord, MetadataScope,
    MetadataStoreError, PersistentMetadataStore, RuntimeMetadataTable, SqliteMetadataStore,
    dispatch_label_method, dispatch_labels_delete, dispatch_labels_get, dispatch_labels_set,
    labels_to_json_value, parse_labels_param, parse_run_id_param,
};
pub(crate) use metadata::{METADATA_TABLE, create_metadata_table};
pub use routing::WILDCARD_ROUTE;
pub use routing::route_module_call;
pub use rpc::{
    route_module_call_rpc_json, route_module_call_rpc_subprocess,
    run_rpc_capabilities_boundary_once,
};
pub use session_store::{
    BigQueryAccessToken, BigQueryAccessTokenSource, BigQueryGcConfig, BigQuerySessionStoreAdapter,
    BigQuerySessionStoreError, GcErrorCallback, JsonFileSessionStore, JsonFileSessionStoreError,
    JsonStoreLockRecord, SessionPersistenceRow, SessionStoreContract, SessionStoreKind,
    materialize_latest_session_rows, materialize_live_session_rows, run_periodic_gc,
    run_periodic_gc_with_error_callback, session_store_contracts,
};
pub use supervisor::{run_discovered_module_once, run_module_boundary_once};

use event_transport::{insert_event_sorted, merge_unified_events};
use supervisor::supervise_module_start;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NormalizationError {
    InvalidJson,
    InvalidSchema,
    MissingField(&'static str),
    InvalidFieldType(&'static str),
    SourceMismatch { expected: &'static str, got: String },
}

impl std::fmt::Display for NormalizationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidJson => write!(f, "invalid JSON"),
            Self::InvalidSchema => write!(f, "invalid schema"),
            Self::MissingField(field) => write!(f, "missing field: {field}"),
            Self::InvalidFieldType(field) => write!(f, "invalid field type: {field}"),
            Self::SourceMismatch { expected, got } => {
                write!(f, "source mismatch: expected {expected}, got {got}")
            }
        }
    }
}

impl std::error::Error for NormalizationError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeBoundaryError {
    Process(ProcessBoundaryError),
    Normalize(NormalizationError),
    Mcp(McpBoundaryError),
}

impl std::fmt::Display for RuntimeBoundaryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Process(err) => write!(f, "process boundary: {err}"),
            Self::Normalize(err) => write!(f, "normalization: {err}"),
            Self::Mcp(err) => write!(f, "MCP boundary: {err}"),
        }
    }
}

impl std::error::Error for RuntimeBoundaryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Process(err) => Some(err),
            Self::Normalize(err) => Some(err),
            Self::Mcp(err) => Some(err),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpBoundaryError {
    RuntimeUnavailable(String),
    McpRequired {
        module_id: String,
        flow: String,
    },
    Timeout {
        module_id: String,
        operation: String,
        timeout_ms: u64,
    },
    ConnectionFailed {
        module_id: String,
        reason: String,
    },
    ToolListFailed {
        module_id: String,
        reason: String,
    },
    ToolNotFound {
        module_id: String,
        tool: String,
        available_tools: Vec<String>,
    },
    ToolCallFailed {
        module_id: String,
        tool: String,
        reason: String,
    },
    CloseFailed {
        module_id: String,
        reason: String,
    },
    /// The module server's close returned, but did not settle by Meerkat's
    /// final facts (for example, a process outside its group kept its stdout
    /// open).
    CloseUnsettled {
        module_id: String,
        class: crate::unified_runtime::McpCloseUnsettledClass,
    },
    OperationFailedWithCloseFailure {
        primary: Box<McpBoundaryError>,
        close: Box<McpBoundaryError>,
    },
    InvalidToolPayload {
        module_id: String,
        tool: String,
        reason: String,
    },
    InvalidJsonResponse {
        module_id: String,
        tool: String,
        response: String,
    },
}

impl std::fmt::Display for McpBoundaryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RuntimeUnavailable(msg) => write!(f, "runtime unavailable: {msg}"),
            Self::McpRequired { module_id, flow } => {
                write!(f, "MCP required for module {module_id} flow {flow}")
            }
            Self::Timeout {
                module_id,
                operation,
                timeout_ms,
            } => {
                write!(
                    f,
                    "timeout for module {module_id} operation {operation} after {timeout_ms}ms"
                )
            }
            Self::ConnectionFailed { module_id, reason } => {
                write!(f, "connection failed for module {module_id}: {reason}")
            }
            Self::ToolListFailed { module_id, reason } => {
                write!(f, "tool list failed for module {module_id}: {reason}")
            }
            Self::ToolNotFound {
                module_id,
                tool,
                available_tools,
            } => {
                write!(
                    f,
                    "tool {tool} not found for module {module_id} (available: {})",
                    available_tools.join(", ")
                )
            }
            Self::ToolCallFailed {
                module_id,
                tool,
                reason,
            } => {
                write!(
                    f,
                    "tool call {tool} failed for module {module_id}: {reason}"
                )
            }
            Self::CloseFailed { module_id, reason } => {
                write!(f, "close failed for module {module_id}: {reason}")
            }
            Self::CloseUnsettled { module_id, class } => {
                write!(f, "close for module {module_id} did not settle: {class}")
            }
            Self::OperationFailedWithCloseFailure { primary, close } => {
                write!(f, "operation failed: {primary}; close also failed: {close}")
            }
            Self::InvalidToolPayload {
                module_id,
                tool,
                reason,
            } => {
                write!(
                    f,
                    "invalid tool payload for module {module_id} tool {tool}: {reason}"
                )
            }
            Self::InvalidJsonResponse {
                module_id,
                tool,
                response,
            } => {
                write!(
                    f,
                    "invalid JSON response for module {module_id} tool {tool}: {response}"
                )
            }
        }
    }
}

impl std::error::Error for McpBoundaryError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigResolutionError {
    ModuleNotConfigured(String),
    ModuleNotDiscovered(String),
}

impl std::fmt::Display for ConfigResolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ModuleNotConfigured(id) => write!(f, "module not configured: {id}"),
            Self::ModuleNotDiscovered(id) => write!(f, "module not discovered: {id}"),
        }
    }
}

impl std::error::Error for ConfigResolutionError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeFromConfigError {
    Config(ConfigResolutionError),
    Runtime(RuntimeBoundaryError),
}

impl std::fmt::Display for RuntimeFromConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Config(err) => write!(f, "config resolution: {err}"),
            Self::Runtime(err) => write!(f, "runtime boundary: {err}"),
        }
    }
}

impl std::error::Error for RuntimeFromConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Config(err) => Some(err),
            Self::Runtime(err) => Some(err),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RpcRuntimeError {
    Process(ProcessBoundaryError),
    Capabilities(RpcCapabilitiesError),
}

impl std::fmt::Display for RpcRuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Process(err) => write!(f, "process boundary: {err}"),
            Self::Capabilities(err) => write!(f, "capabilities: {err}"),
        }
    }
}

impl std::error::Error for RpcRuntimeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Process(err) => Some(err),
            Self::Capabilities(err) => Some(err),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaselineRuntimeError {
    Process(ProcessBoundaryError),
    InvalidRepoPathJson,
    MissingRepoRoot,
    InvalidRepoRoot,
    Baseline(BaselineVerificationError),
}

impl std::fmt::Display for BaselineRuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Process(err) => write!(f, "process boundary: {err}"),
            Self::InvalidRepoPathJson => write!(f, "invalid repo path JSON"),
            Self::MissingRepoRoot => write!(f, "missing repo root"),
            Self::InvalidRepoRoot => write!(f, "invalid repo root"),
            Self::Baseline(err) => write!(f, "baseline verification: {err}"),
        }
    }
}

impl std::error::Error for BaselineRuntimeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Process(err) => Some(err),
            Self::Baseline(err) => Some(err),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetaRuntimeError {
    Config(ConfigResolutionError),
    MemoryBackend(LocalJsonMemoryStoreError),
}

impl std::fmt::Display for MetaRuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Config(err) => write!(f, "config resolution: {err}"),
            Self::MemoryBackend(err) => write!(f, "memory backend: {err}"),
        }
    }
}

impl std::error::Error for MetaRuntimeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Config(err) => Some(err),
            Self::MemoryBackend(err) => Some(err),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecisionRuntimeError {
    Policy(DecisionPolicyError),
}

impl std::fmt::Display for DecisionRuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Policy(err) => write!(f, "decision policy: {err}"),
        }
    }
}

impl std::error::Error for DecisionRuntimeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Policy(err) => Some(err),
        }
    }
}

/// Raw policy documents a host hands to [`build_runtime_decision_state`].
///
/// `trusted_meta_toml` is the trust manifest (an empty string declares no
/// modules) and `release_metadata_json` is the release metadata document;
/// both are parsed and validated by the builder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeDecisionInputs {
    pub bigquery: BigQueryNaming,
    pub trusted_meta_toml: String,
    pub auth: AuthPolicy,
    pub trusted_oidc: TrustedOidcRuntimeConfig,
    pub console: ConsolePolicy,
    pub ops: RuntimeOpsPolicy,
    pub release_metadata_json: String,
}

/// The policies the runtime consults while serving.
///
/// At serve time the runtime reads `bigquery` (session-store descriptor),
/// `modules`, `console`, and, only when a token has to be verified,
/// `trusted_oidc`. Every field is public: [`build_runtime_decision_state`]
/// is the validating constructor and [`RuntimeDecisionState::local_console`]
/// is the explicit opt-out for a console that trusts no identity provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeDecisionState {
    pub bigquery: BigQueryNaming,
    pub modules: Vec<ModuleConfig>,
    pub auth: AuthPolicy,
    pub trusted_oidc: TrustedOidcRuntimeConfig,
    pub console: ConsolePolicy,
    pub ops: RuntimeOpsPolicy,
    pub release_metadata: ReleaseMetadata,
}

/// OIDC discovery document carried by [`RuntimeDecisionState::local_console`].
/// The issuer resolves nowhere; the document exists so the snapshot parses.
const LOCAL_CONSOLE_DISCOVERY_JSON: &str = r#"{"issuer":"https://noop.example.com","authorization_endpoint":"https://noop.example.com/auth","token_endpoint":"https://noop.example.com/token","jwks_uri":"https://noop.example.com/.well-known/jwks.json","response_types_supported":["code"],"subject_types_supported":["public"],"id_token_signing_alg_values_supported":["RS256"]}"#;
/// A JWKS with no keys: no token can verify against it.
const LOCAL_CONSOLE_JWKS_JSON: &str = r#"{"keys":[]}"#;
const LOCAL_CONSOLE_AUDIENCE: &str = "local-console";
const LOCAL_CONSOLE_BIGQUERY_DATASET: &str = "default_dataset";
const LOCAL_CONSOLE_BIGQUERY_TABLE: &str = "default_table";
const CANONICAL_SUPPORT_MATRIX: &str = "same-as-meerkat";

impl RuntimeDecisionState {
    /// Decision state for a console that trusts no identity provider.
    ///
    /// This is the explicit opt-out from console authentication for local or
    /// host-protected deployments, and the single owner of the placeholder
    /// snapshot both gateway binaries used to hand-build: a discovery document
    /// whose issuer resolves nowhere, a JWKS with no keys, no trusted modules,
    /// the default auth and ops policies, and the canonical release metadata.
    ///
    /// At serve time the runtime reads `console`, the session-store naming
    /// (`bigquery`) and `modules` (the trusted module set `console_ingress`
    /// consults). The first two are the parameters; `bigquery` defaults to
    /// `default_dataset` / `default_table` (the SDK gateway's naming) when
    /// `None`. `modules` starts empty, which is what both bundled gateways
    /// serve; a Rust host with trusted modules adds them through
    /// [`with_modules`](Self::with_modules) instead of falling back to a
    /// struct literal.
    ///
    /// The default stays fail-closed: `ConsolePolicy::default()` has
    /// `require_app_auth: true`, and because this state carries no keys every
    /// token is refused, so a console built from it with the default policy
    /// answers every request with 401. Pass a policy with
    /// `require_app_auth: false` to open the console, and do so only where the
    /// host itself provides the protection (a loopback bind, a reverse proxy
    /// that authenticates).
    ///
    /// The result deliberately bypasses [`build_runtime_decision_state`]:
    /// there is nothing to validate about a snapshot that trusts nothing.
    pub fn local_console(console: ConsolePolicy, bigquery: Option<BigQueryNaming>) -> Self {
        Self {
            bigquery: bigquery.unwrap_or_else(|| BigQueryNaming {
                dataset: LOCAL_CONSOLE_BIGQUERY_DATASET.to_string(),
                table: LOCAL_CONSOLE_BIGQUERY_TABLE.to_string(),
            }),
            modules: Vec::new(),
            auth: AuthPolicy::default(),
            trusted_oidc: TrustedOidcRuntimeConfig {
                discovery_json: LOCAL_CONSOLE_DISCOVERY_JSON.to_string(),
                jwks_json: LOCAL_CONSOLE_JWKS_JSON.to_string(),
                audience: LOCAL_CONSOLE_AUDIENCE.to_string(),
                require_verified_email: false,
            },
            console,
            ops: RuntimeOpsPolicy::default(),
            release_metadata: ReleaseMetadata {
                targets: REQUIRED_RELEASE_TARGETS
                    .iter()
                    .map(|target| (*target).to_string())
                    .collect(),
                support_matrix: CANONICAL_SUPPORT_MATRIX.to_string(),
            },
        }
    }

    /// Replace the trusted module set a state built by
    /// [`local_console`](Self::local_console) serves. The validating
    /// constructor derives this set from the trust manifest; a host that
    /// opted out of console auth but still runs trusted modules declares them
    /// here so the exported constructor covers that host too.
    pub fn with_modules(mut self, modules: Vec<ModuleConfig>) -> Self {
        self.modules = modules;
        self
    }
}

/// The OIDC snapshot a console verifies bearer tokens against: the provider's
/// discovery document, its JWKS, and the audience tokens must carry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedOidcRuntimeConfig {
    pub discovery_json: String,
    pub jwks_json: String,
    pub audience: String,
    /// Admit only tokens whose `email_verified` claim is `true` and never
    /// fall back to `sub` as the allowlist principal. Set by
    /// `provider: "oidc"` configurations: a public issuer such as Google
    /// mints tokens for unverified addresses too, and the allowlist compares
    /// addresses. Off (the historical behaviour) for the HS256 development
    /// issuer and hand-built states.
    #[serde(default)]
    pub require_verified_email: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalJsonMemoryStoreError {
    InvalidConfig(String),
    Io(String),
    Serialize(String),
    InvalidStoreData(String),
    ExternalCallFailed(String),
}

impl std::fmt::Display for LocalJsonMemoryStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfig(msg) => write!(f, "invalid config: {msg}"),
            Self::Io(msg) => write!(f, "I/O error: {msg}"),
            Self::Serialize(msg) => write!(f, "serialization error: {msg}"),
            Self::InvalidStoreData(msg) => write!(f, "invalid store data: {msg}"),
            Self::ExternalCallFailed(msg) => write!(f, "external call failed: {msg}"),
        }
    }
}

impl std::error::Error for LocalJsonMemoryStoreError {}

/// Operational-ledger backend that persists assertions/conflicts as local
/// JSON, with an optional HTTP health gate applied before every read/write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalJsonMemoryBackendConfig {
    pub state_path: String,
    #[serde(default)]
    pub health_check_endpoint: Option<String>,
}

/// The operational-ledger backend, tagged by `kind`. `local_json` is the
/// only backend: any other `kind`, including the predecessor's retired
/// value, is refused at decode and never reinterpreted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MemoryBackendConfig {
    LocalJson(LocalJsonMemoryBackendConfig),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LocalJsonMemoryStoreAdapter {
    health_check_endpoint: Option<String>,
    state_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeOptions {
    pub on_failure_retry_budget: u32,
    pub always_restart_budget: u32,
    #[serde(default)]
    pub supervisor_restart_backoff_ms: u64,
    #[serde(default)]
    pub supervisor_test_force_terminate_failure: bool,
    #[serde(default)]
    pub memory_backend: Option<MemoryBackendConfig>,
    #[serde(default = "default_implicit_delegate_idle_retire_secs")]
    pub implicit_delegate_idle_retire_secs: Option<u64>,
    #[serde(default = "default_implicit_delegate_idle_sweep_interval_ms")]
    pub implicit_delegate_idle_sweep_interval_ms: u64,
}

fn default_implicit_delegate_idle_retire_secs() -> Option<u64> {
    Some(300)
}

fn default_implicit_delegate_idle_sweep_interval_ms() -> u64 {
    10_000
}

impl Default for RuntimeOptions {
    fn default() -> Self {
        Self {
            on_failure_retry_budget: 1,
            always_restart_budget: 1,
            supervisor_restart_backoff_ms: 0,
            supervisor_test_force_terminate_failure: false,
            memory_backend: None,
            implicit_delegate_idle_retire_secs: default_implicit_delegate_idle_retire_secs(),
            implicit_delegate_idle_sweep_interval_ms:
                default_implicit_delegate_idle_sweep_interval_ms(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LifecycleStage {
    MobStarted,
    ModulesStarted,
    MergedStreamStarted,
    ShutdownRequested,
    ShutdownComplete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleEvent {
    pub seq: u64,
    pub stage: LifecycleStage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModuleHealthState {
    Starting,
    Healthy,
    Failed,
    Restarting,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleHealthTransition {
    pub module_id: String,
    pub from: Option<ModuleHealthState>,
    pub to: ModuleHealthState,
    pub attempt: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupervisorReport {
    pub transitions: Vec<ModuleHealthTransition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeShutdownReport {
    pub terminated_modules: Vec<String>,
    /// Module processes whose termination was not confirmed by this shutdown.
    /// They stay owned by the runtime handle (see `physical_cleanup`).
    pub orphan_processes: u32,
    /// The owners' own record of this shutdown: confirmed reaps, and every
    /// process still retained because its termination was not confirmed.
    #[serde(default = "ModuleCleanupObservation::settled")]
    pub physical_cleanup: ModuleCleanupObservation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingResolveRequest {
    pub recipient: String,
    #[serde(default)]
    pub channel: Option<String>,
    #[serde(default)]
    pub retry_max: Option<u32>,
    #[serde(default)]
    pub backoff_ms: Option<u64>,
    #[serde(default)]
    pub rate_limit_per_minute: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeRoute {
    pub route_key: String,
    pub recipient: String,
    #[serde(default)]
    pub channel: Option<String>,
    pub sink: String,
    pub target_module: String,
    #[serde(default)]
    pub retry_max: Option<u32>,
    #[serde(default)]
    pub backoff_ms: Option<u64>,
    #[serde(default)]
    pub rate_limit_per_minute: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingResolution {
    pub route_id: String,
    pub recipient: String,
    pub channel: String,
    pub sink: String,
    pub target_module: String,
    pub retry_max: u32,
    pub backoff_ms: u64,
    pub rate_limit_per_minute: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliverySendRequest {
    pub resolution: RoutingResolution,
    pub payload: Value,
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryAttempt {
    pub attempt: u32,
    pub status: String,
    pub backoff_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryRecord {
    pub delivery_id: String,
    pub route_id: String,
    pub recipient: String,
    pub sink: String,
    pub target_module: String,
    pub payload: Value,
    pub status: String,
    pub attempts: Vec<DeliveryAttempt>,
    pub first_attempt_ms: u64,
    pub final_attempt_ms: u64,
    pub idempotency_key: Option<String>,
    pub sink_adapter: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryHistoryRequest {
    #[serde(default)]
    pub recipient: Option<String>,
    #[serde(default)]
    pub sink: Option<String>,
    #[serde(default = "default_delivery_history_limit")]
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryHistoryResponse {
    pub deliveries: Vec<DeliveryRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryStoreInfo {
    pub store: String,
    pub record_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryIndexRequest {
    pub entity: String,
    pub topic: String,
    #[serde(default)]
    pub store: Option<String>,
    #[serde(default)]
    pub fact: Option<String>,
    #[serde(default)]
    pub metadata: Option<Value>,
    #[serde(default)]
    pub conflict: Option<bool>,
    #[serde(default)]
    pub conflict_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryAssertion {
    pub assertion_id: String,
    pub entity: String,
    pub topic: String,
    pub store: String,
    pub fact: String,
    #[serde(default)]
    pub metadata: Option<Value>,
    pub indexed_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryConflictSignal {
    pub entity: String,
    pub topic: String,
    pub store: String,
    #[serde(default)]
    pub reason: Option<String>,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryIndexResult {
    pub entity: String,
    pub topic: String,
    pub store: String,
    #[serde(default)]
    pub assertion_id: Option<String>,
    pub conflict_active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryQueryRequest {
    #[serde(default)]
    pub entity: Option<String>,
    #[serde(default)]
    pub topic: Option<String>,
    #[serde(default)]
    pub store: Option<String>,
    /// Case-insensitive substring filter applied across entity, topic, and
    /// fact (reason for conflict signals), after the exact filters above.
    #[serde(default)]
    pub query: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryQueryResult {
    pub assertions: Vec<MemoryAssertion>,
    pub conflicts: Vec<MemoryConflictSignal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
struct PersistedMemoryState {
    #[serde(default)]
    assertions: Vec<MemoryAssertion>,
    #[serde(default)]
    conflicts: Vec<MemoryConflictSignal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryIndexError {
    EntityRequired,
    TopicRequired,
    UnsupportedStore(String),
    FactRequiredWhenConflictUnset,
    BackendPersistFailed(LocalJsonMemoryStoreError),
}

impl std::fmt::Display for MemoryIndexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EntityRequired => write!(f, "entity is required"),
            Self::TopicRequired => write!(f, "topic is required"),
            Self::UnsupportedStore(store) => write!(f, "unsupported store: {store}"),
            Self::FactRequiredWhenConflictUnset => {
                write!(f, "fact is required when conflict is unset")
            }
            Self::BackendPersistFailed(err) => write!(f, "backend persist failed: {err}"),
        }
    }
}

impl std::error::Error for MemoryIndexError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::BackendPersistFailed(err) => Some(err),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GatingRiskTier {
    R0,
    R1,
    R2,
    R3,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatingEvaluateRequest {
    pub action: String,
    pub actor_id: String,
    pub risk_tier: GatingRiskTier,
    #[serde(default)]
    pub rationale: Option<String>,
    #[serde(default)]
    pub requested_approver: Option<String>,
    #[serde(default)]
    pub approval_recipient: Option<String>,
    #[serde(default)]
    pub approval_channel: Option<String>,
    #[serde(default)]
    pub approval_timeout_ms: Option<u64>,
    #[serde(default)]
    pub entity: Option<String>,
    #[serde(default)]
    pub topic: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GatingOutcome {
    Allowed,
    AllowedWithAudit,
    PendingApproval,
    SafeDraft,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatingEvaluateResult {
    pub action_id: String,
    pub action: String,
    pub actor_id: String,
    pub risk_tier: GatingRiskTier,
    pub outcome: GatingOutcome,
    #[serde(default)]
    pub pending_id: Option<String>,
    #[serde(default)]
    pub fallback_reason: Option<String>,
}

/// Origin supplied by the host at the action boundary, never inferred from
/// actor_id or accepted from ordinary gating RPC parameters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatingOrigin {
    pub identity: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatingPendingEntry {
    pub pending_id: String,
    pub action_id: String,
    pub action: String,
    pub actor_id: String,
    pub risk_tier: GatingRiskTier,
    #[serde(default)]
    pub requested_approver: Option<String>,
    #[serde(default)]
    pub approval_recipient: Option<String>,
    #[serde(default)]
    pub approval_channel: Option<String>,
    #[serde(default)]
    pub approval_route_id: Option<String>,
    #[serde(default)]
    pub approval_delivery_id: Option<String>,
    pub created_at_ms: u64,
    pub deadline_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<GatingOrigin>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GatingDecision {
    Approve,
    Reject,
    Escalate,
}

/// Audit entries after a cursor, oldest first. `gap` means the cursor is no
/// longer retained by this owner (evicted, minted by an earlier owner
/// incarnation, or unknown): `entries` then restart from the oldest retained
/// entry and the reader must resynchronize. The audit view is bounded and
/// resumable with explicit gap detection; it is not a complete history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatingAuditPage {
    pub entries: Vec<GatingAuditEntry>,
    pub gap: bool,
}

/// Where an escalated gate goes next. `routes` maps the escalated entry's
/// requested approver to its successor approver id; `default` covers entries
/// without a requested approver or without a specific route. With neither,
/// escalation is refused and the entry stays pending: clearing the requested
/// approver is not proof that every `gating.decide` holder is qualified.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatingEscalationRoutes {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub routes: BTreeMap<String, String>,
}

impl GatingEscalationRoutes {
    /// Refuse empty or padded approver ids in the default or any route.
    pub fn validate(&self) -> Result<(), String> {
        let valid = |id: &str| !id.is_empty() && id.trim() == id;
        if self.default.as_deref().is_some_and(|id| !valid(id))
            || self
                .routes
                .iter()
                .any(|(from, to)| !valid(from) || !valid(to))
        {
            return Err("escalation approver ids must be non-empty and unpadded".to_string());
        }
        Ok(())
    }

    /// The successor approver for an entry with this requested approver.
    pub fn successor_for(&self, requested_approver: Option<&str>) -> Option<&str> {
        requested_approver
            .and_then(|approver| self.routes.get(approver))
            .or(self.default.as_ref())
            .map(String::as_str)
    }
}

/// An authenticated principal deciding a gate, resolved by the caller from
/// its authentication and the live access bindings before the owner sees it.
/// Request fields are never identity evidence for this path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GatingDecider {
    /// The authenticated individual.
    pub principal: String,
    /// Approver identities the principal currently holds.
    pub held_approver_ids: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatingDecideRequest {
    pub pending_id: String,
    pub approver_id: String,
    pub decision: GatingDecision,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatingDecisionResult {
    pub pending_id: String,
    pub action_id: String,
    pub approver_id: String,
    pub decision: GatingDecision,
    pub outcome: GatingOutcome,
    pub decided_at_ms: u64,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub next_pending_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatingAuditEntry {
    pub audit_id: String,
    pub timestamp_ms: u64,
    pub event_type: String,
    pub action_id: String,
    #[serde(default)]
    pub pending_id: Option<String>,
    pub actor_id: String,
    pub risk_tier: GatingRiskTier,
    pub outcome: GatingOutcome,
    pub detail: Value,
}

/// Versioned owner state for a trusted host's scoped persistence adapter.
/// Exporting this value does not enable persistence by itself. Hosts must save
/// atomically after mutations and restore before exposing their runtime.
///
/// Version 1 snapshots hold sequential legacy IDs (`gate-<kind>-<sequence>`).
/// Version 2 snapshots also hold epoch IDs (`gate-<kind>-v2-<epoch>-<sequence>`).
/// A restoring owner keeps every restored ID as it is and mints new IDs under
/// its own fresh epoch, so they do not alias restored IDs (barring a random
/// epoch collision) and never alias legacy IDs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatingStateSnapshot {
    pub version: u32,
    /// The exporting owner's epoch (version 2). Absent in version 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_epoch: Option<String>,
    /// The exporting owner's next sequence, including IDs no longer retained
    /// in the log. Version 1: the legacy sequence. Version 2: the sequence
    /// under `owner_epoch`.
    pub next_sequence: u64,
    /// In the owner's insertion order, including unattributed legacy records.
    pub pending: Vec<GatingPendingEntry>,
    pub audit: Vec<GatingAuditEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatingStateRestoreError {
    RuntimeNotPristine,
    UnsupportedVersion(u32),
    InvalidSnapshot(&'static str),
}

impl std::fmt::Display for GatingStateRestoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RuntimeNotPristine => write!(f, "gating restore requires a pristine owner"),
            Self::UnsupportedVersion(version) => {
                write!(f, "unsupported gating snapshot version: {version}")
            }
            Self::InvalidSnapshot(reason) => write!(f, "invalid gating snapshot: {reason}"),
        }
    }
}

impl std::error::Error for GatingStateRestoreError {}

/// One resolved gating pending entry (decision or timeout), pushed
/// synchronously to registered observers. The gating subsystem itself is
/// poll-only; this seam exists so components that staged work behind a
/// pending entry (the memory steward's quarantine-promotions, §10.2) can
/// react without polling. Observers must not block — defer real work onto
/// the runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatingResolutionNotice {
    pub pending_id: String,
    pub action_id: String,
    pub approved: bool,
    /// Set when an escalation minted a successor pending entry.
    pub next_pending_id: Option<String>,
    /// `approval_decided`, `rejection_decided`, `escalation_decided`, or
    /// `timeout_fallback`.
    pub cause: String,
}

pub trait GatingResolutionObserver: Send + Sync {
    fn on_gating_resolution(&self, notice: &GatingResolutionNotice);
}

/// Registered observers; newtype so `MetaRuntimeHandle` keeps `Debug`.
#[derive(Default)]
pub(crate) struct GatingResolutionObservers(Vec<Arc<dyn GatingResolutionObserver>>);

impl std::fmt::Debug for GatingResolutionObservers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "GatingResolutionObservers({})", self.0.len())
    }
}

impl GatingResolutionObservers {
    pub(crate) fn push(&mut self, observer: Arc<dyn GatingResolutionObserver>) {
        self.0.push(observer);
    }

    pub(crate) fn observers(&self) -> Vec<Arc<dyn GatingResolutionObserver>> {
        self.0.clone()
    }
}

/// Resolution notices produced by one owner operation, with the observers
/// that receive them. Observers may edit access or take other owners, so a
/// caller holding an access decision fence or the owner must deliver after
/// releasing them.
#[must_use = "deliver the notices after releasing guards"]
pub struct GatingNoticeBatch {
    notices: Vec<GatingResolutionNotice>,
    observers: Vec<Arc<dyn GatingResolutionObserver>>,
}

impl GatingNoticeBatch {
    pub fn deliver(self) {
        for notice in &self.notices {
            for observer in &self.observers {
                observer.on_gating_resolution(notice);
            }
        }
    }
}

/// Why the gating owner cannot mint another gate identity. Minting fails
/// closed instead of reusing, wrapping or saturating an identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatingIdUnavailable {
    /// No owner epoch: the platform's entropy source failed at boot.
    EpochUnavailable,
    /// The owner's sequence cannot cover the operation.
    SequenceExhausted,
}

impl GatingIdUnavailable {
    /// The `fallback_reason` an evaluation refused for this reason reports.
    pub fn fallback_reason(self) -> &'static str {
        match self {
            Self::EpochUnavailable => "gating_identity_unavailable",
            Self::SequenceExhausted => "gating_sequence_exhausted",
        }
    }
}

impl std::fmt::Display for GatingIdUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EpochUnavailable => write!(f, "gating owner has no identity epoch"),
            Self::SequenceExhausted => write!(f, "gating owner identity sequence is exhausted"),
        }
    }
}

impl std::error::Error for GatingIdUnavailable {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatingDecideError {
    UnknownPendingId(String),
    SelfApprovalForbidden,
    ApproverMismatch {
        expected: String,
        provided: String,
    },
    /// An authenticated decision named no principal.
    EmptyPrincipal,
    /// Escalation has no configured successor for this entry; it stays pending.
    NoEscalationRoute {
        requested_approver: Option<String>,
    },
    /// The owner cannot mint the identities the decision records.
    IdsUnavailable(GatingIdUnavailable),
}

impl std::fmt::Display for GatingDecideError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownPendingId(id) => write!(f, "unknown pending id: {id}"),
            Self::SelfApprovalForbidden => write!(f, "self-approval is forbidden"),
            Self::ApproverMismatch { expected, provided } => {
                write!(f, "approver mismatch: expected {expected}, got {provided}")
            }
            Self::EmptyPrincipal => write!(f, "deciding principal must not be empty"),
            Self::NoEscalationRoute { requested_approver } => match requested_approver {
                Some(approver) => write!(f, "no escalation route from {approver}"),
                None => write!(f, "no default escalation route"),
            },
            Self::IdsUnavailable(reason) => write!(f, "{reason}"),
        }
    }
}

impl std::error::Error for GatingDecideError {}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeliveryIdempotencyEntry {
    delivery_id: String,
    payload: Value,
    canonical_resolution: RoutingResolution,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct RouterBoundaryOverrides {
    channel: Option<String>,
    sink: Option<String>,
    target_module: Option<String>,
    retry_max: Option<u32>,
    backoff_ms: Option<u64>,
    rate_limit_per_minute: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct DeliveryBoundaryOutcome {
    sink_adapter: Option<String>,
    force_fail: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct DeliveryRateWindowKey {
    route_id: String,
    recipient: String,
    sink: String,
    window_start_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct MemoryConflictKey {
    entity: String,
    topic: String,
    store: String,
}

#[derive(Debug)]
pub struct MetaRuntimeHandle {
    config: MetaConfig,
    runtime_options: RuntimeOptions,
    loaded_modules: BTreeSet<String>,
    live_children: BTreeMap<String, module_process::ModuleProcess>,
    /// Module processes whose termination was not confirmed: still owned,
    /// never dropped as if settled.
    retained_cleanup: module_process::RetainedModuleCleanup,
    pub lifecycle_events: Vec<LifecycleEvent>,
    pub supervisor_report: SupervisorReport,
    pub merged_events: Vec<EventEnvelope<UnifiedEvent>>,
    routing_sequence: u64,
    routing_resolutions: BTreeMap<String, RoutingResolution>,
    routing_resolution_order: Vec<String>,
    runtime_routes: BTreeMap<String, RuntimeRoute>,
    delivery_sequence: u64,
    delivery_runtime_epoch_ms: u64,
    delivery_now_floor_ms: u64,
    delivery_clock_ms: u64,
    delivery_history: Vec<DeliveryRecord>,
    delivery_idempotency: BTreeMap<String, DeliveryIdempotencyEntry>,
    delivery_idempotency_by_delivery: BTreeMap<String, Vec<String>>,
    delivery_rate_window_counts: BTreeMap<DeliveryRateWindowKey, u32>,
    /// This owner's random epoch, carried by every gate identity it mints.
    /// `None` when entropy was unavailable at boot: minting then fails.
    gating_epoch: Option<String>,
    /// Next sequence under `gating_epoch`; starts at 0 for every owner.
    gating_sequence: u64,
    gating_pending: BTreeMap<String, GatingPendingEntry>,
    gating_pending_order: Vec<String>,
    gating_audit: Vec<GatingAuditEntry>,
    gating_resolution_observers: GatingResolutionObservers,
    /// Notices queued by the current operation; delivered when it finishes.
    gating_notice_queue: Vec<GatingResolutionNotice>,
    gating_escalation_routes: GatingEscalationRoutes,
    /// Wakes live readers when the audit changes. Carries no data.
    gating_changes: tokio::sync::broadcast::Sender<()>,
    memory_sequence: u64,
    memory_assertions: Vec<MemoryAssertion>,
    memory_conflicts: BTreeMap<MemoryConflictKey, MemoryConflictSignal>,
    memory_backend: Option<LocalJsonMemoryStoreAdapter>,
    running: bool,
}

impl MetaRuntimeHandle {
    /// Register a gating-resolution observer (see
    /// [`GatingResolutionObserver`]). Registration is append-only for the
    /// process lifetime.
    pub fn register_gating_resolution_observer(
        &mut self,
        observer: Arc<dyn GatingResolutionObserver>,
    ) {
        self.gating_resolution_observers.push(observer);
    }

    pub fn lifecycle_events(&self) -> &[LifecycleEvent] {
        &self.lifecycle_events
    }

    pub fn supervisor_report(&self) -> &SupervisorReport {
        &self.supervisor_report
    }

    #[doc(hidden)]
    pub fn inject_test_events(&mut self, events: Vec<EventEnvelope<UnifiedEvent>>) {
        for event in events {
            insert_event_sorted(&mut self.merged_events, event);
        }
    }

    fn next_sequence(counter: &mut u64) -> u64 {
        let seq = *counter;
        *counter = counter.saturating_add(1);
        seq
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleRouteRequest {
    pub module_id: String,
    pub method: String,
    pub params: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleRouteResponse {
    pub module_id: String,
    pub method: String,
    pub payload: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModuleRouteError {
    UnloadedModule(String),
    ModuleRuntime(RuntimeBoundaryError),
    UnexpectedRouteResponse,
}

impl std::fmt::Display for ModuleRouteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnloadedModule(id) => write!(f, "unloaded module: {id}"),
            Self::ModuleRuntime(err) => write!(f, "module runtime: {err}"),
            Self::UnexpectedRouteResponse => write!(f, "unexpected route response"),
        }
    }
}

impl std::error::Error for ModuleRouteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ModuleRuntime(err) => Some(err),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutingResolveError {
    RouterModuleNotLoaded,
    DeliveryModuleNotLoaded,
    EmptyRecipient,
    InvalidChannel,
    InvalidRateLimitPerMinute,
    RetryMaxExceedsCap { provided: u32, cap: u32 },
    RouterBoundary(RuntimeBoundaryError),
}

impl std::fmt::Display for RoutingResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RouterModuleNotLoaded => write!(f, "router module not loaded"),
            Self::DeliveryModuleNotLoaded => write!(f, "delivery module not loaded"),
            Self::EmptyRecipient => write!(f, "empty recipient"),
            Self::InvalidChannel => write!(f, "invalid channel"),
            Self::InvalidRateLimitPerMinute => write!(f, "invalid rate limit per minute"),
            Self::RetryMaxExceedsCap { provided, cap } => {
                write!(f, "retry max {provided} exceeds cap {cap}")
            }
            Self::RouterBoundary(err) => write!(f, "router boundary: {err}"),
        }
    }
}

impl std::error::Error for RoutingResolveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::RouterBoundary(err) => Some(err),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliverySendError {
    DeliveryModuleNotLoaded,
    InvalidRouteTarget(String),
    InvalidRouteId,
    UnknownRouteId(String),
    ForgedResolution,
    InvalidRecipient,
    InvalidSink,
    InvalidIdempotencyKey,
    IdempotencyPayloadMismatch,
    RateLimited {
        sink: String,
        window_start_ms: u64,
        limit: u32,
    },
    DeliveryBoundary(RuntimeBoundaryError),
}

impl std::fmt::Display for DeliverySendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DeliveryModuleNotLoaded => write!(f, "delivery module not loaded"),
            Self::InvalidRouteTarget(target) => write!(f, "invalid route target: {target}"),
            Self::InvalidRouteId => write!(f, "invalid route id"),
            Self::UnknownRouteId(id) => write!(f, "unknown route id: {id}"),
            Self::ForgedResolution => write!(f, "forged resolution"),
            Self::InvalidRecipient => write!(f, "invalid recipient"),
            Self::InvalidSink => write!(f, "invalid sink"),
            Self::InvalidIdempotencyKey => write!(f, "invalid idempotency key"),
            Self::IdempotencyPayloadMismatch => write!(f, "idempotency payload mismatch"),
            Self::RateLimited {
                sink,
                window_start_ms,
                limit,
            } => {
                write!(
                    f,
                    "rate limited on sink {sink} (window {window_start_ms}ms, limit {limit})"
                )
            }
            Self::DeliveryBoundary(err) => write!(f, "delivery boundary: {err}"),
        }
    }
}

impl std::error::Error for DeliverySendError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::DeliveryBoundary(err) => Some(err),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeRouteMutationError {
    EmptyRouteKey,
    EmptyRecipient,
    InvalidChannel,
    EmptySink,
    EmptyTargetModule,
    InvalidRateLimitPerMinute,
    RetryMaxExceedsCap { provided: u32, cap: u32 },
    RouteNotFound(String),
}

impl std::fmt::Display for RuntimeRouteMutationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyRouteKey => write!(f, "empty route key"),
            Self::EmptyRecipient => write!(f, "empty recipient"),
            Self::InvalidChannel => write!(f, "invalid channel"),
            Self::EmptySink => write!(f, "empty sink"),
            Self::EmptyTargetModule => write!(f, "empty target module"),
            Self::InvalidRateLimitPerMinute => write!(f, "invalid rate limit per minute"),
            Self::RetryMaxExceedsCap { provided, cap } => {
                write!(f, "retry max {provided} exceeds cap {cap}")
            }
            Self::RouteNotFound(key) => write!(f, "route not found: {key}"),
        }
    }
}

impl std::error::Error for RuntimeRouteMutationError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RpcRouteError {
    InvalidRequest,
    BoundaryProcess(ProcessBoundaryError),
    Route(ModuleRouteError),
    InvalidResponse,
}

impl std::fmt::Display for RpcRouteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRequest => write!(f, "invalid request"),
            Self::BoundaryProcess(err) => write!(f, "boundary process: {err}"),
            Self::Route(err) => write!(f, "route: {err}"),
            Self::InvalidResponse => write!(f, "invalid response"),
        }
    }
}

impl std::error::Error for RpcRouteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::BoundaryProcess(err) => Some(err),
            Self::Route(err) => Some(err),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeMutationError {
    Config(ConfigResolutionError),
    Runtime(RuntimeBoundaryError),
}

impl std::fmt::Display for RuntimeMutationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Config(err) => write!(f, "config resolution: {err}"),
            Self::Runtime(err) => write!(f, "runtime boundary: {err}"),
        }
    }
}

impl std::error::Error for RuntimeMutationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Config(err) => Some(err),
            Self::Runtime(err) => Some(err),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscribeScope {
    Mob,
    Agent,
    Interaction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscribeRequest {
    pub scope: SubscribeScope,
    pub last_event_id: Option<String>,
    pub agent_id: Option<String>,
}

impl Default for SubscribeRequest {
    fn default() -> Self {
        Self {
            scope: SubscribeScope::Mob,
            last_event_id: None,
            agent_id: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubscribeError {
    EmptyCheckpoint,
    UnknownCheckpoint(String),
    MissingAgentId,
    InvalidAgentId,
}

impl std::fmt::Display for SubscribeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyCheckpoint => write!(f, "empty checkpoint"),
            Self::UnknownCheckpoint(id) => write!(f, "unknown checkpoint: {id}"),
            Self::MissingAgentId => write!(f, "missing agent id"),
            Self::InvalidAgentId => write!(f, "invalid agent id"),
        }
    }
}

impl std::error::Error for SubscribeError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscribeKeepAlive {
    pub interval_ms: u64,
    pub event: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscribeResponse {
    pub scope: SubscribeScope,
    pub replay_from_event_id: Option<String>,
    pub keep_alive: SubscribeKeepAlive,
    pub keep_alive_comment: String,
    pub event_frames: Vec<String>,
    pub events: Vec<EventEnvelope<UnifiedEvent>>,
}

const SSE_KEEP_ALIVE_INTERVAL_MS: u64 = 15_000;
const SSE_KEEP_ALIVE_EVENT_NAME: &str = "keep-alive";
const SSE_KEEP_ALIVE_COMMENT_FRAME: &str = ": keep-alive\n\n";
const SUBSCRIBE_REPLAY_EVENT_CAP: usize = 3;
const DELIVERY_HISTORY_LIMIT_DEFAULT: usize = 20;
const DELIVERY_HISTORY_LIMIT_MAX: usize = 200;
const ROUTING_RESOLUTION_LIMIT_MAX: usize = 512;
pub const ROUTING_RETRY_MAX_CAP: u32 = 10;
const DELIVERY_RATE_WINDOW_MS: u64 = 60_000;
const DELIVERY_RATE_WINDOWS_RETAINED: u64 = 2;
const DELIVERY_CLOCK_STEP_MS: u64 = 1_000;
const GATING_APPROVAL_TIMEOUT_DEFAULT_MS: u64 = 60_000;
/// Lower bound on a caller-supplied R3 approval timeout. Without it a value of
/// `0` (or a few ms) makes `deadline_at_ms == created_at_ms`, so the pending
/// entry is treated as already-expired by `refresh_gating_timeouts` on the very
/// next gating RPC and is replaced with a `timeout_fallback` SafeDraft before
/// any approver can act — R3 approval can never complete. 1 second is the floor
/// that keeps a `PendingApproval` outcome actually approvable.
const GATING_APPROVAL_TIMEOUT_MIN_MS: u64 = 1_000;
/// Upper bound on a caller-supplied R3 approval timeout. Without it a huge
/// `approval_timeout_ms` makes `created_at_ms.saturating_add(timeout_ms)`
/// saturate to `u64::MAX`, so `now_ms >= deadline_at_ms` is never true and the
/// pending approval can never time out to a safe draft. 7 days is far beyond
/// any real human-approval window.
const GATING_APPROVAL_TIMEOUT_MAX_MS: u64 = 7 * 24 * 60 * 60 * 1_000;
const GATING_AUDIT_MAX_RETAINED: usize = 512;
/// Format tag of newly minted gate identities, `gate-<kind>-v2-<epoch>-<sequence>`.
const GATING_ID_FORMAT: &str = "v2";
/// Identities one evaluation can mint: action, pending entry, its audit
/// record and the approval notification audit record.
const GATING_IDS_PER_EVALUATION: u64 = 4;
/// Identities one decision can mint: an escalation successor and two audit records.
const GATING_IDS_PER_DECISION: u64 = 3;
const GATING_PENDING_MAX_RETAINED: usize = 512;
const MEMORY_ASSERTIONS_MAX_RETAINED: usize = 4_096;
const MEMORY_SUPPORTED_STORES: [&str; 5] = [
    "knowledge_graph",
    "vector",
    "timeline",
    "todo",
    "top_of_mind",
];
const MEMORY_LEDGER_HEALTHCHECK_TIMEOUT: Duration = Duration::from_secs(2);
const CONSOLE_EXPERIENCE_ROUTE: &str = "/console/experience";
const CONSOLE_MODULES_ROUTE: &str = "/console/modules";
const EVENTS_SUBSCRIBE_METHOD: &str = "meta/events/subscribe";

fn default_delivery_history_limit() -> usize {
    DELIVERY_HISTORY_LIMIT_DEFAULT
}

pub fn run_meerkat_baseline_verification_once(
    command: &str,
    args: &[String],
    env: &[(String, String)],
    timeout: Duration,
) -> Result<BaselineVerificationReport, BaselineRuntimeError> {
    let line = run_process_json_line(command, args, env, timeout)
        .map_err(BaselineRuntimeError::Process)?;
    let value: Value =
        serde_json::from_str(&line).map_err(|_| BaselineRuntimeError::InvalidRepoPathJson)?;
    let repo = value
        .as_object()
        .and_then(|obj| obj.get("repo_root"))
        .ok_or(BaselineRuntimeError::MissingRepoRoot)?
        .as_str()
        .ok_or(BaselineRuntimeError::InvalidRepoRoot)?;
    if repo.trim().is_empty() {
        return Err(BaselineRuntimeError::InvalidRepoRoot);
    }
    verify_meerkat_baseline_symbols(Some(std::path::Path::new(repo)))
        .map_err(BaselineRuntimeError::Baseline)
}

/// Validates the trusted OIDC snapshot a console needs to verify tokens: a
/// non-empty audience, a discovery document with `issuer` and `jwks_uri`, and
/// a JWKS with at least one key.
///
/// [`build_runtime_decision_state`] runs this only when
/// `console.require_app_auth` is true. An open console never consults the
/// snapshot unless an ABAC access controller is installed and a caller
/// volunteers a token; a broken snapshot then degrades that caller to
/// anonymous instead of refusing at startup. A host that wants the startup
/// refusal on an open console calls this itself.
pub fn validate_trusted_oidc_runtime_config(
    config: &TrustedOidcRuntimeConfig,
) -> Result<(), DecisionPolicyError> {
    if config.audience.trim().is_empty() {
        return Err(DecisionPolicyError::InvalidTrustedAuthConfig(
            "trusted OIDC audience must not be empty".to_string(),
        ));
    }
    parse_oidc_discovery_json(&config.discovery_json).map_err(|err| {
        DecisionPolicyError::InvalidTrustedAuthConfig(format!(
            "invalid trusted OIDC discovery: {err:?}"
        ))
    })?;
    parse_jwks_json(&config.jwks_json).map_err(|err| {
        DecisionPolicyError::InvalidTrustedAuthConfig(format!("invalid trusted JWKS: {err:?}"))
    })?;
    Ok(())
}

/// Validates `input` and assembles the runtime's decision state.
///
/// Checks, in order: BigQuery naming; the trust manifest (TOML, an empty
/// manifest declares no modules); the ops policy; the release metadata (JSON
/// parse, then the four required registries and
/// `support_matrix == "same-as-meerkat"`); and, only when
/// `input.console.require_app_auth` is true, the trusted OIDC snapshot via
/// [`validate_trusted_oidc_runtime_config`]. Every refusal is a typed
/// [`DecisionRuntimeError::Policy`]; nothing here panics.
///
/// `RuntimeDecisionState` has public fields, so a host may also build it as a
/// struct literal and skip these checks; [`RuntimeDecisionState::local_console`]
/// is the supported shortcut for a console that trusts no provider.
pub fn build_runtime_decision_state(
    input: RuntimeDecisionInputs,
) -> Result<RuntimeDecisionState, DecisionRuntimeError> {
    validate_bigquery_naming(&input.bigquery).map_err(DecisionRuntimeError::Policy)?;
    let modules = load_trusted_meta_modules_from_toml(&input.trusted_meta_toml)
        .map_err(DecisionRuntimeError::Policy)?;
    validate_runtime_ops_policy(&input.ops).map_err(DecisionRuntimeError::Policy)?;
    let release_metadata = parse_release_metadata_json(&input.release_metadata_json)
        .map_err(DecisionRuntimeError::Policy)?;
    validate_release_metadata(&release_metadata).map_err(DecisionRuntimeError::Policy)?;
    if input.console.require_app_auth {
        validate_trusted_oidc_runtime_config(&input.trusted_oidc)
            .map_err(DecisionRuntimeError::Policy)?;
    }

    Ok(RuntimeDecisionState {
        bigquery: input.bigquery,
        modules,
        auth: input.auth,
        trusted_oidc: input.trusted_oidc,
        console: input.console,
        ops: input.ops,
        release_metadata,
    })
}

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// A fresh random 128-bit epoch for one gating owner. Every gate identity the
/// owner mints carries it, so a later process or another owner does not
/// reissue the same identity, barring a random epoch collision. Legacy
/// sequential IDs have a different form and never match. Failed entropy
/// leaves the owner unable to mint, with no fallback.
fn new_gating_epoch() -> Option<String> {
    use rand_core::RngCore;
    let mut bytes = [0_u8; 16];
    rand_core::OsRng.try_fill_bytes(&mut bytes).ok()?;
    Some(format!("{:032x}", u128::from_be_bytes(bytes)))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod decision_state_tests {
    use super::*;
    use crate::auth::OidcContractError;

    const CANONICAL_RELEASE_JSON: &str = r#"{"targets":["crates.io","npm","pypi","github-releases"],"support_matrix":"same-as-meerkat"}"#;
    const ONE_KEY_JWKS_JSON: &str =
        r#"{"keys":[{"kty":"oct","alg":"HS256","k":"c2VjcmV0LXNlY3JldC1zZWNyZXQ"}]}"#;
    const VALID_DISCOVERY_JSON: &str =
        r#"{"issuer":"http://127.0.0.1/tests","jwks_uri":"http://127.0.0.1/tests/jwks.json"}"#;

    fn open_console() -> ConsolePolicy {
        ConsolePolicy {
            require_app_auth: false,
            ..ConsolePolicy::default()
        }
    }

    fn snapshot_that_trusts_nothing() -> TrustedOidcRuntimeConfig {
        TrustedOidcRuntimeConfig {
            discovery_json: "{}".to_string(),
            jwks_json: r#"{"keys":[]}"#.to_string(),
            audience: String::new(),
            require_verified_email: false,
        }
    }

    fn one_key_snapshot() -> TrustedOidcRuntimeConfig {
        TrustedOidcRuntimeConfig {
            discovery_json: VALID_DISCOVERY_JSON.to_string(),
            jwks_json: ONE_KEY_JWKS_JSON.to_string(),
            audience: "tests".to_string(),
            require_verified_email: false,
        }
    }

    fn inputs(
        console: ConsolePolicy,
        trusted_oidc: TrustedOidcRuntimeConfig,
    ) -> RuntimeDecisionInputs {
        RuntimeDecisionInputs {
            bigquery: BigQueryNaming {
                dataset: "decision_tests".to_string(),
                table: "rows".to_string(),
            },
            trusted_meta_toml: String::new(),
            auth: AuthPolicy::default(),
            trusted_oidc,
            console,
            ops: RuntimeOpsPolicy::default(),
            release_metadata_json: CANONICAL_RELEASE_JSON.to_string(),
        }
    }

    fn invalid_trusted_auth_message(err: DecisionRuntimeError) -> String {
        match err {
            DecisionRuntimeError::Policy(DecisionPolicyError::InvalidTrustedAuthConfig(msg)) => msg,
            other => panic!("expected InvalidTrustedAuthConfig, got {other:?}"),
        }
    }

    #[test]
    fn auth_off_accepts_a_snapshot_that_trusts_nothing() {
        let state =
            build_runtime_decision_state(inputs(open_console(), snapshot_that_trusts_nothing()))
                .expect("open console needs no trusted keys");

        assert!(!state.console.require_app_auth);
        assert!(state.modules.is_empty());
        assert_eq!(state.trusted_oidc.jwks_json, r#"{"keys":[]}"#);
    }

    #[test]
    fn auth_on_accepts_a_one_key_snapshot_and_an_empty_manifest() {
        let state =
            build_runtime_decision_state(inputs(ConsolePolicy::default(), one_key_snapshot()))
                .expect("one trusted key is a complete snapshot");

        assert!(state.console.require_app_auth);
        assert!(state.modules.is_empty());
    }

    #[test]
    fn auth_on_refuses_an_empty_jwks() {
        let mut snapshot = one_key_snapshot();
        snapshot.jwks_json = r#"{"keys":[]}"#.to_string();

        let err = build_runtime_decision_state(inputs(ConsolePolicy::default(), snapshot))
            .expect_err("auth on with no keys must be refused at build time");

        assert!(invalid_trusted_auth_message(err).contains("JWKS"));
    }

    #[test]
    fn auth_on_refuses_an_empty_audience() {
        let mut snapshot = one_key_snapshot();
        snapshot.audience = "   ".to_string();

        let err = build_runtime_decision_state(inputs(ConsolePolicy::default(), snapshot))
            .expect_err("auth on with no audience must be refused at build time");

        assert!(invalid_trusted_auth_message(err).contains("audience"));
    }

    #[test]
    fn auth_on_refuses_discovery_without_jwks_uri() {
        let mut snapshot = one_key_snapshot();
        snapshot.discovery_json = r#"{"issuer":"http://127.0.0.1/tests"}"#.to_string();

        let err = build_runtime_decision_state(inputs(ConsolePolicy::default(), snapshot))
            .expect_err("auth on with an incomplete discovery document must be refused");

        assert!(invalid_trusted_auth_message(err).contains("discovery"));
    }

    #[test]
    fn release_metadata_parse_error_names_json_at_build() {
        let mut input = inputs(open_console(), snapshot_that_trusts_nothing());
        input.release_metadata_json = "not json".to_string();

        let err = build_runtime_decision_state(input)
            .expect_err("malformed release metadata must be refused");

        match err {
            DecisionRuntimeError::Policy(DecisionPolicyError::TomlParse(msg)) => {
                assert!(msg.starts_with("release metadata JSON: "), "{msg}");
            }
            other => panic!("expected TomlParse, got {other:?}"),
        }
    }

    #[test]
    fn release_metadata_is_still_validated_with_auth_off() {
        let mut input = inputs(open_console(), snapshot_that_trusts_nothing());
        input.release_metadata_json =
            r#"{"targets":["crates.io"],"support_matrix":"same-as-meerkat"}"#.to_string();

        let err = build_runtime_decision_state(input)
            .expect_err("incomplete release metadata must be refused");

        assert_eq!(
            err,
            DecisionRuntimeError::Policy(DecisionPolicyError::MissingReleaseTarget(
                "npm".to_string()
            ))
        );
    }

    #[test]
    fn validate_trusted_oidc_runtime_config_is_callable_on_its_own() {
        validate_trusted_oidc_runtime_config(&one_key_snapshot()).expect("complete snapshot");

        let err = validate_trusted_oidc_runtime_config(&snapshot_that_trusts_nothing())
            .expect_err("empty snapshot");
        assert!(matches!(
            err,
            DecisionPolicyError::InvalidTrustedAuthConfig(_)
        ));
    }

    #[test]
    fn local_console_keeps_the_given_policy_and_naming() {
        let naming = BigQueryNaming {
            dataset: "tux_local".to_string(),
            table: "runtime_events".to_string(),
        };
        let policy = ConsolePolicy {
            require_app_auth: false,
            read_only: true,
            fetch_timeout_ms: Some(1_500),
            ..ConsolePolicy::default()
        };

        let state = RuntimeDecisionState::local_console(policy.clone(), Some(naming.clone()));

        assert_eq!(state.console, policy);
        assert_eq!(state.bigquery, naming);
        assert!(state.modules.is_empty());
    }

    #[test]
    fn local_console_defaults_to_the_sdk_gateway_naming() {
        let state = RuntimeDecisionState::local_console(open_console(), None);

        assert_eq!(state.bigquery.dataset, "default_dataset");
        assert_eq!(state.bigquery.table, "default_table");
        validate_bigquery_naming(&state.bigquery).expect("default naming is valid");
    }

    #[test]
    fn local_console_trusts_no_keys() {
        let state = RuntimeDecisionState::local_console(open_console(), None);

        assert_eq!(
            parse_jwks_json(&state.trusted_oidc.jwks_json),
            Err(OidcContractError::MissingKeys)
        );
        parse_oidc_discovery_json(&state.trusted_oidc.discovery_json)
            .expect("placeholder discovery document parses");
        assert!(matches!(
            validate_trusted_oidc_runtime_config(&state.trusted_oidc),
            Err(DecisionPolicyError::InvalidTrustedAuthConfig(_))
        ));
    }

    #[test]
    fn local_console_release_metadata_is_canonical() {
        let state = RuntimeDecisionState::local_console(open_console(), None);

        validate_release_metadata(&state.release_metadata).expect("canonical release metadata");
    }

    #[test]
    fn local_console_with_the_default_policy_stays_closed() {
        let state = RuntimeDecisionState::local_console(ConsolePolicy::default(), None);

        assert!(state.console.require_app_auth);
    }

    #[test]
    fn local_console_with_modules_carries_the_trusted_set() {
        let module = ModuleConfig {
            id: "recorder".to_string(),
            command: "recorder".to_string(),
            args: vec!["--flag".to_string()],
            restart_policy: RestartPolicy::Never,
        };

        let state = RuntimeDecisionState::local_console(open_console(), None)
            .with_modules(vec![module.clone()]);

        assert_eq!(state.modules, vec![module]);
        assert_eq!(state.console, open_console());
    }
}
