use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, anyhow};
use meerkat::{AgentFactory, Config, FactoryAgentBuilder, PersistentSessionService};
use meerkat_meta::contact_directory::ContactDirectory;
use meerkat_meta::runtime::cross_mob_control::{
    ControlAuthorizer, ControlGrantTable, ControlListenAddr,
};
use meerkat_meta::shutdown_signal::ShutdownSignal;
use meerkat_meta::{
    Base64BlobStoreAdapter, BigQueryNaming, BinaryBlobStore, ConsolePolicy, ConsoleUiConfig,
    ConventionalPaths, GatewayPeerKeys, META_CONTRACT_VERSION, MetaStorageLayout,
    MobBootstrapOptions, MobBootstrapSpec, ObjectStoreBlobStore, RuntimeDecisionState,
    load_console_ui_config_from_path_for_realm,
    mob_handle_runtime::mob_definition_may_use_image_generation,
};
use meerkat_mob::ids::AgentIdentity;
use meerkat_mob::{MobDefinition, MobStorage, ProfileName, SpawnMemberSpec};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, BufReader};

const FALLBACK_TEMPLATE_VERSION: &str = "tux-fallback-v2";

/// Durable schedule service + the concrete persistent session service it shares
/// with the firing host (the `dyn MobSessionService` returned for the spec can't
/// drive the runtime-backed schedule host, which needs the concrete type).
type ScheduleHostInputs = (
    meerkat::ScheduleService,
    meerkat_meta::schedule_wiring::ScheduleMobTargetRegistry,
    Arc<PersistentSessionService<FactoryAgentBuilder>>,
    PathBuf,
    meerkat_meta::schedule_wiring::ScheduleFiringHostBinding,
);
/// WorkGraph wiring from a durable attach: the realm-scoped service, the
/// tool-plane admission slot the bootstrap spec must register, and the state
/// dir for the cross-process admission sidecar (the store file is shareable
/// across processes).
type WorkGraphParts = (
    meerkat::WorkGraphService,
    meerkat_meta::workgraph_admission::WorkGraphAdmissionSlot,
    PathBuf,
);

/// This binary's own tracing target. Feeds
/// `gateway_composition::default_tracing_filter`, which builds the same
/// filter string this binary used to carry as its own constant: this crate's
/// own targets at INFO, dependencies at WARN. The one-time head-canonical
/// conversion and the storage maintenance verbs report progress at INFO from
/// `meerkat_meta`; a blanket "warn" default hid them.
const GATEWAY_TRACING_TARGET: &str = "meta_gateway";
type PersistentSessionServiceParts = (
    Arc<dyn meerkat_mob::MobSessionService>,
    Arc<meerkat_runtime::MeerkatMachine>,
    Arc<dyn BinaryBlobStore>,
    Option<ScheduleHostInputs>,
    Option<WorkGraphParts>,
    meerkat_meta::storage_health::ResolvedStorageSummary,
    Arc<dyn meerkat_meta::identity_first::CommittedBoundaryRecoverer>,
    Arc<dyn meerkat_runtime::RuntimeStore>,
    ConsoleLiveInputs,
    meerkat_meta::mob_handle_runtime::MobRuntimeDelivery,
    meerkat_meta::memory::dispatch_taint::HostDefaultDecoratorSource,
);
type ConsoleLiveInputs = (
    Arc<PersistentSessionService<FactoryAgentBuilder>>,
    Arc<meerkat_runtime::MeerkatMachine>,
    AgentFactory,
    Config,
    AgentMobToolsSlot,
);
type AgentMobToolsSlot =
    Arc<std::sync::RwLock<Option<Arc<dyn meerkat_core::service::MobToolsFactory>>>>;

#[derive(Debug, Deserialize)]
struct InitParams {
    workspace_root: Option<PathBuf>,
    project_root: Option<PathBuf>,
    context_root: Option<PathBuf>,
    runtime_root: Option<PathBuf>,
    store_path: Option<PathBuf>,
    persistent_sessions: Option<bool>,
    /// Explicit authenticated HTTP voice registration, using the shared
    /// OpenAI profile shape. Requires persistent sessions and auth_config.
    console_voice: Option<Value>,
    /// Same explicit JWT console authentication contract as rpc_gateway.
    auth_config: Option<Value>,
    realm: Option<String>,
    isolated: Option<bool>,
    surface: Option<String>,
    runtime_profile: Option<String>,
    console_read_only: Option<bool>,
    /// Run this gateway identity-first (meerkat-studio ask K0): durable
    /// identities with continuity records, lease-fenced embodiment, and the
    /// tolerant identity lifecycle paths — instead of the session-owned
    /// mob-member surface (where the ask-20 retire/respawn class lives).
    identity_first: Option<bool>,
    /// Desired identity roster for `identity_first: true` — restored at boot
    /// and reconciled thereafter. `meta/ensure_member` adds to it at
    /// runtime.
    identity_roster: Option<Vec<meerkat_meta::identity_first::DurableAgentSpec>>,
    /// Mob announcements: the object `rpc_gateway` takes as
    /// `runtime_options.mob_announcements` (`{enabled, batch?,
    /// retention_ms?, member_limits?, child_member_limits?}`), through the
    /// same parser and the same refusals. Needs the identity-first runtime
    /// (the default) and `persistent_sessions: true`.
    #[serde(default)]
    mob_announcements: Option<Value>,
    /// Member role migrations this activation is authorized to perform, e.g.
    /// `[{"identity": "domain:automation", "from_role": "domain"}]`.
    ///
    /// A durable member whose role changed refuses to resume
    /// (`MobError::MemberRoleMigrationRequired`) until the host names it here,
    /// which is deliberate: an unintended role edit must not silently restamp
    /// a member's durable role, comms name and binding. Declaring the
    /// migration is the host asserting "yes, this identity moved, and it moved
    /// FROM this role".
    ///
    /// Boot-scoped. It is read once into the session bridge, never persisted,
    /// and gone the moment it is absent from the next boot payload. Meerkat
    /// re-verifies the declared predecessor against durable state, so a
    /// mistyped `from_role` refuses rather than restamps, and it ignores the
    /// declaration entirely once the roles already agree - so leaving a
    /// completed migration in the payload is inert, not a repeat restamp.
    #[serde(default)]
    role_migrations: Option<Vec<meerkat_meta::identity_first::RoleMigrationDeclaration>>,
    /// Host-level session-compaction policy for every agent this gateway
    /// builds (`{"auto_compact_threshold": 120000, ...}`). Absent, the
    /// gateway inherits meerkat's model-aware default —
    /// `context_window * 4 / 5`, i.e. `840_000` tokens on a million-token
    /// model, which in practice never fires. Validated at init through
    /// [`meerkat_meta::parse_compaction_policy`].
    compaction: Option<Value>,
    /// Meerkat host `config.toml` for every agent this gateway builds: the
    /// `[self_hosted]`, `[realm]` and `[models]` tables meerkat keeps out of
    /// `mob.toml`. Absent, `<workspace>/.rkat/config.toml` is adopted when it
    /// exists; otherwise agents build from meerkat's default config as
    /// before. A named file that is missing or malformed refuses init. A
    /// relative path resolves against the launch directory.
    meerkat_config_path: Option<PathBuf>,
    /// The provider credentials directory every agent this gateway builds
    /// resolves and persists provider credentials in, in place of the
    /// process default (`<config dir>/meerkat/credentials`, which every
    /// process of the OS user shares). An absolute path, opened once at
    /// init (the directory is created when missing); a root that cannot be
    /// opened refuses init, never falling back to the default. It joins the
    /// resume key: a launch never resumes a runtime that keeps its
    /// credentials elsewhere.
    provider_credentials_root: Option<PathBuf>,
    /// HTTP listener bind address, `HOST:PORT` (default `127.0.0.1:0`:
    /// loopback, ephemeral port). Wins over `--http-listen` and
    /// `META_HTTP_LISTEN_ADDR`. A non-loopback address is refused unless
    /// `allow_remote` acknowledges the exposure. Without auth_config the
    /// console is open, so loopback remains the default protection.
    http_listen: Option<String>,
    /// Explicit acknowledgement that `http_listen` exposes the listener
    /// beyond this host; also `--allow-remote` or `META_HTTP_ALLOW_REMOTE=1`.
    /// Same word and rule as `--allow-remote` on `rkat-rpc --tcp`.
    allow_remote: Option<bool>,
    /// Base URL clients reach this gateway at through a proxy. Reported back
    /// as `http_public_base_url` (and remembered for a resumed launch); never
    /// used to bind.
    http_public_base_url: Option<String>,
    /// Present ONLY so it can be refused.
    ///
    /// `meta_gateway` does not build the application tool-policy registry;
    /// `rpc_gateway` does. `InitParams` carries no `deny_unknown_fields`, so
    /// without this field serde drops the key and a host that compiled,
    /// reviewed and deployed an artifact gets no registry, no error and no
    /// warning - the exact "looks deployed, does nothing" state the
    /// wrong-id-space refusal in `member_tool_policy` exists to prevent, one
    /// layer up and on the other binary. Refusing is not a narrower feature
    /// than implementing it here; it is the difference between a host knowing
    /// and not knowing.
    #[serde(default)]
    application_tool_policies: Option<Value>,
    /// Present ONLY so it can be refused. An authenticated prepared
    /// application needs this binary's roster owner (`MutableRosterProvider`)
    /// and its live-registry resume comparison to carry the authenticated
    /// selection, and neither does yet; accepting it would let a resumed init
    /// or the workspace definition stand in for the authenticated one. It
    /// takes effect on the rpc_gateway binary, through its native host.
    #[serde(default)]
    prepared_application: Option<Value>,
    /// Present ONLY so it can be refused, same rule as `application_tool_policies`.
    ///
    /// The whole `runtime_options` object (demo_llm, max_sessions,
    /// identity_bootstrap_mode, experimental_live, and #376's
    /// declare_spec_update) is an rpc_gateway protocol; this binary parses none
    /// of it. `InitParams` carries no `deny_unknown_fields`, so without this
    /// field serde dropped the object and every option inside it in silence -
    /// an operator who declared a spec update against this binary would boot into
    /// the exact pinned-definition refusal the declaration exists to clear, with
    /// no sign it was ever read. Both SDKs let the operator point at either
    /// binary (`gateway_bin` / `gatewayBin`), so the wrong-binary case is real.
    #[serde(default)]
    runtime_options: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct RuntimeRegistry {
    entries: Vec<RuntimeRegistryEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RuntimeRegistryEntry {
    key: String,
    runtime_id: String,
    http_base_url: String,
    pid: u32,
    updated_at_ms: u64,
    /// Dialable cross-mob control-listener address of the live gateway, so
    /// a resumed launch can still report it. Serde-defaulted: registries
    /// written by older gateways simply resume with no control address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    control_listen_address: Option<String>,
    /// Proxy-facing base URL the live gateway was launched with, so a
    /// resumed launch reports the same one. Serde-defaulted for registries
    /// written by older gateways.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    http_public_base_url: Option<String>,
}

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn short_hash(value: &str) -> String {
    value.chars().take(8).collect()
}

fn load_registry(path: &Path) -> RuntimeRegistry {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Escalation routes from the optional `[escalation]` table of
/// `config/gating.toml`. Other tables in that file are left to their own
/// readers; an unknown key inside `[escalation]` fails the load.
fn load_gating_escalation_routes(
    path: &Path,
) -> anyhow::Result<meerkat_meta::GatingEscalationRoutes> {
    #[derive(serde::Deserialize)]
    struct GatingToml {
        #[serde(default)]
        escalation: Option<meerkat_meta::GatingEscalationRoutes>,
    }
    let text = std::fs::read_to_string(path)?;
    let parsed: GatingToml = toml::from_str(&text)?;
    let routes = parsed.escalation.unwrap_or_default();
    routes.validate().map_err(|error| anyhow!(error))?;
    Ok(routes)
}

fn save_registry(path: &Path, registry: &RuntimeRegistry) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(registry)?;
    fs::write(path, text)?;
    Ok(())
}

async fn url_is_alive(url: &str) -> bool {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
    {
        Ok(client) => client,
        Err(_) => return false,
    };

    client
        .get(format!("{}/healthz", url.trim_end_matches('/')))
        .send()
        .await
        .map(|response| response.status().is_success())
        .unwrap_or(false)
}

fn conventional_paths(workspace_root: &Path) -> ConventionalPaths {
    ConventionalPaths::discover(
        workspace_root.join("config"),
        workspace_root.join("deployment"),
    )
    .with_meerkat_config_from_workspace(workspace_root)
}

fn collect_recursive_files(root: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_recursive_files(&path, files);
        } else if path.is_file() {
            files.push(path);
        }
    }
}

/// The resume key of a launch with `--allow-flow-editor-host-deploy`. Such a
/// runtime lets callers without an access grant run `rkat` on this host, so
/// it must never answer a launch that did not ask for that, and a launch that
/// did must get a runtime that allows it. Keys of launches without the flag
/// stay as they were, so their live runtimes keep resuming.
fn resume_key_with_flow_editor_host_deploy(key: String, allowed: bool) -> String {
    if allowed {
        format!(
            "{:x}",
            Sha256::digest(format!("{key}\nflow-editor-host-deploy"))
        )
    } else {
        key
    }
}

/// A launch that names a provider credentials root resumes only a runtime
/// whose agents keep their credentials under that same root; without one
/// the key is unchanged.
fn resume_key_with_provider_credentials_root(key: String, root: Option<&Path>) -> String {
    match root {
        Some(root) => {
            let mut digest = Sha256::new();
            digest.update(key.as_bytes());
            digest.update(b"\nprovider-credentials-root\n");
            digest.update(root.as_os_str().as_encoded_bytes());
            format!("{:x}", digest.finalize())
        }
        None => key,
    }
}

#[allow(clippy::too_many_arguments)]
fn config_fingerprint(
    workspace_root: &Path,
    realm: Option<&str>,
    isolated: bool,
    runtime_profile: &str,
    persistent_sessions: bool,
    console_read_only: bool,
    runtime_root: &Path,
    store_path: &Path,
    project_root: &Path,
    context_root: Option<&Path>,
    compaction: Option<&meerkat_core::config::CompactionRuntimeConfig>,
    control_listen: Option<&ControlListenAddr>,
    http_listen: SocketAddr,
    http_public_base_url: Option<&str>,
    paths: &ConventionalPaths,
) -> anyhow::Result<String> {
    let mut hasher = Sha256::new();
    let realpath = workspace_root
        .canonicalize()
        .unwrap_or_else(|_| workspace_root.to_path_buf());
    hasher.update(realpath.to_string_lossy().as_bytes());
    hasher.update(b"\n");
    hasher.update(realm.unwrap_or("").as_bytes());
    hasher.update(b"\n");
    hasher.update(if isolated { b"1" } else { b"0" });
    hasher.update(b"\n");
    hasher.update(runtime_profile.as_bytes());
    hasher.update(b"\n");
    hasher.update(if persistent_sessions { b"1" } else { b"0" });
    hasher.update(b"\n");
    hasher.update(if console_read_only { b"1" } else { b"0" });
    hasher.update(b"\n");
    hasher.update(runtime_root.to_string_lossy().as_bytes());
    hasher.update(b"\n");
    hasher.update(store_path.to_string_lossy().as_bytes());
    hasher.update(b"\n");
    hasher.update(project_root.to_string_lossy().as_bytes());
    hasher.update(b"\n");
    if let Some(ctx) = context_root {
        hasher.update(ctx.to_string_lossy().as_bytes());
    }
    hasher.update(b"\n");
    // The compaction policy is baked into every agent this runtime builds, so
    // a launch that changes it must NOT resume a runtime built with the old
    // one — that would be the dead-knob failure with extra steps.
    if let Some(policy) = compaction {
        hasher.update(policy.auto_compact_threshold.to_le_bytes());
        hasher.update(if policy.auto_compact_threshold_explicit {
            b"1"
        } else {
            b"0"
        });
        hasher.update(policy.recent_turn_budget.to_le_bytes());
        hasher.update(policy.max_summary_tokens.to_le_bytes());
        hasher.update(policy.min_turns_between_compactions.to_le_bytes());
        hasher.update(
            u64::try_from(policy.history_retained_rewrites)
                .unwrap_or(u64::MAX)
                .to_le_bytes(),
        );
    }
    hasher.update(b"\n");
    // The control listener binds at launch; resuming a live gateway that was
    // launched without (or with a different) --control-listen would silently
    // drop the flag, so the address participates in the resume key.
    if let Some(addr) = control_listen {
        hasher.update(addr.to_string().as_bytes());
    }
    hasher.update(b"\n");
    // The HTTP listener likewise binds at launch: a relaunch that asks for a
    // different address must not resume a live runtime bound somewhere else
    // and report that one as if it were the requested bind.
    hasher.update(http_listen.to_string().as_bytes());
    hasher.update(b"\n");
    // The advertised base is reported back and remembered in the registry; a
    // launch that declares a different one must not be answered with the
    // previous launch's value, so it joins the key beside the listen address.
    hasher.update(http_public_base_url.unwrap_or("").as_bytes());
    hasher.update(b"\n");
    hasher.update(env!("CARGO_PKG_VERSION").as_bytes());

    let definition_json = workspace_root.join("definition.json");
    if paths.mob_toml.is_some() {
        hasher.update(b"\nworkspace-config");
    } else if definition_json.exists() {
        hasher.update(b"\ndefinition-json");
    } else {
        // Version the generated fallback runtime separately so local TUX
        // launches do not resume older minimal runtimes after capability
        // changes such as new profiles, tools, or wiring defaults.
        hasher.update(b"\nfallback-template:");
        hasher.update(FALLBACK_TEMPLATE_VERSION.as_bytes());
    }

    let mut files = Vec::new();
    if let Some(path) = &paths.mob_toml {
        files.push(path.clone());
    }
    if let Some(path) = &paths.gating_toml {
        files.push(path.clone());
    }
    if let Some(path) = &paths.console_toml {
        files.push(path.clone());
    }
    if let Some(path) = &paths.routing_toml {
        files.push(path.clone());
    }
    files.extend(paths.schedule_files.clone());
    // The host config is baked into every agent (self-hosted aliases, realm
    // bindings, custom models), so a changed or re-pointed config.toml must
    // not resume a runtime built from the old one. `.rkat/` is outside the
    // scanned config roots below, so it rides here by name.
    if let Some(path) = &paths.meerkat_config_toml {
        files.push(path.clone());
    }
    if definition_json.exists() {
        files.push(definition_json);
    }
    let manifest_toml = workspace_root.join("manifest.toml");
    if manifest_toml.exists() {
        files.push(manifest_toml);
    }
    // Scan workspace root and any override roots for config files
    let mut scan_roots = vec![workspace_root.to_path_buf()];
    if project_root != workspace_root {
        scan_roots.push(project_root.to_path_buf());
    }
    if let Some(ctx) = context_root
        && ctx != workspace_root
    {
        scan_roots.push(ctx.to_path_buf());
    }
    for root in &scan_roots {
        for extra_dir in ["skills", "hooks", "mcp", "config"] {
            collect_recursive_files(&root.join(extra_dir), &mut files);
        }
    }
    files.sort();

    for path in files {
        hasher.update(b"\nfile:");
        hasher.update(path.to_string_lossy().as_bytes());
        if let Ok(bytes) = fs::read(&path) {
            hasher.update(b"\n");
            hasher.update(bytes);
        }
    }

    Ok(format!("{:x}", hasher.finalize()))
}

fn minimal_definition(runtime_id: &str) -> anyhow::Result<MobDefinition> {
    MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "{runtime_id}"
orchestrator = "alpha"

[profiles.alpha]
model = "gpt-5.5"
skills = ["alpha-role"]
peer_description = "Runtime guide -- expands this runtime into a small mob and coordinates peers"
external_addressable = true

[profiles.alpha.tools]
builtins = true
comms = true
mob = true
mob_tasks = true

[profiles.worker]
model = "gpt-5.5"
skills = ["worker-role"]
peer_description = "General-purpose peer meerkat"
external_addressable = true

[profiles.worker.tools]
builtins = true
comms = true
mob_tasks = true

[wiring]
auto_wire_orchestrator = true

[skills.alpha-role]
source = "inline"
content = """
## Role
You are Alpha, the runtime guide for a lightweight Meerkat workspace.

## What You Can Do
- Answer directly when the job is simple.
- Grow the runtime into a small mob when parallel work helps.
- Spawn classic sub-agents for delegated background work.
- Spawn peer meerkats when a longer-lived collaborator should appear in the runtime.

## Preferred Growth Pattern
- For quick delegated work, use sub-agent tools.
- For visible collaborators inside this runtime, use mob tools to spawn worker peers.
- When you spawn worker peers, they should appear in the shared runtime UI.

## Coordination
- Use mob tools to spawn, list, wire, and retire meerkats.
- Use peers() and send() when peers are available.
- If asked to create a small team, prefer spawning `worker` peers unless the user clearly asks for classic sub-agents.

## Communication Style
Be explicit about whether you used a sub-agent or spawned a peer meerkat.
"""

[skills.worker-role]
source = "inline"
content = """
You are a general-purpose worker meerkat inside a lightweight runtime.
Complete assigned tasks concisely and report status back to Alpha.
If peer messaging is available, use it to report completion or blockers.
"""
"#
    ))
    .map_err(|error| anyhow!("invalid fallback mob definition: {error}"))
}

