//! `rkat mob host` — the mob member-host daemon (multi-host mobs D6,
//! §7.2 steps 1 & 3, §21.3).
//!
//! Composition order (the rkat-rpc main template with the R2 corrections):
//! realm (workspace-derived; `--isolated` typed-rejects) → realm lease →
//! persistence + chain-composed config (`[mob_host]`, flags override file) →
//! factory + runtime-backed session service + `MeerkatMachine` → host
//! identity (0700 dir / 0600 key) → host comms runtime (no listener of its
//! own) → host acceptor (TcpBindPolicy first) → `MobHostActor`
//! (recover-or-create, token mint, descriptor publish, responder drain) →
//! schedule host (real session-target delivery + `NoopScheduleMobHost`) →
//! optional live plane (DEC-P2-8: inert-but-honest until 6b) → run until
//! ctrl-c → shutdown in reverse.

use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::watch;

use meerkat::surface::spawn_runtime_backed_schedule_host;
use meerkat::{AgentFactory, ScheduleService, ScheduleToolDispatcher};
use meerkat::{FactoryAgentBuilder, PersistentSessionService};
use meerkat_core::config::MobHostConfig;
use meerkat_core::connection::{
    CredentialSourceSpec, resolve_auth_binding_candidates_for_provider,
};
use meerkat_core::service::SessionBuildOptions;
use meerkat_core::{Config, Provider, RealmId};
use meerkat_core::{RealmConfig, RealmSelection};
use meerkat_mob::runtime::MobSessionService;
use meerkat_mob::runtime::host_actor::{
    HostCapabilityFacts, HostDescriptorSink, MobHostActorConfig, ProviderPresenceProbe,
    ProviderPresenceProbeError, RuntimeStoreHostBindingPersistence, build_host_comms_runtime,
    spawn_mob_host_actor,
};
use meerkat_mob::runtime::host_materialize::{HostMemberSubstrate, MaterializePreflightProbe};
use meerkat_mob::runtime::host_observation::HostMemberObservation;
use meerkat_rpc::secure_rpc::{TcpBindPolicy, validate_tcp_bind_policy};
use meerkat_store::{RealmBackend, RealmOrigin};

use crate::RuntimeScope;

/// CLI arguments for `rkat mob host` (flags override `[mob_host]` file
/// values per the layered config doctrine, DEC-P2-11).
#[derive(Clone, Default)]
pub(crate) struct MobHostArgs {
    pub listen_tcp: Option<String>,
    pub advertise_tcp: Option<String>,
    pub live_ws: Option<String>,
    pub live_ws_advertise: Option<String>,
    pub identity_dir: Option<PathBuf>,
    pub descriptor_out: Option<PathBuf>,
    pub allow_remote: bool,
    pub pairing_password: Option<String>,
    pub pairing_password_env: Option<String>,
    pub pairing_password_file: Option<PathBuf>,
}

impl std::fmt::Debug for MobHostArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MobHostArgs")
            .field("listen_tcp", &self.listen_tcp)
            .field("advertise_tcp", &self.advertise_tcp)
            .field("live_ws", &self.live_ws)
            .field("live_ws_advertise", &self.live_ws_advertise)
            .field("identity_dir", &self.identity_dir)
            .field("descriptor_out", &self.descriptor_out)
            .field("allow_remote", &self.allow_remote)
            .field(
                "pairing_password",
                &self.pairing_password.as_ref().map(|_| "[REDACTED]"),
            )
            .field("pairing_password_env", &self.pairing_password_env)
            .field("pairing_password_file", &self.pairing_password_file)
            .finish()
    }
}

/// Typed startup refusals for the daemon — every one fires BEFORE any side
/// effect on the realm.
#[derive(Debug, thiserror::Error)]
pub(crate) enum MobHostStartupError {
    #[error(
        "rkat mob host cannot run under an isolated realm: the daemon's bind facts and \
         host identity must survive restarts (R2); pass --realm or run inside a workspace"
    )]
    IsolatedRealmUnsupported,
    #[error(
        "--live-ws requires --live-ws-advertise (or [mob_host].live_ws_advertise): the \
             advertised live URL is a bind-ceremony fact and is never derived from the local \
             bind address on a multi-host daemon (DL6)"
    )]
    LiveWsAdvertiseRequired,
    #[error("invalid live-ws advertise URL '{url}': must be an absolute ws:// or wss:// base URL")]
    LiveWsAdvertiseInvalid { url: String },
    #[error("invalid [mob_host].{field}: value must be greater than zero")]
    ResourceBoundZero { field: &'static str },
    #[error(
        "use only one of --pairing-password, --pairing-password-env, or --pairing-password-file"
    )]
    PairingPasswordSourceConflict,
    #[error("failed to read environment variable '{name}' for mob host pairing password: {detail}")]
    PairingPasswordEnv { name: String, detail: String },
    #[error("failed to read mob host pairing password file '{}': {detail}", path.display())]
    PairingPasswordFile { path: PathBuf, detail: String },
    #[error("invalid mob host pairing password: {detail}")]
    PairingPasswordInvalid { detail: String },
    #[error(
        "mob host pairing may listen only on a loopback address; use the 0600 descriptor out-of-band or tunnel a loopback listener (got {addr})"
    )]
    PairingRequiresLoopback { addr: std::net::SocketAddr },
    #[cfg(not(feature = "openai-realtime"))]
    #[error(
        "a live transport is configured but this rkat build does not include the \
         openai-realtime feature"
    )]
    LiveTransportUnavailable,
    #[error(transparent)]
    TcpBindPolicy(#[from] meerkat_rpc::secure_rpc::TcpBindPolicyError),
    #[error("mob host credential backend unavailable: {detail}")]
    CredentialBackend { detail: String },
}

/// Owns a spawned startup side effect until normal shutdown. Any later `?`
/// aborts the task before returning, so a failed host transaction cannot
/// detach a listener and keep its port bound inside the calling runtime.
#[cfg(feature = "openai-realtime")]
struct AbortOnDropTask<T> {
    handle: Option<tokio::task::JoinHandle<T>>,
}

#[cfg(feature = "openai-realtime")]
impl<T> AbortOnDropTask<T> {
    fn new(handle: tokio::task::JoinHandle<T>) -> Self {
        Self {
            handle: Some(handle),
        }
    }

    async fn abort_and_join(mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
            let _ = handle.await;
        }
    }
}

#[cfg(feature = "openai-realtime")]
impl<T> Drop for AbortOnDropTask<T> {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}

/// Effective daemon composition after flag-over-file resolution.
#[derive(Clone, PartialEq)]
pub(crate) struct ResolvedMobHostComposition {
    pub listen_tcp: String,
    pub advertise_tcp: Option<String>,
    pub live_ws: Option<String>,
    pub live_ws_advertise: Option<String>,
    pub identity_dir: PathBuf,
    pub descriptor_out: PathBuf,
    pub max_connections: Option<usize>,
    pub read_deadline: Option<Duration>,
    pub pairing_rate: Option<u32>,
    pub live_buffer_events: Option<NonZeroUsize>,
    pub pairing_password: Option<String>,
}

