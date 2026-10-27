//! Shared composition-root scaffolding for the two gateway binaries
//! (simplification item 10, "unify gateway composition roots").
//!
//! # Charter, and how it differs from [`crate::gateway_wiring`]
//!
//! [`crate::gateway_wiring`] owns ONE thing: opening the identity substrate
//! (continuity store + lease provider) so both gateways get the same durable
//! identity semantics. That module is deliberately narrow.
//!
//! This module owns the COMPOSITION ROOT itself: the process-level boot
//! scaffolding a gateway binary performs, in order, around the runtime it
//! builds. Five bands live here:
//!
//! 1. The boot preamble - things done before any runtime exists (tokio
//!    runtime, tracing subscriber, argv parsing).
//! 2. Explicit compatibility-profile declarations. The two binaries retain
//!    their existing wire/config shapes, but those differences can no longer
//!    be mistaken for independent composition authority.
//! 3. A type-state runtime root - prepare, bootstrap, profile enrichment,
//!    activate - which is the only gateway path into `UnifiedRuntime`.
//! 4. Post-bootstrap surface attachment that both binaries perform with
//!    identical observable behaviour (the schedule firing host).
//! 5. Loopback HTTP admission and ordered cooperative shutdown. HTTP stops
//!    first, profile-owned drains quiesce second, runtime authority cleanup
//!    completes third, and registry/process cleanup is last.
//!
//! The rule for adding to this module: a seam belongs here when BOTH
//! binaries must do it and the observable behaviour (log strings, ordering,
//! error posture) is already identical. Substrate construction belongs in
//! `gateway_wiring`; if a seam here starts opening stores, it has forked the
//! charter and should move.
//!
//! # Compatibility differences intentionally retained
//!
//! This is the structural history the item asks for. Every claim below was
//! read off the source in this tree, and is cited BY SYMBOL, never by line
//! number: both binaries are under active edit, and a line-numbered survey
//! decays into a survey that sends its reader to the wrong place while still
//! looking authoritative.
//!
//! The former roots are `run` in `src/bin/meta_gateway.rs` and
//! `run_persistent_inner` in `src/rpc_gateway.rs`. They now translate
//! their compatibility inputs into [`GatewayRuntimeBootstrapPlan`] and use
//! [`GatewayComposition`] for runtime and lifecycle ownership.
//!
//! - **Init params.** `meta_gateway` deserializes a typed `InitParams`
//!   struct straight off the TOP LEVEL of `params` (`parse_init_request`).
//!   `rpc_gateway` reads exactly one ad-hoc top-level key
//!   (`params.get("mob_config")`) and routes everything else through
//!   `parse_gateway_runtime_options`, which reads the NESTED
//!   `params.runtime_options` object. These are not merely two different
//!   types: `console_read_only` and `compaction` exist in BOTH vocabularies
//!   at DIFFERENT nesting depths, so converging them is wire-visible to both
//!   SDKs. They also disagree on strictness - `InitParams` derives a plain
//!   `Deserialize` and silently ignores unknown keys, while
//!   `parse_gateway_runtime_options` refuses them ("unsupported
//!   runtime_options fields"). Largest single remaining piece.
//!
//! - **Bootstrap inputs.** The console profile keeps empty events, default
//!   runtime options and in-memory metadata. The stdio profile keeps its
//!   configured options and persistent metadata. Both flow through the same
//!   exact low-level bootstrap call.
//!
//! - **Console log store.** `rpc_gateway` installs `SqliteConsoleLogStore`
//!   for persistent launches (`set_console_log_store`). `meta_gateway`
//!   never calls `set_console_log_store`; its console timeline is in-memory
//!   by contract, declared through `StorageSlotSummary::declared_ephemeral`
//!   for both the `console` and the `metadata` slot.
//!
//! - **Identity mechanism.** Two different attachment paths, not two
//!   configurations of one. `meta_gateway` builds an
//!   `IdentityFirstRuntimeContext` and calls
//!   `attach_identity_first_context`, default ON
//!   (`params.identity_first.unwrap_or(true)`). `rpc_gateway` builds a
//!   `crate::rpc::IdentityFirstContext` and threads it through the stdio
//!   dispatcher rather than attaching it to the runtime; it is gated on the
//!   SDK's `has_roster_provider` flag.
//!
//! - **Console identity seam.** Each binary installs a DIFFERENT one, and
//!   neither installs the other's: `meta_gateway` calls
//!   `runtime.set_console_identity_roster`, `rpc_gateway` calls
//!   `runtime.set_console_operator_resolver`.
//!
//! - **Memory / steward / gating / job surfaces.** Present only in
//!   `rpc_gateway`: `GatewayMobPurposeSource`, `GatewayMemoryGatingBridge`,
//!   `GatewayMemoryConflictBridge`, the `StewardEngine` slot,
//!   `register_gating_resolution_observer`, `set_memory_panel_store`,
//!   `set_error_hook` and `set_job_health_projection`. `meta_gateway`
//!   wires none of them, so the agent-memory panel, the promotion gate and
//!   the job-health projection are structurally absent from the
//!   console-only gateway.
//!
//! - **Runnable hosts.** `rpc_gateway` composes a `gateway_runnable_host`
//!   from the steward dream plus SDK-declared
//!   `runtime_options.host_runnables`, and warns when they are configured
//!   with no host to run them. `meta_gateway` passes `None` and has no
//!   host-runnable vocabulary at all. This is the one divergence
//!   [`spawn_gateway_schedule_host`] below makes explicit rather than
//!   implicit: it is now a named field, not a hard-coded argument.
//!
//! - **Callback plane.** The stdio compatibility adapter owns `StdioCallbackBridge`,
//!   `DetachedCallbackJobRuntime`, `CallbackToolDispatcher` and
//!   `StdioCallbackAgentBuilder` - ~2,600 lines from the first of those to
//!   the start of `run_persistent`. `meta_gateway` has no callback plane;
//!   it builds agents directly through `FactoryAgentBuilder`. These cannot
//!   converge without the init-param convergence above, because the callback
//!   plane only exists when an SDK host is on the other end of stdin.
//!
//! - **Registry / resume.** `meta_gateway` maintains a cross-process
//!   runtime registry keyed by a config fingerprint and can answer
//!   `launch_state: "resumed"` without booting a runtime at all.
//!   `rpc_gateway` has no such concept.
//!
//! Profile-specific enrichment remains explicit between bootstrap and
//! activation. This is the checked seam list, not a closed census:
//! `set_contact_directory`,
//! `set_access_controller`, `set_gateway_peer_keys`,
//! `start_control_listener`, `with_durable_runtime_store`,
//! `with_session_runtime_adapter`, `with_workgraph_service`,
//! `with_workgraph_admission_slot`, `with_workgraph_admission_sidecar`,
//! `with_runtime_archived_terminal_authority`,
//! `with_committed_boundary_recoverer`, `with_runtime_services`,
//! `with_image_generation_machine`, and the direct `spec.runtime_adapter` /
//! `spec.resolved_storage` / `spec.binary_blob_store` /
//! `spec.committed_boundary_recoverer` assignments.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use meerkat::surface::ScheduleHostHandle;
use meerkat::{
    PersistentSessionService, ScheduleRunnableHost, ScheduleService, SessionAgentBuilder,
    WorkGraphService,
};
use meerkat_mob_mcp::MobMcpState;
use meerkat_runtime::MeerkatMachine;

use crate::mob_composition_manifest::CompositionAuthority;
use crate::mob_handle_runtime::MobBootstrapSpec;
use crate::runtime::cross_mob_control::ControlListenAddr;
use crate::runtime::{
    InMemoryMetadataStore, PersistentMetadataStore, RuntimeDecisionState, RuntimeOptions,
};
use crate::schedule_wiring::{
    ScheduleClaimWatchdogConfig, ScheduleFiringHostBinding, ScheduleMobTargetRegistry,
};
use crate::types::{EventEnvelope, MetaConfig, UnifiedEvent};
use crate::unified_runtime::{
    UnifiedRuntime, UnifiedRuntimeBootstrapError, UnifiedRuntimeShutdownReport,
};

// ---------------------------------------------------------------------------
// Compatibility profiles
// ---------------------------------------------------------------------------

/// The two shipped binaries are compatibility profiles over one composition
/// root, not independent runtime assemblers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GatewayCompatibilityProfile {
    /// `meta_gateway`: console/admin HTTP, top-level init params, registry
    /// resume and maintenance verbs.
    ConsoleHttp,
    /// `rpc_gateway --persistent`: SDK stdin JSON-RPC, callbacks and the
    /// private bounded shutdown handshake.
    StdioRpc,
}

/// Wire-visible differences which must remain explicit while the runtime
/// composition and lifecycle are shared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GatewayCompatibilityContract {
    pub top_level_permissive_init: bool,
    pub registry_resume: bool,
    pub post_init_stdin_rpc: bool,
    pub callback_plane: bool,
    pub single_shot: bool,
    pub maintenance_verbs: bool,
}

impl GatewayCompatibilityProfile {
    pub const fn contract(self) -> GatewayCompatibilityContract {
        match self {
            Self::ConsoleHttp => GatewayCompatibilityContract {
                top_level_permissive_init: true,
                registry_resume: true,
                post_init_stdin_rpc: false,
                callback_plane: false,
                single_shot: false,
                maintenance_verbs: true,
            },
            Self::StdioRpc => GatewayCompatibilityContract {
                top_level_permissive_init: false,
                registry_resume: false,
                post_init_stdin_rpc: true,
                callback_plane: true,
                single_shot: true,
                maintenance_verbs: false,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Typed runtime composition stages
// ---------------------------------------------------------------------------

/// Exact low-level inputs to `UnifiedRuntime::bootstrap_with_options`.
///
/// The console profile uses [`Self::console_http`], which reproduces
/// `UnifiedRuntime::bootstrap` exactly. The stdio profile supplies its
/// existing events, options and metadata authority. Keeping both behind this
/// one call prevents a third bootstrap path without changing either wire
/// schema.
pub struct GatewayRuntimeBootstrapPlan {
    mob_spec: MobBootstrapSpec,
    module_config: MetaConfig,
    module_agent_events: Vec<EventEnvelope<UnifiedEvent>>,
    timeout: Duration,
    runtime_options: RuntimeOptions,
    persistent_metadata: Arc<dyn PersistentMetadataStore>,
}

impl GatewayRuntimeBootstrapPlan {
    pub fn console_http(
        mob_spec: MobBootstrapSpec,
        module_config: MetaConfig,
        timeout: Duration,
    ) -> Self {
        Self {
            mob_spec,
            module_config,
            module_agent_events: Vec::new(),
            timeout,
            runtime_options: RuntimeOptions::default(),
            persistent_metadata: Arc::new(InMemoryMetadataStore::new()),
        }
    }

    pub fn stdio_rpc(
        mob_spec: MobBootstrapSpec,
        module_config: MetaConfig,
        module_agent_events: Vec<EventEnvelope<UnifiedEvent>>,
        timeout: Duration,
        runtime_options: RuntimeOptions,
        persistent_metadata: Arc<dyn PersistentMetadataStore>,
    ) -> Self {
        Self {
            mob_spec,
            module_config,
            module_agent_events,
            timeout,
            runtime_options,
            persistent_metadata,
        }
    }
}

pub struct PreparedGateway {
    plan: GatewayRuntimeBootstrapPlan,
}

pub struct BootstrappedGateway {
    runtime: UnifiedRuntime,
}

pub struct ActiveGateway {
    runtime: Arc<UnifiedRuntime>,
    event_drain_task: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl ActiveGateway {
    async fn stop_event_drain(&self) {
        let task = self.event_drain_task.lock().await.take();
        if let Some(task) = task {
            task.abort();
            let _ = task.await;
        }
    }
}

impl Drop for ActiveGateway {
    fn drop(&mut self) {
        // An error before ordered shutdown must not leave a detached task
        // retaining the runtime and forwarding into abandoned surfaces.
        if let Some(task) = self.event_drain_task.get_mut().take() {
            task.abort();
        }
    }
}

/// Type-state composition root. Profile-specific config parsing and optional
/// seams happen around this root, but runtime bootstrap and process lifecycle
/// have one owner.
pub struct GatewayComposition<State> {
    profile: GatewayCompatibilityProfile,
    state: State,
}

impl GatewayComposition<PreparedGateway> {
    pub fn prepare(
        profile: GatewayCompatibilityProfile,
        plan: GatewayRuntimeBootstrapPlan,
    ) -> Self {
        Self {
            profile,
            state: PreparedGateway { plan },
        }
    }

    pub async fn bootstrap(
        self,
    ) -> Result<GatewayComposition<BootstrappedGateway>, UnifiedRuntimeBootstrapError> {
        let plan = self.state.plan;
        let runtime = crate::boxed_future::boxed(|| {
            UnifiedRuntime::bootstrap_with_options(
                plan.mob_spec,
                plan.module_config,
                plan.module_agent_events,
                plan.timeout,
                plan.runtime_options,
                plan.persistent_metadata,
            )
        })
        .await?;
        Ok(GatewayComposition {
            profile: self.profile,
            state: BootstrappedGateway { runtime },
        })
    }
}

impl GatewayComposition<BootstrappedGateway> {
    pub fn runtime(&self) -> &UnifiedRuntime {
        &self.state.runtime
    }

    pub fn runtime_mut(&mut self) -> &mut UnifiedRuntime {
        &mut self.state.runtime
    }

    /// The bootstrapped runtime as a shared reference, for a driver that shuts
    /// it down without activating it.
    pub(crate) fn into_shared_runtime(self) -> Arc<UnifiedRuntime> {
        Arc::new(self.state.runtime)
    }

    pub fn activate(self) -> GatewayComposition<ActiveGateway> {
        let runtime = Arc::new(self.state.runtime);
        // Both gateways host their own HTTP server instead of calling
        // UnifiedRuntime::serve, so the shared lifecycle owns event draining.
        let event_drain_task = Arc::clone(&runtime).spawn_event_drain_task();
        GatewayComposition {
            profile: self.profile,
            state: ActiveGateway {
                runtime,
                event_drain_task: tokio::sync::Mutex::new(Some(event_drain_task)),
            },
        }
    }
}

impl GatewayComposition<ActiveGateway> {
    pub fn profile(&self) -> GatewayCompatibilityProfile {
        self.profile
    }

    pub fn runtime(&self) -> &Arc<UnifiedRuntime> {
        &self.state.runtime
    }

    /// Stop and join the shared event-drain task. For the persistent driver's
    /// retained cleanup, which shuts the runtime down itself.
    pub(crate) async fn stop_event_drain(&self) {
        self.state.stop_event_drain().await;
    }
}

// ---------------------------------------------------------------------------
// Shared HTTP admission and ordered shutdown
// ---------------------------------------------------------------------------

pub const GATEWAY_HTTP_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);
/// Covers every bounded phase inside `UnifiedRuntime::shutdown`, including
/// `RETIRED_SUPERVISOR_JOIN_BUDGET`, `MOB_TERMINAL_SHUTDOWN_BUDGET`,
/// `CHILD_MOB_STAND_DOWN_BUDGET` and `MEMBER_MCP_SETTLEMENT_BUDGET`, so this
/// outer timeout can never cut the runtime off before its per-member mob
/// shutdown report, its child-mob stand-down or its member MCP settlement.
/// Asserted component-by-component in
/// `advertised_shutdown_horizon_covers_every_bounded_gateway_phase`.
pub const GATEWAY_RUNTIME_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(377);

/// The address a gateway HTTP listener binds when nothing selects another:
/// loopback, ephemeral port. Every shipped release before the `bind` door
/// existed used exactly this, and it is still the default on both binaries.
pub const DEFAULT_GATEWAY_HTTP_LISTEN: SocketAddr =
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);

/// A bound gateway HTTP listener plus the two base URLs it can be reached at:
/// the same-host form ([`http_base_url`](Self::http_base_url)) and, when the
/// launch declared one, the proxy-facing form
/// ([`advertised_base_url`](Self::advertised_base_url)).
pub struct GatewayHttpBinding {
    listener: tokio::net::TcpListener,
    local_addr: SocketAddr,
    advertised_base_url: Option<String>,
}

impl GatewayHttpBinding {
    /// Bind loopback on an ephemeral port: the default posture, and the only
    /// one both binaries had until the `http_listen` door was added.
    pub async fn bind_loopback() -> std::io::Result<Self> {
        Self::bind(DEFAULT_GATEWAY_HTTP_LISTEN).await
    }

    /// Bind an explicit address.
    ///
    /// This constructor performs NO exposure check. Callers run
    /// [`validate_http_bind_policy`] on `listen` first; both binaries do so
    /// before any bootstrap work, so a refused address costs no runtime
    /// build. Unspecified addresses (`0.0.0.0`, `::`) bind every interface;
    /// [`http_base_url`](Self::http_base_url) still reports the loopback form
    /// a same-host client dials.
    pub async fn bind(listen: SocketAddr) -> std::io::Result<Self> {
        let listener = tokio::net::TcpListener::bind(listen).await?;
        let local_addr = listener.local_addr()?;
        Ok(Self {
            listener,
            local_addr,
            advertised_base_url: None,
        })
    }

    /// Record the base URL clients on the far side of a proxy use to reach
    /// this listener (`https://mob.example.com`, `http://192.168.0.10:8080`).
    /// It is carried into the init result as `http_public_base_url` and is
    /// never used to bind. A trailing `/` is dropped so route concatenation
    /// stays uniform with [`http_base_url`](Self::http_base_url).
    pub fn with_advertised_base_url(mut self, base_url: Option<String>) -> Self {
        self.advertised_base_url = base_url.map(|url| url.trim_end_matches('/').to_string());
        self
    }

    /// The kernel-assigned port (meaningful for `HOST:0` binds).
    pub fn port(&self) -> u16 {
        self.local_addr.port()
    }

    /// The address the listener is actually bound to, as the kernel reports
    /// it (`0.0.0.0:8080` stays `0.0.0.0:8080`).
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// The address a client ON THIS HOST dials: the bound address with an
    /// unspecified IP mapped to the loopback of the same family.
    pub fn reachable_addr(&self) -> SocketAddr {
        loopback_reachable_addr(self.local_addr)
    }

    /// `http://<reachable_addr>`: what the init result reports as
    /// `http_base_url`. For the default loopback bind this is byte-identical
    /// to every earlier release (`http://127.0.0.1:<port>`); for `0.0.0.0` it
    /// is still a URL that resolves on this host, because the SDK that
    /// spawned the gateway shares its network namespace and dials this form
    /// for SSE, multipart and console RPC.
    pub fn http_base_url(&self) -> String {
        format!("http://{}", self.reachable_addr())
    }

    /// The proxy-facing base URL declared at launch, if any.
    pub fn advertised_base_url(&self) -> Option<&str> {
        self.advertised_base_url.as_deref()
    }

    pub fn serve(self, app: Router) -> GatewayHttpServer {
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
        // The server's event streams end when its HTTP drain begins, so the
        // bounded drain can join them. They stay open while the server only
        // stops accepting connections, an in-flight drain included.
        let (streams_end_tx, streams_end_rx) = tokio::sync::watch::channel(false);
        let app = app.layer(axum::Extension(crate::http_sse::StreamShutdown::new(
            streams_end_rx,
        )));
        let task = tokio::spawn(async move {
            axum::serve(self.listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.changed().await;
                })
                .await
        });
        GatewayHttpServer {
            shutdown_tx,
            streams_end_tx,
            task: Some(task),
        }
    }
}

/// Map an unspecified bind address (`0.0.0.0`, `::`) to the loopback address
/// of the same family, keeping the port. Every other address is already the
/// one a same-host client dials: a listener bound to one interface does not
/// accept on loopback, so rewriting `192.168.0.10:8080` to `127.0.0.1:8080`
/// would advertise a URL nothing answers.
pub fn loopback_reachable_addr(bound: SocketAddr) -> SocketAddr {
    match bound.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => {
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), bound.port())
        }
        IpAddr::V6(ip) if ip.is_unspecified() => {
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), bound.port())
        }
        _ => bound,
    }
}