fn load_definition(
    workspace_root: &Path,
    fingerprint: &str,
    paths: &ConventionalPaths,
) -> anyhow::Result<(MobDefinition, bool)> {
    if let Some(path) = &paths.mob_toml {
        let text = fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        // The init refusal reports this error's Display, which for a context
        // error is the context alone; carry meerkat's refusal (and the
        // upgrade note for a refused profile prompt key) in the message.
        let definition = MobDefinition::from_toml(&text).map_err(|error| {
            anyhow!(
                "failed to parse {}: {}",
                path.display(),
                meerkat_meta::gateway_composition::describe_mob_definition_error(&error)
            )
        })?;
        // The definition parser drops `[self_hosted]` and `[realm]` without a
        // diagnostic; refuse them by name (the typed message names
        // `meerkat_config_path` as the file that carries them).
        meerkat_meta::gateway_composition::refuse_host_config_tables_in_mob_toml(&text)
            .map_err(|table| anyhow!("{}: {table}", path.display()))?;
        return Ok((definition, true));
    }

    let definition_json_path = workspace_root.join("definition.json");
    if definition_json_path.exists() {
        let text = fs::read_to_string(&definition_json_path)
            .with_context(|| format!("failed to read {}", definition_json_path.display()))?;
        let definition = serde_json::from_str::<MobDefinition>(&text)
            .with_context(|| format!("failed to parse {}", definition_json_path.display()))?;
        return Ok((definition, true));
    }

    let runtime_id = format!("tux-{}", short_hash(fingerprint));
    Ok((minimal_definition(&runtime_id)?, false))
}

/// The `meerkat::Config` every agent this gateway builds starts from: the
/// host `config.toml` when one was resolved (the only carrier of
/// `[self_hosted]`, `[realm]` and `[models]`), else meerkat's default, with
/// the gateway's compaction declaration layered on top. One producer for both
/// launch arms, so the persistent and ephemeral paths cannot drift on which
/// tables reach the factory.
fn gateway_agent_config(
    host_config: Option<&Config>,
    compaction: Option<&meerkat_core::config::CompactionRuntimeConfig>,
) -> anyhow::Result<Config> {
    let mut config = host_config.cloned().unwrap_or_default();
    if let Some(policy) = compaction {
        // The compaction slot of this config is what meerkat's
        // `AgentFactory::build_agent` turns into the session compactor; an
        // absent declaration inherits the model-aware `context_window * 4 / 5`
        // trigger.
        meerkat_meta::apply_compaction_policy(&mut config, policy).map_err(|e| anyhow!("{e}"))?;
    }
    // This gateway builds its own `FactoryAgentBuilder` from this config, so
    // Meta's transcript retention default is applied here as well.
    Ok(meerkat_meta::with_meta_compaction_defaults(config))
}

/// The builder every member of this gateway is built through, in the
/// persistent and the ephemeral arm alike. Each construction is prepared
/// only after its configured MCP servers have connected or failed, so the
/// surface captured for it carries them.
fn member_agent_builder(factory: AgentFactory, config: Config) -> FactoryAgentBuilder {
    let mut builder = FactoryAgentBuilder::new(factory, config);
    builder.wait_for_mcp = true;
    builder
}

/// Returns (session_service, runtime_adapter, binary_blob_store).
///
/// The session service and explicit runtime adapter share the same runtime
/// store. The service acquires that adapter before its schedule, voice, and
/// mob hosts are composed.
#[allow(clippy::too_many_arguments)]
fn build_persistent_session_service(
    layout: &MetaStorageLayout,
    runtime_root: PathBuf,
    project_root: PathBuf,
    context_root: Option<PathBuf>,
    image_generation: bool,
    mcp_apps: bool,
    realm_id: &str,
    host_config: Option<&Config>,
    compaction: Option<&meerkat_core::config::CompactionRuntimeConfig>,
    member_mcp_custody: &Arc<meerkat_meta::MemberMcpCustody>,
    provider_auth_persistence: Option<meerkat::ProviderAuthPersistence>,
) -> anyhow::Result<PersistentSessionServiceParts> {
    let store_dir = layout.state_dir().to_path_buf();
    fs::create_dir_all(&store_dir)
        .with_context(|| format!("failed to create {}", store_dir.display()))?;
    // Core's HeadCanonical pair on `runtime.sqlite`: the session store and
    // the runtime store share the one database, as Core's realm composes
    // them. Fail-closed (M4): an open failure is a startup error; this
    // surface's ephemeral launch mode (persistent_sessions = false) is the
    // declared in-memory path.
    let meerkat_realm = layout
        .meerkat_level_realm()
        .map_err(|e| anyhow!("{e}"))?
        .clone();
    let (session_store, runtime_store) =
        meerkat_meta::mob_handle_runtime::open_native_session_pair(&store_dir, &meerkat_realm)
            .map_err(|error| anyhow!("{error}"))?;
    // H2: probe the incremental capability on the same store the session
    // service receives below, so whole-blob degradation is loud and
    // health-visible.
    let session_store_incremental = meerkat_meta::storage_health::probe_session_store_incremental(
        &session_store,
        "SqliteSessionStore",
    );

    let binary_blob_store: Arc<dyn BinaryBlobStore> =
        Arc::new(ObjectStoreBlobStore::local(layout.blob_root())?);
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(Base64BlobStoreAdapter::new(binary_blob_store.clone()));
    let adapter = Arc::new(
        meerkat_runtime::MeerkatMachine::persistent(
            Arc::clone(&runtime_store),
            Arc::clone(&blob_store),
        )
        .context("failed to acquire the gateway runtime machine")?,
    );
    let mut factory = AgentFactory::new(store_dir)
        .session_store(session_store.clone())
        .runtime_root(runtime_root)
        .project_root(project_root)
        .builtins(true)
        .shell(true)
        .mob(true)
        .comms(true)
        .memory(true)
        .mcp_apps(mcp_apps);
    // The launch's provider credential store replaces the process default
    // before the factory is cloned for live or consumed by its builder.
    if let Some(persistence) = provider_auth_persistence {
        factory = factory.with_provider_auth_persistence(persistence);
    }
    if image_generation {
        factory = factory.with_image_generation_machine(
            meerkat_meta::mob_handle_runtime::weak_image_generation_machine(&adapter),
        );
    }
    if let Some(context_root) = context_root {
        factory = factory.context_root(context_root);
    }
    // Member MCP routers go into the gateway's custody, which the runtime's
    // shutdown (and again the executor-exit boundary) closes and settles.
    factory = member_mcp_custody.install(factory);

    let config = gateway_agent_config(host_config, compaction)?;
    let live_factory = factory.clone();
    let live_config = config.clone();
    let mut builder = member_agent_builder(factory, config);
    let host_default_decorator = Arc::clone(&builder.default_agent_llm_client_decorator);
    builder.default_blob_store = Some(blob_store.clone());
    let jobs_path =
        meerkat_store::realm_paths_in(layout.state_dir(), meerkat_realm.as_str()).jobs_sqlite_path;
    let jobs: Arc<dyn meerkat::DetachedJobStore> = Arc::new(
        meerkat::SqliteDetachedJobStore::open(jobs_path.clone())
            .with_context(|| format!("failed to open {}", jobs_path.display()))?,
    );
    let runtime_delivery = meerkat_meta::mob_handle_runtime::MobRuntimeDelivery::new(
        Arc::clone(&runtime_store),
        Arc::clone(&jobs),
    );
    builder.default_detached_job_store = Some(Arc::clone(&jobs));
    builder.default_shell_job_delivery_projector =
        Some(meerkat::JobOutboxProjector::new_for_realm(
            jobs,
            runtime_delivery.inbox(),
            meerkat_realm.as_str(),
        ));

    // Attach meerkat's per-session schedule tools so members whose profile sets
    // tools.schedule=true get the meerkat_schedule_* surface; the returned
    // service backs the firing host spawned once the runtime has booted.
    let (schedule_tools, schedule_slot) =
        match meerkat_meta::schedule_wiring::attach_schedule_tools_with_identity_targets_reporting(
            &builder,
            layout.state_dir(),
        ) {
            Ok(tools) => (
                Some(tools),
                meerkat_meta::storage_health::StorageSlotSummary::persistent(
                    "schedule",
                    "SqliteScheduleStore",
                ),
            ),
            Err(error) => (
                None,
                meerkat_meta::storage_health::StorageSlotSummary::degraded(
                    "schedule",
                    format!("schedule store failed to open; schedule tools disabled: {error}"),
                ),
            ),
        };
    // WorkGraph: durable store beside the schedule store, realm scoped to
    // the mob definition id (member tools + overlays + console RPCs). The
    // state dir travels along so the bootstrap spec can place the
    // cross-process admission sidecar beside the store.
    let workgraph_state_dir = layout.state_dir().to_path_buf();
    let (workgraph, workgraph_slot) =
        match meerkat_meta::workgraph_wiring::attach_workgraph_tools_reporting(
            &builder,
            &workgraph_state_dir,
            realm_id,
        ) {
            Ok((service, admission_slot)) => (
                Some((service, admission_slot, workgraph_state_dir)),
                meerkat_meta::storage_health::StorageSlotSummary::persistent(
                    "workgraph",
                    "SqliteWorkGraphStore",
                ),
            ),
            Err(error) => (
                None,
                meerkat_meta::storage_health::StorageSlotSummary::degraded(
                    "workgraph",
                    format!("workgraph store failed to open; workgraph disabled: {error}"),
                ),
            ),
        };
    let agent_mob_tools_slot = Arc::clone(&builder.default_mob_tools);
    let service = Arc::new(PersistentSessionService::new(
        builder,
        64,
        session_store,
        Arc::clone(&runtime_store),
        blob_store,
    ));
    service
        .acquire_canonical_runtime_adapter(Some(Arc::clone(&adapter)))
        .context("failed to acquire the persistent gateway session runtime owner")?;
    let schedule_host_inputs = schedule_tools.map(|tools| {
        (
            tools.service,
            tools.mob_target_registry,
            Arc::clone(&service),
            layout.schedule_db(),
            tools.firing_host_binding,
        )
    });
    // Blob slot resolved fail-closed above (local disk under
    // <store_dir>/blobs). The console timeline and metadata cursor of this
    // surface are in-memory by contract (`UnifiedRuntime::bootstrap` keeps
    // its defaults) — a declared default, documented and health-visible
    // (M4), not an error.
    let mut slots = vec![
        meerkat_meta::storage_health::StorageSlotSummary::persistent(
            "sessions",
            "SqliteSessionStore",
        ),
        meerkat_meta::storage_health::StorageSlotSummary::persistent(
            "runtime",
            "SqliteRuntimeStore",
        ),
        meerkat_meta::storage_health::StorageSlotSummary::persistent(
            "jobs",
            "SqliteDetachedJobStore",
        ),
        meerkat_meta::storage_health::blob_slot_summary(
            meerkat_meta::storage_health::BlobDurability::PersistentDisk,
        ),
        meerkat_meta::storage_health::StorageSlotSummary::declared_ephemeral(
            "console",
            "InMemoryConsoleLogStore",
            "declared default of this surface (UnifiedRuntime::bootstrap keeps in-memory \
             console/metadata)",
        ),
        meerkat_meta::storage_health::StorageSlotSummary::declared_ephemeral(
            "metadata",
            "InMemoryMetadataStore",
            "declared default of this surface (UnifiedRuntime::bootstrap keeps in-memory \
             console/metadata)",
        ),
        schedule_slot,
        workgraph_slot,
        meerkat_meta::storage_health::StorageSlotSummary::persistent(
            "jobs",
            "SqliteDetachedJobStore",
        ),
    ];
    slots.extend(meerkat_meta::storage_health::scratch_ring_buffer_slots());
    // Heal seam (2026-07-29 incident): the CONCRETE persistent service is the
    // committed-boundary recoverer; cast here, before the tuple erases it to
    // `dyn MobSessionService` (mirrors rpc_gateway.rs).
    let committed_boundary_recoverer: Arc<
        dyn meerkat_meta::identity_first::CommittedBoundaryRecoverer,
    > = service.clone();
    let voice_inputs = (
        Arc::clone(&service),
        Arc::clone(&adapter),
        live_factory,
        live_config,
        agent_mob_tools_slot,
    );
    Ok((
        service,
        adapter,
        binary_blob_store,
        schedule_host_inputs,
        workgraph,
        meerkat_meta::storage_health::ResolvedStorageSummary::new(
            meerkat_meta::storage_health::BlobDurability::PersistentDisk,
            Some(session_store_incremental),
        )
        .with_slots(slots)
        .with_runtime_store_locator(Some(
            meerkat_meta::storage_health::RuntimeStoreLocator::SqliteFile {
                path: layout.runtime_db(),
            },
        )),
        committed_boundary_recoverer,
        runtime_store,
        voice_inputs,
        runtime_delivery,
        host_default_decorator,
    ))
}

/// Session-store naming the standalone gateway projects into its session-store
/// descriptor. Read at serve time, so it is pinned here rather than inherited
/// from the shared constructor's SDK-gateway default.
const TUX_BIGQUERY_DATASET: &str = "tux_local";
const TUX_BIGQUERY_TABLE: &str = "runtime_events";

/// Default standalone console snapshot. Explicit auth_config replaces its
/// authentication fields at launch; otherwise the loopback host is the
/// protection. Shared non-auth defaults come from the keyless snapshot owner.
fn runtime_decision_state(
    console_ui: ConsoleUiConfig,
    console_read_only: bool,
) -> RuntimeDecisionState {
    RuntimeDecisionState::local_console(
        ConsolePolicy {
            require_app_auth: false,
            read_only: console_read_only,
            fetch_timeout_ms: None,
            ui: console_ui,
        },
        Some(BigQueryNaming {
            dataset: TUX_BIGQUERY_DATASET.to_string(),
            table: TUX_BIGQUERY_TABLE.to_string(),
        }),
    )
}

fn print_json_line(value: &Value) {
    let line = serde_json::to_string(value)
        .unwrap_or_else(|_| r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32603,"message":"serialization failed"}}"#.to_string());
    let mut stdout = io::stdout().lock();
    let _ = writeln!(stdout, "{line}");
    let _ = stdout.flush();
}

fn parse_init_request(line: &str) -> anyhow::Result<(Value, InitParams)> {
    let raw: Value = serde_json::from_str(line).context("failed to parse init request")?;
    let method = raw
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if method != "meta/init" {
        return Err(anyhow!("expected meta/init, got {method}"));
    }
    let params = raw.get("params").cloned().unwrap_or_else(|| json!({}));
    // Key presence, not the deserialized Option: an explicit null must be
    // refused like any other supplied value, not read as absence.
    let prepared_application_supplied = params.get("prepared_application").is_some();
    let parsed: InitParams = serde_json::from_value(params).context("invalid init params")?;
    if parsed.runtime_options.is_some() {
        return Err(anyhow!(
            "runtime_options is not implemented by meta_gateway; it takes effect only on the \
             rpc_gateway binary (demo_llm, max_sessions, identity_bootstrap_mode, \
             experimental_live, declare_spec_update). Refusing rather than ignoring: a silently \
             dropped option arms nothing while looking deployed, and a dropped \
             declare_spec_update leaves the pinned-definition refusal in place while the \
             operator believes they cleared it"
        ));
    }
    if prepared_application_supplied || parsed.prepared_application.is_some() {
        return Err(anyhow!(
            "prepared_application is not implemented by meta_gateway; it takes effect only \
             through rpc_gateway::run_persistent_with_host in a host-composed application (for \
             example toolkit_gateway), whose native host authenticates it. Refusing \
             rather than ignoring: this binary would otherwise start from its workspace \
             definition or resume an existing runtime while the host believes the prepared \
             application is in force"
        ));
    }
    if let Some(value) = parsed.mob_announcements.as_ref() {
        // The same parser and refusals as rpc_gateway's
        // runtime_options.mob_announcements. Enabled, it needs the durable
        // identity substrate and the persistent stores its database sits
        // beside.
        let enabled = meerkat_meta::mob_announcements::parse_options(value, "mob_announcements")
            .map_err(|error| anyhow!("{error}"))?
            .is_some();
        if enabled && parsed.identity_first == Some(false) {
            return Err(anyhow!(
                "mob_announcements needs the identity-first runtime and is refused with \
                 identity_first: false"
            ));
        }
        if enabled && parsed.persistent_sessions != Some(true) {
            return Err(anyhow!(
                "mob_announcements needs persistent_sessions: true: its database sits beside \
                 the persistent stores"
            ));
        }
    }
    if parsed.application_tool_policies.is_some() {
        return Err(anyhow!(
            "application_tool_policies is not implemented by meta_gateway; it takes effect only \
             on the rpc_gateway binary. Refusing rather than ignoring: a silently dropped policy \
             arms nothing while looking deployed, and the only later evidence would be an absence \
             of denials, which is what an armed and fully granted policy also produces"
        ));
    }
    if let Some(auth) = parsed.auth_config.as_ref() {
        meerkat_meta::console_auth_config::parse_console_auth_config(auth)
            .map_err(|error| anyhow!("{error}"))?;
    }
    if let Some(root) = parsed.provider_credentials_root.as_deref() {
        meerkat_meta::gateway_composition::validate_provider_credentials_root(root)
            .map_err(|error| anyhow!("provider_credentials_root: {error}"))?;
    }
    if let Some(voice) = parsed.console_voice.as_ref() {
        if !cfg!(feature = "openai-live") {
            return Err(anyhow!(
                "console_voice requires a gateway built with openai-live"
            ));
        }
        if !parsed.persistent_sessions.unwrap_or(false) || parsed.auth_config.is_none() {
            return Err(anyhow!(
                "console_voice requires persistent_sessions and auth_config"
            ));
        }
        meerkat_meta::public_live_config::PublicLiveRegistration::parse(voice)
            .map_err(|error| anyhow!("console_voice: {error}"))?;
    }
    Ok((raw.get("id").cloned().unwrap_or(Value::Null), parsed))
}

/// Resolve the HTTP listen address: init param, then `--http-listen`, then
/// `META_HTTP_LISTEN_ADDR`, then loopback on an ephemeral port. Pure so the
/// precedence is testable without touching the process environment. An empty
/// environment value counts as unset, like `env_bool`.
fn resolve_http_listen(
    init_param: Option<&str>,
    flag: Option<SocketAddr>,
    env_value: Option<&str>,
) -> Result<SocketAddr, String> {
    if let Some(value) = init_param {
        return meerkat_meta::gateway_composition::parse_http_listen_addr(value)
            .map_err(|error| error.to_string());
    }
    if let Some(addr) = flag {
        return Ok(addr);
    }
    match env_value.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => meerkat_meta::gateway_composition::parse_http_listen_addr(value)
            .map_err(|error| format!("META_HTTP_LISTEN_ADDR: {error}")),
        None => Ok(meerkat_meta::gateway_composition::DEFAULT_GATEWAY_HTTP_LISTEN),
    }
}

fn env_bool(name: &str) -> anyhow::Result<Option<bool>> {
    let Ok(value) = std::env::var(name) else {
        return Ok(None);
    };
    let normalized = value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "" => Ok(None),
        "1" | "true" | "yes" | "on" => Ok(Some(true)),
        "0" | "false" | "no" | "off" => Ok(Some(false)),
        _ => Err(anyhow!("{name} must be a boolean value")),
    }
}

fn init_response(
    request_id: Value,
    runtime_id: &str,
    http_base_url: &str,
    launch_state: &str,
    control_listen_address: Option<&str>,
    http_public_base_url: Option<&str>,
) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "result": {
            "contract_version": META_CONTRACT_VERSION,
            "runtime_id": runtime_id,
            "http_base_url": http_base_url,
            // Proxy-facing base the launch advertised (http_public_base_url
            // init param); null when none was declared. Never bound.
            // Wire-additive optional field.
            "http_public_base_url": http_public_base_url,
            "launch_state": launch_state,
            // Dialable address of the cross-mob control listener when the
            // gateway was launched with --control-listen (real bound port
            // for tcp://host:0); null otherwise. Peers put this address in
            // their contact directories. Wire-additive optional field.
            "control_listen_address": control_listen_address,
        }
    })
}

/// Marker error: `run()` has already written the init reply for this failure
/// on the request id (`-32602` for a refused declaration, `-32603` for a
/// failed bind), so `main` must not write a second, id-less one.
#[derive(Debug)]
struct InitReplied;

impl std::fmt::Display for InitReplied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("meta/init was refused; the reply is already on stdout")
    }
}

impl std::error::Error for InitReplied {}

/// Write a typed init refusal on the request id and return the marker
/// `run()` propagates. Invalid declarations are `-32602` (the same code
/// `rpc_gateway` answers with), so an SDK gets a caller action rather than an
/// "internal error" with a null id.
fn refuse_init(request_id: &Value, code: i64, message: String) -> anyhow::Error {
    tracing::warn!(code, %message, "meta/init refused");
    print_json_line(&init_error(request_id.clone(), code, message));
    anyhow::Error::new(InitReplied)
}

fn init_error(request_id: Value, code: i64, message: String) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "error": {
            "code": code,
            "message": message,
        }
    })
}

const STORAGE_MIGRATE_USAGE: &str = "usage: meta_gateway storage-migrate --state-dir <dir> \
     [--apply] [--adopt <path>] [--session-db <file>] [--json]\n\
     Fenced offline migration of one Meta state directory \
     (storage-unification M6): ledger baseline, legacy-spelling renames, \
     twin reconciliation, leftover census.\n\
     Dry-run by default; --apply mutates under the exclusive maintenance \
     fence. --adopt <path> resolves a divergent file-name twin by adopting \
     that copy and archiving the rest read-only (requires --apply).\n\
     --session-db <file> takes the session database a 0.9.3 meta_gateway's \
     store_path named into the layout as sessions.sqlite3 (a twin of any other \
     sessions copy), then leaves a retired stamp at its old path that a 0.9.3 \
     gateway refuses. Stop every process using the directory first.\n\
     --acknowledge-skipped <SESSION_ID> (repeatable) authorises the ledger \
     bump to proceed despite a blob row that cannot be parsed as a session. \
     Acknowledgement is BY ROW ID, never by count: a row you did not name \
     still blocks, so a later run cannot silently authorise a different set \
     than the one you read.\n\
     Exit codes: 0 clean, 1 refusals or fence/store failure, 2 usage error.";