impl std::fmt::Debug for ResolvedMobHostComposition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedMobHostComposition")
            .field("listen_tcp", &self.listen_tcp)
            .field("advertise_tcp", &self.advertise_tcp)
            .field("live_ws", &self.live_ws)
            .field("live_ws_advertise", &self.live_ws_advertise)
            .field("identity_dir", &self.identity_dir)
            .field("descriptor_out", &self.descriptor_out)
            .field("max_connections", &self.max_connections)
            .field("read_deadline", &self.read_deadline)
            .field("pairing_rate", &self.pairing_rate)
            .field("live_buffer_events", &self.live_buffer_events)
            .field(
                "pairing_password",
                &self.pairing_password.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

/// Flags override file values; absent both, the documented defaults apply.
pub(crate) fn resolve_mob_host_composition(
    args: &MobHostArgs,
    file: &MobHostConfig,
    state_root: &std::path::Path,
) -> Result<ResolvedMobHostComposition, MobHostStartupError> {
    let listen_tcp = args
        .listen_tcp
        .clone()
        .or_else(|| file.listen_tcp.clone())
        .unwrap_or_else(|| "127.0.0.1:0".to_string());
    let advertise_tcp = args
        .advertise_tcp
        .clone()
        .or_else(|| file.advertise_tcp.clone());
    let live_ws = args.live_ws.clone().or_else(|| file.live_ws.clone());
    let mut live_ws_advertise = args
        .live_ws_advertise
        .clone()
        .or_else(|| file.live_ws_advertise.clone());
    if live_ws.is_some() {
        let Some(url) = live_ws_advertise.as_deref() else {
            return Err(MobHostStartupError::LiveWsAdvertiseRequired);
        };
        let parsed =
            url::Url::parse(url).map_err(|_| MobHostStartupError::LiveWsAdvertiseInvalid {
                url: url.to_string(),
            })?;
        if !matches!(parsed.scheme(), "ws" | "wss")
            || parsed.cannot_be_a_base()
            || parsed.host_str().is_none_or(|host| host.is_empty())
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(MobHostStartupError::LiveWsAdvertiseInvalid {
                url: url.to_string(),
            });
        }
        let canonical_live_ws_advertise = parsed.as_str().trim_end_matches('/').to_string();
        live_ws_advertise = Some(canonical_live_ws_advertise);
    }
    if file.max_connections == Some(0) {
        return Err(MobHostStartupError::ResourceBoundZero {
            field: "max_connections",
        });
    }
    if file.read_deadline_ms == Some(0) {
        return Err(MobHostStartupError::ResourceBoundZero {
            field: "read_deadline_ms",
        });
    }
    let live_buffer_events = file
        .live_buffer_events
        .map(|capacity| {
            NonZeroUsize::new(capacity).ok_or(MobHostStartupError::ResourceBoundZero {
                field: "live_buffer_events",
            })
        })
        .transpose()?;
    let pairing_password = resolve_mob_host_pairing_password(args, file, |name| {
        let value = std::env::var_os(name).ok_or_else(|| "variable is not set".to_string())?;
        value
            .into_string()
            .map_err(|_| "value is not valid Unicode".to_string())
    })?;
    Ok(ResolvedMobHostComposition {
        listen_tcp,
        advertise_tcp,
        live_ws,
        live_ws_advertise,
        identity_dir: args
            .identity_dir
            .clone()
            .or_else(|| file.identity_dir.clone())
            .unwrap_or_else(|| state_root.join("host-identity")),
        descriptor_out: args
            .descriptor_out
            .clone()
            .or_else(|| file.descriptor_out.clone())
            .unwrap_or_else(|| PathBuf::from("host-binding.json")),
        max_connections: file.max_connections,
        read_deadline: file.read_deadline_ms.map(Duration::from_millis),
        pairing_rate: file.pairing_rate,
        live_buffer_events,
        pairing_password,
    })
}

/// Resolve the runtime-only host pairing secret without ever rendering its
/// value into diagnostics. Explicit CLI sources override the programmatic
/// config slot; serialized realm config cannot populate that slot because it
/// remains `#[serde(skip)]`.
fn resolve_mob_host_pairing_password(
    args: &MobHostArgs,
    file: &MobHostConfig,
    env_lookup: impl FnOnce(&str) -> Result<String, String>,
) -> Result<Option<String>, MobHostStartupError> {
    let supplied = usize::from(args.pairing_password.is_some())
        + usize::from(args.pairing_password_env.is_some())
        + usize::from(args.pairing_password_file.is_some());
    if supplied > 1 {
        return Err(MobHostStartupError::PairingPasswordSourceConflict);
    }
    let password = if let Some(password) = args.pairing_password.clone() {
        Some(password)
    } else if let Some(name) = args.pairing_password_env.as_deref() {
        Some(
            env_lookup(name).map_err(|detail| MobHostStartupError::PairingPasswordEnv {
                name: name.to_string(),
                detail,
            })?,
        )
    } else if let Some(path) = args.pairing_password_file.as_ref() {
        Some(
            std::fs::read_to_string(path)
                .map(|value| value.trim_end_matches(['\r', '\n']).to_string())
                .map_err(|error| MobHostStartupError::PairingPasswordFile {
                    path: path.clone(),
                    detail: error.to_string(),
                })?,
        )
    } else {
        file.pairing_password.clone()
    };
    if let Some(password) = password.as_deref() {
        meerkat_comms::validate_pairing_secret(password).map_err(|error| {
            MobHostStartupError::PairingPasswordInvalid {
                detail: error.to_string(),
            }
        })?;
    }
    Ok(password)
}