// ---------------------------------------------------------------------------
// HTTP listener exposure policy
// ---------------------------------------------------------------------------

/// Exposure policy for a gateway HTTP listener.
///
/// Mirrors `meerkat_rpc::secure_rpc::TcpBindPolicy`, the gate behind
/// `--allow-remote` on `rkat-rpc --tcp` and `rkat mob host --listen-tcp`, so
/// a Meta operator meets the same word with the same meaning: `allow_remote`
/// is an explicit transport-exposure acknowledgement, not an auth mechanism.
/// Meta does not depend on meerkat-rpc and the meerkat facade does not
/// re-export the type, so the rule lives here rather than behind a new
/// dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HttpBindPolicy {
    pub allow_remote: bool,
}

impl HttpBindPolicy {
    pub const fn local_only() -> Self {
        Self {
            allow_remote: false,
        }
    }

    pub const fn allow_remote() -> Self {
        Self { allow_remote: true }
    }

    /// The policy both gateways resolve at init: a non-loopback bind is
    /// permitted when the launch acknowledged it (`allow_remote`) OR when the
    /// console enforces app auth on every request, because then the listener
    /// carries its own access boundary. `meta_gateway` builds its decision
    /// state with `require_app_auth: false` and has no auth ingress, so on
    /// that binary only the acknowledgement can open the gate.
    pub fn for_gateway(allow_remote: bool, decisions: &RuntimeDecisionState) -> Self {
        Self {
            allow_remote: allow_remote || ConsoleAuthPosture::of(decisions).is_enforced(),
        }
    }
}

/// Which gateway binary is speaking, for refusals and log lines that name
/// their surface. The set is closed (the two bundled binaries) and `Display`
/// is the binary name, so a call site cannot misspell the surface into an
/// operator-facing message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatewaySurface {
    /// The SDK stdin JSON-RPC gateway.
    RpcGateway,
    /// The standalone console/HTTP gateway.
    MetaGateway,
}

impl GatewaySurface {
    /// The binary name as it appears in messages and log lines.
    pub const fn name(self) -> &'static str {
        match self {
            Self::RpcGateway => "rpc_gateway",
            Self::MetaGateway => "meta_gateway",
        }
    }
}

impl std::fmt::Display for GatewaySurface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// How the console on a gateway listener treats its callers, classified once
/// from the decision state the console will serve with.
///
/// One three-state fact with one classifier, consumed by the bind gate
/// ([`HttpBindPolicy::for_gateway`] opens on `Enforced` alone), by
/// [`warn_on_non_loopback_bind`] (which names the posture) and by
/// `rpc_gateway`'s startup warning (which fires on `ClosedToEveryCaller`).
/// A `require_app_auth` console with an empty JWKS protects the listener by
/// refusing everyone, which is not the same as authenticating it, and is
/// deliberately not enough to open the bind gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleAuthPosture {
    /// `require_app_auth` is false: every request is admitted.
    Open,
    /// `require_app_auth` is true but the trusted JWKS carries no key, so no
    /// bearer token can verify and every request is refused with 401. This is
    /// what an SDK launch gets with neither `runtime_options.auth_config` nor
    /// `console_require_app_auth = false`.
    ClosedToEveryCaller,
    /// `require_app_auth` is true and the trusted JWKS carries at least one
    /// key: the console authenticates its callers on its own.
    Enforced,
}

impl ConsoleAuthPosture {
    /// Classify `decisions`. The trusted JWKS is parsed here and nowhere else.
    pub fn of(decisions: &RuntimeDecisionState) -> Self {
        if !decisions.console.require_app_auth {
            return Self::Open;
        }
        if crate::parse_jwks_json(&decisions.trusted_oidc.jwks_json).is_ok() {
            Self::Enforced
        } else {
            Self::ClosedToEveryCaller
        }
    }

    /// Whether the console authenticates its callers on its own: the only
    /// posture that opens the bind gate without `allow_remote`.
    pub const fn is_enforced(self) -> bool {
        matches!(self, Self::Enforced)
    }
}

/// Why a requested HTTP listen address was refused before the listener opened.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum HttpBindPolicyError {
    /// The address is not loopback and neither the acknowledgement nor
    /// enforced console auth permits the exposure.
    RemoteBindRequiresExplicitAllow {
        surface: GatewaySurface,
        listen: SocketAddr,
    },
}

impl std::fmt::Display for HttpBindPolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RemoteBindRequiresExplicitAllow { surface, listen } => write!(
                f,
                "{surface} HTTP listen address `{listen}` is not loopback. Meta binds 127.0.0.1 \
                 unless the console enforces app auth (auth_config with a trusted signing key) or \
                 the launch acknowledges the exposure with allow_remote \
                 (runtime_options.allow_remote = true on rpc_gateway; the allow_remote init param, \
                 --allow-remote, or META_HTTP_ALLOW_REMOTE=1 on meta_gateway). allow_remote is \
                 an exposure acknowledgement, not an auth mechanism: pass it only with an \
                 authenticating proxy in front of this listener"
            ),
        }
    }
}

impl std::error::Error for HttpBindPolicyError {}

/// Enforce a loopback-only HTTP listener unless `policy` permits remote.
///
/// Pure and synchronous so both binaries can run it before bootstrap and a
/// refusal costs nothing but the init reply.
pub fn validate_http_bind_policy(
    surface: GatewaySurface,
    listen: SocketAddr,
    policy: HttpBindPolicy,
) -> Result<(), HttpBindPolicyError> {
    if policy.allow_remote || listen.ip().is_loopback() {
        return Ok(());
    }
    Err(HttpBindPolicyError::RemoteBindRequiresExplicitAllow { surface, listen })
}

/// The loud warning every non-loopback bind logs, whatever opened the gate.
/// Mirrors `meta_flow_editor`'s `--listen` warning: an operator must not be
/// able to expose the console, JSON-RPC, blob, SSE and live routes to a
/// network without a line in the log saying so. Emitted at WARN so it passes
/// the default `warn,meerkat_meta=info` filter on both binaries.
pub fn warn_on_non_loopback_bind(
    surface: GatewaySurface,
    bound: SocketAddr,
    decisions: &RuntimeDecisionState,
) {
    if bound.ip().is_loopback() {
        return;
    }
    let console_auth = match ConsoleAuthPosture::of(decisions) {
        ConsoleAuthPosture::Enforced => "console app auth is enforced",
        ConsoleAuthPosture::ClosedToEveryCaller => {
            "the console requires app auth but trusts no key (refuses every caller)"
        }
        ConsoleAuthPosture::Open => "the console is OPEN (no app auth)",
    };
    tracing::warn!(
        %bound,
        "{surface} HTTP listener is bound to a NON-LOOPBACK address; {console_auth}; every route \
         on this listener (console, JSON-RPC, blobs, SSE, live) is reachable by anyone who can \
         reach {bound}. Bind 127.0.0.1 (the default) unless an authenticating proxy fronts this \
         listener"
    );
}

/// Why an `http_listen` or `http_public_base_url` declaration was refused
/// before anything was bound. Every caller formats it into an init reply or a
/// launch error with `Display`; the variants exist so the shape is matchable
/// and the texts have one owner.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum HttpExposureParseError {
    /// The listen value is not `HOST:PORT` with an IP literal.
    ListenAddress {
        value: String,
        source: std::net::AddrParseError,
    },
    /// `--http-listen` was the last argument.
    ListenFlagMissingValue,
    /// `--http-listen` carried a value that is not `HOST:PORT`.
    ListenFlagAddress {
        value: String,
        source: std::net::AddrParseError,
    },
    /// The advertised base is not an absolute `http://` or `https://` URL.
    PublicBaseUrlNotAbsoluteHttp { value: String },
    /// The advertised base is a bare scheme with no host.
    PublicBaseUrlNoHost { value: String },
}

impl std::fmt::Display for HttpExposureParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fn listen_address(
            f: &mut std::fmt::Formatter<'_>,
            value: &str,
            source: &std::net::AddrParseError,
        ) -> std::fmt::Result {
            write!(
                f,
                "`{value}` is not a HOST:PORT socket address ({source}); use an IP literal such \
                 as 127.0.0.1:0 or 0.0.0.0:8080"
            )
        }
        match self {
            Self::ListenAddress { value, source } => listen_address(f, value, source),
            Self::ListenFlagMissingValue => f.write_str(
                "--http-listen requires an address (HOST:PORT, e.g. 127.0.0.1:8080 or 0.0.0.0:8080)",
            ),
            Self::ListenFlagAddress { value, source } => {
                f.write_str("--http-listen: ")?;
                listen_address(f, value, source)
            }
            Self::PublicBaseUrlNotAbsoluteHttp { value } => write!(
                f,
                "`{value}` must be an absolute http:// or https:// URL, e.g. https://mob.example.com"
            ),
            Self::PublicBaseUrlNoHost { value } => write!(f, "`{value}` names no host"),
        }
    }
}

impl std::error::Error for HttpExposureParseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ListenAddress { source, .. } | Self::ListenFlagAddress { source, .. } => {
                Some(source)
            }
            Self::ListenFlagMissingValue
            | Self::PublicBaseUrlNotAbsoluteHttp { .. }
            | Self::PublicBaseUrlNoHost { .. } => None,
        }
    }
}

/// Parse a `HOST:PORT` HTTP listen address. IP literals only (`127.0.0.1:0`,
/// `0.0.0.0:8080`, `[::]:8080`), the same rule as `meta_flow_editor
/// --listen`; no DNS resolution, so a hostname is a typed refusal here rather
/// than a bind that resolves differently on the next host.
pub fn parse_http_listen_addr(value: &str) -> Result<SocketAddr, HttpExposureParseError> {
    value
        .trim()
        .parse::<SocketAddr>()
        .map_err(|source| HttpExposureParseError::ListenAddress {
            value: value.to_string(),
            source,
        })
}