/// Maintenance verb: `meta_gateway storage-migrate`. Runs the five-case
/// M6 migration pass and prints the report. Like the H3 verb, it is a
/// standalone argv verb bypassing the stdin init handshake — migration is
/// never an eager side effect of ordinary gateway startup.
fn run_storage_migrate(args: &[String]) -> i32 {
    let mut state_dir: Option<PathBuf> = None;
    let mut adopt: Option<PathBuf> = None;
    let mut session_db: Option<PathBuf> = None;
    let mut apply = false;
    let mut json = false;
    let mut acknowledged_rows: std::collections::BTreeSet<String> =
        std::collections::BTreeSet::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--state-dir" => match iter.next() {
                Some(value) => state_dir = Some(PathBuf::from(value)),
                None => {
                    eprintln!("--state-dir requires a directory\n{STORAGE_MIGRATE_USAGE}");
                    return 2;
                }
            },
            "--adopt" => match iter.next() {
                Some(value) => adopt = Some(PathBuf::from(value)),
                None => {
                    eprintln!("--adopt requires a path\n{STORAGE_MIGRATE_USAGE}");
                    return 2;
                }
            },
            "--session-db" => match iter.next() {
                Some(value) => session_db = Some(PathBuf::from(value)),
                None => {
                    eprintln!("--session-db requires a file\n{STORAGE_MIGRATE_USAGE}");
                    return 2;
                }
            },
            "--acknowledge-skipped" => match iter.next() {
                Some(value) => {
                    acknowledged_rows.insert(value.clone());
                }
                None => {
                    eprintln!(
                        "--acknowledge-skipped requires a session id\n{STORAGE_MIGRATE_USAGE}"
                    );
                    return 2;
                }
            },
            "--apply" => apply = true,
            "--json" => json = true,
            "--help" | "-h" => {
                println!("{STORAGE_MIGRATE_USAGE}");
                return 0;
            }
            other => {
                eprintln!("unknown argument {other:?}\n{STORAGE_MIGRATE_USAGE}");
                return 2;
            }
        }
    }
    let Some(state_dir) = state_dir else {
        eprintln!("--state-dir is required\n{STORAGE_MIGRATE_USAGE}");
        return 2;
    };
    if adopt.is_some() && !apply {
        eprintln!("--adopt requires --apply\n{STORAGE_MIGRATE_USAGE}");
        return 2;
    }
    let mode = if apply {
        meerkat_meta::MigrateMode::Apply
    } else {
        meerkat_meta::MigrateMode::DryRun
    };
    let report = meerkat_meta::migrate_state_dir_with_session_db(
        &state_dir,
        mode,
        adopt.as_deref(),
        &acknowledged_rows,
        session_db.as_deref(),
    );
    if json {
        match serde_json::to_string_pretty(&report) {
            Ok(text) => println!("{text}"),
            Err(error) => {
                eprintln!("failed to serialize migrate report: {error}");
                return 1;
            }
        }
    } else {
        print_migrate_report_text(&report);
    }
    i32::from(report.has_errors())
}

fn print_migrate_report_text(report: &meerkat_meta::MetaMigrateReport) {
    let mode = match report.mode {
        meerkat_meta::MigrateMode::Apply => "apply",
        _ => "dry-run",
    };
    println!(
        "Storage migrate ({mode}) over {} ({} database(s) fenced):",
        report.state_dir.display(),
        report.fenced_databases.len()
    );
    if let Some(session_db) = &report.session_db {
        let path = session_db.path.display();
        match &session_db.outcome {
            meerkat_meta::SessionDbOutcome::WouldMove => {
                println!("session database {path}: joins the sessions slot (see renames/twins)");
            }
            meerkat_meta::SessionDbOutcome::Moved { tripwire } => println!(
                "session database {path}: moved into the layout; retired stamp left at {}",
                tripwire.display()
            ),
            meerkat_meta::SessionDbOutcome::AlreadyMoved => {
                println!("session database {path}: already moved (it holds the retired stamp)");
            }
            meerkat_meta::SessionDbOutcome::AlreadyInLayout => {
                println!("session database {path}: already in the layout");
            }
            meerkat_meta::SessionDbOutcome::TripwireOwed => println!(
                "session database {path}: an earlier run moved it and did not write its retired \
                 stamp; --apply writes it"
            ),
            meerkat_meta::SessionDbOutcome::Refused { refusal } => {
                println!("session database {path}: REFUSED: {refusal}");
            }
            _ => println!("session database {path}: {:?}", session_db.outcome),
        }
    }
    if let Some(backfill) = &report.head_canonical_backfill {
        // The head-canonical crossing is the one irreversible thing this verb
        // does, so its outcome is printed before anything else and states
        // explicitly whether the ledger advanced.
        println!(
            "head-canonical backfill: {} converted of {} pending",
            backfill.converted.len(),
            backfill.examined
        );
        if !backfill.skipped_unparseable.is_empty() {
            println!(
                "  {} blob row(s) could not be parsed as sessions and are NOT converted; \
                 the ledger will not advance until each is acknowledged by id \
                 (--acknowledge-skipped <SESSION_ID>): {}",
                backfill.skipped_unparseable.len(),
                backfill.skipped_unparseable.join(", ")
            );
        }
        for (session_id, failure) in &backfill.failures {
            if session_id.is_empty() {
                println!("  REFUSED: {failure}");
            } else {
                println!("  FAILED {session_id}: {failure}");
            }
        }
        if backfill.ledger_stamped {
            println!(
                "  ledger STAMPED v2 - the corpus is wholly head-canonical; \
                 rollback to a pre-head-canonical release is no longer possible"
            );
        } else if backfill.applied {
            println!(
                "  ledger LEFT AT v1 - the crossing is incomplete, so rollback \
                 remains available; re-run to resume"
            );
        }
    }
    for twin in &report.twins {
        println!("twin [{}]:", twin.slot);
        for path in &twin.paths {
            println!("  copy: {}", path.display());
        }
        println!(
            "  rows: {} equal across all copies; byte-identical: {}",
            twin.rows_equal, twin.byte_identical
        );
        for row in &twin.rows {
            let status = match &row.status {
                meerkat_meta::DivergenceStatus::Equal => "equal".to_string(),
                meerkat_meta::DivergenceStatus::Divergent => "divergent".to_string(),
                meerkat_meta::DivergenceStatus::OnlyIn { location } => {
                    format!("only in {}", location.display())
                }
                _ => "unknown".to_string(),
            };
            println!("    {}: {status}", row.key);
        }
        match &twin.resolution {
            meerkat_meta::TwinResolution::Refused { reason } => {
                println!("  resolution: REFUSED — {reason}");
            }
            meerkat_meta::TwinResolution::Deduped { kept, archived } => {
                println!("  resolution: deduped, kept {}", kept.display());
                for archive in archived {
                    println!("    archived read-only: {}", archive.display());
                }
            }
            meerkat_meta::TwinResolution::Adopted { adopted, archived } => {
                println!("  resolution: adopted {}", adopted.display());
                for archive in archived {
                    println!("    archived read-only: {}", archive.display());
                }
            }
            _ => println!("  resolution: unknown"),
        }
        for note in &twin.notes {
            println!("  note: {note}");
        }
        for error in &twin.errors {
            println!("  error: {error}");
        }
    }
    for rename in &report.renames {
        let action = match rename.action {
            meerkat_meta::RenameAction::WouldRename => "would rename",
            meerkat_meta::RenameAction::Renamed => "renamed",
            meerkat_meta::RenameAction::Refused => "REFUSED",
            _ => "unknown",
        };
        println!(
            "rename [{}]: {} -> {} ({action}, {} sibling(s), wal checkpointed: {})",
            rename.slot,
            rename.from.display(),
            rename.to.display(),
            rename.siblings.len(),
            rename.wal_checkpointed
        );
    }
    for entry in &report.ledger {
        let action = match entry.action {
            meerkat_meta::LedgerBaselineAction::WouldStamp => "would-stamp",
            meerkat_meta::LedgerBaselineAction::Recorded => "recorded",
            meerkat_meta::LedgerBaselineAction::Stamped => "stamped",
            meerkat_meta::LedgerBaselineAction::AlreadyCurrent => "already-current",
            meerkat_meta::LedgerBaselineAction::ReportOnly => "report-only",
            meerkat_meta::LedgerBaselineAction::Exempt => "exempt",
            _ => "unknown",
        };
        let describe =
            |version: Option<i64>| version.map_or_else(|| "none".to_string(), |v| v.to_string());
        println!(
            "ledger {} [{}]: {} -> {} ({action})",
            entry.database.display(),
            entry.domain,
            describe(entry.before),
            describe(entry.after)
        );
    }
    for finding in &report.findings {
        let path = finding
            .path
            .as_ref()
            .map(|path| format!(" at {}", path.display()))
            .unwrap_or_default();
        println!("leftover [{}] {}{path}", finding.code, finding.message);
    }
    for note in &report.notes {
        println!("note: {note}");
    }
    for error in &report.errors {
        println!("error: {error}");
    }
    println!("storage migrate: {} error(s)", report.errors.len());
}

const STORAGE_ADOPT_PREDECESSOR_USAGE: &str = "usage: meta_gateway storage-adopt-predecessor \
     --state-dir <dir> [--gateway-home <dir>] [--predecessor-home <dir>] \
     [--user-gateway-home] [--mob-config <file>]... [--acknowledge-ignored-keys] \
     [--apply] [--json]\n\
     One-shot, offline adoption of one predecessor (pre-0.9) installation: \
     renames the metadata and console database files and the gateway's \
     memory ledger file (at its one fixed path in the state directory; no \
     other path is searched or touched), rewrites the schema ledger domains \
     and the metadata table, renames the creator field of each mob \
     composition manifest (*.composition.json) in the state directory, \
     keeping every other byte, retargets the steward dream schedule, and \
     moves a predecessor gateway home in scope with its established peer \
     key, host pins, control audiences and flow-editor drafts.\n\
     Scope: the state directory alone unless a gateway home is in scope. \
     --gateway-home names the home a predecessor home becomes, and the \
     predecessor home defaults to its predecessor sibling. \
     --user-gateway-home brings the invoking user's own gateway home into \
     scope (the derived XDG home and its predecessor sibling); it is never \
     in scope otherwise. An unfinished adoption resumes with the homes its \
     journal recorded; named homes that differ are refused.\n\
     --state-dir must be the installation's state directory, exactly as the \
     startup refusal names it: the persistent_state of rpc_gateway and the \
     SDK builders, or the store_path directory of meta_gateway (the parent \
     of an explicit session database file). Adoption never infers it.\n\
     --mob-config names the mob definition the installation boots with \
     (mob.toml, or a definition.json); repeat it for each mob. Planning \
     reads the definition each persistent mob storage holds now, from a \
     private copy of its event log (the store itself is never opened \
     through its owner), and names every key that the predecessor release \
     which created the store ignored (model_fallback before 0.8.34, \
     tools.deny before 0.8.46), that Meta 0.9 applies and that the held \
     definition does not record. A key a later release recorded there is in \
     force and is not named. Named keys refuse adoption before any change: \
     remove them to keep the behaviour the store has run with, or pass \
     --acknowledge-ignored-keys and then advance the stored definition once \
     with the one-shot mob definition update to apply them; resuming never \
     applies them, and an authoritative resume that keeps them refuses as \
     divergent. A --mob-config that does not parse, or that declares a \
     top-level [self_hosted] or [realm] table, is refused before planning.\n\
     Precondition: every process using the installation is stopped; the \
     run refuses while a runtime registry lists a live process, and fences \
     every database it touches. The fence covers SQLite databases only: \
     nothing fences the memory ledger's JSON writer or the composition \
     manifests, so the stopped installation is what keeps writers away, and \
     a ledger or manifest changed since planning is detected, not \
     prevented. A leftover stage of an interrupted ledger write or manifest \
     rename, a renamed ledger beside the predecessor one, or a manifest \
     whose creator field cannot be judged, is refused for manual \
     resolution. Each rename is made durable with directory fsyncs on Unix \
     only.\n\
     Dry-run by default and read-only. --apply writes the adoption journal \
     (meta-adoption.json in the state directory) before its first change \
     and stops at the first failure; rerun to resume an unfinished \
     adoption. Startup refuses until the journal records it complete.\n\
     Exit codes: 0 clean, 1 refusals or failures, 2 usage error.";

/// Maintenance verb: `meta_gateway storage-adopt-predecessor`. Never an
/// eager side effect of startup: startup refuses predecessor state and
/// points here.
fn run_storage_adopt_predecessor(args: &[String]) -> i32 {
    let mut state_dir: Option<PathBuf> = None;
    let mut gateway_home: Option<PathBuf> = None;
    let mut predecessor_home: Option<PathBuf> = None;
    let mut user_gateway_home = false;
    let mut mob_configs: Vec<PathBuf> = Vec::new();
    let mut acknowledge_ignored_keys = false;
    let mut apply = false;
    let mut json = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let slot = match arg.as_str() {
            "--state-dir" => &mut state_dir,
            "--gateway-home" => &mut gateway_home,
            "--predecessor-home" => &mut predecessor_home,
            "--user-gateway-home" => {
                user_gateway_home = true;
                continue;
            }
            "--mob-config" => {
                let Some(file) = iter.next() else {
                    eprintln!("--mob-config requires a file\n{STORAGE_ADOPT_PREDECESSOR_USAGE}");
                    return 2;
                };
                mob_configs.push(PathBuf::from(file));
                continue;
            }
            "--acknowledge-ignored-keys" => {
                acknowledge_ignored_keys = true;
                continue;
            }
            "--apply" => {
                apply = true;
                continue;
            }
            "--json" => {
                json = true;
                continue;
            }
            "--help" | "-h" => {
                println!("{STORAGE_ADOPT_PREDECESSOR_USAGE}");
                return 0;
            }
            other => {
                eprintln!("unknown argument {other:?}\n{STORAGE_ADOPT_PREDECESSOR_USAGE}");
                return 2;
            }
        };
        match iter.next() {
            Some(value) => *slot = Some(PathBuf::from(value)),
            None => {
                eprintln!("{arg} requires a directory\n{STORAGE_ADOPT_PREDECESSOR_USAGE}");
                return 2;
            }
        }
    }
    let Some(state_dir) = state_dir else {
        eprintln!("--state-dir is required\n{STORAGE_ADOPT_PREDECESSOR_USAGE}");
        return 2;
    };
    if predecessor_home.is_some() && gateway_home.is_none() && !user_gateway_home {
        eprintln!(
            "--predecessor-home needs the gateway home it becomes: name it with \
             --gateway-home, or pass --user-gateway-home for the derived home\n\
             {STORAGE_ADOPT_PREDECESSOR_USAGE}"
        );
        return 2;
    }
    if acknowledge_ignored_keys && mob_configs.is_empty() {
        eprintln!(
            "--acknowledge-ignored-keys needs a definition to check: name it with \
             --mob-config\n{STORAGE_ADOPT_PREDECESSOR_USAGE}"
        );
        return 2;
    }
    // Each definition is read the way meta_gateway reads its workspace
    // definition: a file that does not parse, or a mob.toml that declares a
    // host-config table, is refused here, before planning.
    let mut definitions = Vec::with_capacity(mob_configs.len());
    for path in &mob_configs {
        match load_adoption_mob_config(path) {
            Ok(definition) => definitions.push((path.display().to_string(), definition)),
            Err(reason) => {
                eprintln!("--mob-config {}: {reason}", path.display());
                return 1;
            }
        }
    }
    // Named homes bind; derived ones are defaults that an unfinished
    // adoption replaces with the homes its journal recorded.
    let homes_explicit = gateway_home.is_some() || predecessor_home.is_some();
    // The invoking user's own gateway home is that user's state, not part of
    // the installation `--state-dir` names: it is derived only on request.
    let gateway_home = match gateway_home {
        Some(home) => Some(home),
        None if user_gateway_home => match meerkat_meta::storage_layout::derived_gateway_home() {
            Ok(home) => Some(home),
            Err(error) => {
                eprintln!("--user-gateway-home: {error}");
                return 1;
            }
        },
        None => None,
    };
    let predecessor_home = predecessor_home.or_else(|| {
        gateway_home
            .as_deref()
            .and_then(meerkat_meta::storage_predecessor::predecessor_gateway_home_beside)
            .filter(|path| path.is_dir())
    });
    let request = meerkat_meta::storage_predecessor::PredecessorAdoptRequest {
        state_dir,
        predecessor_home,
        gateway_home,
        homes_explicit,
        mode: if apply {
            meerkat_meta::MigrateMode::Apply
        } else {
            meerkat_meta::MigrateMode::DryRun
        },
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("failed to start the adoption runtime: {error}");
            return 1;
        }
    };
    let report = runtime.block_on(
        meerkat_meta::storage_predecessor::adopt_predecessor_state_with_definitions(
            &request,
            &meerkat_meta::storage_predecessor::PredecessorDefinitionCheck::new(
                definitions,
                acknowledge_ignored_keys,
            ),
        ),
    );
    if json {
        match serde_json::to_string_pretty(&report) {
            Ok(text) => println!("{text}"),
            Err(error) => {
                eprintln!("failed to serialize adoption report: {error}");
                return 1;
            }
        }
    } else {
        let mode = if apply { "apply" } else { "dry-run" };
        println!(
            "Predecessor adoption ({mode}) of {}: changed state: {}, complete: {}",
            report.state_dir.display(),
            report.changed_state,
            report.complete
        );
        for action in &report.actions {
            let outcome = match &action.outcome {
                meerkat_meta::storage_predecessor::PredecessorOutcome::Planned => {
                    "planned".to_string()
                }
                meerkat_meta::storage_predecessor::PredecessorOutcome::Applied => {
                    "applied".to_string()
                }
                meerkat_meta::storage_predecessor::PredecessorOutcome::Refused(reason) => {
                    format!("refused, nothing changed: {reason}")
                }
                meerkat_meta::storage_predecessor::PredecessorOutcome::Failed(reason) => {
                    format!("FAILED after changes, rerun to resume: {reason}")
                }
            };
            println!(
                "  {:?} {}: {} -> {} ({outcome})",
                action.item, action.subject, action.from, action.to
            );
        }
        for ignored in &report.ignored_keys {
            println!(
                "  IgnoredKeys {} ({}): {} ignored by predecessor release {} ({})",
                ignored.manifest,
                ignored.source,
                ignored.keys.join(", "),
                ignored.created_by,
                if ignored.acknowledged {
                    "acknowledged"
                } else {
                    "refused, nothing changed"
                }
            );
        }
        for note in &report.notes {
            println!("  note: {note}");
        }
        for error in &report.errors {
            println!("  ERROR: {error}");
        }
    }
    i32::from(report.has_errors())
}

/// Read a `--mob-config` definition the way meta_gateway reads its
/// workspace definition: a `.json` file as a serialized definition (its
/// `definition.json`), anything else as `mob.toml` through Meerkat's
/// definition parser (with the upgrade note a refused profile prompt key
/// carries), then the refusal of the host-config tables that parser would
/// drop.
fn load_adoption_mob_config(path: &Path) -> Result<MobDefinition, String> {
    let text = fs::read_to_string(path).map_err(|error| format!("failed to read: {error}"))?;
    if path
        .extension()
        .is_some_and(|extension| extension == "json")
    {
        return serde_json::from_str::<MobDefinition>(&text)
            .map_err(|error| format!("not a mob definition: {error}"));
    }
    let definition = MobDefinition::from_toml(&text).map_err(|error| {
        meerkat_meta::gateway_composition::describe_mob_definition_error(&error)
    })?;
    meerkat_meta::gateway_composition::refuse_host_config_tables_in_mob_toml(&text)
        .map_err(|table| table.to_string())?;
    Ok(definition)
}

const IDENTITY_CENSUS_USAGE: &str = "usage: meta_gateway identity-census --state-dir <dir> \
     [--scratch-dir <dir>] [--json]\n\
     Offline, read-only census of the identities a native (0.9.4) state \
     directory holds. Per identity: the session, generation and mob member \
     id of Core's last binding, whether the member is live and its runtime \
     id while it is, the runtime id under which runtime.sqlite holds the \
     session's authority, whether Core's catalog marks the session \
     lifecycle-terminal, whether a held completion is owed, and the pending \
     first-boot row. The bindings are read from the directory's mob store \
     the way the gateway's identity views read them; in-memory mob storage \
     keeps none across a restart, so those facts are null there. Run it on \
     a directory no gateway uses (a copy is fine): a directory a runtime \
     holds is refused, and so is one that is not in the native layout \
     (migrate it with storage-migrate-native first). It writes nothing in \
     the directory: it reads copies of its top-level files, taken into a \
     temporary directory (or under --scratch-dir) and removed when the run \
     ends. That location needs free space for those files, runtime.sqlite \
     included; the registered backups, the quarantined files and the -shm \
     indexes are not copied.\n\
     --json prints the report as JSON (format meerkat-meta.identity-census, \
     version 1).\n\
     Exit codes: 0 read, 1 refused, 2 usage error.";

/// Maintenance verb: `meta_gateway identity-census`.
fn run_identity_census(args: &[String]) -> i32 {
    let mut state_dir: Option<PathBuf> = None;
    let mut scratch_dir: Option<PathBuf> = None;
    let mut json = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--state-dir" => match iter.next() {
                Some(value) => state_dir = Some(PathBuf::from(value)),
                None => {
                    eprintln!("--state-dir requires a directory\n{IDENTITY_CENSUS_USAGE}");
                    return 2;
                }
            },
            "--scratch-dir" => match iter.next() {
                Some(value) => scratch_dir = Some(PathBuf::from(value)),
                None => {
                    eprintln!("--scratch-dir requires a directory\n{IDENTITY_CENSUS_USAGE}");
                    return 2;
                }
            },
            "--json" => json = true,
            "--help" | "-h" => {
                println!("{IDENTITY_CENSUS_USAGE}");
                return 0;
            }
            other => {
                eprintln!("unknown argument {other:?}\n{IDENTITY_CENSUS_USAGE}");
                return 2;
            }
        }
    }
    let Some(state_dir) = state_dir else {
        eprintln!("--state-dir is required\n{IDENTITY_CENSUS_USAGE}");
        return 2;
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("failed to start the census runtime: {error}");
            return 1;
        }
    };
    let report = runtime.block_on(
        meerkat_meta::storage_migrate_native::identity_census_offline(
            &state_dir,
            scratch_dir.as_deref(),
        ),
    );
    if json {
        match serde_json::to_string_pretty(&report) {
            Ok(text) => println!("{text}"),
            Err(error) => {
                eprintln!("failed to serialize the identity census: {error}");
                return 1;
            }
        }
    } else {
        print!("{}", identity_census_text(&report));
    }
    i32::from(report.refusal.is_some())
}