fn ensure_pairing_listener_is_loopback(
    listen_addr: std::net::SocketAddr,
    pairing_enabled: bool,
) -> Result<(), MobHostStartupError> {
    if pairing_enabled && !listen_addr.ip().is_loopback() {
        return Err(MobHostStartupError::PairingRequiresLoopback { addr: listen_addr });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tier-1 provider presence probe (R7, gotcha 13 — presence only, no network)
// ---------------------------------------------------------------------------

/// Presence-level provider resolvability probe over the daemon's
/// chain-composed effective config + the factory's token store.
///
/// Lives at the composition locus (the CLI names both `AgentFactory` and the
/// mob-side probe trait; the facade cannot name meerkat-mob types — the
/// dependency edge runs mob→facade).
pub(crate) struct FactoryProviderPresenceProbe {
    config: Config,
    realm: Option<RealmId>,
    token_store: Option<Arc<dyn meerkat_core::auth::TokenStore>>,
}

impl FactoryProviderPresenceProbe {
    /// Build the probe. A faulted token store is a typed startup error —
    /// provider presence with a broken credential backend fails closed.
    pub(crate) fn new(
        factory: &AgentFactory,
        config: Config,
        realm: Option<RealmId>,
    ) -> Result<Self, MobHostStartupError> {
        let token_store = factory.resolution_token_store().map_err(|err| {
            MobHostStartupError::CredentialBackend {
                detail: err.to_string(),
            }
        })?;
        Ok(Self {
            config,
            realm,
            token_store,
        })
    }
}

#[async_trait]
impl ProviderPresenceProbe for FactoryProviderPresenceProbe {
    async fn resolvable_providers(&self) -> Result<Vec<Provider>, ProviderPresenceProbeError> {
        probe_resolvable_providers_with_lookup(
            &self.config,
            self.realm.as_ref(),
            self.token_store.as_ref(),
            &|key| std::env::var(key).ok(),
        )
        .await
    }
}

#[async_trait]
impl MaterializePreflightProbe for FactoryProviderPresenceProbe {
    async fn binding_resolvable(
        &self,
        binding: Option<&meerkat_core::AuthBindingRef>,
        provider: Provider,
    ) -> Result<bool, ProviderPresenceProbeError> {
        probe_binding_presence_with_lookup(
            &self.config,
            provider,
            binding,
            self.realm.as_ref(),
            self.token_store.as_ref(),
            &|key| std::env::var(key).ok(),
        )
        .await
    }
}

/// Tier-2 per-binding presence walk (the W2.5 recipe): `None` binding walks
/// the provider's default chain (env-default included); `Some(ref)` walks
/// the NAMED realm chain strictly. Presence-level reads only — zero network,
/// zero OAuth (gotcha 13). A binding that names no candidate chain is typed
/// absence (`Ok(false)`), never a probe fault.
async fn probe_binding_presence_with_lookup(
    config: &Config,
    provider: Provider,
    binding: Option<&meerkat_core::AuthBindingRef>,
    realm: Option<&RealmId>,
    token_store: Option<&Arc<dyn meerkat_core::auth::TokenStore>>,
    lookup: &(dyn Fn(&str) -> Option<String> + Sync),
) -> Result<bool, ProviderPresenceProbeError> {
    let Ok(candidates) =
        resolve_auth_binding_candidates_for_provider(config, provider, binding, realm, true)
    else {
        return Ok(false);
    };
    for target in candidates {
        match &target.auth_profile.source {
            CredentialSourceSpec::InlineSecret { .. } => return Ok(true),
            CredentialSourceSpec::Env { env, fallback } => {
                if env_secret_present(lookup, env, fallback) {
                    return Ok(true);
                }
            }
            CredentialSourceSpec::ManagedStore => {
                if let Some(store) = token_store {
                    let key = meerkat_core::auth::TokenKey::from_auth_binding(&target.auth_binding);
                    match store.load(&key).await {
                        Ok(Some(_)) => return Ok(true),
                        Ok(None) => {}
                        Err(err) => {
                            return Err(ProviderPresenceProbeError::CredentialBackend {
                                detail: err.to_string(),
                            });
                        }
                    }
                }
            }
            // External resolvers, credential commands, and platform defaults
            // require invocation — a presence probe declines them.
            _ => {}
        }
    }
    Ok(false)
}

/// One env var (primary + ordered fallbacks) is "present" when either its
/// `RKAT_`-prefixed override or the var itself is set — mirroring
/// `resolve_simple_secret`'s canonical precedence, presence-only.
fn env_secret_present(
    lookup: &(dyn Fn(&str) -> Option<String> + Sync),
    var: &str,
    fallback: &[String],
) -> bool {
    std::iter::once(var)
        .chain(fallback.iter().map(String::as_str))
        .any(|candidate| {
            let rkat_override = if candidate.starts_with("RKAT_") {
                None
            } else {
                lookup(&format!("RKAT_{candidate}"))
            };
            rkat_override.or_else(|| lookup(candidate)).is_some()
        })
}

/// Tier-1 presence probe: pure config candidate walk
/// (`resolve_auth_binding_candidates_for_provider`, no I/O) + presence-level
/// reads only (env key presence; token-store row presence). Explicitly NOT
/// called: `ProviderRuntimeRegistry::resolve`, OAuth refresh, any provider
/// endpoint, external resolvers, credential commands, cloud metadata.
async fn probe_resolvable_providers_with_lookup(
    config: &Config,
    realm: Option<&RealmId>,
    token_store: Option<&Arc<dyn meerkat_core::auth::TokenStore>>,
    lookup: &(dyn Fn(&str) -> Option<String> + Sync),
) -> Result<Vec<Provider>, ProviderPresenceProbeError> {
    let mut resolvable = Vec::new();
    for provider in Provider::ALL_CONCRETE {
        let Ok(candidates) =
            resolve_auth_binding_candidates_for_provider(config, *provider, None, realm, true)
        else {
            // No candidate chain for this provider — typed absence, not a
            // probe fault.
            continue;
        };
        let mut present = false;
        for target in candidates {
            match &target.auth_profile.source {
                CredentialSourceSpec::InlineSecret { .. } => {
                    present = true;
                }
                CredentialSourceSpec::Env { env, fallback } => {
                    if env_secret_present(lookup, env, fallback) {
                        present = true;
                    }
                }
                CredentialSourceSpec::ManagedStore => {
                    if let Some(store) = token_store {
                        let key =
                            meerkat_core::auth::TokenKey::from_auth_binding(&target.auth_binding);
                        match store.load(&key).await {
                            Ok(Some(_)) => present = true,
                            Ok(None) => {}
                            Err(err) => {
                                return Err(ProviderPresenceProbeError::CredentialBackend {
                                    detail: err.to_string(),
                                });
                            }
                        }
                    }
                }
                // External resolvers, credential commands, and platform
                // defaults require invocation — a tier-1 presence probe
                // declines them (gotcha 13).
                _ => {}
            }
            if present {
                break;
            }
        }
        if present {
            resolvable.push(*provider);
        }
    }
    Ok(resolvable)
}

// ---------------------------------------------------------------------------
// Descriptor file sink (§7.2 step 1, §20.3 — 0600, typed contract)
// ---------------------------------------------------------------------------

pub(crate) struct DescriptorFileSink {
    path: PathBuf,
}

impl DescriptorFileSink {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl HostDescriptorSink for DescriptorFileSink {
    fn publish(&self, descriptor_json: &str) -> Result<(), String> {
        write_descriptor_0600(&self.path, descriptor_json).map_err(|err| {
            format!(
                "failed to write host binding descriptor to {}: {err}",
                self.path.display()
            )
        })
    }
}

/// Write the descriptor with owner-only permissions. Unlike the member
/// `--comms-binding-out` precedent this is 0600 and carries the TYPED
/// `WireHostBindingDescriptor` (serialized by the actor), never ad-hoc JSON.
fn write_descriptor_0600(path: &std::path::Path, descriptor_json: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    if parent != std::path::Path::new(".") {
        std::fs::create_dir_all(parent)?;
    }
    let file_name = path.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "descriptor path has no file name",
        )
    })?;
    let temp_path = parent.join(format!(
        ".{}.{}.tmp",
        file_name.to_string_lossy(),
        uuid::Uuid::new_v4().simple()
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&temp_path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(descriptor_json.as_bytes())?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp_path, path)?;
        // The rename is the visibility commit. Directory sync is best-effort:
        // after commit the sink must return Ok rather than report an error
        // that falsely promises the new descriptor was never exposed.
        if let Ok(directory) = std::fs::File::open(parent) {
            let _ = directory.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

// ---------------------------------------------------------------------------
// The daemon
// ---------------------------------------------------------------------------

/// R2: the daemon's realm must be durable/rediscoverable. `--isolated`
/// resolves to a generated throwaway realm — typed reject BEFORE any side
/// effect (first statement of [`run_mob_host`]).
fn ensure_durable_realm(origin_hint: RealmOrigin) -> Result<(), MobHostStartupError> {
    if origin_hint == RealmOrigin::Generated {
        return Err(MobHostStartupError::IsolatedRealmUnsupported);
    }
    Ok(())
}

pub(crate) async fn run_mob_host(args: MobHostArgs, scope: &RuntimeScope) -> anyhow::Result<()> {
    ensure_durable_realm(scope.origin_hint)?;

    // 2. Chain-composed effective config; honor `[mob_host].realm` when no
    //    explicit realm was selected (flags > file > workspace derivation).
    let (config, realm_root) = crate::load_config(scope).await?;
    let mut scope = scope.clone();
    let mut config = config;
    let mut realm_root = realm_root;
    if let Some(file_realm) = config.mob_host.realm.clone()
        && scope.origin_hint == RealmOrigin::Workspace
        && file_realm != scope.locator.realm.as_str()
    {
        let realm_cfg = RealmConfig {
            selection: RealmSelection::Explicit {
                realm_id: file_realm.clone(),
            },
            instance_id: scope.instance_id.clone(),
            backend_hint: None,
            state_root: Some(scope.locator.state_root.clone()),
        };
        scope.locator = realm_cfg.resolve_locator()?;
        scope.origin_hint = RealmOrigin::Explicit;
        let (reloaded_config, reloaded_root) = crate::load_config(&scope).await?;
        config = reloaded_config;
        realm_root = reloaded_root;
    }
    let realm_id = scope.locator.realm.clone();
    let composition =
        resolve_mob_host_composition(&args, &config.mob_host, &scope.locator.state_root)?;

    // 3. DL7: TcpBindPolicy on BOTH binds BEFORE anything opens a socket.
    let policy = if args.allow_remote {
        TcpBindPolicy::allow_remote()
    } else {
        TcpBindPolicy::local_only()
    };
    validate_tcp_bind_policy("mob-host", &composition.listen_tcp, policy)
        .map_err(MobHostStartupError::TcpBindPolicy)?;
    let listen_addr: std::net::SocketAddr = composition.listen_tcp.parse().map_err(|err| {
        anyhow::anyhow!(
            "invalid mob host listen_tcp '{}': {err}",
            composition.listen_tcp
        )
    })?;
    ensure_pairing_listener_is_loopback(listen_addr, composition.pairing_password.is_some())?;
    if let Some(live_ws) = composition.live_ws.as_deref() {
        validate_tcp_bind_policy("live-ws", live_ws, policy)
            .map_err(MobHostStartupError::TcpBindPolicy)?;
    }
    // Feature preflight: a configured live transport without the realtime
    // feature is a typed startup refusal, never a silently dead flag.
    #[cfg(not(feature = "openai-realtime"))]
    if composition.live_ws.is_some() {
        return Err(MobHostStartupError::LiveTransportUnavailable.into());
    }

    // 4. Realm lease: blocks destructive prune while the daemon lives.
    let lease = meerkat_store::start_realm_lease_in(
        &scope.locator.state_root,
        realm_id.as_str(),
        scope.instance_id.as_deref(),
        "rkat-mob-host",
    )
    .await?;

    // 5. Persistence + factory + runtime-backed session service + machine.
    let (manifest, persistence) = crate::create_persistence_bundle(&scope).await?;
    let durable_sessions = matches!(manifest.backend, RealmBackend::Sqlite);
    let store_path = crate::realm_store_path(&manifest, &scope);
    let runtime_store = persistence.runtime_store();
    let session_store = persistence.session_store();
    let schedule_service = ScheduleService::new(persistence.schedule_store());

    let project_root = scope
        .context_root
        .clone()
        .unwrap_or_else(|| realm_root.clone());
    let mut factory = AgentFactory::new(store_path)
        .session_store(session_store)
        .runtime_root(realm_root.clone())
        .project_root(project_root)
        .builtins(true)
        .shell(true)
        .schedule(config.tools.schedule_enabled)
        .memory(true);
    if let Some(context_root) = scope.context_root.clone() {
        factory = factory.context_root(context_root);
    }
    if let Some(user_root) = scope.user_config_root.clone() {
        factory = factory.user_config_root(user_root);
    }

    // Tier-1 probe material resolves BEFORE the factory moves into the
    // service builder (fail closed on a faulted credential backend).
    let probe = Arc::new(FactoryProviderPresenceProbe::new(
        &factory,
        config.clone(),
        Some(realm_id.clone()),
    )?);

    // Phase 6b (DEC-P6B-L9/L14): per-open realtime credential resolution
    // over the MEMBER HOST'S OWN realm chain (§16.5). The daemon exposes no
    // `config/set` surface, so the filesystem chain IS its entire config
    // truth: a `MemoryConfigStore` over the chain-loaded head config plus a
    // `FilesystemRealmConfigSource` over the state root re-read the chain
    // per open ("visible to the very next live/open" for file edits). Built
    // BEFORE `AgentFactory` moves into the service builder (the `probe`
    // pre-move pattern above).
    #[cfg(feature = "openai-realtime")]
    let live_session_factory: Option<
        Arc<dyn meerkat_client::realtime_session::RealtimeSessionFactory>,
    > = if composition.live_ws.is_some() {
        let live_config_store: Arc<dyn meerkat_core::ConfigStore> = Arc::new(
            meerkat_core::MemoryConfigStore::new(config.clone(), meerkat_models::canonical()),
        );
        let global_doc = Config::global_config_path()
            .unwrap_or_else(|| scope.locator.state_root.join("global").join("config.toml"));
        let realm_config_source: Arc<dyn meerkat_core::RealmConfigSource> =
            Arc::new(meerkat_store::FilesystemRealmConfigSource::new(
                scope.locator.state_root.clone(),
                global_doc,
                meerkat_models::canonical(),
            ));
        Some(
            meerkat::session_runtime::realtime_credentials::build_per_open_realtime_session_factory(
                &factory,
                live_config_store,
                realm_config_source,
                realm_id.clone(),
            ),
        )
    } else {
        None
    };

    let default_schedule_tools = Some(Arc::new(ScheduleToolDispatcher::new(
        schedule_service.clone(),
    )) as Arc<dyn meerkat_core::AgentToolDispatcher>);
    // Workgraph is a CONTROLLING-host surface: remote members are
    // admission-denied for workgraph tools (plan A3; the phase-3 placement
    // gate), so the member-host daemon mounts no workgraph service and no
    // workgraph tool dispatcher.
    let (service, runtime_adapter) = crate::build_cli_runtime_backed_service_with_defaults(
        factory,
        config.clone(),
        persistence,
        default_schedule_tools,
        None,
    );
    let service = Arc::new(service);

    // 6. Schedule host (§18.5): realm-local stores, REAL session-target
    //    delivery, NoopScheduleMobHost, no runnable registry — a mob-target
    //    schedule on this daemon fails typed at fire. No workgraph service:
    //    attention overlays are controlling-host facts (plan A3).
    let schedule_host = spawn_runtime_backed_schedule_host(
        Arc::clone(&service),
        Arc::clone(&runtime_adapter),
        config.clone(),
        schedule_service,
        SessionBuildOptions::default(),
        None,
        None,
        format!("rkat-mob-host:{}", realm_id.as_str()),
    );

    // 7. Live plane (DEC-P2-8, served since 6b): four facade roles over the
    //    daemon's session service; the live bridge responder arms serve the
    //    listener through the machine-injected `ServiceMemberLiveHost`, and
    //    the advertised URL is a real bind-ceremony fact.
    #[cfg(feature = "openai-realtime")]
    let live = if let Some(live_ws_addr) = composition.live_ws.as_deref() {
        let listener = tokio::net::TcpListener::bind(live_ws_addr).await?;
        let local_addr = listener.local_addr()?;
        let projection = Arc::new(meerkat::surface::ServiceLiveProjection::new(
            Arc::clone(&service),
            Arc::clone(&runtime_adapter),
        ));
        let projection_sink: Arc<dyn meerkat_live::LiveProjectionSink> = projection.clone();
        let close_feedback: Arc<dyn meerkat_live::LiveChannelCloseFeedback> = projection.clone();
        let status_feedback: Arc<dyn meerkat_live::LiveChannelStatusFeedback> = projection.clone();
        let token_authority: Arc<dyn meerkat_live::LiveWsTokenAuthority> = projection;
        let host = Arc::new(
            meerkat_live::LiveAdapterHost::new(projection_sink)
                .with_tool_timeout(meerkat_live::DEFAULT_LIVE_TOOL_TIMEOUT),
        );
        // DEC-P6B-L11 (§16.2 tool parity): live tool calls raised mid-turn
        // on a member-host channel dispatch into the owning session's
        // dispatcher through the canonical service seam.
        host.set_live_tool_dispatcher(Arc::new(meerkat::surface::ServiceLiveToolDispatcher::new(
            Arc::clone(&service),
        )));
        let ws_state = Arc::new(meerkat_live::LiveWsState::new(
            Arc::clone(&host),
            close_feedback,
            status_feedback,
            token_authority,
        ));
        let ws_state_task = Arc::clone(&ws_state);
        let handle = tokio::spawn(async move {
            meerkat_live::serve_live_ws_listener(listener, ws_state_task).await
        });
        // Phase 6b (ADJ-P6B-16): the live bridge responder arms serve this
        // listener through the ONE extracted pipeline. Install the
        // machine-injected member live host — bootstrap URLs mint against
        // the operator-declared advertise URL (DL6), tokens against the
        // owning session's machine, credentials per open over the daemon's
        // own realm chain. Not composed ⇒ not installed: the arms reply
        // typed `LiveTransportUnavailable` (the fail-closed backstop behind
        // the controlling host's `host_live_endpoints` gate).
        match (
            live_session_factory.as_ref(),
            composition.live_ws_advertise.as_deref(),
        ) {
            (Some(session_factory), Some(advertise)) => {
                runtime_adapter.set_member_live_host(Arc::new(
                    meerkat::surface::ServiceMemberLiveHost::new(
                        meerkat::surface::ServiceMemberLiveHostConfig {
                            service: Arc::clone(&service),
                            runtime_adapter: Arc::clone(&runtime_adapter),
                            host: Arc::clone(&host),
                            ws_state: Some(Arc::clone(&ws_state)),
                            base_url: Some(advertise.to_string()),
                            session_factory: Arc::clone(session_factory),
                            realm_id: Some(realm_id.clone()),
                            instance_id: scope.instance_id.clone(),
                            backend: Some(manifest.backend.as_str().to_string()),
                        },
                    ),
                ));
            }
            _ => {
                // `--live-ws` requires `--live-ws-advertise` and composes
                // the per-open factory above; reaching here means the live
                // plane is only partially composed — leave the slot empty
                // so the bridge arms degrade typed instead of minting
                // unreachable bootstraps.
                eprintln!(
                    "rkat mob host live-ws: live plane partially composed; \
                     member live bridge arms will reply live_transport_unavailable"
                );
            }
        }
        eprintln!(
            "rkat mob host live-ws listening on ws://{local_addr}{} (advertised: {})",
            meerkat_live::LIVE_WS_PATH,
            composition.live_ws_advertise.as_deref().unwrap_or_default(),
        );
        Some(AbortOnDropTask::new(handle))
    } else {
        None
    };
    let live_endpoint = composition
        .live_ws
        .is_some()
        .then(|| composition.live_ws_advertise.clone())
        .flatten();

    // 8. Host identity (§20.3): 0700 dir BEFORE key material lands (the key
    //    file itself is written 0600 + zeroized by `Keypair::save`).
    tokio::fs::create_dir_all(&composition.identity_dir).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        tokio::fs::set_permissions(
            &composition.identity_dir,
            std::fs::Permissions::from_mode(0o700),
        )
        .await?;
    }
    let host_keypair =
        Arc::new(meerkat_comms::Keypair::load_or_generate(&composition.identity_dir).await?);

    // 9. Host comms runtime: host identity, machine-gated classification +
    //    peer request/response authority, NO listener of its own — ingress
    //    arrives through the acceptor demux.
    let participant_name = format!("rkat-mob-host/{}", realm_id.as_str());
    let host_comms = build_host_comms_runtime(
        &participant_name,
        meerkat_comms::Keypair::from_secret(host_keypair.secret_bytes()),
    )?;

    // 10. Host acceptor (D1): identity registry + pairing watch slot; the
    //     §21.2 pre-auth bounds come from `[mob_host]`.
    let registry = Arc::new(meerkat_comms::HostAcceptorIdentityRegistry::new());
    let (descriptor_watch_tx, descriptor_watch_rx) = watch::channel(String::new());
    let mut bounds = meerkat_comms::HostAcceptorBounds::default();
    if let Some(max_connections) = composition.max_connections {
        bounds.max_connections = max_connections;
    }
    if let Some(read_deadline) = composition.read_deadline {
        bounds.read_deadline = read_deadline;
    }
    if let Some(per_minute) = composition.pairing_rate {
        bounds.pairing_rate = meerkat_comms::PairingRateLimit {
            max_attempts: per_minute,
            window: Duration::from_secs(60),
        };
    }
    let pairing =
        composition
            .pairing_password
            .clone()
            .map(|password| meerkat_comms::HostPairingConfig {
                password,
                host_keypair: Arc::clone(&host_keypair),
                participant_name: participant_name.clone(),
                descriptor_json: descriptor_watch_rx,
            });
    let acceptor = meerkat_comms::spawn_host_acceptor(meerkat_comms::HostAcceptorConfig {
        listen_tcp: listen_addr,
        advertise_address: composition.advertise_tcp.clone(),
        registry: Arc::clone(&registry),
        pairing,
        bounds,
    })
    .await?;
    let advertised_address = acceptor.advertised_address().to_string();

    // 11. The actor: recover-or-create authority state from the R8 rows,
    //     register the host identity on the demux, mint the one-time token,
    //     publish the 0600 descriptor, revive recorded members from their
    //     stored specs (A20 — zero bridge traffic), start the bridge
    //     responder. The member substrate (DEC-P3H-2) injects the SAME
    //     runtime-backed session service + MeerkatMachine the daemon
    //     composed above; `MaterializeMember`/`ReleaseMember` build against
    //     it through the one `build_agent_config` compiler.
    let member_identity_root =
        meerkat::canonical_session_comms_identity_root(scope.user_config_root.as_deref())
            .map_err(|detail| anyhow::anyhow!("mob host member identity root: {detail}"))?;
    let durable_log: Option<Arc<dyn meerkat_runtime::member_observation::DurableEventLogRead>> =
        durable_sessions.then(|| {
            Arc::new(ServiceDurableEventLogRead {
                service: Arc::clone(&service),
            }) as Arc<dyn meerkat_runtime::member_observation::DurableEventLogRead>
        });
    let actor = spawn_mob_host_actor(MobHostActorConfig {
        host_runtime: Arc::clone(&host_comms.runtime),
        host_dsl: Arc::clone(&host_comms.dsl),
        host_inbox_sender: host_comms.inbox_sender.clone(),
        host_keypair: Arc::clone(&host_keypair),
        registry,
        persistence: Arc::new(RuntimeStoreHostBindingPersistence::new(runtime_store)),
        probe: Arc::clone(&probe) as Arc<dyn ProviderPresenceProbe>,
        capability_facts: HostCapabilityFacts {
            durable_sessions,
            memory_store: cfg!(feature = "memory-store"),
            mcp: cfg!(feature = "mcp"),
        },
        advertised_address: advertised_address.clone(),
        live_endpoint,
        descriptor_watch_tx,
        descriptor_sink: Arc::new(DescriptorFileSink::new(composition.descriptor_out.clone())),
        member_host: Some(HostMemberSubstrate {
            session_service: Arc::clone(&service) as Arc<dyn MobSessionService>,
            runtime_adapter: Arc::clone(&runtime_adapter),
            durable_event_log: durable_log.clone(),
            realm_backend_persistent: durable_sessions,
            member_identity_root,
            preflight_probe: probe as Arc<dyn MaterializePreflightProbe>,
        }),
    })
    .await?;

    // 11b. Member observation substrate (§7.4 phase 6, DEC-P6E-2): the
    //      machine-wide injected host serving `ReadMemberHistory` /
    //      `PollMemberEvents` / directed-turn admission over the facade
    //      service's durable event log (None on Memory backends — the
    //      bounded ring substitutes) + the actor's observation projection.
    let mut observation = HostMemberObservation::new(
        Arc::clone(&service) as Arc<dyn MobSessionService>,
        durable_log,
        actor.observation_watch(),
        actor.observation_pending_sender(),
        actor.observation_ack_sender(),
    );
    if let Some(capacity) = composition.live_buffer_events {
        observation = observation.with_event_ring_capacity(capacity);
    }
    let observation = Arc::new(observation);
    runtime_adapter.set_member_observation_host(observation.clone());
    let member_observation_recovery = observation.recover_pending_turns().await;

    eprintln!(
        "rkat mob host serving realm '{}' on {} (descriptor: {})",
        realm_id.as_str(),
        advertised_address,
        composition.descriptor_out.display()
    );

    // 12. Run until ctrl-c; shutdown in reverse composition order.
    tokio::signal::ctrl_c().await?;
    eprintln!("rkat mob host shutting down");
    acceptor.shutdown().await;
    if let Some(owner) = member_observation_recovery {
        owner.shutdown().await;
    }
    actor.shutdown().await;
    if let Some(schedule_host) = schedule_host {
        schedule_host.shutdown().await;
    }
    #[cfg(feature = "openai-realtime")]
    if let Some(task) = live {
        task.abort_and_join().await;
    }
    lease.shutdown().await;
    Ok(())
}

/// Durable event-log read adapter over the daemon's concrete facade service
/// (DEC-P6E-2): hands `HostMemberObservation` the
/// `event_log_read_from`/`event_log_latest_seq` reads without widening
/// `MobSessionService`.
struct ServiceDurableEventLogRead {
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
}

#[async_trait]
impl meerkat_runtime::member_observation::DurableEventLogRead for ServiceDurableEventLogRead {
    async fn read_from(
        &self,
        session: &meerkat_core::types::SessionId,
        from_seq: u64,
        max_rows: usize,
    ) -> Result<
        Option<
            Vec<(
                u64,
                meerkat_core::event::EventEnvelope<meerkat_core::event::AgentEvent>,
            )>,
        >,
        meerkat_runtime::member_observation::MemberObservationError,
    > {
        match self
            .service
            .event_log_read_from_bounded(session, from_seq, max_rows)
            .await
        {
            Ok(Some(rows)) => Ok(Some(
                rows.into_iter()
                    .map(|row| (row.seq, row.to_envelope()))
                    .collect(),
            )),
            Ok(None) => Ok(None),
            // Projection halt / store fault: serve typed-unavailable, never
            // a laundered empty page (the log has a hole from the halt).
            Err(error) => Err(
                meerkat_runtime::member_observation::MemberObservationError::Unavailable {
                    reason: error.to_string(),
                },
            ),
        }
    }

    async fn latest_seq(
        &self,
        session: &meerkat_core::types::SessionId,
    ) -> Result<Option<u64>, meerkat_runtime::member_observation::MemberObservationError> {
        self.service
            .event_log_latest_seq(session)
            .await
            .map_err(|error| {
                meerkat_runtime::member_observation::MemberObservationError::Unavailable {
                    reason: error.to_string(),
                }
            })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[cfg(feature = "openai-realtime")]
    #[tokio::test]
    async fn live_task_guard_releases_listener_on_startup_rollback() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test live listener");
        let address = listener.local_addr().expect("read test listener address");
        let task = tokio::spawn(async move {
            let _listener = listener;
            std::future::pending::<()>().await;
        });
        let guard = AbortOnDropTask::new(task);
        tokio::task::yield_now().await;

        drop(guard);

        let rebound = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                match tokio::net::TcpListener::bind(address).await {
                    Ok(listener) => break listener,
                    Err(_) => tokio::task::yield_now().await,
                }
            }
        })
        .await
        .expect("dropping the startup guard must abort the listener task");
        drop(rebound);
    }

    fn file_config() -> MobHostConfig {
        MobHostConfig {
            listen_tcp: Some("127.0.0.1:7801".to_string()),
            advertise_tcp: Some("tcp://198.51.100.7:7801".to_string()),
            identity_dir: Some(PathBuf::from("/from-file/identity")),
            descriptor_out: Some(PathBuf::from("/from-file/host-binding.json")),
            max_connections: Some(9),
            read_deadline_ms: Some(1500),
            pairing_rate: Some(3),
            live_buffer_events: Some(2048),
            ..MobHostConfig::default()
        }
    }

    #[test]
    fn flags_override_file_values() {
        let args = MobHostArgs {
            listen_tcp: Some("127.0.0.1:9000".to_string()),
            identity_dir: Some(PathBuf::from("/from-flag/identity")),
            ..MobHostArgs::default()
        };
        let resolved =
            resolve_mob_host_composition(&args, &file_config(), std::path::Path::new("/state"))
                .expect("composition resolves");
        assert_eq!(resolved.listen_tcp, "127.0.0.1:9000");
        assert_eq!(resolved.identity_dir, PathBuf::from("/from-flag/identity"));
        // Absent flags inherit file values.
        assert_eq!(
            resolved.advertise_tcp.as_deref(),
            Some("tcp://198.51.100.7:7801")
        );
        assert_eq!(
            resolved.descriptor_out,
            PathBuf::from("/from-file/host-binding.json")
        );
        assert_eq!(resolved.max_connections, Some(9));
        assert_eq!(resolved.read_deadline, Some(Duration::from_millis(1500)));
        assert_eq!(resolved.pairing_rate, Some(3));
        assert_eq!(
            resolved.live_buffer_events.map(NonZeroUsize::get),
            Some(2048)
        );
    }

    #[test]
    fn pairing_password_sources_resolve_without_persisted_config() {
        let env_args = MobHostArgs {
            pairing_password_env: Some("RKAT_MOB_HOST_PAIRING_PASSWORD".to_string()),
            ..MobHostArgs::default()
        };
        let env_secret =
            resolve_mob_host_pairing_password(&env_args, &MobHostConfig::default(), |name| {
                assert_eq!(name, "RKAT_MOB_HOST_PAIRING_PASSWORD");
                Ok("env-secret-012345678901234567890123".to_string())
            })
            .expect("environment-backed host pairing secret resolves");
        assert_eq!(
            env_secret.as_deref(),
            Some("env-secret-012345678901234567890123")
        );

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("pairing-secret");
        std::fs::write(&path, "file-secret-01234567890123456789012\r\n")
            .expect("write host pairing secret fixture");
        let file_args = MobHostArgs {
            pairing_password_file: Some(path),
            ..MobHostArgs::default()
        };
        let file_secret =
            resolve_mob_host_pairing_password(&file_args, &MobHostConfig::default(), |_| {
                Err("environment must not be read".to_string())
            })
            .expect("file-backed host pairing secret resolves");
        assert_eq!(
            file_secret.as_deref(),
            Some("file-secret-01234567890123456789012")
        );
    }

    #[test]
    fn empty_or_short_pairing_password_sources_fail_preflight() {
        for weak_secret in ["", "short"] {
            let error = resolve_mob_host_pairing_password(
                &MobHostArgs {
                    pairing_password_env: Some("RKAT_MOB_HOST_PAIRING_PASSWORD".to_string()),
                    ..MobHostArgs::default()
                },
                &MobHostConfig::default(),
                |_| Ok(weak_secret.to_string()),
            )
            .expect_err("weak environment-backed pairing secret must fail preflight");
            assert!(matches!(
                error,
                MobHostStartupError::PairingPasswordInvalid { .. }
            ));
        }

        let dir = tempfile::tempdir().expect("tempdir");
        for (name, weak_secret) in [("empty", ""), ("short", "short\n")] {
            let path = dir.path().join(name);
            std::fs::write(&path, weak_secret).expect("write weak pairing secret fixture");
            let error = resolve_mob_host_pairing_password(
                &MobHostArgs {
                    pairing_password_file: Some(path),
                    ..MobHostArgs::default()
                },
                &MobHostConfig::default(),
                |_| Err("environment must not be read".to_string()),
            )
            .expect_err("weak file-backed pairing secret must fail preflight");
            assert!(matches!(
                error,
                MobHostStartupError::PairingPasswordInvalid { .. }
            ));
        }
    }

    #[test]
    fn pairing_password_sources_are_mutually_exclusive_without_secret_disclosure() {
        let args = MobHostArgs {
            pairing_password: Some("raw-secret-must-not-appear-in-errors".to_string()),
            pairing_password_env: Some("RKAT_MOB_HOST_PAIRING_PASSWORD".to_string()),
            ..MobHostArgs::default()
        };
        let error = resolve_mob_host_pairing_password(&args, &MobHostConfig::default(), |_| {
            Ok("unused".to_string())
        })
        .expect_err("multiple host pairing secret sources must fail closed");
        assert!(matches!(
            error,
            MobHostStartupError::PairingPasswordSourceConflict
        ));
        assert!(!error.to_string().contains("raw-secret-must-not-appear"));

        let env_error = resolve_mob_host_pairing_password(
            &MobHostArgs {
                pairing_password_env: Some("MISSING_MOB_HOST_SECRET".to_string()),
                ..MobHostArgs::default()
            },
            &MobHostConfig::default(),
            |_| Err("not present".to_string()),
        )
        .expect_err("missing environment source must be typed");
        assert!(matches!(
            env_error,
            MobHostStartupError::PairingPasswordEnv { .. }
        ));
        assert_eq!(
            env_error.to_string(),
            "failed to read environment variable 'MISSING_MOB_HOST_SECRET' for mob host pairing password: not present"
        );
    }

    #[test]
    fn mob_host_debug_surfaces_redact_pairing_passwords() {
        let secret = "debug-secret-must-never-appear-0123456789";
        let args = MobHostArgs {
            pairing_password: Some(secret.to_string()),
            ..MobHostArgs::default()
        };
        let args_debug = format!("{args:?}");
        assert!(args_debug.contains("[REDACTED]"));
        assert!(!args_debug.contains(secret));

        let resolved = resolve_mob_host_composition(
            &args,
            &MobHostConfig::default(),
            std::path::Path::new("/state"),
        )
        .expect("valid host pairing secret resolves");
        let resolved_debug = format!("{resolved:?}");
        assert!(resolved_debug.contains("[REDACTED]"));
        assert!(!resolved_debug.contains(secret));
    }

    #[test]
    fn pairing_listener_is_loopback_only_before_daemon_side_effects() {
        for loopback in ["127.0.0.1:0", "[::1]:0"] {
            ensure_pairing_listener_is_loopback(loopback.parse().expect("loopback address"), true)
                .expect("pairing may be tunnelled through loopback");
        }
        let remote: std::net::SocketAddr = "0.0.0.0:7801".parse().expect("wildcard address");
        assert!(matches!(
            ensure_pairing_listener_is_loopback(remote, true),
            Err(MobHostStartupError::PairingRequiresLoopback { addr }) if addr == remote
        ));
        ensure_pairing_listener_is_loopback(remote, false)
            .expect("ordinary signed host traffic remains remotely bindable");
    }

    #[test]
    fn defaults_apply_when_flags_and_file_are_absent() {
        let resolved = resolve_mob_host_composition(
            &MobHostArgs::default(),
            &MobHostConfig::default(),
            std::path::Path::new("/state"),
        )
        .expect("composition resolves");
        assert_eq!(resolved.listen_tcp, "127.0.0.1:0");
        assert_eq!(resolved.identity_dir, PathBuf::from("/state/host-identity"));
        assert_eq!(resolved.descriptor_out, PathBuf::from("host-binding.json"));
        assert_eq!(resolved.live_buffer_events, None);
    }

    #[test]
    fn zero_resource_bounds_are_typed_startup_rejections() {
        let cases = [
            (
                "max_connections",
                MobHostConfig {
                    max_connections: Some(0),
                    ..MobHostConfig::default()
                },
            ),
            (
                "read_deadline_ms",
                MobHostConfig {
                    read_deadline_ms: Some(0),
                    ..MobHostConfig::default()
                },
            ),
            (
                "live_buffer_events",
                MobHostConfig {
                    live_buffer_events: Some(0),
                    ..MobHostConfig::default()
                },
            ),
        ];
        for (expected_field, config) in cases {
            let error = resolve_mob_host_composition(
                &MobHostArgs::default(),
                &config,
                std::path::Path::new("/state"),
            )
            .expect_err("zero resource bound must reject before startup side effects");
            assert!(matches!(
                error,
                MobHostStartupError::ResourceBoundZero { field }
                    if field == expected_field
            ));
        }
    }

    #[test]
    fn isolated_realm_is_a_typed_startup_reject() {
        // §W4.1 row: `--isolated` ⇒ typed reject (R2 — bind facts and host
        // identity must survive restarts). The guard is the FIRST statement
        // of run_mob_host, so pinning the decision table pins the daemon.
        assert!(matches!(
            ensure_durable_realm(RealmOrigin::Generated),
            Err(MobHostStartupError::IsolatedRealmUnsupported)
        ));
        assert!(ensure_durable_realm(RealmOrigin::Workspace).is_ok());
        assert!(ensure_durable_realm(RealmOrigin::Explicit).is_ok());
    }

    #[test]
    fn live_ws_without_advertise_is_a_typed_startup_reject() {
        let args = MobHostArgs {
            live_ws: Some("127.0.0.1:7802".to_string()),
            ..MobHostArgs::default()
        };
        let err = resolve_mob_host_composition(
            &args,
            &MobHostConfig::default(),
            std::path::Path::new("/state"),
        )
        .expect_err("live_ws without advertise must reject");
        assert!(matches!(err, MobHostStartupError::LiveWsAdvertiseRequired));
    }

    #[test]
    fn live_ws_advertise_must_be_ws_or_wss() {
        let args = MobHostArgs {
            live_ws: Some("127.0.0.1:7802".to_string()),
            live_ws_advertise: Some("https://host.example/live".to_string()),
            ..MobHostArgs::default()
        };
        let err = resolve_mob_host_composition(
            &args,
            &MobHostConfig::default(),
            std::path::Path::new("/state"),
        )
        .expect_err("non-ws scheme must reject");
        assert!(matches!(
            err,
            MobHostStartupError::LiveWsAdvertiseInvalid { .. }
        ));
    }

    #[test]
    fn live_ws_advertise_must_be_an_absolute_base_url() {
        for invalid in [
            "ws://",
            "ws:///live",
            "wss://host.example/live?token=secret",
            "wss://host.example/live#fragment",
            "wss://user:secret@host.example/live",
        ] {
            let args = MobHostArgs {
                live_ws: Some("127.0.0.1:7802".to_string()),
                live_ws_advertise: Some(invalid.to_string()),
                ..MobHostArgs::default()
            };
            let err = resolve_mob_host_composition(
                &args,
                &MobHostConfig::default(),
                std::path::Path::new("/state"),
            )
            .expect_err("malformed live base URL must reject");
            assert!(matches!(
                err,
                MobHostStartupError::LiveWsAdvertiseInvalid { .. }
            ));
        }
    }

    #[test]
    fn live_ws_advertise_is_canonicalized_for_path_joining() {
        for (advertised, expected) in [
            ("wss://host.example/", "wss://host.example"),
            ("wss://host.example/prefix/", "wss://host.example/prefix"),
            (
                " \twSs://HOST.EXAMPLE/prefix/\r\n",
                "wss://host.example/prefix",
            ),
        ] {
            let args = MobHostArgs {
                live_ws: Some("127.0.0.1:7802".to_string()),
                live_ws_advertise: Some(advertised.to_string()),
                ..MobHostArgs::default()
            };
            let resolved = resolve_mob_host_composition(
                &args,
                &MobHostConfig::default(),
                std::path::Path::new("/state"),
            )
            .expect("valid live advertise base");
            assert_eq!(resolved.live_ws_advertise.as_deref(), Some(expected));
        }
    }

    #[test]
    fn descriptor_file_is_written_0600_and_atomically_replaced() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("host-binding.json");
        let sink = DescriptorFileSink::new(path.clone());
        sink.publish("{\"kind\":\"host\"}").expect("first write");
        sink.publish("{\"kind\":\"host\",\"v\":2}")
            .expect("rewrite");
        let contents = std::fs::read_to_string(&path).expect("read back");
        assert!(contents.contains("\"v\":2"));
        assert_eq!(
            std::fs::read_dir(dir.path())
                .expect("list descriptor directory")
                .count(),
            1,
            "successful atomic replacement leaves no staging file"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path)
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "descriptor must be owner-only");
        }
    }

    #[tokio::test]
    async fn binding_presence_walk_is_presence_only_and_fails_closed_on_unknown_binding() {
        let config = Config::default();
        let with_anthropic = |key: &str| -> Option<String> {
            (key == "ANTHROPIC_API_KEY").then(|| "present".to_string())
        };
        // None binding ⇒ the provider's default chain (env-default included).
        let present = probe_binding_presence_with_lookup(
            &config,
            Provider::Anthropic,
            None,
            None,
            None,
            &with_anthropic,
        )
        .await
        .expect("probe runs");
        assert!(present);
        let absent = probe_binding_presence_with_lookup(
            &config,
            Provider::OpenAI,
            None,
            None,
            None,
            &with_anthropic,
        )
        .await
        .expect("probe runs");
        assert!(!absent);
        // A named binding that resolves no candidate chain is typed absence,
        // never a probe fault (the machine composes the reject verdict).
        let binding = meerkat_core::AuthBindingRef {
            realm: meerkat_core::connection::RealmId::parse("nonexistent-realm").expect("realm id"),
            binding: meerkat_core::connection::BindingId::parse("nonexistent-binding")
                .expect("binding id"),
            profile: None,
            origin: meerkat_core::connection::BindingOrigin::Configured,
        };
        let unresolved = probe_binding_presence_with_lookup(
            &config,
            Provider::Anthropic,
            Some(&binding),
            None,
            None,
            &with_anthropic,
        )
        .await
        .expect("probe runs");
        assert!(!unresolved);
    }

    #[tokio::test]
    async fn probe_reports_env_backed_provider_presence_only() {
        // Default config: every provider resolves through the synthetic
        // env-default candidate; presence is decided by the injected lookup
        // (RKAT_ prefix wins), with zero network by construction — the probe
        // takes no client and no resolver registry.
        let config = Config::default();
        let with_anthropic = |key: &str| -> Option<String> {
            (key == "ANTHROPIC_API_KEY").then(|| "present".to_string())
        };
        let resolved = probe_resolvable_providers_with_lookup(&config, None, None, &with_anthropic)
            .await
            .expect("probe runs");
        assert!(resolved.contains(&Provider::Anthropic));
        assert!(!resolved.contains(&Provider::OpenAI));

        let with_rkat_override = |key: &str| -> Option<String> {
            (key == "RKAT_OPENAI_API_KEY").then(|| "present".to_string())
        };
        let resolved =
            probe_resolvable_providers_with_lookup(&config, None, None, &with_rkat_override)
                .await
                .expect("probe runs");
        assert!(resolved.contains(&Provider::OpenAI));

        let empty = |_: &str| -> Option<String> { None };
        let resolved = probe_resolvable_providers_with_lookup(&config, None, None, &empty)
            .await
            .expect("probe runs");
        assert!(resolved.is_empty());
    }
}