/// Parse the proxy-facing base URL a launch advertises. It must be an absolute
/// `http://` or `https://` URL; a trailing `/` is dropped so `{base}/console`
/// concatenation matches the local `http_base_url`.
pub fn parse_http_public_base_url(value: &str) -> Result<String, HttpExposureParseError> {
    let trimmed = value.trim();
    if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
        return Err(HttpExposureParseError::PublicBaseUrlNotAbsoluteHttp {
            value: value.to_string(),
        });
    }
    let base = trimmed.trim_end_matches('/');
    if base == "http:" || base == "https:" {
        return Err(HttpExposureParseError::PublicBaseUrlNoHost {
            value: value.to_string(),
        });
    }
    Ok(base.to_string())
}

/// Extract and validate the optional `--http-listen HOST:PORT` flag, the
/// sibling of [`parse_control_listen_arg`] for the HTTP listener. Same
/// argv convention: `args` is argv without the program name.
pub fn parse_http_listen_arg(
    args: &[String],
) -> Result<Option<SocketAddr>, HttpExposureParseError> {
    let Some(position) = args.iter().position(|arg| arg == "--http-listen") else {
        return Ok(None);
    };
    let Some(value) = args.get(position + 1) else {
        return Err(HttpExposureParseError::ListenFlagMissingValue);
    };
    value
        .trim()
        .parse::<SocketAddr>()
        .map(Some)
        .map_err(|source| HttpExposureParseError::ListenFlagAddress {
            value: value.clone(),
            source,
        })
}

pub struct GatewayHttpServer {
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    /// Flipped as the HTTP drain begins ([`Self::shutdown`]): the event
    /// streams the server serves end on it.
    streams_end_tx: tokio::sync::watch::Sender<bool>,
    task: Option<tokio::task::JoinHandle<std::io::Result<()>>>,
}

#[derive(Debug)]
pub enum GatewayHttpDrainOutcome {
    Completed(std::io::Result<()>),
    TimedOut,
    JoinFailed(String),
}

impl GatewayHttpServer {
    /// Stop accepting connections. Requests and streams already open
    /// continue until [`Self::shutdown`] joins them within its drain bound.
    /// Idempotent.
    pub fn stop_accepting(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    /// Stop admission and join the actual HTTP task. This is shared by normal
    /// active shutdown and failures during later gateway composition.
    pub async fn shutdown(mut self) -> GatewayHttpDrainOutcome {
        self.stop_accepting();
        // The drain begins: the open event streams end, so it can join them.
        self.streams_end_tx.send_replace(true);
        if let Some(mut task) = self.task.take() {
            match tokio::time::timeout(GATEWAY_HTTP_DRAIN_TIMEOUT, &mut task).await {
                Ok(Ok(result)) => GatewayHttpDrainOutcome::Completed(result),
                Ok(Err(error)) => GatewayHttpDrainOutcome::JoinFailed(error.to_string()),
                Err(_) => {
                    task.abort();
                    let _ = task.await;
                    GatewayHttpDrainOutcome::TimedOut
                }
            }
        } else {
            GatewayHttpDrainOutcome::Completed(Ok(()))
        }
    }

    /// Wait for an unexpected server exit. Cancellation-safe: when used in a
    /// `select!` and another branch wins, the owned task remains available to
    /// the ordered shutdown path.
    pub async fn wait(&mut self) -> GatewayHttpDrainOutcome {
        let Some(task) = self.task.as_mut() else {
            return GatewayHttpDrainOutcome::Completed(Ok(()));
        };
        let outcome = match task.await {
            Ok(result) => GatewayHttpDrainOutcome::Completed(result),
            Err(error) => GatewayHttpDrainOutcome::JoinFailed(error.to_string()),
        };
        self.task = None;
        outcome
    }
}

pub struct GatewayShutdownOutcome<Cleanup> {
    pub http: GatewayHttpDrainOutcome,
    pub runtime: GatewayRuntimeShutdown,
    pub cleanup: Cleanup,
}

/// What a gateway saw of its runtime's shutdown.
#[derive(Debug)]
pub struct GatewayRuntimeShutdown {
    /// The runtime's own report; `None` when the horizon cut the observation
    /// off (or the task failed): cleanup is then NOT attested.
    pub report: Option<UnifiedRuntimeShutdownReport>,
    /// The member MCP settlement at the gateway's executor-exit boundary. It
    /// runs after the observation ends, whatever ended it, so every member MCP
    /// close the runtime admitted finishes its native attempt before the
    /// gateway answers and its executor is destroyed.
    pub member_mcp_at_exit: crate::MemberMcpSettlement,
}

async fn ordered_shutdown_tail<
    BeforeRuntime,
    BeforeFuture,
    RuntimeFuture,
    RuntimeOutput,
    Cleanup,
    CleanupFuture,
    CleanupOutput,
>(
    before_runtime: BeforeRuntime,
    runtime_shutdown: RuntimeFuture,
    cleanup: Cleanup,
) -> (RuntimeOutput, CleanupOutput)
where
    BeforeRuntime: FnOnce() -> BeforeFuture,
    BeforeFuture: std::future::Future<Output = ()>,
    RuntimeFuture: std::future::Future<Output = RuntimeOutput>,
    Cleanup: FnOnce() -> CleanupFuture,
    CleanupFuture: std::future::Future<Output = CleanupOutput>,
{
    before_runtime().await;
    let runtime = runtime_shutdown.await;
    let cleanup = cleanup().await;
    (runtime, cleanup)
}

impl GatewayComposition<ActiveGateway> {
    /// One authoritative shutdown order for both gateway profiles:
    ///
    /// 1. stop HTTP admission and bound its outer-handler drain;
    /// 2. run the profile's pre-runtime hook, then stop and join the shared
    ///    event-drain task;
    /// 3. cooperatively await the runtime's authority cleanup within the
    ///    established SDK horizon;
    /// 4. only then run profile cleanup (for example registry removal).
    pub async fn shutdown<BeforeRuntime, BeforeFuture, Cleanup, CleanupFuture, CleanupOutput>(
        &self,
        server: GatewayHttpServer,
        before_runtime: BeforeRuntime,
        cleanup: Cleanup,
    ) -> GatewayShutdownOutcome<CleanupOutput>
    where
        BeforeRuntime: FnOnce() -> BeforeFuture,
        BeforeFuture: std::future::Future<Output = ()>,
        Cleanup: FnOnce() -> CleanupFuture,
        CleanupFuture: std::future::Future<Output = CleanupOutput>,
    {
        let http = server.shutdown().await;

        let runtime_shutdown = shutdown_gateway_runtime(Arc::clone(&self.state.runtime));
        let (runtime, cleanup) = ordered_shutdown_tail(
            || async {
                before_runtime().await;
                self.state.stop_event_drain().await;
            },
            runtime_shutdown,
            cleanup,
        )
        .await;
        GatewayShutdownOutcome {
            http,
            runtime,
            cleanup,
        }
    }
}

/// Observe the runtime's authority cleanup within the established SDK horizon,
/// then hold the gateway's executor-exit boundary for its member MCP closes.
///
/// The entered `UnifiedRuntime::shutdown` runs in its own task, which owns a
/// runtime reference, so the horizon bounds only this observation: at the
/// horizon the shutdown is not cancelled here, and it keeps running with its
/// cleanup owners (module processes included) until it completes or the
/// process exits. A gateway process exits after its shutdown answer, so a
/// report of `None`, meaning the horizon elapsed (or the task failed), attests
/// nothing. The horizon never discards a member MCP close still in its native
/// attempt: the member MCP settlement runs again here, after the observation
/// and before the gateway answers, and waits for every close's native
/// outcome, past a deadline Meerkat broke if it must, so the executor this
/// returns to owns no pending member MCP close. As long as Meerkat keeps its
/// own close bound, that takes at most
/// [`MEMBER_MCP_SETTLEMENT_BUDGET`](crate::MEMBER_MCP_SETTLEMENT_BUDGET).
pub(crate) async fn shutdown_gateway_runtime(
    runtime: Arc<UnifiedRuntime>,
) -> GatewayRuntimeShutdown {
    shutdown_gateway_runtime_within(runtime, GATEWAY_RUNTIME_SHUTDOWN_TIMEOUT).await
}

async fn shutdown_gateway_runtime_within(
    runtime: Arc<UnifiedRuntime>,
    horizon: Duration,
) -> GatewayRuntimeShutdown {
    let shutdown = tokio::spawn({
        let runtime = Arc::clone(&runtime);
        async move { runtime.shutdown().await }
    });
    let report = observe_runtime_shutdown(shutdown, horizon).await;
    let member_mcp_at_exit = runtime.settle_member_mcp_to_exit().await;
    GatewayRuntimeShutdown {
        report,
        member_mcp_at_exit,
    }
}

async fn observe_runtime_shutdown(
    shutdown: tokio::task::JoinHandle<UnifiedRuntimeShutdownReport>,
    horizon: Duration,
) -> Option<UnifiedRuntimeShutdownReport> {
    match tokio::time::timeout(horizon, shutdown).await {
        Ok(Err(error)) => {
            tracing::warn!(%error, "gateway runtime shutdown task failed; cleanup not attested");
            None
        }
        Ok(Ok(report)) => {
            if !report.cleanup_completed() {
                tracing::warn!(
                    drain_timed_out = report.drain.timed_out,
                    mob_stop = ?report.mob_stop,
                    identity_authority_release = ?report.identity_authority_release,
                    orphan_processes = report.module_shutdown.orphan_processes,
                    "gateway runtime shutdown completed without cleanup attestation"
                );
            }
            Some(report)
        }
        Err(_) => {
            tracing::warn!(
                timeout_ms = horizon.as_millis(),
                "gateway runtime shutdown exceeded its bounded horizon; the entered shutdown \
                 keeps running only until the process exits, and cleanup is not attested"
            );
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Boot preamble
// ---------------------------------------------------------------------------

/// Worker stack size for gateway tokio runtimes.
///
/// Meerkat's generated machine-authority apply path needs deep worker stacks;
/// this mirrors meerkat-rpc's explicit 16 MiB tokio worker sizing. Both
/// gateway binaries had this constant inline and identical.
const GATEWAY_WORKER_STACK_BYTES: usize = 16 * 1024 * 1024;

/// Build the multi-threaded tokio runtime both gateway binaries run on.
///
/// Its worker count is meerkat's `RKAT_WORKER_THREADS`, then Tokio's
/// `TOKIO_WORKER_THREADS`, as
/// [`meerkat_runtime::host_stack::worker_threads_from_env`] validates them;
/// with neither set, Tokio starts one worker per available CPU. A value that
/// is not a whole number of at least 1 is refused as an `InvalidInput` error
/// naming the variable, before any worker starts.
///
/// The error is returned rather than handled: the two binaries report a
/// build failure differently on purpose - `meta_gateway` emits a JSON-RPC
/// init error on stdout (its parent is an SDK reading the handshake),
/// `rpc_gateway` writes to stderr. Only the builder itself is shared.
pub fn gateway_tokio_runtime() -> std::io::Result<tokio::runtime::Runtime> {
    let worker_threads = meerkat_runtime::host_stack::worker_threads_from_env()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    gateway_tokio_runtime_with(worker_threads)
}

/// [`gateway_tokio_runtime`] with its worker count already resolved; `None`
/// keeps Tokio's one worker per available CPU.
fn gateway_tokio_runtime_with(
    worker_threads: Option<std::num::NonZeroUsize>,
) -> std::io::Result<tokio::runtime::Runtime> {
    let mut builder = tokio::runtime::Builder::new_multi_thread();
    builder
        .enable_all()
        .thread_stack_size(GATEWAY_WORKER_STACK_BYTES);
    if let Some(workers) = worker_threads {
        builder.worker_threads(workers.get());
    }
    builder.build()
}

/// Default tracing filter for a gateway binary when `RUST_LOG` is unset:
/// this crate's own targets at INFO, the binary's own target at INFO,
/// dependencies at WARN.
///
/// The binary target is a parameter and NOT a shared constant. Collapsing
/// the two filters into one string would silence whichever binary lost its
/// target, and the operationally significant boot phases (the one-time
/// head-canonical conversion, continuity repair, storage-verb migration
/// progress) report at INFO. A 2026-07 production deploy was aborted because
/// a supervisor read a silent-but-working migration as a hang.
pub fn default_tracing_filter(binary_target: &str) -> String {
    format!("warn,meerkat_meta=info,{binary_target}=info")
}

/// Install the gateway tracing subscriber on stderr, then the panic hook.
///
/// Stderr, never stdout: stdout carries the init JSON handshake and the
/// storage verbs' report output. `RUST_LOG` overrides the default filter.
///
/// The panic hook goes in here rather than in each binary's `main` so the
/// two gateways cannot drift on it, and so a panic is reported through the
/// same subscriber, with the same timestamps and filter, as the run-loop exit
/// line the binaries log. See [`install_gateway_panic_hook`].
pub fn init_gateway_tracing(binary_target: &str) {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new(default_tracing_filter(binary_target))
            }),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();
    install_gateway_panic_hook();
}

/// Report every panic through `tracing::error!` before the previously
/// installed hook (normally the default one) prints its own message.
///
/// A panic on a tokio worker unwinds one task; the runtime and the process
/// keep going, and the only trace is the default hook's plain-text line on
/// stderr: no timestamp, no target, and gone if stderr was redirected away
/// from the log stream. The panic in that case surfaces days later as "a
/// member stopped answering" with nothing to grep for. Routing it through
/// tracing puts the thread, source location and payload in the log stream
/// itself, at ERROR so no sane filter drops it, next to the line that names
/// which `select!` branch ended the gateway. Together they let an operator
/// tell a crash-shaped exit from a supervisor's SIGTERM from a stdin EOF,
/// which one report of "the gateway process exited, all endpoints 000" could
/// not.
///
/// The previous hook still runs afterwards, so `RUST_BACKTRACE` keeps
/// working and a host that installed its own hook before calling
/// [`init_gateway_tracing`] keeps it. Re-entrancy is safe: tracing drops an
/// event emitted from inside a subscriber callback rather than recursing, so
/// a panic raised while formatting a log line does not abort the process
/// from inside this hook.
pub fn install_gateway_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log_panic(info);
        previous(info);
    }));
}

/// The tracing half of the panic hook, separate so the log shape is one
/// place. Fields: `thread`, `file`, `line`, `column`, `payload`.
fn log_panic(info: &std::panic::PanicHookInfo<'_>) {
    let payload = panic_payload_text(info.payload());
    let current = std::thread::current();
    let thread = current.name().unwrap_or("<unnamed>");
    match info.location() {
        Some(location) => tracing::error!(
            thread,
            file = location.file(),
            line = location.line(),
            column = location.column(),
            payload,
            "panic"
        ),
        None => tracing::error!(thread, payload, "panic (location unavailable)"),
    }
}