fn identity_census_text(
    report: &meerkat_meta::storage_migrate_native::IdentityCensusReport,
) -> String {
    use meerkat_meta::storage_migrate_native::{IdentityCensusRefusal, PendingFirstBootRow};
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "Identity census of {}:", report.state_dir.display());
    if let Some(refusal) = &report.refusal {
        let _ = writeln!(out, "Refused: {refusal:?}");
        if let IdentityCensusRefusal::NeedsMigration { remedy, .. }
        | IdentityCensusRefusal::MigrationIncomplete { remedy, .. } = refusal
        {
            let _ = writeln!(out, "  {remedy}");
        }
        return out;
    }
    match &report.mob_store {
        Some(mob_store) => {
            let _ = writeln!(out, "  mob store: {}", mob_store.display());
        }
        None => {
            let _ = writeln!(out, "  mob store: none");
        }
    }
    if let Some(note) = report.mob_store_note {
        let _ = writeln!(out, "  note: {note}");
    }
    for entry in &report.identities {
        let mut line = format!(
            "  {}: session {}, generation {}, member {}, {}",
            entry.identity,
            entry.session_id.as_deref().unwrap_or("-"),
            entry
                .generation
                .map_or_else(|| "-".to_owned(), |generation| generation.to_string()),
            entry.member_id.as_deref().unwrap_or("-"),
            if entry.live { "live" } else { "not live" },
        );
        if let Some(agent_runtime_id) = &entry.agent_runtime_id {
            let _ = write!(line, ", agent runtime {agent_runtime_id}");
        }
        let _ = write!(
            line,
            ", runtime {}",
            entry.runtime_id.as_deref().unwrap_or("-")
        );
        if entry.retired {
            line.push_str(", retired");
        }
        if entry.held_completion_owed {
            line.push_str(", held completion owed");
        }
        match &entry.pending_first_boot_row {
            Some(PendingFirstBootRow::Resume { session, .. }) => {
                let _ = write!(line, ", first-boot resume row pending (session {session})");
            }
            Some(PendingFirstBootRow::Fresh {
                retired_session, ..
            }) => {
                let _ = write!(
                    line,
                    ", first-boot fresh row pending (retired session {retired_session})"
                );
            }
            Some(_) => line.push_str(", first-boot row pending"),
            None => {}
        }
        let _ = writeln!(out, "{line}");
    }
    out
}

const IDENTITY_RESET_USAGE: &str = "usage: meta_gateway identity-reset \
     --state-dir <dir> --identity <identity> [--mob-store <path>] [--apply] [--json]\n\
     Give one identity of a stopped state directory a fresh life at its next \
     start. Nothing in Core is written now: the reset is a row in the \
     directory's first-boot manifest, naming the session of Core's last \
     binding of the identity, and the next roster compile that lists the \
     identity carries it out. A live member is replaced by its successor on a \
     fresh session and stays live; a retired one gets a fresh session and \
     stays dormant. The old session stays in Core's store, bound to nothing. \
     Unlike the online reset, this offline reset distills nothing of the old \
     session into agent memory.\n\
     Stop every process that uses the directory first: a process hosting a \
     runtime is refused, and the directory is held as storage-migrate-native \
     holds it. The directory must be in the native layout (run \
     storage-migrate-native first), and Core's member history must bind the \
     identity to a session. An identity a host adopted (Core holds an identity \
     intent row for it) is refused: change it through the host that declared \
     it. A manifest that already holds another row for the identity is \
     refused: start the runtime once so it is carried out. An identity that \
     owes its host a held completion (a held-completion mark, and no binding \
     in Core) is refused: run the host's held bootstrap. Beside a binding the \
     mark is stale: the next start drops it, and the report says so.\n\
     A dry run (the default) checks everything and reports the session it \
     would retire and whether its binding is live or retired; --apply writes \
     the row, replacing the manifest atomically; identity-reset keeps no copy \
     of the manifest it rewrites. If the manifest becomes unreadable, move it \
     aside. If no runtime has started since storage-migrate-native --apply, \
     restore the copy that run kept and run identity-reset again for every \
     reset made since; otherwise restore no older copy, start with no \
     manifest, and run identity-reset again for each reset made since the \
     last start. Removing the manifest also drops a migration row still \
     pending for an identity the roster does not list: that identity starts \
     fresh when it is added back, and nothing reports it, so compare against \
     the last identity-census output before removing it.\n\
     The mob store is <dir>/event_log.sqlite3 when present; name another with \
     --mob-store, as for storage-migrate-native.\n\
     Exit codes: 0 done, 1 refused, 2 usage error.";

/// Maintenance verb: `meta_gateway identity-reset`.
fn run_identity_reset(args: &[String]) -> i32 {
    let mut state_dir: Option<PathBuf> = None;
    let mut identity: Option<String> = None;
    let mut mob_store: Option<PathBuf> = None;
    let mut apply = false;
    let mut json = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--state-dir" => match iter.next() {
                Some(value) => state_dir = Some(PathBuf::from(value)),
                None => {
                    eprintln!("--state-dir requires a directory\n{IDENTITY_RESET_USAGE}");
                    return 2;
                }
            },
            "--identity" => match iter.next() {
                Some(value) => identity = Some(value.clone()),
                None => {
                    eprintln!("--identity requires an identity\n{IDENTITY_RESET_USAGE}");
                    return 2;
                }
            },
            "--mob-store" => match iter.next() {
                Some(value) => mob_store = Some(PathBuf::from(value)),
                None => {
                    eprintln!("--mob-store requires a path\n{IDENTITY_RESET_USAGE}");
                    return 2;
                }
            },
            "--apply" => apply = true,
            "--json" => json = true,
            "--help" | "-h" => {
                println!("{IDENTITY_RESET_USAGE}");
                return 0;
            }
            other => {
                eprintln!("unknown argument {other:?}\n{IDENTITY_RESET_USAGE}");
                return 2;
            }
        }
    }
    let (Some(state_dir), Some(identity)) = (state_dir, identity) else {
        eprintln!("--state-dir and --identity are required\n{IDENTITY_RESET_USAGE}");
        return 2;
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("failed to start the reset runtime: {error}");
            return 1;
        }
    };
    let mut request =
        meerkat_meta::storage_migrate_native::IdentityResetRequest::new(&state_dir, identity);
    if let Some(mob_store) = mob_store {
        request = request.with_mob_store(mob_store);
    }
    if apply {
        request = request.with_apply();
    }
    let report = runtime.block_on(meerkat_meta::storage_migrate_native::reset_identity(
        &request,
    ));
    if json {
        match serde_json::to_string_pretty(&report) {
            Ok(text) => println!("{text}"),
            Err(error) => {
                eprintln!("failed to serialize the reset report: {error}");
                return 1;
            }
        }
    } else {
        let mode = if apply { "apply" } else { "dry-run" };
        println!(
            "Identity reset ({mode}) of {} in {}:",
            report.identity,
            report.state_dir.display()
        );
        if let Some(session) = &report.retired_session {
            let state = report.binding_state.unwrap_or("unknown");
            println!("  retired session: {session} (binding {state})");
        }
        let next_start = if report.binding_state == Some("live") {
            "the next start replaces the live member by its successor on a fresh session; the \
             identity stays live"
        } else {
            "the next start gives the identity a fresh session; it stays dormant"
        };
        if report.stale_held_completion_mark {
            println!("  a stale held-completion mark names the identity; the next start drops it");
        }
        match &report.refusal {
            Some(refusal) => println!("  REFUSED: {refusal}"),
            None if report.already_pending => {
                println!("  the same reset row is already pending; nothing to write");
            }
            None if report.written => println!("  wrote the reset row: {next_start}"),
            None => println!("  clean: --apply writes the reset row, and {next_start}"),
        }
    }
    i32::from(report.refused())
}

const STORAGE_MIGRATE_NATIVE_USAGE: &str = "usage: meta_gateway storage-migrate-native \
     --state-dir <dir> [--apply] [--scratch-dir <dir>] [--mob-store <path>] \
     [--continuity-export <file>] [--discard-bodyless <session id>]... [--json]\n\
     One-way migration of a 0.9.3 state directory (a gateway's, or an \
     application's that embeds the runtime builder) onto Meerkat's \
     native session and identity stores. Stop every process that uses the \
     directory first: a process hosting a runtime is refused, and while the \
     census copies the databases every store operation of any other process \
     fails.\n\
     A directory already in the native layout has nothing to migrate.\n\
     A dry run (the default) runs the census: it classifies every session by \
     comparing runtime.sqlite with the session projection, on scratch \
     copies, and changes no database in the state directory (a read-only \
     open of a WAL database may leave its empty -wal and -shm files, and the \
     maintenance fence empty *.mpresence lock files beside the databases).\n\
     --apply carries out a clean census: it backs up runtime.sqlite and each \
     projection database beside them (registered *.pre-* backups the storage \
     doctor lists), converts runtime.sqlite in place onto Meerkat's native \
     session store, imports each session only the projection holds, writes \
     the first-boot manifest of the identities (with a registered copy \
     beside it), and retires the projection: sessions.sqlite3 becomes a \
     retired stamp that 0.9.3 refuses, and continuity.sqlite3 keeps its \
     identity records without session rows. Every step is journaled: a run \
     that stopped part way leaves its journal, a 0.9.4 runtime refuses to \
     start on the directory, and running --apply again finishes it once \
     Core has claimed runtime.sqlite for the conversion, or starts over with \
     a new census before that.\n\
     The scratch copies go in a temporary directory beside the state \
     directory (in its parent), or under --scratch-dir, and are removed \
     when the run ends.\n\
     The mob store is <dir>/event_log.sqlite3 when present. An application \
     that kept its durable mob store under another name or elsewhere names it \
     with --mob-store; a directory holding another store's composition \
     manifest is refused until --mob-store names that store.\n\
     A host whose sessions a continuity provider held passes the file the \
     continuity export scripts wrote with --continuity-export, and as \
     --state-dir either a new directory or the host's own state directory \
     (which holds its runtime.sqlite and no local projection). The census \
     then compares the export with Core's sessions and each listed identity \
     with the mob storage.\n\
     A session whose WholeBlob authority in runtime.sqlite references a \
     missing body cannot be loaded by any release and refuses the census; \
     --discard-bodyless <session id> (repeatable) lets --apply discard that \
     authority after its backup.\n\
     Exit codes: 0 clean, 1 refusals, 2 usage error.";

/// Maintenance verb: `meta_gateway storage-migrate-native`.
fn run_storage_migrate_native(args: &[String]) -> i32 {
    let mut state_dir: Option<PathBuf> = None;
    let mut scratch_dir: Option<PathBuf> = None;
    let mut mob_store: Option<PathBuf> = None;
    let mut continuity_export: Option<PathBuf> = None;
    let mut discard_bodyless: Vec<String> = Vec::new();
    let mut apply = false;
    let mut json = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--state-dir" => match iter.next() {
                Some(value) => state_dir = Some(PathBuf::from(value)),
                None => {
                    eprintln!("--state-dir requires a directory\n{STORAGE_MIGRATE_NATIVE_USAGE}");
                    return 2;
                }
            },
            "--scratch-dir" => match iter.next() {
                Some(value) => scratch_dir = Some(PathBuf::from(value)),
                None => {
                    eprintln!("--scratch-dir requires a directory\n{STORAGE_MIGRATE_NATIVE_USAGE}");
                    return 2;
                }
            },
            "--mob-store" => match iter.next() {
                Some(value) => mob_store = Some(PathBuf::from(value)),
                None => {
                    eprintln!("--mob-store requires a path\n{STORAGE_MIGRATE_NATIVE_USAGE}");
                    return 2;
                }
            },
            "--continuity-export" => match iter.next() {
                Some(value) => continuity_export = Some(PathBuf::from(value)),
                None => {
                    eprintln!(
                        "--continuity-export requires a file\n{STORAGE_MIGRATE_NATIVE_USAGE}"
                    );
                    return 2;
                }
            },
            "--discard-bodyless" => match iter.next() {
                Some(value) => discard_bodyless.push(value.clone()),
                None => {
                    eprintln!(
                        "--discard-bodyless requires a session id\n{STORAGE_MIGRATE_NATIVE_USAGE}"
                    );
                    return 2;
                }
            },
            "--apply" => apply = true,
            "--json" => json = true,
            "--help" | "-h" => {
                println!("{STORAGE_MIGRATE_NATIVE_USAGE}");
                return 0;
            }
            other => {
                eprintln!("unknown argument {other:?}\n{STORAGE_MIGRATE_NATIVE_USAGE}");
                return 2;
            }
        }
    }
    let Some(state_dir) = state_dir else {
        eprintln!("--state-dir is required\n{STORAGE_MIGRATE_NATIVE_USAGE}");
        return 2;
    };
    let mode = if apply {
        meerkat_meta::MigrateMode::Apply
    } else {
        meerkat_meta::MigrateMode::DryRun
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("failed to start the migration runtime: {error}");
            return 1;
        }
    };
    let mut request =
        meerkat_meta::storage_migrate_native::NativeMigrateRequest::new(&state_dir, mode);
    if let Some(scratch_dir) = scratch_dir {
        request = request.with_scratch_dir(scratch_dir);
    }
    if let Some(mob_store) = mob_store {
        request = request.with_mob_store(mob_store);
    }
    if let Some(continuity_export) = continuity_export {
        request = request.with_continuity_export(continuity_export);
    }
    for session_id in discard_bodyless {
        request = request.with_discard_bodyless(session_id);
    }
    let report =
        runtime.block_on(meerkat_meta::storage_migrate_native::migrate_state_dir_native(&request));
    if json {
        match serde_json::to_string_pretty(&report) {
            Ok(text) => println!("{text}"),
            Err(error) => {
                eprintln!("failed to serialize the migration report: {error}");
                return 1;
            }
        }
    } else {
        print!("{}", native_migrate_report_text(&report));
    }
    i32::from(report.has_refusals())
}

fn native_migrate_report_text(
    report: &meerkat_meta::storage_migrate_native::NativeMigrateReport,
) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let mode = match report.mode {
        meerkat_meta::MigrateMode::Apply => "apply",
        _ => "dry-run",
    };
    let shape = report
        .shape
        .map(|shape| format!("{shape:?}"))
        .unwrap_or_else(|| "unknown".to_owned());
    let _ = writeln!(
        out,
        "Native storage migration ({mode}) of {} (shape: {shape}):",
        report.state_dir.display()
    );
    if let Some(layout) = &report.layout {
        let _ = writeln!(out, "  layout: {layout}");
    }
    if let Some(mob_storage) = &report.mob_storage {
        let _ = writeln!(out, "  mob storage: {mob_storage}");
    }
    if let Some(export) = &report.continuity_export {
        let _ = writeln!(
            out,
            "  continuity export: {} ({:?}; written by {}; {} entries, {} ready; entries_sha256 {})",
            export.path.display(),
            export.mode,
            export.writer,
            export.entries,
            export.ready,
            export.entries_sha256
        );
    }
    for row in &report.sessions {
        let verdict = if row.admitted { "ok" } else { "REFUSED" };
        let identities = if row.identities.is_empty() {
            String::new()
        } else {
            format!(" [{}]", row.identities.join(", "))
        };
        let _ = writeln!(
            out,
            "  {verdict} {:?} {}{identities}: {}",
            row.class, row.session_id, row.detail
        );
        if let Some(effect) = &row.apply_effect {
            let _ = writeln!(out, "      on --apply: {effect}");
        }
        if let Some(action) = &row.action {
            let _ = writeln!(out, "      action: {action}");
        }
    }
    for (class, count) in report.class_counts() {
        let _ = writeln!(out, "  {class}: {count}");
    }
    let _ = writeln!(out, "  identity census: {:?}", report.identity_census);
    for row in &report.identities {
        let verdict = if row.admitted { "ok" } else { "REFUSED" };
        let _ = writeln!(
            out,
            "  {verdict} {:?} {}: {}",
            row.class, row.identity, row.detail
        );
        if let Some(action) = &row.action {
            let _ = writeln!(out, "      action: {action}");
        }
    }
    for row in &report.first_boot {
        let _ = writeln!(out, "  first boot: {row:?}");
    }
    if let Some(applied) = &report.applied {
        let _ = writeln!(
            out,
            "  applied{}: {} runtime(s) seeded from their committed bodies, {} converted from \
             session rows",
            if applied.resumed {
                " (finished an interrupted run)"
            } else if applied.restarted {
                " (started over: an interrupted run had not started converting runtime.sqlite)"
            } else {
                ""
            },
            applied.runtimes_seeded,
            applied.runtimes_derived
        );
        for imported in &applied.imported {
            let _ = writeln!(
                out,
                "  imported: {} (row {:?}, crossing {:?}, activation {:?})",
                imported.session_id, imported.row, imported.crossing, imported.activation
            );
        }
        if let Some(digest) = &applied.first_boot_manifest_sha256 {
            let _ = writeln!(out, "  first-boot manifest written: rows_sha256 {digest}");
        }
        for retired in &applied.retired {
            let _ = writeln!(out, "  retired: {retired}");
        }
        for discarded in &applied.discarded {
            let leftovers = discarded
                .leftover_rows
                .iter()
                .map(|(table, rows)| format!("{table}: {rows}"))
                .collect::<Vec<_>>();
            let _ = writeln!(
                out,
                "  discarded the body-less WholeBlob authority of session {} (runtime {}); rows \
                 Core keeps for it: {}",
                discarded.session_id,
                discarded.runtime_id,
                if leftovers.is_empty() {
                    "none".to_owned()
                } else {
                    leftovers.join(", ")
                }
            );
        }
        for backup in &applied.backups {
            let _ = writeln!(out, "  backup: {}", backup.display());
        }
    }
    if matches!(
        report.layout,
        Some(meerkat_meta::storage_migrate_native::NativeLayoutStatus::Native)
    ) {
        let _ = writeln!(out, "Nothing to migrate.");
    } else if report.applied.is_some() && report.refusals.is_empty() {
        let _ = writeln!(out, "Applied: the directory is in the native layout.");
    } else if report.refusals.is_empty() {
        let _ = writeln!(out, "Clean: every session is admitted.");
    } else {
        let _ = writeln!(out, "Refused:");
        for refusal in &report.refusals {
            let _ = writeln!(out, "  {refusal:?}");
            for remedy in native_refusal_remedies(&report.state_dir, refusal) {
                let _ = writeln!(out, "      remedy: {remedy}");
            }
        }
    }
    out
}

/// The step the text report names under a refusal the operator answers
/// with a flag or a file.
fn native_refusal_remedies(
    state_dir: &Path,
    refusal: &meerkat_meta::storage_migrate_native::NativeMigrateRefusal,
) -> Vec<String> {
    use meerkat_meta::storage_migrate_native::NativeMigrateRefusal;
    match refusal {
        NativeMigrateRefusal::MobStoreUnnamed { manifests } => manifests
            .iter()
            .map(|manifest| {
                let store = manifest
                    .strip_suffix(".composition.json")
                    .unwrap_or(manifest);
                format!("pass --mob-store {}", state_dir.join(store).display())
            })
            .collect(),
        NativeMigrateRefusal::MobStoreManifestWithoutStore { store, .. } => vec![format!(
            "restore {}, or pass --mob-store <path> naming where that store is now",
            store.display()
        )],
        NativeMigrateRefusal::SpellingTwins { files, .. } => vec![format!(
            "two spellings of one database ({}): run `meta_gateway storage-migrate --state-dir {} \
             --apply` first, which resolves them",
            files.join(", "),
            state_dir.display()
        )],
        NativeMigrateRefusal::UnrecognizedSessionDatabase { file } => vec![format!(
            "run `meta_gateway storage-migrate --state-dir {dir} --session-db {dir}/{file} \
             --apply` before anything else; if a 0.9.4 runtime has already used the directory, \
             that step refuses and the file has to be moved out of the state directory",
            dir = state_dir.display()
        )],
        NativeMigrateRefusal::DiscardNotBodiless { session_id } => vec![format!(
            "the census found no body-less WholeBlob authority for session {session_id}: drop \
             it from --discard-bodyless"
        )],
        NativeMigrateRefusal::NoProjection => vec![format!(
            "if 0.9.3 ever ran with store_path naming a file, run `meta_gateway storage-migrate \
             --state-dir {} --session-db <file> --apply` first",
            state_dir.display()
        )],
        NativeMigrateRefusal::MigrationJournalUnreadable { .. } => vec![format!(
            "restore runtime.sqlite and each projection file in {} from their \
             *.pre-*.native-migration backups, then remove the journal; removing the journal \
             alone leaves a directory neither release reads",
            state_dir.display()
        )],
        _ => Vec::new(),
    }
}

const STORAGE_PRUNE_USAGE: &str = "usage: meta_gateway storage-prune --state-dir <dir> \
     [--apply] [--older-than-days N] [--json]\n\
     Lifecycle of registered maintenance artifacts (`*.pre-*` backups, \
     `*.corrupt-*` quarantines) under one Meta state directory. Never \
     touches anything outside those naming patterns.\n\
     Dry-run by default; --apply deletes artifacts at least \
     --older-than-days old (default 30; 0 = all).\n\
     Exit codes: 0 clean, 1 delete failures, 2 usage error.";