/// `panic!("literal")` carries a `&str`, `panic!("{x}")` carries a `String`;
/// anything else (`panic_any`) has no text to show.
fn panic_payload_text(payload: &(dyn std::any::Any + Send)) -> &str {
    if let Some(text) = payload.downcast_ref::<&str>() {
        text
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.as_str()
    } else {
        "<non-string panic payload>"
    }
}

/// Extract and validate the optional `--control-listen <addr>` flag.
///
/// `args` is argv WITHOUT the program name at both call sites, arrived at
/// two different ways: `meta_gateway` collects `std::env::args().skip(1)`,
/// `rpc_gateway` collects the whole argv and passes `&args[1..]`. The scan is
/// by exact flag match, so the distinction would not matter anyway, but it is
/// stated because it did not hold when this doc was first written.
pub fn parse_control_listen_arg(args: &[String]) -> Result<Option<ControlListenAddr>, String> {
    let Some(position) = args.iter().position(|arg| arg == "--control-listen") else {
        return Ok(None);
    };
    let Some(value) = args.get(position + 1) else {
        return Err(
            "--control-listen requires an address (tcp://host:port or uds:///path)".to_string(),
        );
    };
    ControlListenAddr::parse(value)
        .map(Some)
        .map_err(|error| format!("--control-listen: {error}"))
}

/// `--write-mcp-apps-sandbox <dir>`: write the MCP Apps sandbox proxy this
/// binary embeds ([`crate::mcp_apps_sandbox`]) into `<dir>` and print each
/// written file as `<sha256>  <path>` on stdout. `None` when the flag is
/// absent; otherwise the process exit code: `0` after writing, `1` when a
/// file could not be written, `2` when the directory is missing. Both
/// gateways handle it before anything else, so it reads no init request and
/// starts no runtime. Same argv convention as [`parse_control_listen_arg`].
pub fn write_mcp_apps_sandbox_arg(args: &[String]) -> Option<i32> {
    use sha2::Digest as _;
    let position = args
        .iter()
        .position(|arg| arg == "--write-mcp-apps-sandbox")?;
    let Some(dir) = args
        .get(position + 1)
        .filter(|value| !value.starts_with("--"))
    else {
        eprintln!("--write-mcp-apps-sandbox requires a directory");
        return Some(2);
    };
    let written = match crate::mcp_apps_sandbox::write_mcp_apps_sandbox(Path::new(dir)) {
        Ok(written) => written,
        Err(error) => {
            eprintln!(
                "--write-mcp-apps-sandbox: cannot write the MCP Apps sandbox to {dir}: {error}"
            );
            return Some(1);
        }
    };
    for path in written {
        match std::fs::read(&path) {
            Ok(bytes) => println!("{:x}  {}", sha2::Sha256::digest(&bytes), path.display()),
            Err(error) => {
                eprintln!(
                    "--write-mcp-apps-sandbox: cannot read back {}: {error}",
                    path.display()
                );
                return Some(1);
            }
        }
    }
    Some(0)
}

// ---------------------------------------------------------------------------
// Host configuration ingress
// ---------------------------------------------------------------------------

/// A top-level `mob.toml` table that belongs to meerkat's HOST config
/// (`config.toml`), not to the mob definition.
///
/// `MobDefinition::from_toml` deserializes without `deny_unknown_fields`, so a
/// `[self_hosted]` or `[realm]` table written into `mob.toml` was dropped in
/// silence, and the first self-hosted member then died at build time with
/// "self-hosted model '..' is not registered in config", two layers away from
/// the file that caused it. Both gateways refuse the table at init instead
/// and name the option that carries the host config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum HostConfigTableInMobToml {
    /// `[self_hosted]`: serving endpoints and model aliases.
    SelfHosted,
    /// `[realm]`: backend, auth and binding profiles.
    Realm,
}

impl HostConfigTableInMobToml {
    /// Every table this check refuses, in the order it reports them.
    pub const ALL: [Self; 2] = [Self::SelfHosted, Self::Realm];

    /// The table name as written in TOML.
    pub fn table(self) -> &'static str {
        match self {
            Self::SelfHosted => "self_hosted",
            Self::Realm => "realm",
        }
    }
}

impl std::fmt::Display for HostConfigTableInMobToml {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "mob.toml declares a top-level [{table}] table, which is meerkat host configuration \
             that the mob definition parser ignores; move it into the meerkat config.toml and \
             point the gateway at that file: runtime_options.meerkat_config_path on rpc_gateway, \
             or the meerkat_config_path init param (default <workspace>/.rkat/config.toml) on \
             meta_gateway",
            table = self.table()
        )
    }
}

impl std::error::Error for HostConfigTableInMobToml {}

/// Refuse `[self_hosted]` and `[realm]` at the top level of a `mob.toml`
/// document.
///
/// Runs on the RAW text, because by the time a typed `MobDefinition` exists
/// the tables are already gone. Text that is not valid TOML is accepted here:
/// `MobDefinition::from_toml` owns that verdict and reports it with its own
/// message, and both gateways run it first.
pub fn refuse_host_config_tables_in_mob_toml(
    mob_toml: &str,
) -> Result<(), HostConfigTableInMobToml> {
    let Ok(toml::Value::Table(document)) = toml::from_str::<toml::Value>(mob_toml) else {
        return Ok(());
    };
    match HostConfigTableInMobToml::ALL
        .into_iter()
        .find(|table| document.contains_key(table.table()))
    {
        Some(table) => Err(table),
        None => Ok(()),
    }
}

/// Describe a `MobDefinition::from_toml` failure for the operator who wrote
/// the `mob.toml`, adding what Meerkat's own message cannot know about an
/// upgrade from MobKit.
///
/// Meerkat refuses a profile prompt key (`system_prompt`, `prompt`,
/// `instructions`) because no profile field carries it, and its message names
/// where the member prompt lives. MobKit 0.8.31 and earlier dropped such a key
/// without a warning, so an upgraded config that kept it now stops at init
/// although the key never did anything. The description says so: removing
/// the key keeps the behaviour the deployment had. Every other failure keeps
/// Meerkat's message unchanged.
pub fn describe_mob_definition_error(error: &meerkat_mob::MobError) -> String {
    match error {
        meerkat_mob::MobError::UnsupportedProfileKey { profile, key } => format!(
            "{error}. MobKit 0.8.31 and earlier dropped '{key}' without a warning, so the \
             key had no effect on the members of profile '{profile}', and removing the key \
             keeps that behaviour. To set their prompt, list a skill in the profile \
             (skills = [\"<id>\"] under [profiles.{profile}]) and define it as a [skills.<id>] \
             table with source = \"inline\" and content = \"...\""
        ),
        other => other.to_string(),
    }
}

/// Why a meerkat host `config.toml` could not become a gateway's agent
/// config. Both binaries refuse init on it: an operator who pointed the
/// gateway at a file expects that file in force, and booting from
/// `Config::default()` instead would resurface as an unexplained
/// "self-hosted model is not registered in config" at the first member build.
#[derive(Debug)]
#[non_exhaustive]
pub enum GatewayHostConfigError {
    /// The file could not be read.
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The file is not a valid meerkat config document.
    Parse {
        path: PathBuf,
        source: meerkat::ConfigError,
    },
    /// The file parsed but fails meerkat's own `Config::validate` against the
    /// canonical catalog: a `[self_hosted.models.<id>]` alias naming a server
    /// that is not declared, a `[models.<id>]` row colliding with a catalog
    /// id, a zero compaction threshold. These are exactly the errors `rkat`
    /// and `rkat-rpc` refuse at load and that would otherwise surface at the
    /// first member build.
    Invalid {
        path: PathBuf,
        source: meerkat::ConfigError,
    },
}

impl std::fmt::Display for GatewayHostConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read { path, source } => {
                write!(
                    f,
                    "failed to read meerkat config {}: {source}",
                    path.display()
                )
            }
            Self::Parse { path, source } => {
                write!(f, "meerkat config {} is invalid: {source}", path.display())
            }
            Self::Invalid { path, source } => {
                write!(
                    f,
                    "meerkat config {} does not validate: {source}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for GatewayHostConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Parse { source, .. } | Self::Invalid { source, .. } => Some(source),
        }
    }
}

/// Load a meerkat host `config.toml` as the base config for every agent a
/// gateway builds.
///
/// One explicit file, merged over `Config::default()` with meerkat's own
/// file-merge semantics (`Config::merge_toml_str`, the step `Config::load`
/// applies to a discovered `.rkat/config.toml`) and then validated with
/// meerkat's own `Config::validate` against the canonical model catalog, the
/// step `rkat` and `rkat-rpc` run after every load. No directory walk and no
/// home-directory fallback: the gateway is handed a path, so what it loads is
/// exactly what the operator named. This is what carries `[self_hosted]`,
/// `[realm]` and `[models]` to `FactoryAgentBuilder`; the caller layers the
/// gateway's own overlays (comms transport, compaction policy) on top.
pub fn load_gateway_host_config(path: &Path) -> Result<meerkat::Config, GatewayHostConfigError> {
    let text = std::fs::read_to_string(path).map_err(|source| GatewayHostConfigError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let mut config = meerkat::Config::default();
    let warnings = config
        .merge_toml_str_with_warnings(&text)
        .map_err(|source| GatewayHostConfigError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
    // A persisted legacy shape (a pre-0.8.37 `[model_fallback]` table) loads
    // as no policy, with fallback off; say which file carries it.
    for warning in warnings {
        tracing::warn!(path = %path.display(), %warning, "gateway host config loaded with a warning");
    }
    // Validation is the gateway ingress being at least as strict as meerkat's
    // own: a dangling `[self_hosted.models.<id>] server = ".."` reference
    // passes the parse and would otherwise kill the first member build.
    config
        .validate(meerkat_models::canonical())
        .map_err(|source| GatewayHostConfigError::Invalid {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(config)
}

// ---------------------------------------------------------------------------
// Provider credentials root
// ---------------------------------------------------------------------------

/// Check the provider credentials root a gateway launch names: the
/// credentials directory itself, as `<config dir>/meerkat/credentials` is
/// the process default's. It must be a non-empty absolute path; a relative
/// one would resolve against whatever directory the gateway runs in.
pub fn validate_provider_credentials_root(root: &Path) -> Result<(), String> {
    if root.as_os_str().is_empty() {
        return Err("the provider credentials root must not be empty".to_string());
    }
    if !root.is_absolute() {
        return Err(format!(
            "the provider credentials root must be an absolute path, got {}",
            root.display()
        ));
    }
    Ok(())
}

/// Open the provider credential store at a gateway launch's credentials root
/// (see [`validate_provider_credentials_root`]), creating the directory when
/// it is missing. Every agent factory of the launch holds a clone of the
/// returned store in place of the process default, so provider logins,
/// token refreshes and logouts read and write under the root only, with one
/// refresh authority. Any failure refuses the launch: a launch that names a
/// root never falls back to the process default.
pub fn open_provider_credentials_root(
    root: &Path,
) -> Result<meerkat::ProviderAuthPersistence, String> {
    validate_provider_credentials_root(root)?;
    std::fs::create_dir_all(root).map_err(|error| {
        format!(
            "cannot create the provider credentials root {}: {error}",
            root.display()
        )
    })?;
    meerkat::TokenStoreBackend::File {
        root: root.to_path_buf(),
    }
    .open_with_refresh_authority()
    .map_err(|error| {
        format!(
            "cannot open the provider credentials root {}: {error}",
            root.display()
        )
    })
}

// ---------------------------------------------------------------------------
// Schedule firing host
// ---------------------------------------------------------------------------

/// Whether a gateway's schedule host may take its store's firing authority.
///
/// A candidate launch (`runtime_options.mob_composition.authority =
/// "candidate"`) boots against the state its promoted, authoritative boot
/// adopts, or a copy of it. Its schedule host took the vacant executor lease
/// there and claimed or misfired the due occurrences: a claimed one fired its
/// target beside the authoritative gateway, and a misfired one was lost to
/// it. A candidate therefore runs no firing driver: it takes no executor
/// lease, plans, claims and misfires nothing, fires no host runnable, repairs
/// and creates no schedule, and its schedule tools refuse every change. Due
/// work stays pending for the authoritative boot, whose first tick applies
/// the stored policy exactly as before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScheduleFiringRole {
    /// The launch speaks for the composition and drives due schedule work.
    Executor,
    /// A candidate launch: no firing driver, no executor lease, no durable
    /// schedule writes.
    QuiescedCandidate,
}

impl ScheduleFiringRole {
    /// The role a launch's declared composition authority gives it. Only a
    /// launch that speaks for the composition executes schedules.
    #[must_use]
    pub(crate) fn for_composition_authority(authority: CompositionAuthority) -> Self {
        if authority.speaks_for_composition() {
            Self::Executor
        } else {
            Self::QuiescedCandidate
        }
    }

    /// The role of the launch `runtime` was bootstrapped for: the same
    /// declared composition authority, as the runtime keeps it.
    fn of_runtime(runtime: &UnifiedRuntime) -> Self {
        if runtime.speaks_for_composition() {
            Self::Executor
        } else {
            Self::QuiescedCandidate
        }
    }
}

/// Attach a gateway's state-dir schedule tools under its launch's firing role.
///
/// The tools land in `builder`'s default schedule-tools slot exactly as
/// [`crate::schedule_wiring::attach_schedule_tools_with_identity_targets_reporting`]
/// installs them. A [`ScheduleFiringRole::QuiescedCandidate`] quiesces their
/// write gate before this returns, so no member built or resumed against the
/// slot ever sees a candidate accept a schedule change: the gate's pending
/// state alone would still pass `pause` and `delete`.
/// [`spawn_gateway_schedule_host`] quiesces the same binding again.
pub(crate) fn attach_gateway_schedule_tools(
    builder: &meerkat::FactoryAgentBuilder,
    state_dir: &Path,
    firing_role: ScheduleFiringRole,
) -> Result<crate::schedule_wiring::AttachedScheduleTools, String> {
    let tools = crate::schedule_wiring::attach_schedule_tools_with_identity_targets_reporting(
        builder, state_dir,
    )?;
    if firing_role == ScheduleFiringRole::QuiescedCandidate {
        tools.firing_host_binding.quiesce_for_candidate();
    }
    Ok(tools)
}

/// Everything [`spawn_gateway_schedule_host`] consumes.
///
/// A struct rather than a long argument list because the two binaries built
/// their tuples in DIFFERENT field orders: `rpc_gateway` puts the
/// `MeerkatMachine` adapter fourth, right after the session service, while
/// `meta_gateway`'s `ScheduleHostInputs` alias has no adapter at all and a
/// later `.map` APPENDS it last. That is exactly the kind of divergence that
/// silently swaps two same-typed arguments.
pub struct GatewayScheduleHostInputs<B: SessionAgentBuilder + 'static> {
    pub schedule_service: ScheduleService,
    /// The CONCRETE persistent service: the runtime-backed firing host needs
    /// the typed form, not the erased `dyn MobSessionService` the bootstrap
    /// spec consumes.
    pub session_service: Arc<PersistentSessionService<B>>,
    pub runtime_adapter: Arc<MeerkatMachine>,
    pub schedule_store_path: PathBuf,
    pub firing_host_binding: ScheduleFiringHostBinding,
    /// Host-runnable targets. `rpc_gateway` composes the steward dream plus
    /// SDK-declared runnables here; `meta_gateway` has none.
    pub runnable_host: Option<Arc<dyn ScheduleRunnableHost>>,
    pub workgraph_service: Option<WorkGraphService>,
    pub owner_id: String,
}

/// Point the mob-target registry at the live mob state and repair persisted
/// resumable-session schedules to identity mob targets.
///
/// Split out from [`spawn_gateway_schedule_host`] deliberately:
/// `rpc_gateway` performs steward-dream registration BETWEEN this step and
/// the host spawn, and folding both into one call would have reordered a
/// durable-store boot sequence to no benefit. Returns the mob state so the
/// caller can hand it to the host spawn.
///
/// A runtime bootstrapped for a candidate launch (one that does not speak
/// for its composition) only points the registry at the mob state: the
/// repair rewrites stored schedules, which belongs to the authoritative boot.
pub async fn adopt_schedule_mob_targets(
    runtime: &UnifiedRuntime,
    schedule_service: &ScheduleService,
    mob_target_registry: &ScheduleMobTargetRegistry,
) -> Option<Arc<MobMcpState>> {
    let mob_state = runtime.mob_runtime().agent_mob_mcp_state();
    mob_target_registry.set_mob_state(mob_state.clone());
    if ScheduleFiringRole::of_runtime(runtime) == ScheduleFiringRole::QuiescedCandidate {
        return mob_state;
    }
    match crate::schedule_wiring::repair_resumable_session_targets_to_mob_members(
        schedule_service,
        mob_target_registry,
    )
    .await
    {
        Ok(repaired) if repaired > 0 => {
            tracing::info!(
                repaired,
                "repaired persisted resumable-session schedules to identity mob targets"
            );
        }
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(
                error = %error,
                "failed to repair persisted resumable-session schedules to identity mob targets",
            );
        }
    }
    mob_state
}

/// Spawn the resident claim watchdog and the runtime-backed schedule firing
/// host, bind the firing-intent write gate if the host came up, and report
/// one observed verdict about the pipeline.
///
/// Both gateways ran byte-identical versions of this block, including every
/// log string. The only behavioural difference was the runnable host, which
/// is now the named [`GatewayScheduleHostInputs::runnable_host`] field.
///
/// Ordering is load-bearing and preserved exactly: the watchdog contract is
/// logged BEFORE the watchdog is spawned, `firing_host_binding.bind()`
/// happens only AFTER the host is confirmed up (binding earlier would admit
/// firing-intent writes into a store nothing local drains), and the probe
/// runs only on the bound path.
///
/// A runtime bootstrapped for a candidate launch (one that does not speak for
/// its composition) spawns nothing: no firing host, so no executor lease, no
/// tick and no host runnable fired, and no claim watchdog, whose stall
/// verdicts would describe work the candidate must leave alone. It quiesces
/// the write gate and returns no host, with a watchdog handle whose task has
/// already ended.
pub async fn spawn_gateway_schedule_host<B: SessionAgentBuilder + 'static>(
    runtime: &UnifiedRuntime,
    mob_state: Option<Arc<MobMcpState>>,
    inputs: GatewayScheduleHostInputs<B>,
) -> (Option<ScheduleHostHandle>, tokio::task::JoinHandle<()>) {
    let GatewayScheduleHostInputs {
        schedule_service,
        session_service,
        runtime_adapter,
        schedule_store_path,
        firing_host_binding,
        runnable_host,
        workgraph_service,
        owner_id,
    } = inputs;
    if ScheduleFiringRole::of_runtime(runtime) == ScheduleFiringRole::QuiescedCandidate {
        firing_host_binding.quiesce_for_candidate();
        tracing::info!(
            "schedule firing quiesced for this candidate launch: no schedule firing host runs, so \
             no executor lease is taken and no occurrence is planned, claimed or misfired here; \
             due occurrences stay pending for the authoritative gateway, and schedule tool \
             changes are refused"
        );
        return (None, tokio::spawn(async {}));
    }
    // The firing driver discards its own tick errors upstream, so the claim
    // watchdog is the only thing that turns "everything stays pending
    // forever" into a row-level diagnosis.
    //
    // Stated as a RESIDENT LIVENESS CONTRACT rather than a promise about a
    // future log line: the handle returned below is held for the process
    // lifetime, and these are the numbers it holds the pipeline to. A reader
    // who greps the boot log now learns the cadence and the overdue
    // threshold instead of learning that a watchdog exists.
    let watchdog_config = ScheduleClaimWatchdogConfig::default();
    tracing::info!(
        poll_interval_secs = watchdog_config.poll_interval.as_secs(),
        overdue_threshold_secs = watchdog_config.overdue_threshold.as_secs(),
        heartbeat_polls = watchdog_config.heartbeat_polls,
        "schedule claim watchdog resident: probes the firing pipeline on this cadence, ERROR on \
         a new or changed stall report, WARN heartbeat while it persists, INFO on recovery"
    );
    let watchdog = crate::schedule_wiring::spawn_schedule_claim_watchdog(
        schedule_service.clone(),
        schedule_store_path.clone(),
        watchdog_config,
    );
    let schedule_service_for_probe = schedule_service.clone();
    let schedule_host = crate::schedule_wiring::spawn_schedule_host_with_identity_runtime(
        session_service,
        runtime_adapter,
        schedule_service,
        mob_state,
        runtime.mob_handle(),
        runtime.identity_runtime_ref().cloned(),
        runnable_host,
        workgraph_service,
        owner_id,
    );
    if schedule_host.is_some() {
        // The firing host now drains the store: firing-intent schedule writes
        // (create/update/resume) are admissible from here on (Bug C stopgap -
        // the gate refused them until this point).
        firing_host_binding.bind();
        // One probe NOW, with the host up, so the boot log carries an
        // observed verdict about the firing pipeline instead of a claim about
        // a watchdog that has not ticked yet. Deliberately WARN, not ERROR,
        // for a stall found here: a gateway that was down across a due time
        // legitimately restarts with a backlog the host has not drained yet.
        // The resident watchdog above is what escalates - if the same report
        // is still there on its cadence, it is a real stall and it says so at
        // ERROR.
        crate::schedule_wiring::probe_schedule_firing_pipeline(
            &schedule_service_for_probe,
            &schedule_store_path,
            watchdog_config.overdue_threshold,
        )
        .await
        .log_at_boot(watchdog_config);
    } else {
        // Present-state verdict, not a prediction: the host is NOT running,
        // and the write gate is therefore still closed. Both halves are
        // observed facts at this point in boot.
        tracing::warn!(
            "schedule host did not spawn over the attached schedule store: no firing driver is \
             running in this gateway, and the firing-intent write gate is consequently still \
             closed, so create/update/resume are being refused rather than accepted durably"
        );
        // The old tail of that sentence ("into a store nothing drains")
        // generalized a process-local fact to the whole store. meerkat 0.8.22
        // lets the store answer that question itself: the executor lease is
        // singular per realm store, so the holder (if any) is named rather
        // than guessed at. It is one instantaneous read, not a liveness
        // verdict - a peer mid-restart reads vacant, and a crashed
        // predecessor still reads held until its lease expires, so each arm
        // below says what it actually proves. Read only: the observation
        // carries no bearer token and cannot take firing authority away from
        // whoever holds it.
        match crate::schedule_wiring::observe_schedule_firing_authority(&schedule_service_for_probe)
            .await
        {
            // Bound as `holder_id`, not `owner_id`, for readability only -
            // shadowing the moved-from `owner_id` input would compile fine,
            // but this arm reports ANOTHER process's identity, and neither
            // original call site had a name that could be confused with it
            // in scope here (they used `schedule_owner_id` / `runtime_id`).
            // The log FIELD name is unchanged, so the envelope is identical.
            crate::schedule_wiring::ScheduleFiringAuthority::Held {
                owner_id: holder_id,
                fencing_token,
                expires_in_secs,
            } => tracing::warn!(
                executor_owner_id = %holder_id,
                fencing_token,
                lease_expires_in_secs = expires_in_secs,
                "the schedule store itself reports SOME process holding the realm's singular \
                 firing authority, so durable schedules may still be drained elsewhere; note the \
                 holder can also be this deployment's own crashed predecessor, whose lease stays \
                 live until it expires, so check the owner id and expiry above before concluding \
                 anything is actually draining. This gateway's firing-intent write gate stays \
                 closed either way: it gates on a LOCAL host"
            ),
            crate::schedule_wiring::ScheduleFiringAuthority::Vacant => tracing::warn!(
                "the schedule store itself reports its firing authority vacant AT THIS INSTANT: \
                 no process holds the executor lease. A peer gateway mid-restart is vacant only \
                 until its first tick, so this is proof of an unattended store only if it \
                 persists - the resident claim watchdog is what escalates once durable work \
                 actually goes unclaimed"
            ),
            crate::schedule_wiring::ScheduleFiringAuthority::Unobservable { detail } => {
                tracing::warn!(
                    %detail,
                    "the schedule store cannot report firing authority, so whether any other \
                     process drains this store is unknown from here"
                );
            }
        }
    }
    (schedule_host, watchdog)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn console_drain_test_composition(
        directory: &tempfile::TempDir,
        profile: GatewayCompatibilityProfile,
    ) -> Result<GatewayComposition<BootstrappedGateway>, Box<dyn std::error::Error>> {
        console_drain_test_composition_with(directory, profile, None).await
    }

    /// [`console_drain_test_composition`], its mob running `workgraph`.
    async fn console_drain_test_composition_with(
        directory: &tempfile::TempDir,
        profile: GatewayCompatibilityProfile,
        workgraph: Option<meerkat::WorkGraphService>,
    ) -> Result<GatewayComposition<BootstrappedGateway>, Box<dyn std::error::Error>> {
        let session_path = directory.path().join("sessions");
        std::fs::create_dir_all(&session_path)?;
        let service: Arc<dyn meerkat_mob::MobSessionService> =
            Arc::new(meerkat::build_ephemeral_service(
                meerkat::AgentFactory::new(&session_path),
                meerkat::Config::default(),
                1,
            ));
        // Every composition gets its own mob id. In-process `cargo test` runs
        // these tests concurrently in one process, and the comms participant
        // registry is process-wide, so a shared id fails the second bootstrap
        // with ParticipantNameOccupied (#480).
        let mob_id = format!("gateway-console-drain-{}", uuid::Uuid::new_v4().simple());
        // A mob spawns only members whose profile has comms on.
        let definition = meerkat_mob::MobDefinition::from_toml(&format!(
            "[mob]\nid = \"{mob_id}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\n\
             [profiles.worker.tools]\ncomms = true\n"
        ))?;
        let spec = MobBootstrapSpec::new(definition, meerkat_mob::MobStorage::in_memory(), service)
            .with_options(crate::mob_handle_runtime::MobBootstrapOptions {
                allow_ephemeral_sessions: true,
                notify_orchestrator_on_resume: true,
                default_llm_client: Some(Arc::new(meerkat_client::TestClient::for_provider(
                    meerkat_core::Provider::OpenAI,
                ))),
            })
            .with_workgraph_service(workgraph);
        let config = MetaConfig {
            modules: Vec::new(),
            discovery: crate::types::DiscoverySpec {
                namespace: mob_id.clone(),
                modules: Vec::new(),
            },
            pre_spawn: Vec::new(),
        };
        let plan = match profile {
            GatewayCompatibilityProfile::ConsoleHttp => {
                GatewayRuntimeBootstrapPlan::console_http(spec, config, Duration::from_secs(10))
            }
            GatewayCompatibilityProfile::StdioRpc => GatewayRuntimeBootstrapPlan::stdio_rpc(
                spec,
                config,
                Vec::new(),
                Duration::from_secs(10),
                RuntimeOptions::default(),
                Arc::new(InMemoryMetadataStore::new()),
            ),
        };
        Ok(GatewayComposition::prepare(profile, plan)
            .bootstrap()
            .await?)
    }

    fn console_drain_event(
        event_id: &str,
        event_type: &str,
        payload: serde_json::Value,
    ) -> EventEnvelope<UnifiedEvent> {
        EventEnvelope {
            event_id: event_id.to_string(),
            source: "gateway-console-drain".to_string(),
            timestamp_ms: 1,
            event: UnifiedEvent::Agent {
                agent_id: "rt:worker:1".to_string(),
                event_type: event_type.to_string(),
                payload: Some(payload),
            },
        }
    }

    async fn wait_for_console_drain_event(
        store: &crate::unified_runtime::ConsoleEventStore,
        event_id: &str,
        phase: Option<&str>,
    ) -> Result<
        Vec<crate::console_contracts::ConsoleIdentityEventEnvelope>,
        Box<dyn std::error::Error>,
    > {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let replay = store
                    .replay_all(None)
                    .await
                    .map_err(|error| format!("console replay failed: {error:?}"))?;
                if replay.iter().any(|event| event.event_id == event_id)
                    && store.response_phase_for_identity("worker").await.as_deref() == phase
                {
                    return Ok(replay);
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await?
    }

    /// Both shipped hosts serve their own axum router. Activation must own
    /// the runtime ingress drain or history can look complete while the
    /// console reservation remains waiting and prevents the next send.
    #[tokio::test]
    async fn gateway_activation_drains_console_events_for_both_profiles()
    -> Result<(), Box<dyn std::error::Error>> {
        use serde_json::json;

        for (profile, include_interaction) in [
            (GatewayCompatibilityProfile::ConsoleHttp, true),
            (GatewayCompatibilityProfile::ConsoleHttp, false),
            (GatewayCompatibilityProfile::StdioRpc, true),
            (GatewayCompatibilityProfile::StdioRpc, false),
        ] {
            let directory = tempfile::tempdir()?;
            let composition = console_drain_test_composition(&directory, profile).await?;
            let ingress = composition
                .runtime()
                .install_test_console_event_ingress()
                .await;
            let store = composition.runtime().console_events();
            let interaction = uuid::Uuid::from_u128(41).to_string();
            let expected_interaction = include_interaction.then_some(interaction.as_str());
            let run_id = uuid::Uuid::from_u128(42).to_string();
            let lineage = json!({
                "interaction_id": expected_interaction,
                "run_id": run_id,
            });
            // A run-only terminal clears its current run phase without
            // borrowing or claiming to consume this console reservation.
            composition
                .runtime()
                .reserve_identity_interaction(
                    "worker",
                    Some("rt:worker:1"),
                    &interaction,
                    "console",
                    json!("hello"),
                )
                .await?;
            assert_eq!(
                store.response_phase_for_identity("worker").await.as_deref(),
                Some("waiting"),
                "{profile:?} must establish the pending reservation"
            );

            // Queue a real forwarded envelope before activation. No helper
            // below calls drain_mob_agent_events or projects into the store.
            ingress
                .send(console_drain_event(
                    "gateway-run-start",
                    "run_started",
                    json!({
                        "identity": lineage,
                        "session_id": uuid::Uuid::from_u128(43).to_string(),
                        "input": { "kind": "content", "content": "hello" },
                    }),
                ))
                .await?;
            let composition = composition.activate();
            wait_for_console_drain_event(&store, "gateway-run-start", Some("waiting")).await?;

            ingress
                .send(console_drain_event(
                    "gateway-delta",
                    "text_delta",
                    json!({ "identity": lineage, "delta": "working" }),
                ))
                .await?;
            wait_for_console_drain_event(&store, "gateway-delta", Some("generating")).await?;
            ingress
                .send(console_drain_event(
                    "gateway-run-end",
                    "run_completed",
                    json!({ "identity": lineage, "result": "done" }),
                ))
                .await?;
            let replay = wait_for_console_drain_event(&store, "gateway-run-end", None).await?;
            let projected: Vec<_> = replay
                .iter()
                .filter(|event| event.identity == "worker")
                .collect();
            assert_eq!(
                projected
                    .iter()
                    .map(|event| event.event_type.as_str())
                    .collect::<Vec<_>>(),
                ["run_started", "text_delta", "interaction_complete"],
                "{profile:?} must retain ordered live replay"
            );
            assert!(
                projected.iter().all(|event| {
                    event.interaction_id.as_deref() == expected_interaction
                        && event.data["identity"]["interaction_id"].as_str() == expected_interaction
                        && event.data["identity"]["run_id"].as_str() == Some(run_id.as_str())
                        && event.data["run_id"].as_str() == Some(run_id.as_str())
                }),
                "{profile:?} include_interaction={include_interaction} must retain exact run attribution"
            );

            let drain_abort = composition
                .state
                .event_drain_task
                .lock()
                .await
                .as_ref()
                .ok_or("activated gateway has no event drain")?
                .abort_handle();
            assert!(!drain_abort.is_finished());
            let server = GatewayHttpBinding::bind_loopback()
                .await?
                .serve(Router::new());
            let shutdown = composition
                .shutdown(
                    server,
                    || async {
                        assert!(composition.state.event_drain_task.lock().await.is_some());
                        assert!(!drain_abort.is_finished());
                    },
                    || async {
                        assert!(composition.state.event_drain_task.lock().await.is_none());
                        assert!(drain_abort.is_finished());
                    },
                )
                .await;
            assert!(matches!(
                shutdown.http,
                GatewayHttpDrainOutcome::Completed(Ok(()))
            ));
            assert!(
                shutdown
                    .runtime
                    .report
                    .is_some_and(|report| report.cleanup_completed())
            );
            assert_eq!(
                shutdown.runtime.member_mcp_at_exit,
                crate::MemberMcpSettlement::NothingOwned
            );
            assert!(
                drain_abort.is_finished(),
                "{profile:?} must join the drain at shutdown"
            );
            assert!(composition.state.event_drain_task.lock().await.is_none());
        }
        Ok(())
    }

    /// An open Console with no auth requirement, as a local gateway serves
    /// it.
    fn open_console_decisions() -> crate::runtime::RuntimeDecisionState {
        crate::runtime::RuntimeDecisionState {
            bigquery: crate::decisions::BigQueryNaming {
                dataset: "gateway_stream_dataset".to_string(),
                table: "gateway_stream_table".to_string(),
            },
            modules: vec![],
            auth: crate::decisions::AuthPolicy::default(),
            trusted_oidc: crate::runtime::TrustedOidcRuntimeConfig {
                discovery_json: r#"{"issuer":"https://noop.example.com"}"#.to_string(),
                jwks_json: r#"{"keys":[]}"#.to_string(),
                audience: "gateway-stream-tests".to_string(),
                require_verified_email: false,
            },
            console: crate::decisions::ConsolePolicy {
                require_app_auth: false,
                ..crate::decisions::ConsolePolicy::default()
            },
            ops: crate::decisions::RuntimeOpsPolicy::default(),
            release_metadata: crate::decisions::ReleaseMetadata {
                targets: vec!["crates.io".to_string()],
                support_matrix: "lts".to_string(),
            },
        }
    }

    /// A Console attached to the gateway's event streams when it shuts down
    /// does not hold the shutdown up. The timeline and gating streams end as
    /// the shutdown begins, so the HTTP drain joins them well within its
    /// bound and the shutdown attests complete cleanup; a client that
    /// reconnects finds the listener closed. Such streams otherwise end only
    /// on an auth lapse, a closed channel or a client disconnect, so the
    /// drain ran out its bound and the gateway reported incomplete cleanup.
    #[tokio::test]
    async fn gateway_shutdown_ends_attached_console_streams_within_the_drain_bound()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let composition =
            console_drain_test_composition(&directory, GatewayCompatibilityProfile::ConsoleHttp)
                .await?;
        let app = composition
            .runtime()
            .build_reference_app_router(open_console_decisions());
        let composition = composition.activate();
        let binding = GatewayHttpBinding::bind_loopback().await?;
        let base = binding.http_base_url();
        let server = binding.serve(app);

        let client = reqwest::Client::new();
        let mut streams = Vec::new();
        for path in ["/console/timeline/stream", "/console/gating/stream"] {
            let mut stream = client.get(format!("{base}{path}")).send().await?;
            assert!(stream.status().is_success(), "{path}: {}", stream.status());
            let first = tokio::time::timeout(Duration::from_secs(10), stream.chunk()).await??;
            assert!(
                first.is_some(),
                "{path} is open and has served its first frame"
            );
            streams.push((path, stream));
        }

        let started = tokio::time::Instant::now();
        let shutdown = composition.shutdown(server, || async {}, || async {}).await;
        let elapsed = started.elapsed();
        assert!(
            matches!(shutdown.http, GatewayHttpDrainOutcome::Completed(Ok(()))),
            "the HTTP drain joins the attached streams: {:?} after {elapsed:?}",
            shutdown.http
        );
        assert!(
            elapsed < GATEWAY_HTTP_DRAIN_TIMEOUT,
            "the drain ended within its bound: {elapsed:?}"
        );
        assert!(
            shutdown
                .runtime
                .report
                .is_some_and(|report| report.cleanup_completed()),
            "the shutdown attests complete cleanup"
        );
        for (path, mut stream) in streams {
            let ended = tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    match stream.chunk().await {
                        Ok(Some(_)) => {}
                        Ok(None) | Err(_) => return,
                    }
                }
            })
            .await;
            assert!(ended.is_ok(), "{path} ended with the shutdown");
        }
        assert!(
            client
                .get(format!("{base}/console/timeline/stream"))
                .send()
                .await
                .is_err(),
            "a reconnecting Console finds the listener closed"
        );
        Ok(())
    }

    /// Reads `stream` until it ends (a transport error ends it too).
    async fn read_to_end(stream: &mut reqwest::Response) {
        while let Ok(Some(_)) = stream.chunk().await {}
    }

    /// A client attached to an SDK event stream does not hold the shutdown up
    /// either. The agent, mob, mob structural and WorkGraph fact streams end
    /// when the shutdown's HTTP drain begins, so the drain joins them within
    /// its bound and the shutdown attests complete cleanup.
    #[tokio::test]
    async fn gateway_shutdown_ends_attached_sdk_streams_within_the_drain_bound()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let store: Arc<dyn meerkat::WorkGraphStore> =
            Arc::new(meerkat::MemoryWorkGraphStore::default());
        let composition = console_drain_test_composition_with(
            &directory,
            GatewayCompatibilityProfile::ConsoleHttp,
            Some(meerkat::WorkGraphService::new(store)),
        )
        .await?;
        let app = composition
            .runtime()
            .build_reference_app_router(open_console_decisions());
        let composition = composition.activate();
        Box::pin(
            composition
                .runtime()
                .mob_handle()
                .spawn_spec(meerkat_mob::SpawnMemberSpec::new(
                    meerkat_mob::ProfileName::from("worker"),
                    meerkat_mob::AgentIdentity::from("worker-1"),
                )),
        )
        .await?;
        let binding = GatewayHttpBinding::bind_loopback().await?;
        let base = binding.http_base_url();
        let server = binding.serve(app);

        let client = reqwest::Client::new();
        let mut streams = Vec::new();
        for path in [
            "/agents/worker-1/events",
            "/mob/events",
            crate::unified_runtime::mob_events::MOB_EVENTS_STREAM_PATH,
            crate::http_sse::WORKGRAPH_FACTS_STREAM_PATH,
        ] {
            let stream = client.get(format!("{base}{path}")).send().await?;
            assert!(stream.status().is_success(), "{path}: {}", stream.status());
            assert_eq!(
                stream
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok()),
                Some("text/event-stream"),
                "{path} is an open event stream"
            );
            streams.push((path, stream));
        }

        let started = tokio::time::Instant::now();
        let shutdown = composition.shutdown(server, || async {}, || async {}).await;
        let elapsed = started.elapsed();
        assert!(
            matches!(shutdown.http, GatewayHttpDrainOutcome::Completed(Ok(()))),
            "the HTTP drain joins the attached streams: {:?} after {elapsed:?}",
            shutdown.http
        );
        assert!(
            elapsed < GATEWAY_HTTP_DRAIN_TIMEOUT,
            "the drain ended within its bound: {elapsed:?}"
        );
        assert!(
            shutdown
                .runtime
                .report
                .is_some_and(|report| report.cleanup_completed()),
            "the shutdown attests complete cleanup"
        );
        for (path, mut stream) in streams {
            let ended =
                tokio::time::timeout(Duration::from_secs(5), read_to_end(&mut stream)).await;
            assert!(ended.is_ok(), "{path} ended with the shutdown");
        }
        Ok(())
    }

    /// An attached Console stays live while the gateway only stops accepting
    /// connections. The stdio gateway's in-flight drain phase stops admission
    /// and then drains the runtime's in-flight turns: a turn's event in that
    /// window still reaches the attached per-identity stream, and the
    /// timeline and gating streams stay open. All three end only when the
    /// shutdown's HTTP drain begins, which joins them within its bound.
    #[tokio::test]
    async fn attached_console_streams_stay_live_through_the_in_flight_drain_and_end_at_shutdown()
    -> Result<(), Box<dyn std::error::Error>> {
        use serde_json::json;

        let directory = tempfile::tempdir()?;
        let composition =
            console_drain_test_composition(&directory, GatewayCompatibilityProfile::ConsoleHttp)
                .await?;
        let ingress = composition
            .runtime()
            .install_test_console_event_ingress()
            .await;
        let interaction = uuid::Uuid::from_u128(51).to_string();
        composition
            .runtime()
            .reserve_identity_interaction(
                "worker",
                Some("rt:worker:1"),
                &interaction,
                "console",
                json!("hello"),
            )
            .await?;
        let app = composition
            .runtime()
            .build_reference_app_router(open_console_decisions());
        let composition = composition.activate();
        let binding = GatewayHttpBinding::bind_loopback().await?;
        let base = binding.http_base_url();
        let server = binding.serve(app);

        let client = reqwest::Client::new();
        let mut streams = Vec::new();
        for path in [
            "/console/identity/worker/stream",
            "/console/timeline/stream",
            "/console/gating/stream",
        ] {
            let mut stream = client.get(format!("{base}{path}")).send().await?;
            assert!(stream.status().is_success(), "{path}: {}", stream.status());
            let first = tokio::time::timeout(Duration::from_secs(10), stream.chunk()).await??;
            assert!(
                first.is_some(),
                "{path} is open and has served its first frame"
            );
            streams.push((path, stream));
        }

        // The in-flight drain phase, in the stdio gateway's order: admission
        // stops, then the runtime drains the turns in flight.
        server.stop_accepting();
        let drained = composition
            .runtime()
            .drain_in_flight_turns(std::time::Instant::now() + Duration::from_secs(5))
            .await;
        assert!(
            matches!(
                drained.outcome,
                crate::unified_runtime::InFlightDrainOutcome::Drained(_)
            ),
            "{:?}",
            drained.outcome
        );
        // A turn's event in the drain window reaches the attached Console.
        ingress
            .send(console_drain_event(
                "gateway-stream-live-in-drain",
                "run_started",
                json!({
                    "identity": {
                        "interaction_id": interaction,
                        "run_id": uuid::Uuid::from_u128(52).to_string(),
                    },
                    "session_id": uuid::Uuid::from_u128(53).to_string(),
                    "input": { "kind": "content", "content": "hello" },
                }),
            ))
            .await?;
        let (identity_path, identity_stream) = &mut streams[0];
        let delivered = tokio::time::timeout(Duration::from_secs(10), async {
            let mut read = String::new();
            loop {
                match identity_stream.chunk().await {
                    Ok(Some(chunk)) => {
                        read.push_str(&String::from_utf8_lossy(&chunk));
                        if read.contains("gateway-stream-live-in-drain") {
                            return Ok(());
                        }
                    }
                    Ok(None) => return Err(format!("it ended first; read: {read}")),
                    Err(error) => return Err(format!("it failed first: {error}; read: {read}")),
                }
            }
        })
        .await;
        assert!(
            matches!(delivered, Ok(Ok(()))),
            "{identity_path} delivers the turn's frame in the drain window: {delivered:?}"
        );
        for (path, stream) in &mut streams[1..] {
            let ended = tokio::time::timeout(Duration::from_millis(200), read_to_end(stream)).await;
            assert!(ended.is_err(), "{path} is still open in the drain window");
        }

        let started = tokio::time::Instant::now();
        let shutdown = composition.shutdown(server, || async {}, || async {}).await;
        let elapsed = started.elapsed();
        assert!(
            matches!(shutdown.http, GatewayHttpDrainOutcome::Completed(Ok(()))),
            "the HTTP drain joins the attached streams: {:?} after {elapsed:?}",
            shutdown.http
        );
        assert!(
            elapsed < GATEWAY_HTTP_DRAIN_TIMEOUT,
            "the drain ended within its bound: {elapsed:?}"
        );
        assert!(
            shutdown
                .runtime
                .report
                .is_some_and(|report| report.cleanup_completed()),
            "the shutdown attests complete cleanup"
        );
        for (path, mut stream) in streams {
            let ended =
                tokio::time::timeout(Duration::from_secs(5), read_to_end(&mut stream)).await;
            assert!(ended.is_ok(), "{path} ended at the shutdown's HTTP drain");
        }
        Ok(())
    }

    /// The horizon bounds the observation only: an entered shutdown that
    /// outlives it is not cancelled and still runs to its module phase.
    #[tokio::test]
    async fn horizon_cutoff_keeps_the_entered_shutdown_running()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let runtime =
            console_drain_test_composition(&directory, GatewayCompatibilityProfile::StdioRpc)
                .await?
                .into_shared_runtime();
        assert!(runtime.module_is_running().await);
        let shutdown = shutdown_gateway_runtime_within(Arc::clone(&runtime), Duration::ZERO).await;
        assert!(
            shutdown.report.is_none(),
            "a cut-off observation attests nothing"
        );
        tokio::time::timeout(Duration::from_secs(30), async {
            while runtime.module_is_running().await {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        Ok(())
    }

    /// The horizon branch of the executor-exit boundary. The observation is
    /// cut off at once, so there is no runtime report, and the gateway's
    /// executor-exit settlement still joins the member's stdio close through
    /// its whole native attempt (the server ignores EOF and SIGTERM) before
    /// it returns, reporting the native outcome with the server reaped.
    #[cfg(target_os = "linux")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn horizon_cutoff_still_settles_member_mcp_at_the_exit_boundary() {
        use crate::test_mcp_fixture::{
            FixtureMode, McpFixture, ProcessState, process_state, runtime_with_member_server,
        };

        let fixture = McpFixture::new();
        let custody = crate::MemberMcpCustody::new();
        let (runtime, pid) =
            runtime_with_member_server(&fixture, FixtureMode::Stubborn, &custody).await;
        let shutdown = shutdown_gateway_runtime_within(Arc::new(runtime), Duration::ZERO).await;
        assert!(
            shutdown.report.is_none(),
            "a cut-off observation attests nothing"
        );
        assert!(
            matches!(
                shutdown.member_mcp_at_exit,
                crate::MemberMcpSettlement::Settled { servers: 1, .. }
            ),
            "{:?}",
            shutdown.member_mcp_at_exit
        );
        assert_eq!(process_state(pid), ProcessState::Gone);
    }

    /// The deadline branch of the executor-exit boundary: every observation
    /// expires while the member's close is still in its native attempt (the
    /// server ignores EOF and SIGTERM, so the close needs its graces). The
    /// boundary reports the expired observation as `deadline`, and still
    /// returns only after that close returned its native outcome: the
    /// executor destroyed after it owns no pending close.
    #[cfg(target_os = "linux")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_exit_boundary_joins_a_close_its_observation_deadline_missed() {
        use crate::test_mcp_fixture::{
            FixtureMode, McpFixture, ProcessState, process_state, runtime_with_member_server,
        };

        let fixture = McpFixture::new();
        let custody = crate::MemberMcpCustody::new();
        let (runtime, pid) =
            runtime_with_member_server(&fixture, FixtureMode::Stubborn, &custody).await;
        custody.expire_observations_for_test();
        let shutdown = shutdown_gateway_runtime_within(Arc::new(runtime), Duration::ZERO).await;
        assert_eq!(
            shutdown.member_mcp_at_exit,
            crate::MemberMcpSettlement::Unsettled(crate::MemberMcpUnsettled {
                routers: 1,
                servers: 1,
                deadline: 1,
                ..crate::MemberMcpUnsettled::default()
            }),
            "the close's native outcome was joined, and its router counts under deadline"
        );
        assert_eq!(
            process_state(pid),
            ProcessState::Gone,
            "the boundary returned before the native close did"
        );
    }

    #[tokio::test]
    async fn dropping_active_gateway_cancels_its_event_drain()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let composition =
            console_drain_test_composition(&directory, GatewayCompatibilityProfile::ConsoleHttp)
                .await?
                .activate();
        let runtime = Arc::clone(composition.runtime());
        let drain_abort = composition
            .state
            .event_drain_task
            .lock()
            .await
            .as_ref()
            .ok_or("activated gateway has no event drain")?
            .abort_handle();
        assert!(!drain_abort.is_finished());
        drop(composition);
        tokio::time::timeout(Duration::from_secs(3), async {
            while !drain_abort.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        assert!(runtime.shutdown().await.cleanup_completed());
        Ok(())
    }

    #[test]
    fn compatibility_profiles_pin_the_two_existing_command_surfaces() {
        assert_eq!(
            GatewayCompatibilityProfile::ConsoleHttp.contract(),
            GatewayCompatibilityContract {
                top_level_permissive_init: true,
                registry_resume: true,
                post_init_stdin_rpc: false,
                callback_plane: false,
                single_shot: false,
                maintenance_verbs: true,
            }
        );
        assert_eq!(
            GatewayCompatibilityProfile::StdioRpc.contract(),
            GatewayCompatibilityContract {
                top_level_permissive_init: false,
                registry_resume: false,
                post_init_stdin_rpc: true,
                callback_plane: true,
                single_shot: true,
                maintenance_verbs: false,
            }
        );
    }

    const HOST_CONFIG_FIXTURE: &str = r#"
[self_hosted]
default_model = "gemma-4-31b"

[self_hosted.servers.local]
transport = "openai_compatible"
base_url = "http://127.0.0.1:11434"
api_style = "chat_completions"

[self_hosted.models.gemma-4-31b]
server = "local"
remote_model = "gemma4:31b"

[models.house-model]
provider = "openai"

[realm.global]
default_binding = "local"
"#;

    /// The host config's `[self_hosted]`, `[models]` and `[realm]` tables must
    /// arrive in the loaded `Config`: nothing else in either gateway can
    /// supply them (`TomlDefinition` drops the first and last, and the
    /// registry refuses `[models.<id>] provider = "self_hosted"`).
    #[test]
    fn host_config_carries_self_hosted_models_and_realm_tables()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.toml");
        std::fs::write(&path, HOST_CONFIG_FIXTURE)?;

        let config = load_gateway_host_config(&path)?;

        assert_eq!(
            config
                .self_hosted
                .models
                .get("gemma-4-31b")
                .map(|model| model.server.as_str()),
            Some("local")
        );
        assert!(config.self_hosted.servers.contains_key("local"));
        assert_eq!(
            config.self_hosted.default_model.as_deref(),
            Some("gemma-4-31b")
        );
        assert_eq!(
            config
                .models
                .custom
                .get("house-model")
                .map(|model| model.provider == meerkat_core::Provider::OpenAI),
            Some(true)
        );
        assert!(config.realm.contains_key("global"));
        Ok(())
    }

    #[test]
    fn host_model_fallback_defaults_off_and_preserves_explicit_policy()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.toml");
        for enabled in [false, true] {
            std::fs::write(
                &path,
                format!(
                    "[model_fallback]\nenabled = {enabled}\n\
                     chain = [{{ model = \"gpt-5.5\", provider = \"openai\" }}]\n\
                     [model_fallback.policy]\ntrigger_after_attempts = 5\n"
                ),
            )?;
            let config = load_gateway_host_config(&path)?;
            assert_eq!(config.model_fallback.enabled, Some(enabled));
            assert_eq!(config.model_fallback.chain.len(), 1);
            assert_eq!(config.model_fallback.policy.trigger_after_attempts, 5);
            assert!(!config.model_fallback.policy.cross_provider);
        }
        std::fs::write(&path, "")?;
        assert!(!load_gateway_host_config(&path)?.model_fallback.is_enabled());
        Ok(())
    }

    /// meerkat 0.8.43 loads the two persisted pre-0.8.37 `[model_fallback]`
    /// shapes as no fallback policy (with a typed warning) instead of
    /// refusing them: the gateway starts, with fallback off.
    #[test]
    fn host_model_fallback_legacy_shapes_load_with_fallback_off()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("fallback.toml");
        for declaration in [
            "[model_fallback]\nenabled = true\n",
            "[model_fallback]\nuse_catalog_default_chain = true\n",
        ] {
            std::fs::write(&path, declaration)?;
            assert!(
                !load_gateway_host_config(&path)?.model_fallback.is_enabled(),
                "{declaration}"
            );
        }
        Ok(())
    }

    #[test]
    fn host_model_fallback_invalid_policy_is_an_explicit_init_error()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("fallback.toml");
        for declaration in [
            "[model_fallback]\nenabeld = false\n",
            "[model_fallback.policy]\ncross_providre = true\n",
            "[model_fallback.policy]\ntrigger_after_attempts = 0\n",
            "[model_fallback.policy]\nmin_context_headroom = 1.0\n",
            "[[model_fallback.chain]]\nmodel = \"gpt-5.5\"\nunknown = true\n",
        ] {
            std::fs::write(&path, declaration)?;
            let error = load_gateway_host_config(&path)
                .err()
                .ok_or("invalid fallback unexpectedly accepted")?;
            assert!(error.to_string().contains("fallback.toml"), "{error}");
        }
        Ok(())
    }

    /// A named file that cannot be read or parsed is a typed refusal that
    /// names the path, never a silent `Config::default()`.
    #[test]
    fn host_config_refuses_missing_and_malformed_files_by_path()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let missing = dir.path().join("absent.toml");
        let error = load_gateway_host_config(&missing).err();
        assert!(
            matches!(&error, Some(GatewayHostConfigError::Read { path, .. }) if path == &missing),
            "{error:?}"
        );
        assert!(
            error
                .map(|error| error.to_string())
                .unwrap_or_default()
                .contains("absent.toml")
        );

        let malformed = dir.path().join("broken.toml");
        std::fs::write(&malformed, "[self_hosted\n")?;
        let error = load_gateway_host_config(&malformed).err();
        assert!(
            matches!(&error, Some(GatewayHostConfigError::Parse { path, .. }) if path == &malformed),
            "{error:?}"
        );
        Ok(())
    }

    /// A file that parses but fails meerkat's own `Config::validate` is refused
    /// at load by path, with meerkat's message: here a `[self_hosted.models]`
    /// alias whose server is not declared, the fail-late class the ingress
    /// exists to remove (the member would otherwise die at its first build).
    #[test]
    fn host_config_refuses_a_file_that_does_not_validate_by_path()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let dangling = dir.path().join("dangling.toml");
        std::fs::write(
            &dangling,
            "[self_hosted.models.gemma-4-31b]\nserver = \"nowhere\"\nremote_model = \"gemma4:31b\"\n",
        )?;
        let error = load_gateway_host_config(&dangling).err();
        assert!(
            matches!(&error, Some(GatewayHostConfigError::Invalid { path, .. }) if path == &dangling),
            "{error:?}"
        );
        let message = error.map(|error| error.to_string()).unwrap_or_default();
        assert!(message.contains("dangling.toml"), "{message}");
        assert!(message.contains("references unknown server"), "{message}");
        assert!(message.contains("nowhere"), "{message}");
        Ok(())
    }

    /// `TomlDefinition` has no `deny_unknown_fields`, so the two host-config
    /// tables vanish from a parsed definition without a diagnostic. The raw
    /// check must name the table and the option that carries the host config,
    /// and must leave every table that DOES belong in mob.toml alone.
    #[test]
    fn mob_toml_host_config_tables_are_refused_by_name() {
        let self_hosted = "[mob]\nid = \"m\"\n\n[self_hosted.servers.local]\n\
                           base_url = \"http://127.0.0.1:11434\"\n";
        assert_eq!(
            refuse_host_config_tables_in_mob_toml(self_hosted),
            Err(HostConfigTableInMobToml::SelfHosted)
        );
        let realm = "[mob]\nid = \"m\"\n\n[realm.global]\ndefault_binding = \"local\"\n";
        assert_eq!(
            refuse_host_config_tables_in_mob_toml(realm),
            Err(HostConfigTableInMobToml::Realm)
        );
        for table in HostConfigTableInMobToml::ALL {
            let message = table.to_string();
            assert!(
                message.contains(&format!("[{}]", table.table())),
                "{message}"
            );
            assert!(message.contains("meerkat_config_path"), "{message}");
        }
        let ordinary = "[mob]\nid = \"m\"\n\n[profiles.w]\nmodel = \"house-model\"\n\n\
                        [models.house-model]\nprovider = \"openai\"\n";
        assert_eq!(refuse_host_config_tables_in_mob_toml(ordinary), Ok(()));
        // Invalid TOML is the definition parser's verdict, not this check's.
        assert_eq!(refuse_host_config_tables_in_mob_toml("[mob\n"), Ok(()));
    }

    #[tokio::test]
    async fn shutdown_tail_orders_profile_quiesce_runtime_cleanup_then_registry_cleanup() {
        let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
        let before_observed = Arc::clone(&observed);
        let runtime_observed = Arc::clone(&observed);
        let cleanup_observed = Arc::clone(&observed);

        let (runtime, cleanup) = ordered_shutdown_tail(
            move || async move {
                before_observed
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push("profile_quiesce");
            },
            async move {
                runtime_observed
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push("runtime_shutdown");
                "runtime_report"
            },
            move || async move {
                cleanup_observed
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push("registry_cleanup");
                "cleanup_report"
            },
        )
        .await;

        assert_eq!(runtime, "runtime_report");
        assert_eq!(cleanup, "cleanup_report");
        assert_eq!(
            *observed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            ["profile_quiesce", "runtime_shutdown", "registry_cleanup"]
        );
    }

    #[test]
    fn the_gateway_runtime_starts_the_resolved_worker_count() -> std::io::Result<()> {
        let runtime = gateway_tokio_runtime_with(std::num::NonZeroUsize::new(3))?;
        assert_eq!(runtime.metrics().num_workers(), 3);
        let single = gateway_tokio_runtime_with(std::num::NonZeroUsize::new(1))?;
        assert_eq!(single.metrics().num_workers(), 1);
        Ok(())
    }

    /// Golden envelope for the shared filter builder: the exact strings the
    /// two binaries carried as their own constants before this module
    /// existed. This is a byte-level pin, not a behavioural one - each
    /// binary keeps its own `EnvFilter`-level test proving its target
    /// actually passes at INFO while dependencies stay at WARN.
    #[test]
    fn default_tracing_filter_reproduces_both_binary_constants() {
        assert_eq!(
            default_tracing_filter("rpc_gateway"),
            "warn,meerkat_meta=info,rpc_gateway=info"
        );
        assert_eq!(
            default_tracing_filter("meta_gateway"),
            "warn,meerkat_meta=info,meta_gateway=info"
        );
    }

    /// The filter must keep the crate's own target at INFO for every binary:
    /// dropping `meerkat_meta=info` is the regression that made a working
    /// migration look like a hang, and it would not be visible in a test
    /// that only checked the binary's own target.
    #[test]
    fn default_tracing_filter_always_keeps_the_crate_at_info() {
        for target in ["rpc_gateway", "meta_gateway", "some_future_gateway"] {
            let filter = default_tracing_filter(target);
            assert!(
                filter.contains("meerkat_meta=info"),
                "filter for {target} dropped the crate's own INFO target: {filter}"
            );
            assert!(
                filter.starts_with("warn,"),
                "filter for {target} lost the dependency WARN default: {filter}"
            );
        }
    }

    /// Captures everything a `tracing_subscriber::fmt` subscriber writes so a
    /// test can assert on the rendered line.
    #[derive(Clone, Default)]
    struct CaptureWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl CaptureWriter {
        fn contents(&self) -> String {
            String::from_utf8_lossy(
                &self
                    .0
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            )
            .into_owned()
        }
    }

    impl std::io::Write for CaptureWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CaptureWriter {
        type Writer = CaptureWriter;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// A panic must leave an ERROR line in the tracing stream naming the
    /// thread, the source location and the payload. Without the hook the
    /// only record is the default hook's plain stderr text, which is not in
    /// the log stream at all when stderr is redirected; the tests that
    /// drive the real binaries cannot make a gateway panic on demand, so the
    /// hook is proven here on the test thread with a thread-local subscriber.
    #[test]
    // The panics ARE the subject under test; they are caught two lines later.
    #[allow(clippy::panic)]
    fn panic_hook_reports_thread_location_and_payload_through_tracing() {
        install_gateway_panic_hook();
        let writer = CaptureWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer.clone())
            .with_ansi(false)
            .finish();
        let current = std::thread::current();
        let this_thread = current.name().unwrap_or("<unnamed>").to_string();
        // A second registered Dispatch keeps callsite interest from being
        // cached `never` by threads without a subscriber.
        let _anchor = tracing::Dispatch::new(tracing_subscriber::Registry::default());
        let outcomes = tracing::subscriber::with_default(subscriber, || {
            // A formatted message carries a `String` payload, a literal
            // carries a `&str`; both downcasts must render the text.
            let formatted = std::panic::catch_unwind(|| {
                panic!("gateway panic hook marker {}", 40 + 2);
            });
            let literal = std::panic::catch_unwind(|| {
                panic!("gateway panic hook literal marker");
            });
            (formatted, literal)
        });
        assert!(outcomes.0.is_err() && outcomes.1.is_err());

        let log = writer.contents();
        let error_lines: Vec<&str> = log.lines().filter(|line| line.contains("ERROR")).collect();
        assert!(
            error_lines
                .iter()
                .any(|line| line.contains("gateway panic hook marker 42")),
            "formatted panic payload missing from the tracing stream:\n{log}"
        );
        assert!(
            error_lines
                .iter()
                .any(|line| line.contains("gateway panic hook literal marker")),
            "literal panic payload missing from the tracing stream:\n{log}"
        );
        for line in &error_lines {
            assert!(
                line.contains("gateway_composition.rs"),
                "panic line does not name the source file: {line}"
            );
            assert!(
                line.contains("line=") && line.contains("column="),
                "panic line does not carry the source position: {line}"
            );
            assert!(
                line.contains(&this_thread),
                "panic line does not name the panicking thread {this_thread:?}: {line}"
            );
        }
    }

    #[test]
    fn control_listen_arg_absent_is_none() -> Result<(), String> {
        let args = vec!["--persistent".to_string()];
        assert!(parse_control_listen_arg(&args)?.is_none());
        Ok(())
    }

    #[test]
    fn control_listen_arg_parses_tcp() -> Result<(), String> {
        let args = vec![
            "--persistent".to_string(),
            "--control-listen".to_string(),
            "tcp://127.0.0.1:0".to_string(),
        ];
        assert!(parse_control_listen_arg(&args)?.is_some());
        Ok(())
    }

    /// A trailing `--control-listen` is a launch error, not a silently
    /// ignored flag: both binaries exit 2 on this.
    #[test]
    fn control_listen_arg_without_value_is_an_error() {
        let args = vec!["--control-listen".to_string()];
        let error = parse_control_listen_arg(&args).err().unwrap_or_default();
        assert!(
            error.contains("requires an address"),
            "unexpected error text: {error}"
        );
    }

    /// `inproc` has no listener; the refusal must name the flag so the
    /// operator can tell which argument was rejected.
    #[test]
    fn control_listen_arg_rejects_inproc_and_names_the_flag() {
        let args = vec!["--control-listen".to_string(), "inproc".to_string()];
        let error = parse_control_listen_arg(&args).err().unwrap_or_default();
        assert!(
            error.starts_with("--control-listen: "),
            "refusal must name the flag: {error}"
        );
    }

    // -----------------------------------------------------------------------
    // HTTP listener binding and exposure policy
    // -----------------------------------------------------------------------

    /// A JWKS with one key: what an `auth_config` launch trusts. Any key
    /// shape will do for the gate, which asks only "is there a key".
    const ONE_KEY_JWKS: &str =
        r#"{"keys":[{"kid":"k1","kty":"oct","alg":"HS256","k":"c2VjcmV0LWJ5dGVz"}]}"#;

    fn open_console() -> RuntimeDecisionState {
        RuntimeDecisionState::local_console(
            crate::decisions::ConsolePolicy {
                require_app_auth: false,
                ..crate::decisions::ConsolePolicy::default()
            },
            None,
        )
    }

    fn closed_console_without_keys() -> RuntimeDecisionState {
        RuntimeDecisionState::local_console(crate::decisions::ConsolePolicy::default(), None)
    }

    fn authenticated_console() -> RuntimeDecisionState {
        let mut state = closed_console_without_keys();
        state.trusted_oidc.jwks_json = ONE_KEY_JWKS.to_string();
        state
    }

    /// The default constructor must stay byte-identical to every earlier
    /// release: loopback, ephemeral port, `http://127.0.0.1:<port>`.
    #[tokio::test]
    async fn bind_loopback_reports_the_pre_existing_base_url_form() -> std::io::Result<()> {
        let binding = GatewayHttpBinding::bind_loopback().await?;
        assert_eq!(binding.local_addr().ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(
            binding.http_base_url(),
            format!("http://127.0.0.1:{}", binding.port())
        );
        assert_eq!(binding.advertised_base_url(), None);
        Ok(())
    }

    /// A wildcard bind is reported at the loopback a same-host client can
    /// dial, with the kernel-assigned port, and the advertised base rides
    /// beside it with its trailing slash normalised away.
    #[tokio::test]
    async fn bind_unspecified_reports_a_reachable_loopback_base_url() -> std::io::Result<()> {
        let binding = GatewayHttpBinding::bind("0.0.0.0:0".parse().map_err(std::io::Error::other)?)
            .await?
            .with_advertised_base_url(Some("https://mob.example.com/".to_string()));
        assert!(binding.local_addr().ip().is_unspecified());
        assert_ne!(binding.port(), 0);
        assert_eq!(
            binding.http_base_url(),
            format!("http://127.0.0.1:{}", binding.port())
        );
        assert_eq!(
            binding.reachable_addr(),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), binding.port())
        );
        assert_eq!(
            binding.advertised_base_url(),
            Some("https://mob.example.com")
        );
        Ok(())
    }

    /// The loopback mapping is a pure function of the bound address: both
    /// wildcard families map to their loopback, every concrete address is
    /// left alone (a listener on one interface does not answer on loopback).
    #[test]
    fn loopback_reachable_addr_maps_wildcards_only() -> Result<(), std::net::AddrParseError> {
        assert_eq!(
            loopback_reachable_addr("0.0.0.0:8080".parse()?),
            "127.0.0.1:8080".parse()?
        );
        assert_eq!(
            loopback_reachable_addr("[::]:8080".parse()?),
            "[::1]:8080".parse()?
        );
        assert_eq!(
            loopback_reachable_addr("192.0.2.10:8080".parse()?),
            "192.0.2.10:8080".parse()?
        );
        assert_eq!(
            loopback_reachable_addr("127.0.0.1:41".parse()?),
            "127.0.0.1:41".parse()?
        );
        Ok(())
    }

    /// The gate itself, independent of any decision state: loopback always
    /// passes, non-loopback passes only with the acknowledgement, and the
    /// refusal names the address and every way out.
    #[test]
    fn http_bind_policy_refuses_non_loopback_without_allow_remote()
    -> Result<(), std::net::AddrParseError> {
        for loopback in ["127.0.0.1:0", "[::1]:8080", "127.0.0.2:9"] {
            assert_eq!(
                validate_http_bind_policy(
                    GatewaySurface::MetaGateway,
                    loopback.parse()?,
                    HttpBindPolicy::local_only()
                ),
                Ok(())
            );
        }
        for remote in ["0.0.0.0:8080", "[::]:8080", "192.0.2.10:8080"] {
            let listen: SocketAddr = remote.parse()?;
            let error = validate_http_bind_policy(
                GatewaySurface::RpcGateway,
                listen,
                HttpBindPolicy::local_only(),
            );
            assert_eq!(
                error,
                Err(HttpBindPolicyError::RemoteBindRequiresExplicitAllow {
                    surface: GatewaySurface::RpcGateway,
                    listen,
                })
            );
            let message = error
                .map(|()| String::new())
                .unwrap_or_else(|error| error.to_string());
            assert!(message.contains(remote), "{message}");
            assert!(message.contains("rpc_gateway"), "{message}");
            assert!(message.contains("allow_remote"), "{message}");
            assert!(message.contains("auth_config"), "{message}");
            assert!(message.contains("META_HTTP_ALLOW_REMOTE"), "{message}");
            assert_eq!(
                validate_http_bind_policy(
                    GatewaySurface::MetaGateway,
                    listen,
                    HttpBindPolicy::allow_remote()
                ),
                Ok(())
            );
        }
        Ok(())
    }

    /// The surface enum is what a message names: both binaries spell
    /// themselves the way their `--version` output does.
    #[test]
    fn gateway_surface_display_is_the_binary_name() {
        assert_eq!(GatewaySurface::RpcGateway.to_string(), "rpc_gateway");
        assert_eq!(GatewaySurface::MetaGateway.to_string(), "meta_gateway");
    }

    /// The gateway resolution: enforced console auth opens the gate on its
    /// own; an open console does not; a console that REQUIRES auth but
    /// trusts no key (closed to everyone) does not either, because refusing
    /// everyone is not authenticating anyone; the acknowledgement always does.
    #[test]
    fn http_bind_policy_for_gateway_treats_enforced_console_auth_as_allow() {
        assert_eq!(
            ConsoleAuthPosture::of(&authenticated_console()),
            ConsoleAuthPosture::Enforced
        );
        assert_eq!(
            ConsoleAuthPosture::of(&open_console()),
            ConsoleAuthPosture::Open
        );
        assert_eq!(
            ConsoleAuthPosture::of(&closed_console_without_keys()),
            ConsoleAuthPosture::ClosedToEveryCaller
        );
        assert!(ConsoleAuthPosture::Enforced.is_enforced());
        assert!(!ConsoleAuthPosture::Open.is_enforced());
        assert!(!ConsoleAuthPosture::ClosedToEveryCaller.is_enforced());

        assert_eq!(
            HttpBindPolicy::for_gateway(false, &authenticated_console()),
            HttpBindPolicy::allow_remote()
        );
        assert_eq!(
            HttpBindPolicy::for_gateway(false, &open_console()),
            HttpBindPolicy::local_only()
        );
        assert_eq!(
            HttpBindPolicy::for_gateway(false, &closed_console_without_keys()),
            HttpBindPolicy::local_only()
        );
        assert_eq!(
            HttpBindPolicy::for_gateway(true, &open_console()),
            HttpBindPolicy::allow_remote()
        );
    }

    #[test]
    fn parse_http_listen_addr_accepts_ip_literals_and_refuses_hostnames() {
        assert_eq!(
            parse_http_listen_addr(" 0.0.0.0:8080 ").ok(),
            "0.0.0.0:8080".parse().ok()
        );
        assert_eq!(
            parse_http_listen_addr("[::]:8080").ok(),
            "[::]:8080".parse().ok()
        );
        for bad in ["localhost:8080", "8080", "0.0.0.0", ""] {
            let error = parse_http_listen_addr(bad).err();
            assert!(
                matches!(&error, Some(HttpExposureParseError::ListenAddress { value, .. }) if value == bad),
                "{bad:?}: {error:?}"
            );
            let message = error.map(|error| error.to_string()).unwrap_or_default();
            assert!(message.contains("HOST:PORT"), "{bad:?}: {message}");
        }
    }

    #[test]
    fn parse_http_public_base_url_requires_an_absolute_http_url() {
        assert_eq!(
            parse_http_public_base_url(" https://mob.example.com/ ").as_deref(),
            Ok("https://mob.example.com")
        );
        assert_eq!(
            parse_http_public_base_url("http://192.168.0.10:8080").as_deref(),
            Ok("http://192.168.0.10:8080")
        );
        for bad in ["mob.example.com", "ws://mob.example.com", ""] {
            assert_eq!(
                parse_http_public_base_url(bad),
                Err(HttpExposureParseError::PublicBaseUrlNotAbsoluteHttp {
                    value: bad.to_string()
                }),
                "{bad:?}"
            );
        }
        assert_eq!(
            parse_http_public_base_url("https://"),
            Err(HttpExposureParseError::PublicBaseUrlNoHost {
                value: "https://".to_string()
            })
        );
    }

    /// Without the flag the gateways start as usual; the flag without a
    /// directory (or followed by another flag) is a usage error, exit 2,
    /// that writes nothing.
    #[test]
    fn write_mcp_apps_sandbox_arg_needs_a_directory() {
        let args = |values: &[&str]| values.iter().map(ToString::to_string).collect::<Vec<_>>();
        assert_eq!(
            [
                write_mcp_apps_sandbox_arg(&args(&["--persistent"])),
                write_mcp_apps_sandbox_arg(&args(&["--write-mcp-apps-sandbox"])),
                write_mcp_apps_sandbox_arg(&args(&["--write-mcp-apps-sandbox", "--persistent"])),
            ],
            [None, Some(2), Some(2)]
        );
    }

    #[test]
    fn parse_http_listen_arg_mirrors_the_control_listen_flag() -> Result<(), HttpExposureParseError>
    {
        let absent = vec!["--persistent".to_string()];
        assert!(parse_http_listen_arg(&absent)?.is_none());

        let present = vec!["--http-listen".to_string(), "0.0.0.0:8080".to_string()];
        assert_eq!(
            parse_http_listen_arg(&present)?,
            "0.0.0.0:8080".parse().ok()
        );

        let missing_value = vec!["--http-listen".to_string()];
        let error = parse_http_listen_arg(&missing_value).err();
        assert_eq!(error, Some(HttpExposureParseError::ListenFlagMissingValue));
        let message = error.map(|error| error.to_string()).unwrap_or_default();
        assert!(message.contains("--http-listen requires"), "{message}");

        let malformed = vec!["--http-listen".to_string(), "localhost:8080".to_string()];
        let error = parse_http_listen_arg(&malformed).err();
        assert!(
            matches!(&error, Some(HttpExposureParseError::ListenFlagAddress { value, .. }) if value == "localhost:8080"),
            "{error:?}"
        );
        let message = error.map(|error| error.to_string()).unwrap_or_default();
        assert!(message.starts_with("--http-listen:"), "{message}");
        Ok(())
    }

    /// The firing role follows the launch's declared composition authority
    /// and nothing else: only a launch that speaks for the composition (the
    /// default) may drive its schedule store.
    #[test]
    fn only_an_authoritative_launch_executes_schedules() {
        assert_eq!(
            ScheduleFiringRole::for_composition_authority(CompositionAuthority::Authoritative),
            ScheduleFiringRole::Executor
        );
        assert_eq!(
            ScheduleFiringRole::for_composition_authority(CompositionAuthority::NonAuthoritative),
            ScheduleFiringRole::QuiescedCandidate
        );
        assert_eq!(
            ScheduleFiringRole::for_composition_authority(CompositionAuthority::default()),
            ScheduleFiringRole::Executor,
            "a launch that declares nothing keeps executing schedules"
        );
    }
}