/// Maintenance verb: `meta_gateway storage-prune`.
fn run_storage_prune(args: &[String]) -> i32 {
    let mut state_dir: Option<PathBuf> = None;
    let mut older_than_days: u64 = 30;
    let mut apply = false;
    let mut json = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--state-dir" => match iter.next() {
                Some(value) => state_dir = Some(PathBuf::from(value)),
                None => {
                    eprintln!("--state-dir requires a directory\n{STORAGE_PRUNE_USAGE}");
                    return 2;
                }
            },
            "--older-than-days" => match iter.next().map(|value| value.parse::<u64>()) {
                Some(Ok(days)) => older_than_days = days,
                _ => {
                    eprintln!("--older-than-days requires a number\n{STORAGE_PRUNE_USAGE}");
                    return 2;
                }
            },
            "--apply" => apply = true,
            "--json" => json = true,
            "--help" | "-h" => {
                println!("{STORAGE_PRUNE_USAGE}");
                return 0;
            }
            other => {
                eprintln!("unknown argument {other:?}\n{STORAGE_PRUNE_USAGE}");
                return 2;
            }
        }
    }
    let Some(state_dir) = state_dir else {
        eprintln!("--state-dir is required\n{STORAGE_PRUNE_USAGE}");
        return 2;
    };
    let mode = if apply {
        meerkat_meta::MigrateMode::Apply
    } else {
        meerkat_meta::MigrateMode::DryRun
    };
    let report = meerkat_meta::prune_state_dir(&state_dir, older_than_days, mode);
    if json {
        match serde_json::to_string_pretty(&report) {
            Ok(text) => println!("{text}"),
            Err(error) => {
                eprintln!("failed to serialize prune report: {error}");
                return 1;
            }
        }
    } else {
        let mode = if apply { "apply" } else { "dry-run" };
        println!(
            "Storage prune ({mode}, older than {} day(s)) over {}:",
            report.older_than_days,
            report.state_dir.display()
        );
        if report.artifacts.is_empty() {
            println!("No registered maintenance artifacts found.");
        }
        for artifact in &report.artifacts {
            let action = match artifact.action {
                meerkat_meta::PruneAction::WouldDelete => "would delete",
                meerkat_meta::PruneAction::Deleted => "deleted",
                meerkat_meta::PruneAction::Kept => "kept (younger than threshold)",
                meerkat_meta::PruneAction::DeleteFailed => "DELETE FAILED",
                _ => "unknown",
            };
            println!(
                "  {}  {} bytes, {} day(s) old — {action}",
                artifact.path.display(),
                artifact.bytes,
                artifact.age_days
            );
        }
        for error in &report.errors {
            println!("error: {error}");
        }
        println!("storage prune: {} error(s)", report.errors.len());
    }
    i32::from(report.has_errors())
}

/// Launch-time (argv) inputs `run` resolves against the init params. An init
/// param wins where both name the same thing; the environment
/// (`META_HTTP_LISTEN_ADDR`, `META_HTTP_ALLOW_REMOTE`) sits below both.
struct GatewayLaunchArgs {
    control_listen: Option<ControlListenAddr>,
    /// `--http-listen HOST:PORT`.
    http_listen: Option<SocketAddr>,
    /// `--allow-remote`.
    allow_remote: bool,
    /// `--allow-flow-editor-host-deploy`: the runtime-mounted Flow Editor may
    /// run `rkat` on this host for callers that no enforced access controller
    /// governs. Launch-only, and part of the resume key.
    flow_editor_host_deploy: bool,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!(
            "meta_gateway {} (meerkat-meta console/HTTP gateway)",
            env!("CARGO_PKG_VERSION")
        );
        return;
    }
    // Writes the MCP Apps sandbox proxy this binary embeds and exits: no
    // tracing, no init request, no runtime.
    if let Some(code) = meerkat_meta::gateway_composition::write_mcp_apps_sandbox_arg(&args) {
        std::process::exit(code);
    }
    // Install the tracing subscriber FIRST — before the maintenance verbs,
    // not just before the gateway boot. The storage verbs drive the same
    // migration code whose progress reports through tracing; dispatching
    // them ahead of this init dropped every progress line, and a 2026-07
    // production deploy was aborted because a supervisor read the
    // silent-but-working migration as a hang. Also (meerkat-studio K1/K2):
    // without any init, runtime failures, console internal-error logs, and
    // the schedule claim watchdog's stall diagnosis all vanish on the child
    // gateways their app spawns. Stderr, never stdout: stdout carries the
    // init JSON handshake and the verbs' report output.
    //
    // Default: this crate's own targets at INFO, dependencies at WARN;
    // RUST_LOG overrides everything. Shared with rpc_gateway so the two
    // gateways cannot drift on observability posture.
    meerkat_meta::gateway_composition::init_gateway_tracing(GATEWAY_TRACING_TARGET);
    if args.first().map(String::as_str) == Some("storage-migrate") {
        std::process::exit(run_storage_migrate(&args[1..]));
    }
    if args.first().map(String::as_str) == Some("storage-prune") {
        std::process::exit(run_storage_prune(&args[1..]));
    }
    if args.first().map(String::as_str) == Some("storage-adopt-predecessor") {
        std::process::exit(run_storage_adopt_predecessor(&args[1..]));
    }
    if args.first().map(String::as_str) == Some("storage-migrate-native") {
        std::process::exit(run_storage_migrate_native(&args[1..]));
    }
    if args.first().map(String::as_str) == Some("identity-reset") {
        std::process::exit(run_identity_reset(&args[1..]));
    }
    if args.first().map(String::as_str) == Some("identity-census") {
        std::process::exit(run_identity_census(&args[1..]));
    }
    // Predecessor environment variables would be silently ignored by this
    // build; refuse instead (the maintenance verbs above stay usable).
    if let Err(error) = meerkat_meta::storage_predecessor::refuse_predecessor_environment() {
        eprintln!("{error}");
        print_json_line(&init_error(Value::Null, -32603, error.to_string()));
        std::process::exit(2);
    }
    // --control-listen <tcp://host:port | uds:///path>: bind the cross-mob
    // control listener so remote gateways can wire/unwire/inject/lookup
    // members of this runtime. Validated here so a typo is a launch error,
    // not a silently ignored flag.
    let control_listen = match meerkat_meta::gateway_composition::parse_control_listen_arg(&args) {
        Ok(value) => value,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };
    // --http-listen HOST:PORT / --allow-remote: the HTTP listener's bind
    // address and the exposure acknowledgement a non-loopback bind needs.
    // Init params win over these; META_HTTP_LISTEN_ADDR and
    // META_HTTP_ALLOW_REMOTE sit below them. Validated here so a typo is a
    // launch error, not a silently ignored flag.
    let http_listen = match meerkat_meta::gateway_composition::parse_http_listen_arg(&args) {
        Ok(value) => value,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };
    let allow_remote = args.iter().any(|arg| arg == "--allow-remote");
    let flow_editor_host_deploy = args
        .iter()
        .any(|arg| arg == "--allow-flow-editor-host-deploy");
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        "meta_gateway starting (console/HTTP gateway)"
    );
    // Deep worker stacks for the generated machine-authority apply path; the
    // builder is shared with rpc_gateway, the failure reporting is not (this
    // binary's parent reads a JSON-RPC handshake on stdout).
    let runtime = match meerkat_meta::gateway_composition::gateway_tokio_runtime() {
        Ok(runtime) => runtime,
        Err(error) => {
            let response = init_error(Value::Null, -32603, error.to_string());
            print_json_line(&response);
            std::process::exit(1);
        }
    };
    if let Err(error) = runtime.block_on(Box::pin(run(GatewayLaunchArgs {
        control_listen,
        http_listen,
        allow_remote,
        flow_editor_host_deploy,
    }))) {
        // A refusal `run()` already answered on the request id must not be
        // followed by a second, id-less reply.
        if error.downcast_ref::<InitReplied>().is_none() {
            tracing::error!(error = %format!("{error:#}"), "gateway startup failed");
            let response = init_error(Value::Null, -32603, error.to_string());
            print_json_line(&response);
        }
        std::process::exit(1);
    }
}

/// Which `select!` branch ended `run()`: the `reason=` token of the "run loop
/// ended" log line. Named so an operator can tell a supervisor's signal from
/// a server-task death from a crash, which leaves no such line (only the
/// panic hook's ERROR, or nothing at all when the process was killed from
/// outside).
enum RunEnd {
    /// The HTTP server task ended on its own; the outcome says whether
    /// cleanly. Abnormal outcomes are logged at ERROR and fail `run()`.
    HttpServer(meerkat_meta::gateway_composition::GatewayHttpDrainOutcome),
    /// The stdin guard returned. It parks forever after EOF, so this branch
    /// is not expected to fire; it is named rather than silently absorbed.
    StdinGuard,
    /// SIGINT or SIGTERM.
    Signal(ShutdownSignal),
}

impl RunEnd {
    /// The `reason=` token.
    fn reason(&self) -> &'static str {
        match self {
            Self::HttpServer(_) => "http_server_ended",
            Self::StdinGuard => "stdin_guard_ended",
            Self::Signal(_) => "signal",
        }
    }

    /// The signal that ended the loop, when one did.
    fn signal(&self) -> Option<ShutdownSignal> {
        match self {
            Self::Signal(signal) => Some(*signal),
            Self::HttpServer(_) | Self::StdinGuard => None,
        }
    }
}

async fn run(launch: GatewayLaunchArgs) -> anyhow::Result<()> {
    let GatewayLaunchArgs {
        control_listen,
        http_listen: http_listen_flag,
        allow_remote: allow_remote_flag,
        flow_editor_host_deploy,
    } = launch;
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin);
    let mut init_line = String::new();
    if reader.read_line(&mut init_line).await? == 0 {
        return Err(anyhow!("stdin closed before init request"));
    }

    let (request_id, params) = match parse_init_request(init_line.trim()) {
        Ok(value) => value,
        Err(error) => {
            return Err(refuse_init(&Value::Null, -32602, error.to_string()));
        }
    };

    let workspace_root = params
        .workspace_root
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let workspace_root = workspace_root.canonicalize().unwrap_or(workspace_root);
    let project_root = params
        .project_root
        .unwrap_or_else(|| workspace_root.clone());
    let project_root = project_root.canonicalize().unwrap_or(project_root);
    let context_root = params
        .context_root
        .map(|path| path.canonicalize().unwrap_or(path))
        .or_else(|| Some(project_root.clone()));
    let runtime_root = params
        .runtime_root
        .unwrap_or_else(|| workspace_root.clone());
    let runtime_root = runtime_root.canonicalize().unwrap_or(runtime_root);
    let store_path = params
        .store_path
        .unwrap_or_else(|| runtime_root.join("state"));
    let store_path =
        meerkat_meta::storage_layout::canonicalize_storage_root(&store_path).map_err(|error| {
            refuse_init(
                &request_id,
                -32602,
                format!(
                    "cannot resolve store_path {}: {error}",
                    store_path.display()
                ),
            )
        })?;
    // The path authority for this boot: the state dir (a `store_path` with a
    // file extension is the explicit session-DB override escape hatch) plus
    // the XDG gateway home (runtime registry + peer key).
    let gateway_home = meerkat_meta::storage_layout::default_gateway_home().map_err(|error| {
        refuse_init(
            &request_id,
            -32603,
            format!("resolve gateway state directory: {error}"),
        )
    })?;
    let layout = MetaStorageLayout::standalone_from_store_path(&store_path, gateway_home.clone());
    meerkat_meta::native_startup::refuse_session_database_override(&layout).map_err(|error| {
        refuse_init(
            &request_id,
            meerkat_meta::STORAGE_RESOLUTION_CODE,
            error.to_string(),
        )
    })?;
    let persistent_sessions = params.persistent_sessions.unwrap_or(false);
    let provider_credentials_root = params.provider_credentials_root.clone();
    // Host-level compaction policy: validated once here so a typo or a zero
    // threshold is an init error, not a dead knob or a compaction storm.
    let compaction_policy = params
        .compaction
        .as_ref()
        .map(|value| {
            meerkat_meta::parse_compaction_policy(value).map_err(|error| {
                refuse_init(
                    &request_id,
                    -32602,
                    format!("init params: compaction {error}"),
                )
            })
        })
        .transpose()?;
    // Doctrine default: the identity substrate is ON. `identity_first: false`
    // remains as a one-release opt-out for deployments that need the pure
    // mob-plane console back (changelog-flagged).
    let identity_first = params.identity_first.unwrap_or(true);
    // Mob announcements compose through the one composition every surface
    // shares: the member tool bundle goes on the persistent mob spec, and the
    // owner attaches after the identity-first attachment. The options keep
    // the default peer request window: this gateway's control listener
    // serves keyed injects with it.
    let announcements = match params.mob_announcements.as_ref() {
        Some(value) => meerkat_meta::mob_announcements::parse_options(value, "mob_announcements")
            .map_err(|error| anyhow!("{error}"))?
            .map(meerkat_meta::mob_announcements::AnnouncementComposition::new)
            .transpose()
            .map_err(|error| anyhow!("mob_announcements: {error}"))?,
        None => None,
    };
    let identity_roster_seed = params.identity_roster.clone().unwrap_or_default();
    let role_migration_declarations = params.role_migrations.clone().unwrap_or_default();
    let realm = params.realm.as_deref();
    let isolated = params.isolated.unwrap_or(false);
    let _surface = params.surface.unwrap_or_else(|| "tux".to_string());
    let runtime_profile = params
        .runtime_profile
        .unwrap_or_else(|| "tux-auto".to_string());
    let console_read_only = match params.console_read_only {
        Some(value) => value,
        None => env_bool("META_CONSOLE_READ_ONLY")?.unwrap_or(false),
    };
    // HTTP listener: init param > --http-listen > META_HTTP_LISTEN_ADDR >
    // loopback on an ephemeral port. Resolved before the resume lookup so the
    // address joins the resume key and the exposure gate runs on every
    // launch, resumed or not.
    let http_listen = resolve_http_listen(
        params.http_listen.as_deref(),
        http_listen_flag,
        std::env::var("META_HTTP_LISTEN_ADDR").ok().as_deref(),
    )
    .map_err(|error| {
        refuse_init(
            &request_id,
            -32602,
            format!("init params: http_listen: {error}"),
        )
    })?;
    let allow_remote = match params.allow_remote {
        Some(value) => value,
        None => allow_remote_flag || env_bool("META_HTTP_ALLOW_REMOTE")?.unwrap_or(false),
    };
    let declared_public_base_url = params
        .http_public_base_url
        .as_deref()
        .map(meerkat_meta::gateway_composition::parse_http_public_base_url)
        .transpose()
        .map_err(|error| {
            refuse_init(
                &request_id,
                -32602,
                format!("init params: http_public_base_url: {error}"),
            )
        })?;
    // Exposure gate, before the registry lookup and before any bootstrap. The
    // console this binary serves is open (`runtime_decision_state` hardcodes
    // `require_app_auth: false`), so only `allow_remote` can open it; the gate
    // still reads the real decision state so the rule has one owner. The UI
    // config plays no part in the gate and is loaded later, as before.
    meerkat_meta::gateway_composition::validate_http_bind_policy(
        meerkat_meta::gateway_composition::GatewaySurface::MetaGateway,
        http_listen,
        meerkat_meta::gateway_composition::HttpBindPolicy::for_gateway(
            allow_remote,
            &runtime_decision_state(ConsoleUiConfig::default(), console_read_only),
        ),
    )
    .map_err(|error| refuse_init(&request_id, -32602, format!("init params: {error}")))?;

    let mut paths = conventional_paths(&workspace_root);
    // An explicit init param wins over the workspace convention. The
    // effective path rides `paths` so the resume fingerprint and the loader
    // below read the same file.
    if let Some(explicit) = params.meerkat_config_path.clone() {
        paths.meerkat_config_toml = Some(explicit);
    }
    // Adoption is never invisible: the convention picks up a file the operator
    // may not have written for this gateway, and its content joins the resume
    // key, so the log names the file and which door produced it.
    if let Some(path) = paths.meerkat_config_toml.as_deref() {
        let source = if params.meerkat_config_path.is_some() {
            "meerkat_config_path init param"
        } else {
            "<workspace>/.rkat/config.toml convention"
        };
        tracing::info!(
            path = %path.display(),
            source,
            "meerkat host config adopted; every agent this gateway builds starts from it"
        );
    }
    // Loaded before the registry lookup so a bad file refuses init instead of
    // resuming or creating a runtime built from `Config::default()`.
    let host_config = paths
        .meerkat_config_toml
        .as_deref()
        .map(|path| {
            meerkat_meta::gateway_composition::load_gateway_host_config(path).map_err(|error| {
                refuse_init(
                    &request_id,
                    -32602,
                    format!("init params: meerkat_config_path: {error}"),
                )
            })
        })
        .transpose()?;
    let key = config_fingerprint(
        &workspace_root,
        realm,
        isolated,
        &runtime_profile,
        persistent_sessions,
        console_read_only,
        &runtime_root,
        &store_path,
        &project_root,
        context_root.as_deref(),
        compaction_policy.as_ref(),
        control_listen.as_ref(),
        http_listen,
        declared_public_base_url.as_deref(),
        &paths,
    )?;
    // A launch requesting authenticated voice must not resume an older open
    // console just because its workspace/listener settings happen to match.
    let key = if params.auth_config.is_some() || params.console_voice.is_some() {
        let auth_shape = serde_json::to_vec(&(
            &key,
            params.auth_config.as_ref(),
            params.console_voice.as_ref(),
        ))?;
        format!("{:x}", Sha256::digest(auth_shape))
    } else {
        key
    };
    let key = resume_key_with_flow_editor_host_deploy(key, flow_editor_host_deploy);
    // A launch resumes only a runtime on its own credentials root. The root
    // joins the registry's resume key alone: `key` also names a runtime
    // started without a definition file, and its discovery namespace, which
    // naming a root must not change.
    let resume_key = resume_key_with_provider_credentials_root(
        key.clone(),
        provider_credentials_root.as_deref(),
    );
    let registry_file = layout
        .registry_file()
        .ok_or_else(|| anyhow!("gateway storage layout carries no gateway home"))?;
    let mut registry = load_registry(&registry_file);

    let mut live_entries = Vec::new();
    let mut resumed_entry = None;
    for entry in registry.entries.drain(..) {
        if url_is_alive(&entry.http_base_url).await {
            if entry.key == resume_key {
                resumed_entry = Some(entry.clone());
            }
            live_entries.push(entry);
        }
    }
    registry.entries = live_entries;
    save_registry(&registry_file, &registry)?;

    if let Some(entry) = resumed_entry {
        print_json_line(&init_response(
            request_id,
            &entry.runtime_id,
            &entry.http_base_url,
            "resumed",
            entry.control_listen_address.as_deref(),
            entry.http_public_base_url.as_deref(),
        ));
        return Ok(());
    }

    // Bind the HTTP listener before any bootstrap work and right after the
    // resume lookup: a resumed launch reports the live runtime's listener and
    // must not contend for its port, while a fresh launch on a fixed
    // `http_listen` may find the port taken (EADDRINUSE). Failing here costs
    // the init reply and nothing else: no runtime build and no schedule
    // executor lease left held for its full duration by a process exit. The
    // listener is served once the runtime is up (`http_binding.serve`).
    let http_binding = meerkat_meta::gateway_composition::GatewayHttpBinding::bind(http_listen)
        .await
        .map_err(|error| {
            refuse_init(
                &request_id,
                -32603,
                format!("failed to bind gateway listener {http_listen}: {error}"),
            )
        })?
        .with_advertised_base_url(declared_public_base_url);
    // One Meta runtime per state directory: a launch that opens stores in it
    // (persistent sessions, or the identity substrate) claims it first. The
    // claim is held for the rest of this run; the kernel releases it when
    // the gateway exits.
    let _state_dir_claim = if persistent_sessions || identity_first {
        Some(
            meerkat_meta::state_dir_claim::StateDirClaim::acquire(layout.state_dir()).map_err(
                |error| {
                    refuse_init(
                        &request_id,
                        meerkat_meta::STORAGE_RESOLUTION_CODE,
                        error.to_string(),
                    )
                },
            )?,
        )
    } else {
        None
    };
    // The persistent launch opens Core's stores in the state directory:
    // before any of them opens, no other process hosts runtimes there, the
    // layout is native, and identity bindings still have their runtime store.
    // Its mob storage is always the durable event log.
    if persistent_sessions {
        meerkat_meta::native_startup::refuse_unready_state_dir(
            layout.state_dir(),
            Some(&layout.event_log_db()),
        )
        .await
        .map_err(|error| {
            refuse_init(
                &request_id,
                meerkat_meta::STORAGE_RESOLUTION_CODE,
                error.to_string(),
            )
        })?;
    }
    let http_base_url = http_binding.http_base_url();
    let http_public_base_url = http_binding.advertised_base_url().map(str::to_string);

    std::env::set_current_dir(&workspace_root).ok();
    let (definition, used_workspace_config) = load_definition(&workspace_root, &key, &paths)
        .map_err(|error| refuse_init(&request_id, -32602, error.to_string()))?;
    let console_ui = match &paths.console_toml {
        Some(path) => load_console_ui_config_from_path_for_realm(path, realm)
            .with_context(|| format!("failed to load {}", path.display()))?,
        None => ConsoleUiConfig::default(),
    };
    let escalation_routes = match &paths.gating_toml {
        Some(path) => load_gating_escalation_routes(path)
            .with_context(|| format!("failed to load {}", path.display()))?,
        None => meerkat_meta::GatingEscalationRoutes::default(),
    };
    for origin in [
        Some(http_base_url.as_str()),
        http_public_base_url.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        console_ui
            .validate_mcp_apps_sandbox(Some(origin))
            .map_err(|error| refuse_init(&request_id, -32602, error.to_string()))?;
    }
    let runtime_id = definition.id.to_string();
    let image_generation = mob_definition_may_use_image_generation(&definition);

    // The provider credential store this launch names, opened once: every
    // agent factory below holds it in place of the process default.
    let provider_auth_persistence = provider_credentials_root
        .as_deref()
        .map(meerkat_meta::gateway_composition::open_provider_credentials_root)
        .transpose()
        .map_err(|error| {
            refuse_init(
                &request_id,
                meerkat_meta::STORAGE_RESOLUTION_CODE,
                format!("init params: provider_credentials_root: {error}"),
            )
        })?;
    // Every member factory below composes its MCP routers under this custody.
    let member_mcp_custody = meerkat_meta::MemberMcpCustody::new();
    let session_bootstrap = if persistent_sessions {
        let (
            service,
            adapter,
            binary_blob_store,
            schedule_host_inputs,
            workgraph,
            resolved_storage,
            committed_boundary_recoverer,
            runtime_store,
            voice_inputs,
            runtime_delivery,
            host_default_decorator,
        ) = build_persistent_session_service(
            &layout,
            runtime_root.clone(),
            project_root.clone(),
            context_root.clone(),
            image_generation,
            console_ui.mcp_apps_sandbox_url.is_some(),
            &runtime_id,
            host_config.as_ref(),
            compaction_policy.as_ref(),
            &member_mcp_custody,
            provider_auth_persistence.clone(),
        )?;
        // Pair the schedule wiring with a clone of the runtime adapter so the
        // firing host can be spawned once the runtime has booted (below).
        let schedule_host_inputs =
            schedule_host_inputs.map(|(sched, registry, svc, path, binding)| {
                (sched, registry, svc, path, binding, adapter.clone())
            });
        let workgraph_service = workgraph.as_ref().map(|(service, _, _)| service.clone());
        // The explicit runtime adapter must share the session service's runtime
        // persistence authority or meerkat 0.7 fails the bootstrap closed.
        // `with_session_runtime_adapter` wires the SAME adapter into the session
        // service (mirrors rpc_gateway.rs). This branch already shared via the
        // runtime_store handed to PersistentSessionService, so this is defensive;
        // the default ephemeral branch below is the one that was actually broken.
        // Persistent sessions require persistent mob state. An in-memory mob
        // storage here drops every adopted identity declaration on restart
        // while the session store faithfully survives, which presents as a
        // healthy boot with the right member count and an empty roster of
        // adoptions - the failure announces nothing.
        let (mob_storage, mob_storage_provenance) =
            meerkat_meta::mob_composition_manifest::persistent_mob_storage(layout.event_log_db())?;
        let mut spec = MobBootstrapSpec::new(definition, mob_storage, service)
            // The config `build_persistent_session_service` gave the member
            // builder: the same producer over the same inputs.
            .with_member_model_config(Arc::new(gateway_agent_config(
                host_config.as_ref(),
                compaction_policy.as_ref(),
            )?))
            .with_member_mcp_custody(Arc::clone(&member_mcp_custody))
            .with_host_default_agent_llm_client_decorator(host_default_decorator)
            .with_mob_storage_provenance(mob_storage_provenance)
            // Agent-created child mobs default to durable under this layout.
            .with_meta_storage_layout(layout.clone())
            // The SAME store handed to MeerkatMachine::persistent above.
            .with_durable_runtime_store(&runtime_store)
            // Resume-seam reads must carry the runtime store's archived
            // terminal (at 0.8.11 archive stamps the catalog/lifecycle row,
            // never the session body).
            .with_runtime_archived_terminal_authority(runtime_store)
            .with_session_runtime_adapter(adapter.clone())
            .context("failed to acquire the gateway session runtime owner")?
            .with_workgraph_service(workgraph_service.clone())
            // This is the persistent session builder's shared agent-tool slot.
            // Identity activation needs it even when console voice is absent.
            .with_runtime_delivery(runtime_delivery)
            .with_agent_mob_tools(Arc::clone(&voice_inputs.4))
            .context("failed to install gateway agent mob tools")?;
        spec.committed_boundary_recoverer = Some(committed_boundary_recoverer);
        // The concrete persistent session owner pins controllers for governed
        // binding (the erased MobSessionService carries no pin method).
        spec.controller_pin_source = Some(Arc::clone(&voice_inputs.0) as _);
        if let Some((_, admission_slot, state_dir)) = &workgraph {
            // Durable (cross-process shareable) store: register the tool-plane
            // admission slot and the sidecar lock beside the store.
            spec = spec
                .with_workgraph_admission_slot(admission_slot.clone())
                .with_workgraph_admission_sidecar(state_dir);
        }
        spec.runtime_adapter = Some(adapter);
        spec.binary_blob_store = Some(binary_blob_store);
        let mut resolved_storage = resolved_storage;
        // The M4 census covered nine slots and not this one, which is why an
        // in-memory mob storage on a persistent launch was invisible to
        // healthz and the storage doctor.
        resolved_storage.slots.push(
            meerkat_meta::storage_health::StorageSlotSummary::persistent(
                "mob",
                "MobStorage(sqlite)",
            ),
        );
        spec.resolved_storage = Some(resolved_storage);
        if let Some(announcements) = announcements.as_ref() {
            // The host's own profiles name the member tool bundle; under the
            // ceiling a child mob profile mounts it by naming it in its
            // tools.bundles.
            spec = announcements.register_member_tools(spec);
        }
        (
            spec,
            schedule_host_inputs,
            workgraph_service,
            Some(voice_inputs),
        )
    } else {
        // Build the ephemeral path manually to thread project/context roots
        // into AgentFactory (MobBootstrapSpec::ephemeral doesn't accept them).
        let binary_blob_store: Arc<dyn BinaryBlobStore> = Arc::new(ObjectStoreBlobStore::memory());
        let blob_store: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(Base64BlobStoreAdapter::new(binary_blob_store.clone()));
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let adapter = Arc::new(
            meerkat_runtime::MeerkatMachine::persistent(
                Arc::clone(&runtime_store),
                Arc::clone(&blob_store),
            )
            .context("failed to acquire the gateway runtime machine")?,
        );
        let session_store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let mut factory = AgentFactory::new(&runtime_root)
            .session_store(session_store.clone())
            .runtime_root(runtime_root.clone())
            .project_root(project_root.clone())
            .builtins(true)
            .shell(true)
            .mob(true)
            .comms(true)
            .memory(true)
            .mcp_apps(console_ui.mcp_apps_sandbox_url.is_some());
        // As in the persistent arm: the launch's provider credential store.
        if let Some(persistence) = provider_auth_persistence.clone() {
            factory = factory.with_provider_auth_persistence(persistence);
        }
        if image_generation {
            factory = factory.with_image_generation_machine(
                meerkat_meta::mob_handle_runtime::weak_image_generation_machine(&adapter),
            );
        }
        if let Some(ref ctx) = context_root {
            factory = factory.context_root(ctx.clone());
        }
        factory = member_mcp_custody.install(factory);
        // Same producer as the persistent arm: host config first, the
        // compaction declaration on top.
        let config = gateway_agent_config(host_config.as_ref(), compaction_policy.as_ref())?;
        let member_model_config = Arc::new(config.clone());
        let mut builder = member_agent_builder(factory, config);
        let host_default_decorator = Arc::clone(&builder.default_agent_llm_client_decorator);
        builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(
            session_store.clone(),
        )));
        builder.default_blob_store = Some(blob_store.clone());
        let jobs: Arc<dyn meerkat::DetachedJobStore> =
            Arc::new(meerkat::MemoryDetachedJobStore::new());
        let runtime_delivery = meerkat_meta::mob_handle_runtime::MobRuntimeDelivery::new(
            Arc::clone(&runtime_store),
            Arc::clone(&jobs),
        );
        builder.default_detached_job_store = Some(Arc::clone(&jobs));
        builder.default_shell_job_delivery_projector =
            Some(meerkat::JobOutboxProjector::new_for_realm(
                jobs,
                runtime_delivery.inbox(),
                meerkat_meta::storage_provider::DEFAULT_MEERKAT_LEVEL_REALM_ID,
            ));
        let agent_mob_tools_slot = Arc::clone(&builder.default_mob_tools);

        // The default TUX launch is ephemeral: a memory-backed workgraph
        // keeps the feature available (tools stay profile-gated). Memory
        // store = single process, so no admission sidecar.
        let (ephemeral_workgraph, workgraph_admission_slot) =
            meerkat_meta::workgraph_wiring::attach_workgraph_tools_ephemeral(&builder, &runtime_id);
        let workgraph_service = Some(ephemeral_workgraph);
        let session_service = Arc::new(PersistentSessionService::new(
            builder,
            64,
            session_store,
            Arc::clone(&runtime_store),
            blob_store,
        ));
        session_service
            .acquire_canonical_runtime_adapter(Some(Arc::clone(&adapter)))
            .context("failed to acquire the process-local gateway session runtime owner")?;
        let mut spec = MobBootstrapSpec::new(definition, MobStorage::in_memory(), session_service)
            .with_member_model_config(member_model_config)
            .with_member_mcp_custody(Arc::clone(&member_mcp_custody))
            .with_host_default_agent_llm_client_decorator(host_default_decorator)
            // The SAME store handed to MeerkatMachine::persistent above.
            .with_durable_runtime_store(&runtime_store)
            .with_session_runtime_adapter(adapter.clone())
            .context("failed to acquire the gateway session runtime owner")?
            .with_workgraph_service(workgraph_service.clone())
            .with_workgraph_admission_slot(workgraph_admission_slot)
            .with_runtime_delivery(runtime_delivery)
            .with_agent_mob_tools(agent_mob_tools_slot)
            .context("failed to install ephemeral gateway agent mob tools")?;
        spec.runtime_adapter = Some(adapter);
        spec.binary_blob_store = Some(binary_blob_store);
        // The service's session and blob stores are explicitly process-local;
        // no disk-backed session store is advertised by this launch.
        let mut slots = vec![
            meerkat_meta::storage_health::StorageSlotSummary::declared_ephemeral(
                "sessions",
                "MemoryStore",
                "declared by the default ephemeral launch (persistent_sessions = false)",
            ),
            meerkat_meta::storage_health::StorageSlotSummary::declared_ephemeral(
                "runtime",
                "InMemoryRuntimeStore",
                "declared by the default ephemeral launch",
            ),
            meerkat_meta::storage_health::StorageSlotSummary::declared_ephemeral(
                "jobs",
                "MemoryDetachedJobStore",
                "declared by the default ephemeral launch",
            ),
            meerkat_meta::storage_health::blob_slot_summary(
                meerkat_meta::storage_health::BlobDurability::DeclaredEphemeral,
            ),
            meerkat_meta::storage_health::StorageSlotSummary::declared_ephemeral(
                "console",
                "InMemoryConsoleLogStore",
                "declared default of this surface",
            ),
            meerkat_meta::storage_health::StorageSlotSummary::declared_ephemeral(
                "metadata",
                "InMemoryMetadataStore",
                "declared default of this surface",
            ),
            meerkat_meta::storage_health::StorageSlotSummary::declared_ephemeral(
                "workgraph",
                "MemoryWorkGraphStore",
                "declared by the default ephemeral launch",
            ),
        ];
        slots.push(
            meerkat_meta::storage_health::StorageSlotSummary::declared_ephemeral(
                "jobs",
                "MemoryDetachedJobStore",
                "declared by the ephemeral launch mode",
            ),
        );
        slots.extend(meerkat_meta::storage_health::scratch_ring_buffer_slots());
        // Declared, not silent: this launch keeps mob state in memory by
        // design, and the census now says so rather than omitting the slot.
        slots.push(
            meerkat_meta::storage_health::StorageSlotSummary::declared_ephemeral(
                "mob",
                "MobStorage(in-memory)",
                "declared by the ephemeral launch mode: mob events and adopted identity \
                 declarations do not survive gateway restart",
            ),
        );
        spec.resolved_storage = Some(
            meerkat_meta::storage_health::ResolvedStorageSummary::new(
                meerkat_meta::storage_health::BlobDurability::DeclaredEphemeral,
                None,
            )
            .with_slots(slots),
        );
        // This process-local launch keeps the schedule firing host and voice
        // host disabled; their gateway configuration is persistent-only.
        (spec, None, workgraph_service, None)
    };
    let (session_spec, schedule_host_inputs, workgraph_service, voice_inputs) = session_bootstrap;
    let mob_spec = session_spec.with_options(MobBootstrapOptions {
        allow_ephemeral_sessions: !persistent_sessions,
        notify_orchestrator_on_resume: true,
        default_llm_client: None,
    });

    let bootstrap_plan =
        meerkat_meta::gateway_composition::GatewayRuntimeBootstrapPlan::console_http(
            mob_spec,
            meerkat_meta::MetaConfig {
                modules: Vec::new(),
                discovery: meerkat_meta::DiscoverySpec {
                    namespace: format!("tux.{}", short_hash(&key)),
                    modules: Vec::new(),
                },
                pre_spawn: Vec::new(),
            },
            Duration::from_secs(30),
        );
    let mut composition = meerkat_meta::gateway_composition::GatewayComposition::prepare(
        meerkat_meta::gateway_composition::GatewayCompatibilityProfile::ConsoleHttp,
        bootstrap_plan,
    )
    .bootstrap()
    .await
    .context("failed to bootstrap local runtime")?;
    let runtime = composition.runtime_mut();

    // Load contacts.toml if present. This enables meta/cross_mob/directory
    // (lookup of known mob addresses) without requiring peer mob handles.
    // High-level wire/unwire/send still need peer handles and are gated
    // separately by has_peer_mob_handles().
    let mut control_grants = ControlGrantTable::new();
    if let Some(ref contacts_path) = paths.contacts_toml {
        let contacts_text = fs::read_to_string(contacts_path)
            .with_context(|| format!("failed to read {}", contacts_path.display()))?;
        let directory = ContactDirectory::from_toml(&contacts_text)
            .with_context(|| format!("failed to parse {}", contacts_path.display()))?;
        if let Some(configured) =
            ControlGrantTable::from_toml(&contacts_text).with_context(|| {
                format!(
                    "failed to parse control grants in {}",
                    contacts_path.display()
                )
            })?
        {
            control_grants = configured;
        }
        runtime.set_contact_directory(directory);
    }

    // Opt-in ABAC: a `config/access.toml` wires the access controller into
    // every console and SSE surface. Console admin edits persist back to
    // the same file, so the deployment's access policy survives restarts.
    if let Some(ref access_path) = paths.access_toml {
        let controller = meerkat_meta::AccessController::load(access_path)
            .map_err(|error| anyhow!("failed to load {}: {error}", access_path.display()))?;
        runtime.set_access_controller(controller);
    }
    // The Console's session fence record (revoked sessions, principals whose
    // tokens never expire) lives in the state directory, in a file of its
    // own. A declared-ephemeral state root keeps none: a revocation there is
    // refused rather than lost at the next start.
    if !layout.is_declared_ephemeral() {
        let fences = meerkat_meta::ConsoleSessionFences::open_in_state_dir(layout.state_dir())
            .map_err(|error| anyhow!("failed to open the console session fence record: {error}"))?;
        runtime.set_console_session_fences(fences);
    }
    // Without an enforced access controller the Flow Editor refuses host
    // deploys unless the launch opted in.
    if flow_editor_host_deploy {
        tracing::warn!(
            "--allow-flow-editor-host-deploy: the Flow Editor on this gateway's listener lets \
             every caller that no enforced access controller governs run rkat on this host"
        );
        runtime.allow_flow_editor_host_deploy(true);
    }
    // A host deploy's `rkat` process could not carry this launch's provider
    // credential store, so the runtime refuses one typed.
    if provider_auth_persistence.is_some() {
        runtime.set_explicit_provider_credentials(true);
    }

    // Load (or mint and persist) the gateway's Ed25519 signing keypair.
    // Stored under the same state directory the registry lives in, which
    // already survives across runs. Cross-process peers fetch the
    // resulting pubkey via `meta/peer_pubkey`; inproc-only deployments
    // never use it but it's cheap to keep one ready.
    let peer_keys = GatewayPeerKeys::load_or_create(&gateway_home).with_context(|| {
        format!(
            "failed to load or mint gateway peer key under {}",
            gateway_home.display()
        )
    })?;
    runtime.set_gateway_peer_keys(peer_keys);

    // Identity-first mode (meerkat-studio ask K0): give this gateway the
    // durable-identity substrate — continuity records, lease-fenced
    // embodiment, resume-first restore with the Broken-identity repair task,
    // and the tolerant identity lifecycle paths. Default providers are
    // constructed from the existing store paths; the roster is seeded from
    // init params and extended at runtime by `meta/ensure_member`.
    let _identity_roster_provider: Option<
        Arc<meerkat_meta::identity_first::MutableRosterProvider>,
    > = if identity_first {
        use meerkat_meta::identity_first::{
            AgentRuntimeServices, DurabilityPolicy, IdentityFirstRuntimeContext, IdentityRuntime,
            IdentityRuntimeConfig, MobSessionBridge, MutableRosterProvider,
        };

        let store_dir = layout.state_dir();
        fs::create_dir_all(store_dir)
            .with_context(|| format!("failed to create {}", store_dir.display()))?;
        let continuity_db = layout.continuity_db().map_err(|e| anyhow!("{e}"))?.path;
        let substrate = meerkat_meta::gateway_wiring::open_identity_substrate(&continuity_db)
            .await
            .map_err(|e| anyhow!("{e}"))?;

        let mob_handle = meerkat_meta::gateway_wiring::identity_mob_handle(runtime);
        let mut bridge =
            if let Some(session_service) = runtime.mob_runtime().session_service().cloned() {
                MobSessionBridge::with_session_service(mob_handle.clone(), session_service)
            } else {
                MobSessionBridge::new(mob_handle.clone())
            };
        // Heal seam (2026-07-29 incident): the continuity repair supervisor
        // asks this recoverer to commit the durable head before declaring an
        // identity healed; without it, heal is a cosmetic entry reset that
        // the next materialization re-Breaks.
        if let Some(recoverer) = runtime.mob_runtime().committed_boundary_recoverer() {
            bridge = bridge.with_committed_boundary_recoverer(recoverer);
        }
        // Image deliveries are judged with the member builder's host config,
        // so a self-hosted model's `vision` flag counts.
        if let Some(config) = runtime.mob_runtime().member_model_config() {
            bridge = bridge.with_member_model_config(config);
        }
        // Governed binding pins the member session actor's actual controller
        // through the concrete session owner.
        if let Some(source) = runtime.mob_runtime().controller_pin_source() {
            bridge = bridge.with_controller_pin_source(source);
        }
        if let Some((identity, first, second)) =
            meerkat_meta::identity_first::conflicting_role_migration_declaration(
                &role_migration_declarations,
            )
        {
            anyhow::bail!(
                "role_migrations declares '{identity}' twice with different predecessor roles \
                 ('{first}' and '{second}'); migration authority cannot be resolved by order"
            );
        }
        // A declared migration restamps a member's durable role, comms name
        // and binding, so the boot record must name every one the host armed.
        for declaration in &role_migration_declarations {
            tracing::info!(
                identity = %declaration.identity,
                from_role = %declaration.from_role,
                "activation declares a member role migration"
            );
        }
        bridge = bridge.with_role_migration_declarations(role_migration_declarations);
        let bridge: Arc<dyn meerkat_meta::identity_first::SessionBridge> = Arc::new(bridge);

        let irt = IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: substrate.continuity_store,
            lease_provider: substrate.lease_provider,
            runtime_instance_id: format!("meta-gateway-{}", std::process::id()),
            has_runtime_store: persistent_sessions,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(bridge),
            default_timeout: None,
        })
        .with_runtime_services(AgentRuntimeServices::new(mob_handle.clone()));
        // A member parked with a session repair hold names the exact
        // `rkat session repair-wholeblob` commands for the runtime store this
        // launch opened; the ephemeral launch has nothing to repair.
        irt.set_session_repair_scope(
            runtime
                .resolved_storage()
                .as_ref()
                .and_then(meerkat_meta::identity_first::SessionRepairScope::from_resolved_storage),
        );
        // `storage-migrate-native` leaves the first-boot manifest of a
        // migrated state directory there; the first roster compile carries
        // it out. The ephemeral launch has no state directory to migrate.
        irt.set_first_boot_state_dir(persistent_sessions.then(|| store_dir.to_path_buf()));
        // The held-completion marks a held bootstrap left in the same
        // directory refuse an ordinary wake of their members here too.
        irt.set_held_completion_state_dir(persistent_sessions.then(|| store_dir.to_path_buf()));

        let roster = Arc::new(MutableRosterProvider::new(identity_roster_seed));
        let mob_definition = mob_handle.definition().clone();
        let irt = Arc::new(irt);
        // Register continuity owners before a stopped mob resumes, then apply
        // the roster. The installer owns cleanup if either stage fails.
        runtime.set_console_identity_roster(roster.clone());
        let context = Arc::new(IdentityFirstRuntimeContext::new(
            irt,
            roster.clone(),
            None,
            None,
            Some(mob_definition),
        ));
        runtime
            .install_and_bootstrap_identity_first_context(context, &roster.snapshot())
            .await
            .context("identity-first bootstrap failed")?;
        tracing::info!(
            roster = roster.snapshot().len(),
            continuity_db = %continuity_db.display(),
            "identity-first gateway mode active"
        );
        Some(roster)
    } else {
        runtime
            .activate_without_identity_context()
            .await
            .context("failed to activate classic local runtime")?;
        None
    };

    if !used_workspace_config {
        let mut labels = BTreeMap::new();
        labels.insert("surface".to_string(), "tux".to_string());
        labels.insert("ui".to_string(), "meerkat-tux".to_string());
        if let Some(realm) = realm {
            labels.insert("realm".to_string(), realm.to_string());
        }
        runtime
            .mob_handle()
            .ensure_member(
                SpawnMemberSpec::new(ProfileName::from("alpha"), AgentIdentity::from("alpha"))
                    .with_labels(labels),
            )
            .await
            .map_err(|error| anyhow!("failed to spawn fallback alpha meerkat: {error}"))?;
    }

    // Mob announcements attach once the identity runtime is installed and
    // before the control listener seals its keyed inject plane: the order
    // every surface composes them in.
    if let Some(announcements) = announcements {
        announcements
            .compose(runtime, &layout)
            .map_err(|error| anyhow!("mob_announcements: {error}"))?;
    }

    // Bind the cross-mob control listener after identity-first attachment so
    // its startup log reflects the final authority posture (the handler
    // re-reads the identity slot per request either way).
    if let Some(addr) = control_listen.as_ref() {
        let authorizer = Arc::new(ControlAuthorizer::with_grants_for_audience(
            control_grants,
            runtime.mob_id(),
        ));
        let advertised = runtime
            .start_control_listener_with_authorizer(addr, authorizer)
            .await
            .map_err(|error| anyhow!("--control-listen {addr}: {error}"))?;
        tracing::info!(%advertised, "cross-mob control listener bound");
        // The runtime is composed: seal what the listener's keyed inject
        // plane serves, through the composition every surface shares.
        meerkat_meta::mob_announcements::compose_keyed_inject_plane(
            runtime,
            meerkat_meta::mob_announcements::KeyedInjectPlane::default(),
        )
        .map_err(|error| anyhow!("--control-listen {addr}: {error}"))?;
    }

    // Start schedule delivery only after identity-first attachment. The host
    // owns a snapshot of the authority Arc, so starting it before this point
    // would permanently reject every generated `rt:*` target.
    let (schedule_host, _schedule_watchdog) = if let Some((
        schedule_service,
        mob_target_registry,
        service,
        schedule_store_path,
        schedule_firing_host_binding,
        adapter,
    )) = schedule_host_inputs
    {
        let mob_state = meerkat_meta::gateway_composition::adopt_schedule_mob_targets(
            runtime,
            &schedule_service,
            &mob_target_registry,
        )
        .await;
        // Shared with rpc_gateway: the watchdog liveness contract log, the
        // host spawn, the firing-intent gate bind and the boot probe were
        // byte-identical in both binaries, every log string included. This
        // gateway declares no host runnables (it has no memory steward and no
        // SDK callback plane), which is now an explicit `None` FIELD rather
        // than a hard-coded argument nobody could see was a divergence.
        let (schedule_host, watchdog) =
            meerkat_meta::gateway_composition::spawn_gateway_schedule_host(
                runtime,
                mob_state,
                meerkat_meta::gateway_composition::GatewayScheduleHostInputs {
                    schedule_service,
                    session_service: service,
                    runtime_adapter: adapter,
                    schedule_store_path,
                    firing_host_binding: schedule_firing_host_binding,
                    runnable_host: None,
                    workgraph_service: workgraph_service.clone(),
                    owner_id: runtime_id.clone(),
                },
            )
            .await;
        (schedule_host, Some(watchdog))
    } else {
        (None, None)
    };

    let composition = composition.activate();
    let runtime = composition.runtime();
    runtime
        .set_gating_escalation_routes(escalation_routes)
        .await;
    // The HTTP listener was bound at init, right behind the exposure gate and
    // the resume lookup (`http_binding` above); only serving it waited.

    let control_listen_address = runtime.control_listener_advertised_address();
    registry.entries.retain(|entry| entry.key != resume_key);
    registry.entries.push(RuntimeRegistryEntry {
        key: resume_key.clone(),
        runtime_id: runtime_id.clone(),
        http_base_url: http_base_url.clone(),
        pid: std::process::id(),
        updated_at_ms: current_time_ms(),
        control_listen_address: control_listen_address.clone(),
        http_public_base_url: http_public_base_url.clone(),
    });
    save_registry(&registry_file, &registry)?;

    let mut decisions = runtime_decision_state(console_ui, console_read_only);
    if let Some(auth) = params.auth_config.as_ref() {
        let configured = meerkat_meta::console_auth_config::parse_console_auth_config(auth)
            .map_err(|error| anyhow!("{error}"))?;
        decisions.auth = configured.auth;
        decisions.trusted_oidc = configured.trusted_oidc;
        decisions.console.require_app_auth = true;
    }
    #[cfg(feature = "openai-live")]
    let controller = if let Some(registration) = params.console_voice.as_ref() {
        let (service, machine, factory, config, _) =
            voice_inputs.ok_or_else(|| anyhow!("console_voice requires persistent sessions"))?;
        meerkat_meta::console_voice::ConsoleVoiceController::with_live_host(
            runtime,
            service,
            machine,
            factory,
            config,
            meerkat_meta::public_live_config::PublicLiveRegistration::parse(registration)
                .map_err(|error| anyhow!("console_voice: {error}"))?,
        )
        .map_err(|error| anyhow!("console voice composition failed: {error}"))?
    } else {
        meerkat_meta::console_voice::ConsoleVoiceController::default()
    };
    #[cfg(not(feature = "openai-live"))]
    let controller = {
        let _ = voice_inputs;
        meerkat_meta::console_voice::ConsoleVoiceController::default()
    };
    meerkat_meta::gateway_composition::warn_on_non_loopback_bind(
        meerkat_meta::gateway_composition::GatewaySurface::MetaGateway,
        http_binding.local_addr(),
        &decisions,
    );
    let app = runtime
        .build_reference_app_router(decisions)
        .layer(axum::Extension(controller.clone()));
    // Before anything is served: console sends a previous process accepted
    // and did not see settle are settled from the runtime's evidence.
    runtime.reconcile_interrupted_console_sends().await;
    // Listen for SIGINT and SIGTERM before the init response announces the
    // gateway: a stop signal from then on, also one that lands before the run
    // loop below first waits, runs the graceful shutdown. Registered by that
    // first wait instead, a signal in between found no handler installed and
    // its default action ended the process with the executor lease held.
    let shutdown = meerkat_meta::shutdown_signal::shutdown_signal();
    print_json_line(&init_response(
        request_id,
        &runtime_id,
        &http_base_url,
        "created",
        control_listen_address.as_deref(),
        http_public_base_url.as_deref(),
    ));
    let mut http_server = http_binding.serve(app);

    // `meta_gateway` serves the console/admin API over HTTP. It is NOT the
    // SDK's stdin JSON-RPC gateway — that is the separate `rpc_gateway` binary.
    // The SDK's PersistentTransport drives a gateway by sending JSON-RPC lines
    // (init, then reconcile_identity, send, ...) over stdin; we already consumed
    // the single init line above. If more JSON-RPC arrives on stdin, this binary
    // was misconfigured as the SDK gateway (a common mix-up, since the SDK env
    // var is named META_RPC_GATEWAY_BIN and the release ships this binary).
    // Answer each such line with a clear JSON-RPC error instead of silently
    // ignoring it, so the SDK surfaces an actionable failure immediately rather
    // than blocking forever waiting for a response that never comes.
    let stdin_guard = async move {
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => {
                    // Unlike rpc_gateway, this binary outlives its stdin by
                    // design (see below); say so, or a later exit gets
                    // misattributed to the EOF.
                    tracing::info!(
                        "stdin closed by the launching process; meta_gateway keeps serving HTTP \
                         until SIGINT or SIGTERM"
                    );
                    break;
                }
                Err(error) => {
                    tracing::warn!(
                        %error,
                        "stdin read failed; meta_gateway keeps serving HTTP until SIGINT or SIGTERM"
                    );
                    break;
                }
                Ok(_) => {}
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let id = serde_json::from_str::<Value>(trimmed)
                .ok()
                .and_then(|value| value.get("id").cloned())
                .unwrap_or(Value::Null);
            print_json_line(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {
                    "code": -32601,
                    "message": "meta_gateway serves the console/admin API over HTTP and does not handle SDK stdin JSON-RPC after init. Point your SDK gateway path (META_RPC_GATEWAY_BIN) at the 'rpc_gateway' binary instead."
                }
            }));
        }
        // stdin ended: keep serving HTTP until ctrl-c rather than tearing down
        // the console out from under any live HTTP clients.
        std::future::pending::<()>().await;
    };

    let run_end = tokio::select! {
        // SIGINT *and* SIGTERM: a container stop sends SIGTERM, and waiting
        // on ctrl_c alone meant the graceful path never ran on an ordinary
        // deploy. See `meerkat_meta::shutdown_signal` for why that became
        // load-bearing at 0.8.22 (unreleased schedule executor lease).
        outcome = http_server.wait() => RunEnd::HttpServer(outcome),
        () = stdin_guard => RunEnd::StdinGuard,
        signal = shutdown => RunEnd::Signal(signal),
    };
    // Name the branch that ended run() BEFORE the shutdown sequence, so a
    // wedge in it still leaves the reason in the log. `reason=` is one of
    // `RunEnd`'s tokens; `signal=` is present only when the reason is
    // `signal`.
    let exit_reason = run_end.reason();
    let exit_signal = run_end.signal();
    tracing::info!(
        reason = %exit_reason,
        signal = exit_signal.map(tracing::field::display),
        "meta_gateway run loop ended; running graceful shutdown"
    );
    if let RunEnd::HttpServer(outcome) = run_end {
        let failure = match outcome {
            meerkat_meta::gateway_composition::GatewayHttpDrainOutcome::Completed(Ok(())) => None,
            meerkat_meta::gateway_composition::GatewayHttpDrainOutcome::Completed(Err(error)) => {
                Some(anyhow::Error::new(error).context("gateway HTTP server failed"))
            }
            meerkat_meta::gateway_composition::GatewayHttpDrainOutcome::TimedOut => {
                Some(anyhow!("gateway HTTP server wait timed out"))
            }
            meerkat_meta::gateway_composition::GatewayHttpDrainOutcome::JoinFailed(error) => {
                Some(anyhow!("gateway HTTP server task failed: {error}"))
            }
        };
        if let Some(error) = failure {
            // main() reports this as a JSON-RPC error on stdout and exits 1;
            // the log stream is stderr, so name it there too.
            tracing::error!(
                error = %format!("{error:#}"),
                "meta_gateway exiting without graceful shutdown: the HTTP server task ended \
                 abnormally"
            );
            return Err(error);
        }
    }

    // Release the schedule executor lease on the graceful path.
    //
    // `ScheduleHostHandle::shutdown` is what calls
    // `driver.release_executor_lease()`. Dropping the handle only SIGNALS the
    // supervisor; tokio gives no guarantee that the woken supervisor is polled
    // between main's completion and runtime teardown, so the release may never
    // run. Both gateways previously bound `_schedule_host` and relied on Drop,
    // which left the lease row held: measured on a production store as all four
    // of owner_id / lease_token / acquired_at_ms / expires_at_ms still set 57s
    // past a CLEAN exit.
    //
    // Cost of leaving it held, from `shutdown_signal`: the replacement process
    // gets `AcquireScheduleExecutorLeaseOutcome::Busy`, its tick returns without
    // calling `claim_due_occurrences`, and SCHEDULES DO NOT FIRE for up to
    // `lease_duration` (60s) after every restart. The claim watchdog cannot see
    // it - its overdue threshold is longer than the window.
    //
    // This runs BEFORE `composition.shutdown()` because releasing the lease is a
    // store write, and the composition teardown is what closes the store.
    if let Some(schedule_host) = schedule_host {
        schedule_host.shutdown().await;
    }

    let shutdown = composition
        .shutdown(
            http_server,
            || async {
                if let Err(error) = controller.shutdown().await {
                    tracing::warn!(%error, "console voice shutdown did not attest complete cleanup");
                }
            },
            || async {
                let mut registry = load_registry(&registry_file);
                registry.entries.retain(|entry| entry.key != resume_key);
                save_registry(&registry_file, &registry)
            },
        )
        .await;
    if let Err(error) = shutdown.cleanup {
        tracing::error!(
            reason = %exit_reason,
            error = %error,
            "meta_gateway shutdown finished with a registry cleanup failure; exiting"
        );
        return Err(error);
    }
    // The closing bookend to the "run loop ended" line: if this one is
    // missing from a log that has the first, the shutdown sequence wedged.
    tracing::info!(
        reason = %exit_reason,
        signal = exit_signal.map(tracing::field::display),
        "meta_gateway shutdown complete; exiting"
    );
    if exit_signal.is_some() {
        // Tokio's stdin adapter performs a blocking read that cannot be
        // cancelled by dropping its future; on the signal path stdin may still
        // be open (a terminal, or a parent holding the pipe), so the runtime
        // destructor would wait forever for that helper thread and the process
        // would log "exiting" and then never exit. All Meta/runtime/registry
        // cleanup is complete above; exit explicitly, as rpc_gateway does.
        std::process::exit(0);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both arms build their members through [`member_agent_builder`], which
    /// prepares each member's construction only after its configured MCP
    /// servers have connected or failed.
    #[test]
    fn both_arms_build_members_after_their_mcp_servers_settle() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let builder = member_agent_builder(AgentFactory::new(dir.path()), Config::default());
        assert!(
            builder.wait_for_mcp,
            "a member construction waits for its configured MCP servers"
        );
        Ok(())
    }

    #[test]
    fn gating_toml_escalation_routes_load_strictly() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("gating.toml");
        std::fs::write(
            &path,
            "[actions]\npublish = \"r3\"\n\n[escalation]\ndefault = \"role:on-call\"\n\n[escalation.routes]\n\"person:alice\" = \"role:release-manager\"\n",
        )?;
        let routes = load_gating_escalation_routes(&path)?;
        assert_eq!(routes.default.as_deref(), Some("role:on-call"));
        assert_eq!(
            routes.successor_for(Some("person:alice")),
            Some("role:release-manager")
        );
        std::fs::write(&path, "[actions]\npublish = \"r3\"\n")?;
        assert_eq!(
            load_gating_escalation_routes(&path)?,
            meerkat_meta::GatingEscalationRoutes::default()
        );
        std::fs::write(&path, "[escalation]\ndefualt = \"role:on-call\"\n")?;
        assert!(load_gating_escalation_routes(&path).is_err());
        std::fs::write(&path, "[escalation]\ndefault = \" role:on-call\"\n")?;
        assert!(load_gating_escalation_routes(&path).is_err());
        // A directory that conventional discovery reports at gating.toml is
        // refused too, never read as "no gating".
        assert!(load_gating_escalation_routes(dir.path()).is_err());
        Ok(())
    }

    /// The default tracing filter (RUST_LOG unset) must surface this crate's
    /// own INFO lines — storage-verb/migration progress was invisible at the
    /// old blanket "warn" default — while keeping dependencies at WARN.
    #[test]
    fn default_tracing_filter_surfaces_own_info_keeps_deps_at_warn() -> anyhow::Result<()> {
        use tracing_subscriber::layer::SubscriberExt;
        let filter = tracing_subscriber::EnvFilter::try_new(
            meerkat_meta::gateway_composition::default_tracing_filter(GATEWAY_TRACING_TARGET),
        )?;
        let subscriber = tracing_subscriber::registry().with(filter);
        // A second registered Dispatch keeps callsite interest from being
        // cached `never` by threads without a subscriber.
        let _anchor = tracing::Dispatch::new(tracing_subscriber::Registry::default());
        tracing::subscriber::with_default(subscriber, || {
            assert!(
                tracing::enabled!(
                    target: "meerkat_meta::identity_first::local_store",
                    tracing::Level::INFO
                ),
                "the crate's own INFO lines (conversion progress) must pass the default filter"
            );
            assert!(
                tracing::enabled!(target: "meta_gateway", tracing::Level::INFO),
                "the gateway binary's own INFO lines must pass the default filter"
            );
            assert!(
                !tracing::enabled!(target: "meerkat_runtime::ops_lifecycle", tracing::Level::INFO),
                "dependency INFO noise must stay filtered"
            );
            assert!(
                tracing::enabled!(target: "meerkat_runtime::ops_lifecycle", tracing::Level::WARN),
                "dependency warnings must still pass"
            );
        });
        Ok(())
    }

    #[test]
    fn console_voice_init_requires_persistent_authenticated_host()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut params = json!({
            "persistent_sessions":true,
            "auth_config":{"provider":"jwt","shared_secret":"test-console-signing-key",
                "email_allowlist":["voice@example.com"]},
            "console_voice":{
                "principal":"voice@example.com","realm":"voice",
                "auth_binding":{"realm":"voice","binding":"openai"},"voice":"marin"
            }
        });
        let parse = |params: &Value| {
            parse_init_request(
                &json!({
                    "jsonrpc":"2.0","id":1,"method":"meta/init","params":params
                })
                .to_string(),
            )
        };
        assert_eq!(parse(&params).is_ok(), cfg!(feature = "openai-live"));
        params["persistent_sessions"] = json!(false);
        assert!(parse(&params).is_err());
        params["persistent_sessions"] = json!(true);
        params
            .as_object_mut()
            .ok_or("params must be an object")?
            .remove("auth_config");
        assert!(parse(&params).is_err());
        Ok(())
    }

    /// The other binary implements `application_tool_policies`; this one must
    /// not pretend to. `InitParams` has no `deny_unknown_fields`, so before
    /// this refusal the key was dropped in silence.
    /// `runtime_options` is an rpc_gateway protocol that this binary never
    /// parsed. #376 routed `declare_spec_update` through it, which turned the
    /// silent drop from "an option did nothing" into "a declaration the operator
    /// relied on did nothing". Refuse the object, naming the binary that reads it.
    #[test]
    fn init_refuses_runtime_options_rather_than_dropping_them() {
        let line = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "meta/init",
            "params": {
                "runtime_options": { "declare_spec_update": { "expected_revision": 1 } }
            }
        })
        .to_string();
        let error = parse_init_request(&line)
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default();
        assert!(
            !error.is_empty(),
            "a supplied runtime_options object must not be silently dropped"
        );
        assert!(
            error.contains("rpc_gateway") && error.contains("declare_spec_update"),
            "the refusal must name where the options work and the option that matters: {error}"
        );
    }

    #[test]
    fn init_refuses_application_tool_policies_rather_than_dropping_them() {
        let line = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "meta/init",
            "params": {
                "application_tool_policies": ["{}"]
            }
        })
        .to_string();

        let error = parse_init_request(&line)
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default();
        assert!(
            !error.is_empty(),
            "a supplied application tool policy must not be silently dropped"
        );
        // The refusal has to name the binary that DOES implement it, or the
        // host is told "no" with nowhere to go.
        assert!(
            error.contains("rpc_gateway"),
            "the refusal must name where the parameter works: {error}"
        );
    }

    #[test]
    fn init_refuses_a_prepared_application_rather_than_dropping_it() {
        // A valid carrier, an invalid one, and an explicit null are all a
        // supplied prepared application; none is read as absence.
        for carrier in [
            serde_json::json!({ "bundle_base64": "YQ==", "digest": "sha256:a" }),
            serde_json::json!("not-a-carrier"),
            serde_json::Value::Null,
        ] {
            let line = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "meta/init",
                "params": { "prepared_application": carrier }
            })
            .to_string();

            let error = parse_init_request(&line)
                .err()
                .map(|error| error.to_string())
                .unwrap_or_default();
            assert!(
                error.contains("prepared_application")
                    && error.contains("run_persistent_with_host"),
                "a supplied prepared application ({carrier}) must be refused, naming the \
                 host-composed entry: {error}"
            );
        }
        // Genuine absence still boots this binary's ordinary path.
        let absent = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "meta/init",
            "params": {}
        })
        .to_string();
        assert!(parse_init_request(&absent).is_ok());
    }

    /// The negative half: absence is still absence. A refusal that fired on
    /// every boot would be indistinguishable from one that works.
    #[test]
    fn an_absent_application_tool_policies_key_still_boots() -> anyhow::Result<()> {
        let line = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "meta/init",
            "params": { "console_read_only": true }
        })
        .to_string();
        let (_id, params) = parse_init_request(&line)?;
        assert!(params.application_tool_policies.is_none());
        Ok(())
    }

    #[test]
    fn init_params_parse_http_listen_allow_remote_and_public_base_url() -> anyhow::Result<()> {
        let line = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "meta/init",
            "params": {
                "http_listen": "0.0.0.0:8080",
                "allow_remote": true,
                "http_public_base_url": "https://mob.example.com"
            }
        })
        .to_string();

        let (_id, params) = parse_init_request(&line)?;

        assert_eq!(params.http_listen.as_deref(), Some("0.0.0.0:8080"));
        assert_eq!(params.allow_remote, Some(true));
        assert_eq!(
            params.http_public_base_url.as_deref(),
            Some("https://mob.example.com")
        );
        Ok(())
    }

    /// Precedence: init param, then `--http-listen`, then the environment,
    /// then loopback on an ephemeral port; an empty environment value is
    /// unset, a malformed one is a typed refusal naming the variable.
    #[test]
    fn resolve_http_listen_prefers_init_param_then_flag_then_env() -> Result<(), String> {
        let parse = |value: &str| -> Result<SocketAddr, String> {
            value
                .parse::<SocketAddr>()
                .map_err(|error| error.to_string())
        };
        let flag = parse("192.0.2.10:8080")?;
        assert_eq!(
            resolve_http_listen(Some("0.0.0.0:9090"), Some(flag), Some("[::]:7070"))?,
            parse("0.0.0.0:9090")?
        );
        assert_eq!(
            resolve_http_listen(None, Some(flag), Some("[::]:7070"))?,
            flag
        );
        assert_eq!(
            resolve_http_listen(None, None, Some("[::]:7070"))?,
            parse("[::]:7070")?
        );
        assert_eq!(
            resolve_http_listen(None, None, Some("  "))?,
            meerkat_meta::gateway_composition::DEFAULT_GATEWAY_HTTP_LISTEN
        );
        assert_eq!(
            resolve_http_listen(None, None, None)?,
            meerkat_meta::gateway_composition::DEFAULT_GATEWAY_HTTP_LISTEN
        );
        let error = resolve_http_listen(None, None, Some("localhost:1"))
            .err()
            .unwrap_or_default();
        assert!(error.contains("META_HTTP_LISTEN_ADDR"), "{error}");
        let error = resolve_http_listen(Some("nope"), None, None)
            .err()
            .unwrap_or_default();
        assert!(error.contains("HOST:PORT"), "{error}");
        Ok(())
    }

    /// This binary serves an OPEN console (no auth ingress), so enforced
    /// console auth can never open the exposure gate here: a non-loopback
    /// bind always needs `allow_remote`, and loopback never does.
    #[test]
    fn non_loopback_http_listen_always_needs_allow_remote_on_the_open_console()
    -> Result<(), std::net::AddrParseError> {
        use meerkat_meta::gateway_composition::{
            GatewaySurface, HttpBindPolicy, validate_http_bind_policy,
        };

        let state = runtime_decision_state(ConsoleUiConfig::default(), false);
        let remote: SocketAddr = "0.0.0.0:8080".parse()?;
        let refused = validate_http_bind_policy(
            GatewaySurface::MetaGateway,
            remote,
            HttpBindPolicy::for_gateway(false, &state),
        );
        let message = refused
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default();
        assert!(message.contains("meta_gateway"), "{message}");
        assert!(message.contains("META_HTTP_ALLOW_REMOTE"), "{message}");
        assert_eq!(
            validate_http_bind_policy(
                GatewaySurface::MetaGateway,
                remote,
                HttpBindPolicy::for_gateway(true, &state),
            ),
            Ok(())
        );
        assert_eq!(
            validate_http_bind_policy(
                GatewaySurface::MetaGateway,
                "127.0.0.1:0".parse()?,
                HttpBindPolicy::for_gateway(false, &state),
            ),
            Ok(())
        );
        Ok(())
    }

    /// The init reply carries the advertised base beside the same-host one,
    /// and `null` when none was declared, so an SDK can tell "not declared"
    /// from "declared".
    #[test]
    fn init_response_reports_the_advertised_public_base_url() {
        let with = init_response(
            serde_json::json!(1),
            "rt",
            "http://127.0.0.1:41",
            "created",
            None,
            Some("https://mob.example.com"),
        );
        assert_eq!(
            with["result"]["http_public_base_url"],
            serde_json::json!("https://mob.example.com")
        );
        assert_eq!(
            with["result"]["http_base_url"],
            serde_json::json!("http://127.0.0.1:41")
        );
        let without = init_response(
            serde_json::json!(1),
            "rt",
            "http://127.0.0.1:41",
            "resumed",
            None,
            None,
        );
        assert!(without["result"]["http_public_base_url"].is_null());
    }

    #[cfg(unix)]
    #[test]
    fn config_fingerprint_keeps_absent_store_identity_after_creation() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let workspace = temp.path().join("workspace");
        fs::create_dir(&workspace)?;
        let alias = temp.path().join("workspace-alias");
        std::os::unix::fs::symlink(&workspace, &alias)?;
        let workspace = workspace.canonicalize()?;
        let supplied = alias.join("future/nested/store");
        let paths = conventional_paths(&workspace);
        let fingerprint = |store: &Path| {
            config_fingerprint(
                &workspace,
                None,
                false,
                "tux-auto",
                true,
                false,
                &workspace,
                store,
                &workspace,
                Some(&workspace),
                None,
                None,
                meerkat_meta::gateway_composition::DEFAULT_GATEWAY_HTTP_LISTEN,
                None,
                &paths,
            )
        };
        let before = meerkat_meta::storage_layout::canonicalize_storage_root(&supplied)?;
        let first_key = fingerprint(&before)?;
        let (first_definition, _) = load_definition(&workspace, &first_key, &paths)?;
        assert!(!supplied.exists());
        fs::create_dir_all(&supplied)?;
        let after = meerkat_meta::storage_layout::canonicalize_storage_root(&supplied)?;
        let second_key = fingerprint(&after)?;
        let (second_definition, _) = load_definition(&workspace, &second_key, &paths)?;
        assert_eq!(
            first_key, second_key,
            "directory creation must not change the launch key"
        );
        assert_eq!(
            first_definition, second_definition,
            "the persisted mob definition must stay exact"
        );
        assert_ne!(
            first_key,
            fingerprint(&workspace.join("different-store"))?,
            "a genuinely different storage location must retain a distinct launch key"
        );
        Ok(())
    }

    /// Like `--control-listen`, the HTTP listen address is part of the resume
    /// key: a relaunch asking for a different bind must not resume a live
    /// runtime bound elsewhere.
    #[test]
    fn config_fingerprint_changes_with_http_listen() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let paths = conventional_paths(temp.path());
        let fingerprint = |listen: &str| -> anyhow::Result<String> {
            config_fingerprint(
                temp.path(),
                None,
                false,
                "tux-auto",
                false,
                false,
                temp.path(),
                temp.path(),
                temp.path(),
                None,
                None,
                None,
                listen.parse()?,
                None,
                &paths,
            )
        };
        assert_eq!(fingerprint("127.0.0.1:0")?, fingerprint("127.0.0.1:0")?);
        assert_ne!(fingerprint("127.0.0.1:0")?, fingerprint("0.0.0.0:8080")?);
        Ok(())
    }

    /// The advertised base is reported back from the registry on resume, so a
    /// launch that declares a different one (or drops it) must not resume a
    /// runtime recorded with the previous value: it joins the key beside the
    /// listen address.
    #[test]
    fn config_fingerprint_changes_with_http_public_base_url() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let paths = conventional_paths(temp.path());
        let fingerprint = |base: Option<&str>| -> anyhow::Result<String> {
            config_fingerprint(
                temp.path(),
                None,
                false,
                "tux-auto",
                false,
                false,
                temp.path(),
                temp.path(),
                temp.path(),
                None,
                None,
                None,
                meerkat_meta::gateway_composition::DEFAULT_GATEWAY_HTTP_LISTEN,
                base,
                &paths,
            )
        };
        assert_eq!(
            fingerprint(Some("https://mob.example.com"))?,
            fingerprint(Some("https://mob.example.com"))?
        );
        assert_ne!(
            fingerprint(Some("https://mob.example.com"))?,
            fingerprint(Some("https://other.example.com"))?
        );
        assert_ne!(
            fingerprint(Some("https://mob.example.com"))?,
            fingerprint(None)?
        );
        Ok(())
    }

    #[test]
    fn init_params_parse_console_read_only() -> anyhow::Result<()> {
        let line = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "meta/init",
            "params": {
                "console_read_only": true
            }
        })
        .to_string();

        let (_id, params) = parse_init_request(&line)?;

        assert_eq!(params.console_read_only, Some(true));
        Ok(())
    }

    #[test]
    fn runtime_decision_state_projects_console_read_only() {
        let state = runtime_decision_state(ConsoleUiConfig::default(), true);

        assert!(state.console.read_only);
    }

    #[test]
    fn runtime_decision_state_is_open_and_keeps_the_tux_session_store_naming() {
        let state = runtime_decision_state(ConsoleUiConfig::default(), false);

        assert!(!state.console.require_app_auth);
        assert_eq!(state.bigquery.dataset, "tux_local");
        assert_eq!(state.bigquery.table, "runtime_events");
        assert!(state.modules.is_empty());
    }

    #[test]
    fn init_params_parse_meerkat_config_path() -> anyhow::Result<()> {
        let line = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "meta/init",
            "params": { "meerkat_config_path": "/etc/example/config.toml" }
        })
        .to_string();

        let (_id, params) = parse_init_request(&line)?;

        assert_eq!(
            params.meerkat_config_path.as_deref(),
            Some(Path::new("/etc/example/config.toml"))
        );
        Ok(())
    }

    const HOST_CONFIG_FIXTURE: &str = "[self_hosted.servers.local]\n\
                                       transport = \"openai_compatible\"\n\
                                       base_url = \"http://127.0.0.1:11434\"\n\
                                       api_style = \"chat_completions\"\n\n\
                                       [self_hosted.models.gemma-4-31b]\n\
                                       server = \"local\"\n\
                                       remote_model = \"gemma4:31b\"\n";

    /// The host config is baked into every agent, so the resume key must
    /// change when its content changes or when it is dropped; otherwise a
    /// re-pointed config.toml resumes a runtime built from the old one (the
    /// dead-knob failure the compaction key already guards against).
    /// `.rkat/` is outside the scanned config roots, so this only holds
    /// because the path is pushed by name.
    #[test]
    fn config_fingerprint_changes_with_meerkat_host_config() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let rkat = temp.path().join(".rkat");
        fs::create_dir_all(&rkat)?;
        let config_toml = rkat.join("config.toml");
        fs::write(&config_toml, HOST_CONFIG_FIXTURE)?;
        let paths = conventional_paths(temp.path());
        assert_eq!(
            paths.meerkat_config_toml.as_deref(),
            Some(config_toml.as_path()),
            "the workspace .rkat/config.toml must be adopted by convention"
        );
        let fingerprint = |paths: &ConventionalPaths| {
            config_fingerprint(
                temp.path(),
                None,
                false,
                "tux-auto",
                false,
                false,
                temp.path(),
                temp.path(),
                temp.path(),
                None,
                None,
                None,
                meerkat_meta::gateway_composition::DEFAULT_GATEWAY_HTTP_LISTEN,
                None,
                paths,
            )
        };

        let first = fingerprint(&paths)?;
        fs::write(&config_toml, HOST_CONFIG_FIXTURE.replace("11434", "8000"))?;
        let second = fingerprint(&paths)?;
        assert_ne!(
            first, second,
            "a changed host config must not resume the previous runtime"
        );

        let mut without = paths;
        without.meerkat_config_toml = None;
        assert_ne!(
            fingerprint(&without)?,
            second,
            "dropping the host config must not resume a runtime built with it"
        );
        Ok(())
    }

    /// One producer for both launch arms: the host config's tables reach the
    /// factory config and the compaction declaration still pins on top.
    #[test]
    fn gateway_agent_config_layers_compaction_over_the_host_config() -> anyhow::Result<()> {
        let mut host = Config::default();
        host.merge_toml_str(HOST_CONFIG_FIXTURE)?;
        host.model_fallback.enabled = Some(false);
        host.model_fallback.policy.trigger_after_attempts = 7;
        let pinned = meerkat_meta::parse_compaction_policy(&json!({
            "auto_compact_threshold": 120_000
        }))
        .map_err(|e| anyhow!("{e}"))?;

        let config = gateway_agent_config(Some(&host), Some(&pinned))?;

        assert_eq!(
            config
                .self_hosted
                .models
                .get("gemma-4-31b")
                .map(|model| model.server.as_str()),
            Some("local")
        );
        assert_eq!(config.compaction.auto_compact_threshold, 120_000);
        assert!(config.compaction.auto_compact_threshold_explicit);
        assert_eq!(config.model_fallback, host.model_fallback);

        let defaulted = gateway_agent_config(None, None)?;
        assert!(defaulted.self_hosted.models.is_empty());
        assert!(!defaulted.model_fallback.is_enabled());
        Ok(())
    }

    /// `[self_hosted]` in the workspace mob.toml is refused at load with the
    /// typed message that names the file carrying host config, not dropped.
    #[test]
    fn load_definition_refuses_host_config_tables_in_mob_toml() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let config = temp.path().join("config");
        fs::create_dir_all(&config)?;
        fs::write(
            config.join("mob.toml"),
            "[mob]\nid = \"m\"\n\n[profiles.w]\nmodel = \"gpt-5.5\"\n\n\
             [self_hosted.servers.local]\nbase_url = \"http://127.0.0.1:11434\"\n",
        )?;
        let paths = conventional_paths(temp.path());

        let error = load_definition(temp.path(), "fingerprint", &paths)
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default();

        assert!(
            error.contains("[self_hosted]") && error.contains("meerkat_config_path"),
            "{error}"
        );
        Ok(())
    }

    /// A profile `system_prompt` in the workspace mob.toml is refused, and the
    /// init refusal carries meerkat's message and the upgrade note instead of
    /// only "failed to parse <path>".
    #[test]
    fn load_definition_refusal_carries_the_profile_prompt_upgrade_remedy() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let config = temp.path().join("config");
        fs::create_dir_all(&config)?;
        fs::write(
            config.join("mob.toml"),
            "[mob]\nid = \"m\"\n\n[profiles.planner]\nmodel = \"gpt-5.5\"\n\
             system_prompt = \"Plan.\"\n",
        )?;
        let paths = conventional_paths(temp.path());

        let error = load_definition(temp.path(), "fingerprint", &paths)
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default();

        assert!(
            error.contains("declares 'system_prompt', which a mob profile cannot honour")
                && error.contains("MobKit 0.8.31 and earlier dropped 'system_prompt'")
                && error.contains("so the key had no effect on the members of profile 'planner'")
                && error.contains("removing the key keeps that behaviour"),
            "{error}"
        );
        Ok(())
    }

    /// The provider credentials root is an absolute path, checked at parse:
    /// a relative, empty or non-string root refuses init.
    #[test]
    fn init_params_check_the_provider_credentials_root() -> anyhow::Result<()> {
        let parse = |root: serde_json::Value| {
            parse_init_request(
                &serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "meta/init",
                    "params": { "provider_credentials_root": root }
                })
                .to_string(),
            )
        };

        let (_id, params) = parse(serde_json::json!("/srv/meta/users/a/credentials"))?;
        assert_eq!(
            params.provider_credentials_root.as_deref(),
            Some(Path::new("/srv/meta/users/a/credentials"))
        );
        for refused in [
            serde_json::json!("relative/credentials"),
            serde_json::json!(""),
            serde_json::json!(7),
        ] {
            assert!(
                parse(refused.clone()).is_err(),
                "{refused} must refuse init"
            );
        }
        Ok(())
    }

    #[test]
    fn the_provider_credentials_root_separates_resume_keys() {
        let key = "launch-fingerprint".to_string();
        let a = Path::new("/srv/meta/users/a/credentials");
        let b = Path::new("/srv/meta/users/b/credentials");
        let keyed_a = resume_key_with_provider_credentials_root(key.clone(), Some(a));
        assert_eq!(
            (
                resume_key_with_provider_credentials_root(key.clone(), None),
                keyed_a == key,
                resume_key_with_provider_credentials_root(key.clone(), Some(a)),
                keyed_a == resume_key_with_provider_credentials_root(key.clone(), Some(b)),
            ),
            (key, false, keyed_a, false),
            "without a root the key is unchanged; each root gives its own stable key"
        );
    }

    #[test]
    fn the_flow_editor_host_deploy_opt_in_separates_resume_keys() {
        let key = "launch-fingerprint".to_string();
        let opted_in = resume_key_with_flow_editor_host_deploy(key.clone(), true);
        assert_eq!(
            (
                resume_key_with_flow_editor_host_deploy(key.clone(), false),
                opted_in == key,
                resume_key_with_flow_editor_host_deploy(key.clone(), true),
            ),
            (key, false, opted_in),
            "without the flag the key is unchanged; with it the key differs, stably"
        );
    }

    #[test]
    fn config_fingerprint_changes_with_console_read_only() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let paths = conventional_paths(temp.path());

        let writable = config_fingerprint(
            temp.path(),
            None,
            false,
            "tux-auto",
            false,
            false,
            temp.path(),
            temp.path(),
            temp.path(),
            None,
            None,
            None,
            meerkat_meta::gateway_composition::DEFAULT_GATEWAY_HTTP_LISTEN,
            None,
            &paths,
        )?;
        let read_only = config_fingerprint(
            temp.path(),
            None,
            false,
            "tux-auto",
            false,
            true,
            temp.path(),
            temp.path(),
            temp.path(),
            None,
            None,
            None,
            meerkat_meta::gateway_composition::DEFAULT_GATEWAY_HTTP_LISTEN,
            None,
            &paths,
        )?;

        assert_ne!(writable, read_only);
        Ok(())
    }

    /// A launch that changes the compaction policy must not resume a runtime
    /// built with the previous one: the policy is baked into every agent the
    /// runtime constructs, so reuse would silently keep the old trigger.
    #[test]
    fn config_fingerprint_changes_with_compaction_policy() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let paths = conventional_paths(temp.path());
        let pinned = meerkat_meta::parse_compaction_policy(&json!({
            "auto_compact_threshold": 120_000
        }))
        .map_err(|e| anyhow!("{e}"))?;
        let lower = meerkat_meta::parse_compaction_policy(&json!({
            "auto_compact_threshold": 60_000
        }))
        .map_err(|e| anyhow!("{e}"))?;

        let inherited = config_fingerprint(
            temp.path(),
            None,
            false,
            "tux-auto",
            false,
            false,
            temp.path(),
            temp.path(),
            temp.path(),
            None,
            None,
            None,
            meerkat_meta::gateway_composition::DEFAULT_GATEWAY_HTTP_LISTEN,
            None,
            &paths,
        )?;
        let pinned_key = config_fingerprint(
            temp.path(),
            None,
            false,
            "tux-auto",
            false,
            false,
            temp.path(),
            temp.path(),
            temp.path(),
            None,
            Some(&pinned),
            None,
            meerkat_meta::gateway_composition::DEFAULT_GATEWAY_HTTP_LISTEN,
            None,
            &paths,
        )?;
        let lower_key = config_fingerprint(
            temp.path(),
            None,
            false,
            "tux-auto",
            false,
            false,
            temp.path(),
            temp.path(),
            temp.path(),
            None,
            Some(&lower),
            None,
            meerkat_meta::gateway_composition::DEFAULT_GATEWAY_HTTP_LISTEN,
            None,
            &paths,
        )?;

        assert_ne!(
            inherited, pinned_key,
            "pinning a threshold must not resume an inheriting runtime"
        );
        assert_ne!(
            pinned_key, lower_key,
            "two different thresholds must not share a runtime"
        );
        let wider = meerkat_meta::parse_compaction_policy(&json!({
            "auto_compact_threshold": 120_000,
            "history_retained_rewrites": 128
        }))
        .map_err(|e| anyhow!("{e}"))?;
        let wider_key = config_fingerprint(
            temp.path(),
            None,
            false,
            "tux-auto",
            false,
            false,
            temp.path(),
            temp.path(),
            temp.path(),
            None,
            Some(&wider),
            None,
            meerkat_meta::gateway_composition::DEFAULT_GATEWAY_HTTP_LISTEN,
            None,
            &paths,
        )?;
        assert_ne!(
            pinned_key, wider_key,
            "two different transcript retention windows must not share a runtime"
        );
        Ok(())
    }

    /// The text report says "Nothing to migrate." for a directory the
    /// predicate reads as native, and only then.
    #[test]
    fn the_native_report_says_nothing_to_migrate_only_for_a_native_directory() -> anyhow::Result<()>
    {
        use meerkat_meta::storage_migrate_native::{
            NATIVE_MIGRATION_JOURNAL_FILE_NAME, NativeLayoutStatus, NativeMigrateRequest,
            migrate_state_dir_native,
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let census = |dir: &Path| {
            runtime.block_on(migrate_state_dir_native(&NativeMigrateRequest::new(
                dir,
                meerkat_meta::MigrateMode::DryRun,
            )))
        };

        let native = tempfile::tempdir()?;
        let report = census(native.path());
        assert!(
            matches!(report.layout, Some(NativeLayoutStatus::Native)),
            "{report:?}"
        );
        let text = native_migrate_report_text(&report);
        assert!(text.ends_with("Nothing to migrate.\n"), "{text}");

        let unfinished = tempfile::tempdir()?;
        std::fs::write(
            unfinished.path().join(NATIVE_MIGRATION_JOURNAL_FILE_NAME),
            b"{}",
        )?;
        let report = census(unfinished.path());
        let text = native_migrate_report_text(&report);
        assert!(!text.contains("Nothing to migrate."), "{text}");
        assert!(text.contains("MigrationIncomplete"), "{text}");
        Ok(())
    }

    /// The text report names the remedy under each mob-store refusal: the
    /// --mob-store a foreign store's manifest asks for, and what to do when
    /// the directory's own store's manifest is there without the store.
    #[test]
    fn the_native_report_names_the_mob_store_remedies() -> anyhow::Result<()> {
        use meerkat_meta::storage_migrate_native::{
            NativeMigrateRequest, migrate_state_dir_native,
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let parent = tempfile::tempdir()?;
        let state = parent.path().join("state");
        std::fs::create_dir(&state)?;
        for file in ["runtime.sqlite", "sessions.sqlite3"] {
            std::fs::write(state.join(file), b"")?;
        }
        let census = || {
            runtime.block_on(migrate_state_dir_native(&NativeMigrateRequest::new(
                &state,
                meerkat_meta::MigrateMode::DryRun,
            )))
        };

        std::fs::write(state.join("mob.sqlite3.composition.json"), b"{}")?;
        let text = native_migrate_report_text(&census());
        assert!(
            text.contains(&format!(
                "\n      remedy: pass --mob-store {}\n",
                state.join("mob.sqlite3").display()
            )),
            "{text}"
        );

        std::fs::remove_file(state.join("mob.sqlite3.composition.json"))?;
        std::fs::write(state.join("event_log.sqlite3.composition.json"), b"{}")?;
        let text = native_migrate_report_text(&census());
        assert!(text.contains("MobStoreManifestWithoutStore"), "{text}");
        assert!(
            text.contains(&format!(
                "\n      remedy: restore {}, or pass --mob-store <path> naming where that \
                 store is now\n",
                state.join("event_log.sqlite3").display()
            )),
            "{text}"
        );
        Ok(())
    }

    /// The text report names the mob storage the census read: in memory
    /// when the directory holds no mob store, else the store's path.
    #[test]
    fn the_native_report_names_the_mob_storage_it_read() -> anyhow::Result<()> {
        use meerkat_meta::storage_migrate_native::{
            NativeMigrateRequest, migrate_state_dir_native,
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let parent = tempfile::tempdir()?;
        let state = parent.path().join("state");
        std::fs::create_dir(&state)?;
        for file in ["runtime.sqlite", "sessions.sqlite3"] {
            std::fs::write(state.join(file), b"")?;
        }
        let census = || {
            runtime.block_on(migrate_state_dir_native(&NativeMigrateRequest::new(
                &state,
                meerkat_meta::MigrateMode::DryRun,
            )))
        };
        let text = native_migrate_report_text(&census());
        assert!(text.contains("\n  mob storage: in memory\n"), "{text}");

        let store = state.join("event_log.sqlite3");
        std::fs::write(&store, b"")?;
        let text = native_migrate_report_text(&census());
        assert!(
            text.contains(&format!("\n  mob storage: {}\n", store.display())),
            "{text}"
        );
        Ok(())
    }
}
