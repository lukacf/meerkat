use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;
use mdm_tux::machines::kennel_lease;
use mdm_tux::machines::kennel_target_control::{
    self, Effect as ControlEffect, Event as ControlEvent, State as ControlState,
};
use mdm_tux::{
    ClaimGrant, DEFAULT_OPENAI_MODEL, ExampleGeneratedCommsTrustRouter, KennelPayload,
    KennelTargetState, LeaseTerminationReason, LeaseView, ListScope, ProviderKind,
    SignedKennelEnvelope, TargetListEntry, TargetRegistrationRejectReason, build_signed_envelope,
    load_or_generate_keypair, read_envelope, verify_envelope, write_envelope,
};
use meerkat_mob::definition::{BackendConfig, ExternalBackendConfig, WiringRules};
use meerkat_mob::{
    AgentIdentity, MobBackendKind, MobDefinition, MobId, MobRuntimeMode, Profile, ProfileBinding,
    ProfileName, RuntimeBinding, SpawnMemberSpec, ToolConfig,
};
use meerkat_mob_mcp::MobMcpState;
use parking_lot::Mutex;
use tokio::io::BufReader;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

const DEFAULT_LEASE_TTL_SECS: u64 = 45;
const ACK_WINDOW_MS: i64 = 5_000;
const RECOVERY_WINDOW_MS: i64 = 60_000;

// ── Records (connection metadata — NOT lease state) ──────────────────────────

#[derive(Clone)]
struct TargetRecord {
    target_id: String,
    name: String,
    pubkey: String,
    direct_addr: String,
    rpc_addr: Option<String>,
    #[allow(dead_code)]
    labels: BTreeMap<String, String>,
    #[allow(dead_code)]
    capabilities: BTreeMap<String, bool>,
    tx: mpsc::UnboundedSender<SignedKennelEnvelope>,
    /// Machine-owned kennel control state.
    control_state: ControlState,
}

#[derive(Clone)]
struct TuxRecord {
    #[allow(dead_code)]
    tux_id: String,
    #[allow(dead_code)]
    pubkey: String,
    tx: mpsc::UnboundedSender<SignedKennelEnvelope>,
}

struct KennelState {
    targets: HashMap<String, TargetRecord>,
    tuxes: HashMap<String, TuxRecord>,
    broker_incarnation: String,
    retired_targets: Vec<TargetRecord>,
    peer_updates: Arc<tokio::sync::Mutex<()>>,
}

impl Default for KennelState {
    fn default() -> Self {
        Self {
            targets: HashMap::new(),
            tuxes: HashMap::new(),
            broker_incarnation: uuid::Uuid::new_v4().to_string(),
            retired_targets: Vec::new(),
            peer_updates: Arc::new(tokio::sync::Mutex::new(())),
        }
    }
}

// ── Main ─────────────────────────────────────────────────────────────────────

fn main() -> anyhow::Result<()> {
    meerkat_runtime::host_stack::run_host("mdm-kennel", run)?
}

async fn run() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "warn".into()))
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        eprintln!(
            "Usage: mdm-kennel --listen HOST:PORT [--advertise IP] [--data-dir PATH] \
             [--hive-rpc-port PORT] [--hive-model MODEL --hive-provider PROVIDER] \
             [--experimental-hive-mob]"
        );
        eprintln!(
            "The hive model defaults to gpt-5.5; API-key environment variables do not select it."
        );
        eprintln!(
            "WARNING: hive RPC is unauthenticated plaintext on 0.0.0.0 and exposes shell-capable sessions."
        );
        std::process::exit(1);
    }
    let hive_model =
        find_flag(&args, "--hive-model").unwrap_or_else(|| DEFAULT_OPENAI_MODEL.to_string());
    let hive_provider = match find_flag(&args, "--hive-provider") {
        Some(provider) => Some(
            provider
                .parse::<ProviderKind>()
                .with_context(|| format!("invalid --hive-provider {provider}"))?,
        ),
        None => None,
    };
    let enable_experimental_hive_mob = args.iter().any(|arg| arg == "--experimental-hive-mob");
    let listen = find_flag(&args, "--listen")
        .or_else(|| args.first().cloned())
        .context("--listen HOST:PORT is required")?;
    let data_dir = find_flag(&args, "--data-dir")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".rkat/mdm/kennel")
        });
    let keypair = Arc::new(load_or_generate_keypair(&data_dir.join("identity")).await?);
    let kennel_id = keypair.public_key().to_peer_id().to_string();

    let listener = TcpListener::bind(&listen)
        .await
        .with_context(|| format!("bind kennel listener at {listen}"))?;
    let state = Arc::new(Mutex::new(KennelState::default()));

    // Resolve the externally-reachable IP for addresses advertised to TUX
    // and targets. If --listen is 0.0.0.0:PORT, probe the default route.
    let kennel_host = listen.rsplit_once(':').map(|(h, _)| h).unwrap_or(&listen);
    let advertise_ip = find_flag(&args, "--advertise").unwrap_or_else(|| {
        resolve_advertise_ip(kennel_host).unwrap_or_else(|_| kennel_host.to_string())
    });

    // ── Hive agent: CommsRuntime ────────────────────────────────────────────
    let hive_comms_config = meerkat_comms::ResolvedCommsConfig {
        enabled: true,
        name: "hive".to_string(),
        inproc_namespace: None,
        listen_tcp: None,
        listen_uds: None,
        advertise_address: None,
        event_listen_tcp: None,
        #[cfg(unix)]
        event_listen_uds: None,
        identity_dir: data_dir.join("hive_identity"),
        trusted_peers_path: data_dir.join("hive_trusted_peers.json"),
        comms_config: Default::default(),
        auth: Default::default(),
        require_peer_auth: true,
        allow_external_unauthenticated: false,
        pairing_password: None,
    };
    let session_dir = data_dir.join("hive/sessions");
    let home = dirs::home_dir();
    let hive_config = meerkat_core::Config::load_from(&session_dir, home.as_deref())
        .await
        .unwrap_or_default();
    let host = mdm_tux::runtime::ManagedRpcHost::open(&session_dir, hive_config, hive_comms_config)
        .await?;
    let hive_comms_runtime = host.comms.clone();
    let hive_comms_port = {
        let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
        let local_addr = listener.local_addr()?;
        let kp = hive_comms_runtime.router_arc().keypair_arc();
        let inbox = hive_comms_runtime.router_arc().inbox_sender().clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let (kp, sender) = (kp.clone(), inbox.clone());
                tokio::spawn(async move {
                    let _ = meerkat_comms::handle_connection(stream, true, &kp, &sender).await;
                });
            }
        });
        local_addr.port()
    };

    let hive_config_store = host.config_store.clone();
    let hive_runtime = host.runtime.clone();
    let hive_mob_state = hive_runtime.mob_state().context("managed mob state")?;
    let hive_mob_state_for_kennel = Arc::clone(&hive_mob_state);

    // ── Hive agent: RPC TCP server ──────────────────────────────────────────
    let hive_rpc_port: u16 = find_flag(&args, "--hive-rpc-port")
        .and_then(|p| p.parse().ok())
        .unwrap_or(4801);
    {
        let hive_runtime_clone = Arc::clone(&hive_runtime);
        let hive_config_store_clone = Arc::clone(&hive_config_store);
        tokio::spawn(async move {
            let addr = format!("0.0.0.0:{hive_rpc_port}");
            // RPC-host: inline-hosted TCP JSON-RPC server entry point.
            // serve_tcp is the canonical RPC-host transport binding paired with the
            // SessionRuntime + NotificationSink constructed above; this surface owns
            // the RPC-host role and cannot be lifted without replacing the transport.
            if let Err(e) =
                meerkat_rpc::serve_tcp(&addr, hive_runtime_clone, hive_config_store_clone, None)
                    .await
            {
                eprintln!("[kennel] hive RPC server error: {e}");
            }
        });
    }

    // ── Create hive session (long-lived, resumed on restart) ─────────────
    // The hive session is created BEFORE the mob, so it's always the first
    // session and won't be confused with mob-spawned member sessions.
    // It uses the shared comms runtime (no comms_name) so target peers
    // registered on the factory runtime are visible.
    // ── Create hive session directly on SessionRuntime ────────────────────
    let hive_session_id: Option<String> = {
        // Check for existing sessions (resume on restart)
        let mut build = meerkat::AgentBuildConfig::new(hive_model.clone());
        build.provider = hive_provider.map(|provider| match provider {
            ProviderKind::Openai => meerkat_core::Provider::OpenAI,
            ProviderKind::Anthropic => meerkat_core::Provider::Anthropic,
            ProviderKind::Gemini => meerkat_core::Provider::Gemini,
        });
        build.system_prompt = meerkat::SystemPromptOverride::Set(
            "You are the hive orchestrator for a fleet of managed target agents.\n\
                 Use the 'peers' tool to discover which targets are connected.\n\
                 Use 'send_request' to dispatch tasks to targets and collect responses.\n\
                 Use 'send_message' for fire-and-forget notifications.\n\
                 Always use 'peers' as the source of truth for available targets \
                 before dispatching work."
                .to_string(),
        );
        build.override_builtins = meerkat_core::ToolCategoryOverride::Enable;
        build.override_shell = meerkat_core::ToolCategoryOverride::Enable;
        build.override_mob = meerkat_core::ToolCategoryOverride::Enable;
        match host.managed_session("hive", build, Vec::new()).await {
            Ok(sid) => {
                let sid_str = sid.to_string();
                eprintln!("[kennel] hive session ready: {sid_str}");
                // Verify the session is accessible
                let sessions = hive_runtime
                    .list_sessions(Default::default())
                    .await
                    .unwrap_or_default();
                eprintln!("[kennel] sessions after create: {}", sessions.len());
                for s in &sessions {
                    eprintln!("[kennel]   session: {} state={:?}", s.session_id, s.state);
                }
                Some(sid_str)
            }
            Err(e) => {
                return Err(e.context("create or resume managed hive session"));
            }
        }
    };

    // ── Enable comms drain on hive session ────────────────────────────────
    // The hive session uses the factory's shared comms runtime (no comms_name),
    // so keep_alive / update_peer_ingress_context can't be set through the
    // normal turn/start path. Eagerly attach the drain so target registration
    // can mutate trust only through the session's generated machine owner.
    let hive_session_id_typed = hive_session_id
        .as_ref()
        .map(|sid| uuid::Uuid::parse_str(sid).map(meerkat_core::types::SessionId))
        .transpose()
        .context("hive session id is valid uuid")?;
    if let Some(ref session_id) = hive_session_id_typed {
        hive_runtime
            .enable_autonomous_comms_drain(
                session_id,
                Arc::clone(&hive_comms_runtime) as Arc<dyn meerkat_core::agent::CommsRuntime>,
            )
            .await
            .map_err(|error| anyhow::anyhow!("enable hive comms drain: {}", error.message))?;
        eprintln!("[kennel] hive comms drain enabled for {session_id}");
    }
    let hive_comms_trust = hive_session_id_typed.clone().map(|session_id| {
        Arc::new(ExampleGeneratedCommsTrustRouter::new(
            hive_runtime.runtime_adapter(),
            session_id,
            Arc::clone(&hive_comms_runtime),
        ))
    });

    // ── Create hive mob (external backend) ────────────────────────────────
    let hive_mob_id: Option<MobId> = if enable_experimental_hive_mob {
        let mut profiles = BTreeMap::new();
        profiles.insert(
            ProfileName::from("target"),
            ProfileBinding::Inline(Box::new(Profile {
                model_fallback: None,
                model: hive_model.clone(),
                skills: Vec::new(),
                tools: ToolConfig {
                    comms: true,
                    shell: true,
                    builtins: true,
                    ..Default::default()
                },
                peer_description: "Managed target agent".to_string(),
                external_addressable: true,
                backend: Some(MobBackendKind::External),
                runtime_mode: MobRuntimeMode::TurnDriven,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
                provider: None,
                self_hosted_server_id: None,
                image_generation_provider: None,
                auto_compact_threshold: None,
                resume_overrides: Vec::new(),
            })),
        );

        let mut definition = MobDefinition::explicit(MobId::from("hive-fleet"));
        definition.profiles = profiles;
        definition.wiring = WiringRules {
            auto_wire_orchestrator: true,
            role_wiring: Vec::new(),
        };
        definition.backend = BackendConfig {
            default: MobBackendKind::External,
            external: Some(ExternalBackendConfig {
                address_base: format!("tcp://{advertise_ip}:{hive_comms_port}"),
                supervisor_bridge: None,
            }),
        };
        // The `set_owner_bridge_session_lookup_index` setter was removed upstream. This hive
        // mob is explicit, referenced directly by id (never looked up by bridge session) and
        // never archived, so the prior call had no runtime effect here and is simply dropped.
        // (A mob that needs the binding can still request it via
        // `mob_create_definition_with_owner_bridge_session`.)

        match hive_mob_state_for_kennel
            .mob_create_definition(definition)
            .await
        {
            Ok(mob_id) => {
                eprintln!("[kennel] hive mob created: {mob_id}");
                Some(mob_id)
            }
            Err(e) => {
                eprintln!("[kennel] failed to create hive mob: {e}");
                None
            }
        }
    } else {
        None
    };

    let hive_rpc_addr = format!("tcp://{advertise_ip}:{hive_rpc_port}");
    let hive_comms_addr = format!("tcp://{advertise_ip}:{hive_comms_port}");

    println!("=== MDM Kennel ===");
    println!("listen    : {listen}");
    println!("kennel_id : {kennel_id}");
    println!("advertise : {advertise_ip}");
    println!("hive_rpc  : {hive_rpc_addr}");
    println!("hive_comms: {hive_comms_addr}");
    println!("warning   : hive RPC is unauthenticated plaintext on 0.0.0.0");
    if let Some(mob_id) = &hive_mob_id {
        println!("hive_mob  : {mob_id}");
    }

    tokio::spawn(run_janitor(
        state.clone(),
        keypair.clone(),
        kennel_id.clone(),
        hive_comms_trust.clone(),
        hive_mob_id
            .clone()
            .map(|id| (hive_mob_state_for_kennel.clone(), id)),
    ));

    loop {
        let (stream, _) = listener.accept().await?;
        let state = state.clone();
        let keypair = keypair.clone();
        let kennel_id = kennel_id.clone();
        let hive_rpc_addr = hive_rpc_addr.clone();
        let hive_comms_addr_c = hive_comms_addr.clone();
        let hive_sid = hive_session_id.clone();
        let hive_comms_trust = hive_comms_trust.clone();
        let mob_state = Arc::clone(&hive_mob_state_for_kennel);
        let mob_id = hive_mob_id.clone();
        let hive_comms = Arc::clone(&hive_comms_runtime);
        tokio::spawn(async move {
            if let Err(e) = handle_connection(
                stream,
                state,
                keypair,
                kennel_id,
                HiveConnectionContext {
                    hive_rpc_addr,
                    hive_comms_addr: hive_comms_addr_c,
                    hive_session_id: hive_sid,
                    hive_mob_state: mob_state,
                    hive_mob_id: mob_id,
                    hive_comms_trust,
                    hive_comms_runtime: hive_comms,
                },
            )
            .await
            {
                eprintln!("[kennel] connection error: {e}");
            }
        });
    }
}

// ── Connection handler ───────────────────────────────────────────────────────

enum SessionKind {
    Target(String),
    Tux(String),
}

struct HiveConnectionContext {
    hive_rpc_addr: String,
    hive_comms_addr: String,
    hive_session_id: Option<String>,
    hive_mob_state: Arc<MobMcpState>,
    hive_mob_id: Option<MobId>,
    hive_comms_trust: Option<Arc<ExampleGeneratedCommsTrustRouter>>,
    hive_comms_runtime: Arc<meerkat_comms::CommsRuntime>,
}

async fn handle_connection(
    stream: TcpStream,
    state: Arc<Mutex<KennelState>>,
    keypair: Arc<meerkat_comms::identity::Keypair>,
    kennel_id: String,
    hive: HiveConnectionContext,
) -> anyhow::Result<()> {
    let HiveConnectionContext {
        hive_rpc_addr,
        hive_comms_addr,
        hive_session_id,
        hive_mob_state,
        hive_mob_id,
        hive_comms_trust,
        hive_comms_runtime,
    } = hive;
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let (tx, mut rx) = mpsc::unbounded_channel::<SignedKennelEnvelope>();

    let writer_task = tokio::spawn(async move {
        while let Some(env) = rx.recv().await {
            if let Err(e) = write_envelope(&mut writer, &env).await {
                eprintln!("[kennel] write error: {e}");
                break;
            }
        }
    });

    let mut session_kind: Option<SessionKind> = None;
    let peer_updates = state.lock().peer_updates.clone();
    let result: anyhow::Result<()> = async {
      loop {
        let Some(env) = read_envelope(&mut reader).await? else {
            break;
        };
        let signer = verify_envelope(&env)?;
        let signer_id = signer.to_peer_id().to_string();

        if let Some(registered) = &session_kind {
            match &env.payload {
                KennelPayload::TargetRegister { target_id, .. } => anyhow::ensure!(
                    matches!(registered, SessionKind::Target(id) if id == target_id),
                    "connection cannot change its registered participant"
                ),
                KennelPayload::TuxRegister { tux_id, .. } => anyhow::ensure!(
                    matches!(registered, SessionKind::Tux(id) if id == tux_id),
                    "connection cannot change its registered participant"
                ),
                _ => {}
            }
        }

        match &env.payload {
            KennelPayload::TargetRegister {
                target_id,
                name,
                pubkey,
                direct_addr,
                rpc_addr,
                labels,
                capabilities,
                attached_tux_id,
            } => {
                let _peer_guard = peer_updates.lock().await;
                anyhow::ensure!(target_id == &signer_id, "target signer_id mismatch");
                anyhow::ensure!(pubkey == &env.signer_id, "target pubkey mismatch");
                let target_pubkey = meerkat_comms::identity::PubKey::from_pubkey_string(pubkey)
                    .context("parse target pubkey")?;
                let now_ms = chrono::Utc::now().timestamp_millis();
                match register_target(
                    &state,
                    RegisterTargetArgs {
                        target_id: target_id.clone(),
                        name: name.clone(),
                        pubkey: pubkey.clone(),
                        direct_addr: direct_addr.clone(),
                        rpc_addr: rpc_addr.clone(),
                        labels: labels.clone(),
                        capabilities: capabilities.clone(),
                        attached_tux_id: attached_tux_id.clone(),
                        now_ms,
                        tx: tx.clone(),
                    },
                    &keypair,
                    &kennel_id,
                )? {
                    RegisterTargetOutcome::Registered { post_ack_effects } => {
                        session_kind = Some(SessionKind::Target(target_id.clone()));
                        let Some(hive_comms_trust) = hive_comms_trust.as_ref() else {
                            anyhow::bail!(
                                "generated hive comms trust authority unavailable for target registration"
                            );
                        };
                        hive_comms_trust
                            .add_trusted_peer(name, target_pubkey, direct_addr)
                            .await?;

                        let reply = build_signed_envelope(
                            &keypair,
                            &kennel_id,
                            KennelPayload::TargetRegistered {
                                hive_pubkey: Some(
                                    hive_comms_runtime.public_key().to_pubkey_string(),
                                ),
                                hive_comms_addr: Some(hive_comms_addr.clone()),
                            },
                        )?;
                        let _ = tx.send(reply);
                        {
                            let mut guard = state.lock();
                            dispatch_effects(
                                &post_ack_effects,
                                &mut guard,
                                target_id,
                                &keypair,
                                &kennel_id,
                            );
                        }

                        // Spawn target as external mob member in the hive fleet.
                        // RuntimeBinding::External carries the real target identity
                        // so the mob roster has the correct peer_id and address.
                        if let Some(mob_id) = &hive_mob_id {
                            let mob_state = Arc::clone(&hive_mob_state);
                            let mob_id = mob_id.clone();
                            let name = name.clone();
                            let target_id = target_id.clone();
                            let direct_addr = direct_addr.clone();
                            tokio::spawn(async move {
                                let mut spec = SpawnMemberSpec::new(
                                    ProfileName::from("target"),
                                    AgentIdentity::from(name.clone()),
                                );
                                let bootstrap_token = format!("mdm-tux-{target_id}");
                                spec.binding = Some(RuntimeBinding::External {
                                    peer_id: target_id,
                                    address: direct_addr,
                                    bootstrap_token: Some(bootstrap_token.into()),
                                    pubkey: *target_pubkey.as_bytes(),
                                });
                                match tokio::time::timeout(
                                    std::time::Duration::from_secs(10),
                                    mob_state.mob_spawn_spec(&mob_id, spec),
                                )
                                .await
                                {
                                    Ok(Ok(_)) => {
                                        eprintln!("[kennel] spawned {name} as hive mob member");
                                    }
                                    Ok(Err(e)) => {
                                        eprintln!(
                                            "[kennel] mob spawn {name}: {e} (peer still updated)"
                                        );
                                    }
                                    Err(_) => {
                                        eprintln!(
                                            "[kennel] mob spawn {name}: timed out (peer still updated)"
                                        );
                                    }
                                }
                            });
                        }

                        // Mesh-wire: tell the new target about all existing targets
                        // and tell each existing target about the new one.
                        {
                            let guard = state.lock();
                            for (tid, rec) in &guard.targets {
                                if tid == target_id {
                                    continue;
                                }
                                // Tell new target about existing peer
                                let wire_new = build_signed_envelope(
                                    &keypair,
                                    &kennel_id,
                                    KennelPayload::PeerWire {
                                        peer_name: rec.name.clone(),
                                        peer_id: rec.pubkey.clone(),
                                        peer_addr: rec.direct_addr.clone(),
                                    },
                                );
                                if let Ok(env) = wire_new {
                                    let _ = tx.send(env);
                                }
                                // Tell existing peer about new target
                                let wire_existing = build_signed_envelope(
                                    &keypair,
                                    &kennel_id,
                                    KennelPayload::PeerWire {
                                        peer_name: name.clone(),
                                        peer_id: pubkey.clone(),
                                        peer_addr: direct_addr.clone(),
                                    },
                                );
                                if let Ok(env) = wire_existing {
                                    let _ = rec.tx.send(env);
                                }
                            }
                        }
                    }
                    RegisterTargetOutcome::Rejected { reason, message } => {
                        let reply = build_signed_envelope(
                            &keypair,
                            &kennel_id,
                            KennelPayload::TargetRegistrationRejected { reason, message },
                        )?;
                        let _ = tx.send(reply);
                    }
                }
            }

            KennelPayload::TuxRegister { tux_id, pubkey, .. } => {
                anyhow::ensure!(tux_id == &signer_id, "tux signer_id mismatch");
                anyhow::ensure!(pubkey == &env.signer_id, "tux pubkey mismatch");
                {
                    let mut guard = state.lock();
                    guard.tuxes.insert(
                        tux_id.clone(),
                        TuxRecord {
                            tux_id: tux_id.clone(),
                            pubkey: pubkey.clone(),
                            tx: tx.clone(),
                        },
                    );
                }
                let reply = build_signed_envelope(
                    &keypair,
                    &kennel_id,
                    KennelPayload::TuxRegistered {
                        broker_incarnation: state.lock().broker_incarnation.clone(),
                        hive_rpc_addr: Some(hive_rpc_addr.clone()),
                        hive_session_id: hive_session_id.clone(),
                    },
                )?;
                let _ = tx.send(reply);
                session_kind = Some(SessionKind::Tux(tux_id.clone()));
            }

            KennelPayload::ListTargets { scope } => {
                let tux_id = match &session_kind {
                    Some(SessionKind::Tux(id)) => id.clone(),
                    _ => continue,
                };
                let targets = {
                    let guard = state.lock();
                    list_targets(&guard, &tux_id, *scope)
                };
                let reply = build_signed_envelope(
                    &keypair,
                    &kennel_id,
                    KennelPayload::TargetList {
                        scope: *scope,
                        targets,
                    },
                )?;
                let _ = tx.send(reply);
            }

            KennelPayload::ClaimTargets {
                target_ids,
                lease_ttl_sec,
            } => {
                let tux_id = match &session_kind {
                    Some(SessionKind::Tux(id)) => id.clone(),
                    _ => continue,
                };
                let claims = {
                    let mut guard = state.lock();
                    handle_claim_targets(
                        &mut guard,
                        &tux_id,
                        target_ids,
                        lease_ttl_sec.unwrap_or(DEFAULT_LEASE_TTL_SECS),
                    )
                };
                let reply = build_signed_envelope(
                    &keypair,
                    &kennel_id,
                    KennelPayload::ClaimGranted { claims: claims.clone() },
                )?;
                let _ = tx.send(reply);
                let refused_target_ids = target_ids.iter()
                    .filter(|id| !claims.iter().any(|claim| &claim.target_id == *id))
                    .cloned().collect();
                let _ = tx.send(build_signed_envelope(&keypair, &kennel_id,
                    KennelPayload::ClaimRequestCompleted {
                        in_reply_to: env.message_id.clone(), refused_target_ids,
                    })?);
            }

            KennelPayload::ClaimAck { lease_ids } => {
                let tux_id = match &session_kind {
                    Some(SessionKind::Tux(id)) => id.clone(),
                    _ => continue,
                };
                let mut guard = state.lock();
                handle_claim_ack(&mut guard, &keypair, &kennel_id, &tux_id, lease_ids);
            }

            KennelPayload::RenewLeases {
                lease_ids,
                lease_ttl_sec,
            } => {
                let tux_id = match &session_kind {
                    Some(SessionKind::Tux(id)) => id.clone(),
                    _ => continue,
                };
                let leases = {
                    let mut guard = state.lock();
                    handle_renew_leases(
                        &mut guard,
                        &tux_id,
                        lease_ids,
                        lease_ttl_sec.unwrap_or(DEFAULT_LEASE_TTL_SECS),
                    )
                };
                let reply = build_signed_envelope(
                    &keypair,
                    &kennel_id,
                    KennelPayload::LeasesRenewed { leases },
                )?;
                let _ = tx.send(reply);
            }

            KennelPayload::ReleaseTargets { lease_ids } => {
                if !matches!(&session_kind, Some(SessionKind::Tux(_))) {
                    continue;
                }
                let mut guard = state.lock();
                for lease_id in lease_ids {
                    apply_lease_event(
                        &mut guard,
                        lease_id,
                        ControlEvent::Released {
                            reason: LeaseTerminationReason::ReleasedByTux,
                        },
                        &keypair,
                        &kennel_id,
                    );
                }
                drop(guard);
            }

            KennelPayload::RebindTargets { target_ids } => {
                let tux_id = match &session_kind {
                    Some(SessionKind::Tux(id)) => id.clone(),
                    _ => continue,
                };
                let mut guard = state.lock();
                handle_rebind_targets(&mut guard, &keypair, &kennel_id, &tux_id, target_ids);
            }

            KennelPayload::HivePrompt { prompt } => {
                if !matches!(&session_kind, Some(SessionKind::Tux(_))) {
                    continue;
                }
                // The hive agent is available via the RPC server. Direct TUX
                // to connect there instead of sending prompts over the kennel
                // control channel.
                eprintln!(
                    "[kennel] hive prompt via control channel (redirecting to RPC): {prompt}"
                );
                let reply = build_signed_envelope(
                    &keypair,
                    &kennel_id,
                    KennelPayload::HiveError {
                        message: format!(
                            "Hive agent is available via RPC at {}. \
                             Connect to the hive RPC port directly instead of \
                             sending prompts over the kennel control channel.",
                            hive_rpc_addr,
                        ),
                    },
                );
                if let Ok(env) = reply {
                    let _ = tx.send(env);
                }
            }

            KennelPayload::TuxHeartbeat | KennelPayload::TargetHeartbeat => {}
            _ => {}
        }
      }
      Ok(())
    }.await;

    writer_task.abort();
    let _ = writer_task.await;

    let _peer_guard = peer_updates.lock().await;
    if let Some(kind) = session_kind {
        {
            let mut guard = state.lock();
            match &kind {
                SessionKind::Target(target_id) => {
                    if guard
                        .targets
                        .get(target_id)
                        .is_some_and(|record| record.tx.same_channel(&tx))
                    {
                        handle_target_disconnect(&mut guard, &keypair, &kennel_id, target_id);
                    }
                }
                SessionKind::Tux(tux_id) => {
                    if guard
                        .tuxes
                        .get(tux_id)
                        .is_some_and(|record| record.tx.same_channel(&tx))
                    {
                        handle_tux_disconnect(&mut guard, &keypair, &kennel_id, tux_id);
                    }
                }
            }
        }
    }
    let cleanup = reconcile_retired_targets(
        &state,
        hive_comms_trust.as_deref(),
        &keypair,
        &kennel_id,
        hive_mob_id.as_ref().map(|id| (&hive_mob_state, id)),
    )
    .await;
    result.and(cleanup)
}

// ── Machine-backed operations ────────────────────────────────────────────────

struct RegisterTargetArgs {
    target_id: String,
    name: String,
    pubkey: String,
    direct_addr: String,
    rpc_addr: Option<String>,
    labels: BTreeMap<String, String>,
    capabilities: BTreeMap<String, bool>,
    attached_tux_id: Option<String>,
    now_ms: i64,
    tx: mpsc::UnboundedSender<SignedKennelEnvelope>,
}

enum RegisterTargetOutcome {
    Registered {
        post_ack_effects: Vec<ControlEffect>,
    },
    Rejected {
        reason: TargetRegistrationRejectReason,
        message: String,
    },
}

fn register_target(
    state: &Arc<Mutex<KennelState>>,
    args: RegisterTargetArgs,
    _keypair: &meerkat_comms::identity::Keypair,
    _kennel_id: &str,
) -> anyhow::Result<RegisterTargetOutcome> {
    let RegisterTargetArgs {
        target_id,
        name,
        pubkey,
        direct_addr,
        rpc_addr,
        labels,
        capabilities,
        attached_tux_id,
        now_ms,
        tx,
    } = args;
    let mut guard = state.lock();
    if let Some(existing) = guard
        .targets
        .values()
        .find(|t| t.name == name && t.target_id != target_id)
    {
        return Ok(RegisterTargetOutcome::Rejected {
            reason: TargetRegistrationRejectReason::DuplicateName,
            message: format!(
                "target name '{}' already registered by {}",
                existing.name, existing.target_id
            ),
        });
    }

    if guard.targets.contains_key(&target_id) {
        let effects_to_dispatch: Vec<ControlEffect>;
        {
            let existing = guard
                .targets
                .get_mut(&target_id)
                .expect("checked contains_key");
            let (new_state, effects) = kennel_target_control::transition(
                existing.control_state.clone(),
                ControlEvent::Registered {
                    attached_tux_id,
                    now_ms,
                    recovery_window_ms: RECOVERY_WINDOW_MS,
                },
            )
            .map_err(|e| anyhow::anyhow!("target re-register transition: {e}"))?;
            existing.name = name;
            existing.pubkey = pubkey;
            existing.direct_addr = direct_addr;
            existing.rpc_addr = rpc_addr;
            existing.labels = labels;
            existing.capabilities = capabilities;
            existing.tx = tx;
            existing.control_state = new_state;
            effects_to_dispatch = effects;
        }
        return Ok(RegisterTargetOutcome::Registered {
            post_ack_effects: effects_to_dispatch,
        });
    }

    guard.targets.insert(
        target_id.clone(),
        TargetRecord {
            target_id: target_id.clone(),
            name,
            pubkey,
            direct_addr,
            rpc_addr,
            labels,
            capabilities,
            tx,
            control_state: ControlState::available(target_id.clone()),
        },
    );
    let effects = {
        let target = guard
            .targets
            .get_mut(&target_id)
            .expect("inserted target must exist");
        let (new_state, effects) = kennel_target_control::transition(
            target.control_state.clone(),
            ControlEvent::Registered {
                attached_tux_id,
                now_ms,
                recovery_window_ms: RECOVERY_WINDOW_MS,
            },
        )
        .map_err(|e| anyhow::anyhow!("initial target register transition: {e}"))?;
        target.control_state = new_state;
        effects
    };
    Ok(RegisterTargetOutcome::Registered {
        post_ack_effects: effects,
    })
}

fn list_targets(state: &KennelState, tux_id: &str, scope: ListScope) -> Vec<TargetListEntry> {
    let mut out = Vec::new();
    for target in state.targets.values() {
        let entry = match (&scope, &target.control_state.lease) {
            (ListScope::Available, kennel_lease::State::Available { .. }) => {
                Some(TargetListEntry {
                    target_id: target.target_id.clone(),
                    name: target.name.clone(),
                    state: KennelTargetState::Available,
                    lease_id: None,
                    rpc_addr: target.rpc_addr.clone(),
                })
            }
            (
                ListScope::Mine,
                kennel_lease::State::AwaitingAck {
                    tux_id: owner,
                    lease_id,
                    ..
                },
            ) if owner == tux_id => Some(TargetListEntry {
                target_id: target.target_id.clone(),
                name: target.name.clone(),
                state: KennelTargetState::Claimed,
                lease_id: Some(lease_id.clone()),
                rpc_addr: target.rpc_addr.clone(),
            }),
            (
                ListScope::Mine,
                kennel_lease::State::Claimed {
                    tux_id: owner,
                    lease_id,
                    ..
                },
            ) if owner == tux_id => Some(TargetListEntry {
                target_id: target.target_id.clone(),
                name: target.name.clone(),
                state: KennelTargetState::Claimed,
                lease_id: Some(lease_id.clone()),
                rpc_addr: target.rpc_addr.clone(),
            }),
            (ListScope::Mine, kennel_lease::State::RecoveringClaim { tux_id: owner, .. })
                if owner == tux_id =>
            {
                Some(TargetListEntry {
                    target_id: target.target_id.clone(),
                    name: target.name.clone(),
                    state: KennelTargetState::RecoveringClaim,
                    lease_id: None,
                    rpc_addr: target.rpc_addr.clone(),
                })
            }
            _ => None,
        };
        if let Some(e) = entry {
            out.push(e);
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn handle_claim_targets(
    state: &mut KennelState,
    tux_id: &str,
    target_ids: &[String],
    ttl_secs: u64,
) -> Vec<ClaimGrant> {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let expires_at_ms = now_ms + (ttl_secs as i64 * 1000);
    let ack_deadline_ms = now_ms + ACK_WINDOW_MS;
    let mut claims = Vec::new();

    for target_id in target_ids {
        let Some(target) = state.targets.get_mut(target_id) else {
            continue;
        };
        let lease_id = uuid::Uuid::new_v4().to_string();
        let event = ControlEvent::ClaimRequested {
            target_id: target_id.clone(),
            lease_id: lease_id.clone(),
            tux_id: tux_id.to_string(),
            expires_at_ms,
            ack_deadline_ms,
        };
        let Ok((new_state, _effects)) =
            kennel_target_control::transition(target.control_state.clone(), event)
        else {
            continue; // target not Available
        };
        target.control_state = new_state;
        claims.push(ClaimGrant {
            lease_id,
            target_id: target.target_id.clone(),
            target_name: target.name.clone(),
            target_pubkey: target.pubkey.clone(),
            target_direct_addr: target.direct_addr.clone(),
            rpc_addr: target.rpc_addr.clone(),
            expires_at_ms,
        });
    }
    claims
}

fn handle_claim_ack(
    state: &mut KennelState,
    keypair: &meerkat_comms::identity::Keypair,
    kennel_id: &str,
    tux_id: &str,
    lease_ids: &[String],
) {
    for lease_id in lease_ids {
        let Some(target_id) = find_target_id_by_lease(state, lease_id) else {
            continue;
        };
        let Some(target) = state.targets.get_mut(&target_id) else {
            continue;
        };
        let event = ControlEvent::ClaimAcked {
            lease_id: lease_id.clone(),
            tux_id: tux_id.to_string(),
            now_ms: chrono::Utc::now().timestamp_millis(),
        };
        let Ok((new_state, effects)) =
            kennel_target_control::transition(target.control_state.clone(), event)
        else {
            continue;
        };
        target.control_state = new_state;
        dispatch_effects(&effects, state, &target_id, keypair, kennel_id);
    }
}

fn handle_renew_leases(
    state: &mut KennelState,
    tux_id: &str,
    lease_ids: &[String],
    ttl_secs: u64,
) -> Vec<LeaseView> {
    let new_expires_at_ms = chrono::Utc::now().timestamp_millis() + (ttl_secs as i64 * 1000);
    let mut leases = Vec::new();

    for lease_id in lease_ids {
        let Some(target_id) = find_target_id_by_lease(state, lease_id) else {
            continue;
        };
        let Some(target) = state.targets.get_mut(&target_id) else {
            continue;
        };
        let event = ControlEvent::LeaseRenewed {
            lease_id: lease_id.clone(),
            tux_id: tux_id.to_string(),
            new_expires_at_ms,
            now_ms: chrono::Utc::now().timestamp_millis(),
        };
        let Ok((new_state, _effects)) =
            kennel_target_control::transition(target.control_state.clone(), event)
        else {
            continue;
        };
        target.control_state = new_state;
        leases.push(LeaseView {
            lease_id: lease_id.clone(),
            target_id,
            expires_at_ms: new_expires_at_ms,
        });
    }
    leases
}

fn handle_rebind_targets(
    state: &mut KennelState,
    keypair: &meerkat_comms::identity::Keypair,
    kennel_id: &str,
    tux_id: &str,
    target_ids: &[String],
) {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let new_expires_at_ms = now_ms + (DEFAULT_LEASE_TTL_SECS as i64 * 1000);

    for target_id in target_ids {
        let Some(target) = state.targets.get_mut(target_id) else {
            continue;
        };
        let new_lease_id = uuid::Uuid::new_v4().to_string();
        let event = ControlEvent::Rebound {
            new_lease_id: new_lease_id.clone(),
            tux_id: tux_id.to_string(),
            new_expires_at_ms,
        };
        let Ok((new_state, effects)) =
            kennel_target_control::transition(target.control_state.clone(), event)
        else {
            continue;
        };
        target.control_state = new_state;
        dispatch_effects(&effects, state, target_id, keypair, kennel_id);
    }
}

fn handle_target_disconnect(
    state: &mut KennelState,
    keypair: &meerkat_comms::identity::Keypair,
    kennel_id: &str,
    target_id: &str,
) {
    apply_target_event(
        state,
        target_id,
        ControlEvent::TargetDisconnected {
            now_ms: chrono::Utc::now().timestamp_millis(),
            recovery_window_ms: RECOVERY_WINDOW_MS,
        },
        keypair,
        kennel_id,
    );
}

fn handle_tux_disconnect(
    state: &mut KennelState,
    keypair: &meerkat_comms::identity::Keypair,
    kennel_id: &str,
    tux_id: &str,
) {
    state.tuxes.remove(tux_id);
    let target_ids: Vec<String> = state.targets.keys().cloned().collect();
    for target_id in target_ids {
        let Some(target) = state.targets.get(&target_id) else {
            continue;
        };
        let owner_matches = match &target.control_state.lease {
            kennel_lease::State::AwaitingAck { tux_id: owner, .. }
            | kennel_lease::State::Claimed { tux_id: owner, .. }
            | kennel_lease::State::RecoveringClaim { tux_id: owner, .. } => owner == tux_id,
            kennel_lease::State::Available { .. } => false,
        };
        if owner_matches {
            apply_target_event(
                state,
                &target_id,
                ControlEvent::TuxDisconnected {
                    tux_id: tux_id.to_string(),
                    now_ms: chrono::Utc::now().timestamp_millis(),
                    recovery_window_ms: RECOVERY_WINDOW_MS,
                },
                keypair,
                kennel_id,
            );
        }
    }
}

/// Resolve lease_id routing from canonical machine state, not from a side map.
fn find_target_id_by_lease(state: &KennelState, lease_id: &str) -> Option<String> {
    state.targets.iter().find_map(|(target_id, target)| {
        let state_lease_id = match &target.control_state.lease {
            kennel_lease::State::AwaitingAck { lease_id: lid, .. }
            | kennel_lease::State::Claimed { lease_id: lid, .. } => Some(lid.as_str()),
            kennel_lease::State::RecoveringClaim {
                lease: kennel_lease::RecoveryLease::Assigned(lid),
                ..
            } => Some(lid.as_str()),
            kennel_lease::State::Available { .. }
            | kennel_lease::State::RecoveringClaim {
                lease: kennel_lease::RecoveryLease::PendingRebind,
                ..
            } => None,
        };
        (state_lease_id == Some(lease_id)).then(|| target_id.clone())
    })
}

/// Apply an event to a target by lease_id lookup.
fn apply_lease_event(
    state: &mut KennelState,
    lease_id: &str,
    event: ControlEvent,
    keypair: &meerkat_comms::identity::Keypair,
    kennel_id: &str,
) {
    let Some(target_id) = find_target_id_by_lease(state, lease_id) else {
        eprintln!("[kennel] dropped lease event for unknown lease {lease_id}: {event:?}");
        return;
    };
    apply_target_event(state, &target_id, event, keypair, kennel_id);
}

/// Apply an event to a target by target_id.
fn apply_target_event(
    state: &mut KennelState,
    target_id: &str,
    event: ControlEvent,
    keypair: &meerkat_comms::identity::Keypair,
    kennel_id: &str,
) {
    let Some(target) = state.targets.get_mut(target_id) else {
        eprintln!("[kennel] dropped target event for unknown target {target_id}: {event:?}");
        return;
    };
    let Ok((new_state, effects)) =
        kennel_target_control::transition(target.control_state.clone(), event.clone())
    else {
        eprintln!("[kennel] invalid target control transition for {target_id}: {event:?}");
        return;
    };
    target.control_state = new_state;
    dispatch_effects(&effects, state, target_id, keypair, kennel_id);
}

// ── Effect dispatch ──────────────────────────────────────────────────────────

fn dispatch_effects(
    effects: &[ControlEffect],
    state: &mut KennelState,
    target_id: &str,
    keypair: &meerkat_comms::identity::Keypair,
    kennel_id: &str,
) {
    for effect in effects {
        match effect {
            ControlEffect::Lease(kennel_lease::Effect::SendTargetReleased {
                target_id: _,
                lease_ref,
                reason,
            }) => {
                if let Some(target) = state.targets.get(target_id)
                    && let Ok(env) = build_signed_envelope(
                        keypair,
                        kennel_id,
                        KennelPayload::Released {
                            lease_ref: lease_ref.clone(),
                            reason: *reason,
                        },
                    )
                {
                    let _ = target.tx.send(env);
                }
            }
            ControlEffect::Lease(kennel_lease::Effect::SendClaimReleasedToTux {
                target_id: _,
                lease_ref,
                tux_id,
                reason,
            }) => {
                if let Some(tux) = state.tuxes.get(tux_id)
                    && let Ok(env) = build_signed_envelope(
                        keypair,
                        kennel_id,
                        KennelPayload::ClaimReleased {
                            lease_ref: lease_ref.clone(),
                            target_id: target_id.to_string(),
                            reason: *reason,
                        },
                    )
                {
                    let _ = tux.tx.send(env);
                }
            }
            ControlEffect::Lease(kennel_lease::Effect::SendTargetLostToTux {
                target_id: _,
                tux_id,
                lease_ref,
            }) => {
                // Target lost is still emitted by the lease machine during
                // disconnect recovery; log it but don't send TargetLost
                // payload (removed from protocol). The TUX will discover
                // target loss via its RPC connection.
                eprintln!(
                    "[kennel] target {target_id} lost (lease {lease_ref:?}), notifying tux {tux_id} via claim release"
                );
            }
            ControlEffect::Lease(kennel_lease::Effect::SendLeaseRebound {
                target_id: _,
                lease_id,
                tux_id,
                expires_at_ms,
            }) => {
                // Send LeaseRebound to the target so it knows about the
                // recovered TUX ownership. TUX no longer receives this
                // (it re-claims via the kennel).
                if let Some(target) = state.targets.get(target_id) {
                    let tux_pubkey = state
                        .tuxes
                        .get(tux_id)
                        .map(|t| t.pubkey.clone())
                        .unwrap_or_default();
                    if let Ok(env) = build_signed_envelope(
                        keypair,
                        kennel_id,
                        KennelPayload::LeaseRebound {
                            lease_id: lease_id.clone(),
                            target_id: target_id.to_string(),
                            tux_id: tux_id.clone(),
                            tux_pubkey,
                            tux_direct_addr: String::new(),
                            target_pubkey: target.pubkey.clone(),
                            target_direct_addr: target.direct_addr.clone(),
                            expires_at_ms: *expires_at_ms,
                        },
                    ) {
                        let _ = target.tx.send(env);
                    }
                }
            }
            ControlEffect::Lease(kennel_lease::Effect::RemoveLease { lease_id }) => {
                let _ = lease_id;
            }
            ControlEffect::Lease(kennel_lease::Effect::DropTargetRecord {
                target_id: effect_tid,
            }) => {
                if let Some(record) = state.targets.remove(effect_tid) {
                    state.retired_targets.push(record);
                }
            }
        }
    }
}

// ── Janitor ──────────────────────────────────────────────────────────────────

async fn run_janitor(
    state: Arc<Mutex<KennelState>>,
    keypair: Arc<meerkat_comms::identity::Keypair>,
    kennel_id: String,
    hive_comms_trust: Option<Arc<ExampleGeneratedCommsTrustRouter>>,
    hive_mob: Option<(Arc<MobMcpState>, MobId)>,
) {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let now_ms = chrono::Utc::now().timestamp_millis();
        let peer_updates = state.lock().peer_updates.clone();
        let _peer_guard = peer_updates.lock().await;
        {
            let mut guard = state.lock();
            let target_ids: Vec<String> = guard.targets.keys().cloned().collect();
            for target_id in target_ids {
                apply_target_event(
                    &mut guard,
                    &target_id,
                    ControlEvent::Tick { now_ms },
                    &keypair,
                    &kennel_id,
                );
            }
        }
        if let Err(error) = reconcile_retired_targets(
            &state,
            hive_comms_trust.as_deref(),
            &keypair,
            &kennel_id,
            hive_mob.as_ref().map(|(state, id)| (state, id)),
        )
        .await
        {
            eprintln!("[kennel] peer retirement failed (will retry): {error}");
        }
    }
}

// Call under peer_updates: registration and asynchronous trust removal must not
// overtake each other. Failed removals remain obligations for the janitor.
async fn reconcile_retired_targets(
    state: &Mutex<KennelState>,
    trust: Option<&ExampleGeneratedCommsTrustRouter>,
    keypair: &meerkat_comms::identity::Keypair,
    kennel_id: &str,
    hive_mob: Option<(&Arc<MobMcpState>, &MobId)>,
) -> anyhow::Result<()> {
    loop {
        let Some(retired) = state.lock().retired_targets.last().cloned() else {
            return Ok(());
        };
        if !state.lock().targets.contains_key(&retired.target_id) {
            let trust = trust.context("hive trust owner unavailable during retirement")?;
            let peer_id =
                meerkat_comms::identity::PubKey::from_pubkey_string(&retired.pubkey)?.to_peer_id();
            trust.remove_trusted_peer(&peer_id).await?;
            if let Some((mob_state, mob_id)) = hive_mob
                && let Err(error) = mob_state
                    .mob_retire(mob_id, AgentIdentity::from(retired.name.clone()))
                    .await
            {
                eprintln!(
                    "[kennel] experimental mob retirement {}: {error}",
                    retired.name
                );
            }
            let envelope = build_signed_envelope(
                keypair,
                kennel_id,
                KennelPayload::PeerUnwire {
                    peer_id: retired.pubkey,
                },
            )?;
            for survivor in state.lock().targets.values() {
                let _ = survivor.tx.send(envelope.clone());
            }
        }
        state.lock().retired_targets.pop();
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn find_flag(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

/// Resolve the externally-reachable IP for this host.
///
/// If `listen_host` is a wildcard (`0.0.0.0` or `::`), probe the default
/// route via a non-sending UDP connect to discover the LAN-facing IP.
/// Otherwise return `listen_host` as-is.
fn resolve_advertise_ip(listen_host: &str) -> anyhow::Result<String> {
    if listen_host == "0.0.0.0" || listen_host == "::" || listen_host.is_empty() {
        let sock = std::net::UdpSocket::bind("0.0.0.0:0").context("bind UDP probe socket")?;
        // Connect to a well-known external address (doesn't send any data).
        sock.connect("8.8.8.8:80")
            .context("UDP probe to discover local IP")?;
        Ok(sock.local_addr()?.ip().to_string())
    } else {
        Ok(listen_host.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mdm_tux::machines::kennel_lease::State as LeaseState;
    use meerkat_comms::identity::Keypair;

    #[test]
    fn teardown_is_unconditional_incarnation_fenced_and_unwires_retired_peers() {
        meerkat_runtime::host_stack::run_host("kennel-cleanup-test", || async {
            use tokio::io::AsyncWriteExt;
            let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
            let comms = meerkat_comms::ResolvedCommsConfig {
                enabled: true,
                name: "hive".into(),
                inproc_namespace: None,
                listen_tcp: None,
                listen_uds: None,
                advertise_address: None,
                event_listen_tcp: None,
                #[cfg(unix)]
                event_listen_uds: None,
                identity_dir: root.path().join("identity"),
                trusted_peers_path: root.path().join("peers.json"),
                comms_config: Default::default(),
                auth: Default::default(),
                require_peer_auth: true,
                allow_external_unauthenticated: false,
                pairing_password: None,
            };
            let host =
                mdm_tux::runtime::ManagedRpcHost::open(root.path(), Default::default(), comms)
                    .await
                    .unwrap();
            host.runtime.set_default_llm_client(Some(Arc::new(
                meerkat_client::TestClient::for_provider(meerkat_core::Provider::OpenAI),
            )));
            let mut build = meerkat::AgentBuildConfig::new("gpt-5.5");
            build.provider = Some(meerkat_core::Provider::OpenAI);
            let session_id = host
                .managed_session("hive", build, Vec::new())
                .await
                .unwrap();
            let trust = Arc::new(ExampleGeneratedCommsTrustRouter::new(
                host.runtime.runtime_adapter(),
                session_id.clone(),
                host.comms.clone(),
            ));
            let state = Arc::new(Mutex::new(KennelState::default()));
            let kennel_key = Arc::new(Keypair::generate());
            let target_key = Keypair::generate();
            let peer_key = Keypair::generate();
            let mut clients = Vec::new();
            let mut tasks = Vec::new();
            for (name, key) in [
                ("target", &target_key),
                ("survivor", &peer_key),
                ("target", &target_key),
            ] {
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let client = TcpStream::connect(listener.local_addr().unwrap())
                    .await
                    .unwrap();
                let (server, _) = listener.accept().await.unwrap();
                tasks.push(tokio::spawn(handle_connection(
                    server,
                    state.clone(),
                    kennel_key.clone(),
                    "kennel".into(),
                    HiveConnectionContext {
                        hive_rpc_addr: "tcp://127.0.0.1:1".into(),
                        hive_comms_addr: "tcp://127.0.0.1:2".into(),
                        hive_session_id: Some(session_id.to_string()),
                        hive_mob_state: host.runtime.mob_state().unwrap(),
                        hive_mob_id: None,
                        hive_comms_trust: Some(trust.clone()),
                        hive_comms_runtime: host.comms.clone(),
                    },
                )));
                let (reader, mut writer) = client.into_split();
                let mut reader = BufReader::new(reader);
                write_envelope(
                    &mut writer,
                    &build_signed_envelope(
                        key,
                        "",
                        KennelPayload::TargetRegister {
                            target_id: key.public_key().to_peer_id().to_string(),
                            name: name.into(),
                            pubkey: key.public_key().to_pubkey_string(),
                            direct_addr: "tcp://127.0.0.1:3".into(),
                            rpc_addr: None,
                            labels: Default::default(),
                            capabilities: Default::default(),
                            attached_tux_id: None,
                        },
                    )
                    .unwrap(),
                )
                .await
                .unwrap();
                let reply = tokio::time::timeout(
                    std::time::Duration::from_secs(10),
                    read_envelope(&mut reader),
                )
                .await
                .unwrap()
                .unwrap();
                let Some(reply) = reply else {
                    panic!(
                        "registration closed: {:?}",
                        tasks.pop().unwrap().await.unwrap()
                    );
                };
                assert!(matches!(
                    reply.payload,
                    KennelPayload::TargetRegistered { .. }
                ));
                clients.push((reader, writer));
            }
            let registration_switch_checks = async {
            for (index, (first_target, second_target, change_key)) in [
                (true, true, true),
                (false, false, true),
                (true, false, false),
                (false, true, false),
                (true, true, false),
                (false, false, false),
            ]
            .into_iter()
            .enumerate()
            {
                let first_key = Keypair::generate();
                let other_key = Keypair::generate();
                let second_key = if change_key { &other_key } else { &first_key };
                let registration = |key: &Keypair, target: bool| {
                    if target {
                        KennelPayload::TargetRegister {
                            target_id: key.public_key().to_peer_id().to_string(),
                            name: format!("registration-{index}"),
                            pubkey: key.public_key().to_pubkey_string(),
                            direct_addr: "tcp://127.0.0.1:3".into(),
                            rpc_addr: None,
                            labels: Default::default(),
                            capabilities: Default::default(),
                            attached_tux_id: None,
                        }
                    } else {
                        KennelPayload::TuxRegister {
                            tux_id: key.public_key().to_peer_id().to_string(),
                            pubkey: key.public_key().to_pubkey_string(),
                            attached_target_ids: Vec::new(),
                        }
                    }
                };
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let connection = TcpStream::connect(listener.local_addr().unwrap()).await.unwrap();
                let (socket, _) = listener.accept().await.unwrap();
                let task = tokio::spawn(handle_connection(
                    socket, state.clone(), kennel_key.clone(), "kennel".into(),
                    HiveConnectionContext {
                        hive_rpc_addr: "tcp://127.0.0.1:1".into(),
                        hive_comms_addr: "tcp://127.0.0.1:2".into(),
                        hive_session_id: Some(session_id.to_string()),
                        hive_mob_state: host.runtime.mob_state().unwrap(),
                        hive_mob_id: None,
                        hive_comms_trust: Some(trust.clone()),
                        hive_comms_runtime: host.comms.clone(),
                    },
                ));
                let (reader, mut writer) = connection.into_split();
                let mut reader = BufReader::new(reader);
                for (attempt, (key, target)) in
                    [(&first_key, first_target), (second_key, second_target)].into_iter().enumerate()
                {
                    write_envelope(&mut writer, &build_signed_envelope(
                        key, "", registration(key, target),
                    ).unwrap()).await.unwrap();
                    if attempt == 1 && (change_key || first_target != second_target) {
                        break;
                    }
                    tokio::time::timeout(std::time::Duration::from_secs(5), async {
                        loop {
                            let reply = read_envelope(&mut reader).await.unwrap().unwrap();
                            if matches!(reply.payload, KennelPayload::TargetRegistered { .. })
                                || matches!(reply.payload, KennelPayload::TuxRegistered { .. })
                            {
                                break;
                            }
                        }
                    }).await.unwrap();
                }
                let rejected = change_key || first_target != second_target;
                if !rejected {
                    writer.shutdown().await.unwrap();
                }
                let result = tokio::time::timeout(std::time::Duration::from_secs(5), task)
                    .await.unwrap().unwrap();
                assert_eq!(result.is_err(), rejected, "registration case {index}: {result:?}");
                for key in [&first_key, second_key] {
                    let id = key.public_key().to_peer_id().to_string();
                    assert!(!state.lock().targets.contains_key(&id));
                    assert!(!state.lock().tuxes.contains_key(&id));
                    assert!(!host.comms.trusted_peers_shared().contains(&key.public_key().to_peer_id()));
                }
            }
            };
            // Invalid input on the obsolete socket must not disconnect the replacement.
            clients[0].1.write_all(b"not-json\n").await.unwrap();
            assert!(tasks.remove(0).await.unwrap().is_err());
            let id = target_key.public_key().to_peer_id().to_string();
            assert!(state.lock().targets.contains_key(&id));
            assert_eq!(
                host.runtime
                    .runtime_adapter()
                    .direct_peer_endpoints(&session_id)
                    .await
                    .unwrap()
                    .len(),
                2
            );
            assert!(host.comms.trusted_peers_shared().contains(&target_key.public_key().to_peer_id()));

            // Signature verification failure on the current socket still retires it.
            let mut invalid =
                build_signed_envelope(&target_key, "", KennelPayload::TargetHeartbeat).unwrap();
            invalid.signature = "invalid".into();
            write_envelope(&mut clients[2].1, &invalid).await.unwrap();
            assert!(tasks.pop().unwrap().await.unwrap().is_err());
            assert!(!state.lock().targets.contains_key(&id));
            assert_eq!(
                host.runtime
                    .runtime_adapter()
                    .direct_peer_endpoints(&session_id)
                    .await
                    .unwrap()
                    .len(),
                1
            );
            assert!(!host.comms.trusted_peers_shared().contains(&target_key.public_key().to_peer_id()));
            loop {
                let envelope = tokio::time::timeout(
                    std::time::Duration::from_secs(10),
                    read_envelope(&mut clients[1].0),
                )
                .await
                .unwrap()
                .unwrap()
                .unwrap();
                if let KennelPayload::PeerUnwire { peer_id } = envelope.payload {
                    assert_eq!(peer_id, target_key.public_key().to_pubkey_string());
                    break;
                }
            }
            // Re-enroll the retired name with a new identity, then fail its
            // transport with a real TCP RST rather than a protocol error.
            let replacement_key = Keypair::generate();
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let envelope = build_signed_envelope(
                &replacement_key, "",
                KennelPayload::TargetRegister {
                    target_id: replacement_key.public_key().to_peer_id().to_string(),
                    name: "target".into(),
                    pubkey: replacement_key.public_key().to_pubkey_string(),
                    direct_addr: "tcp://127.0.0.1:4".into(), rpc_addr: None,
                    labels: Default::default(), capabilities: Default::default(),
                    attached_tux_id: None,
                },
            ).unwrap();
            let mut resetter = tokio::process::Command::new("python3")
                .args(["-c", "import socket,struct,sys\ns=socket.create_connection(('127.0.0.1',int(sys.argv[1])))\ns.sendall((sys.argv[2]+'\\n').encode())\nreply=b''\nwhile b'\\n' not in reply: reply+=s.recv(4096)\ns.setsockopt(socket.SOL_SOCKET,socket.SO_LINGER,struct.pack('ii',1,0))\ns.close()"])
                .arg(listener.local_addr().unwrap().port().to_string())
                .arg(serde_json::to_string(&envelope).unwrap())
                .kill_on_drop(true).spawn().unwrap();
            let (server, _) = listener.accept().await.unwrap();
            let reset_handler = tokio::spawn(handle_connection(
                server, state.clone(), kennel_key.clone(), "kennel".into(),
                HiveConnectionContext {
                    hive_rpc_addr: "tcp://127.0.0.1:1".into(),
                    hive_comms_addr: "tcp://127.0.0.1:2".into(),
                    hive_session_id: Some(session_id.to_string()),
                    hive_mob_state: host.runtime.mob_state().unwrap(),
                    hive_mob_id: None,
                    hive_comms_trust: Some(trust.clone()),
                    hive_comms_runtime: host.comms.clone(),
                },
            ));
            assert!(resetter.wait().await.unwrap().success());
            assert!(tokio::time::timeout(std::time::Duration::from_secs(10), reset_handler)
                .await.unwrap().unwrap().is_err());
            assert_eq!(state.lock().targets.len(), 1);
            assert!(!host.comms.trusted_peers_shared().contains(&replacement_key.public_key().to_peer_id()));
            for (_, writer) in &mut clients {
                let _ = writer.shutdown().await;
            }
            for task in tasks {
                task.await.unwrap().unwrap();
            }
            drop(clients);
            registration_switch_checks.await;
            host.shutdown().await.unwrap();
        })
        .unwrap();
    }

    #[test]
    fn claim_ack_transitions_directly_to_claimed() {
        let keypair = Keypair::generate();
        let kennel_id = keypair.public_key().to_peer_id().to_string();
        let (tx, _rx) = mpsc::unbounded_channel();

        let mut state = KennelState::default();
        state.targets.insert(
            "target-1".into(),
            TargetRecord {
                target_id: "target-1".into(),
                name: "target-1".into(),
                pubkey: "ed25519:target-1".into(),
                direct_addr: "tcp://1.2.3.4:9000".into(),
                rpc_addr: None,
                labels: BTreeMap::new(),
                capabilities: BTreeMap::new(),
                tx: tx.clone(),
                control_state: ControlState {
                    connected: true,
                    lease: LeaseState::AwaitingAck {
                        target_id: "target-1".into(),
                        lease_id: "lease-1".into(),
                        tux_id: "tux-1".into(),
                        expires_at_ms: chrono::Utc::now().timestamp_millis() + 10_000,
                        ack_deadline_ms: chrono::Utc::now().timestamp_millis() + 5_000,
                    },
                },
            },
        );
        state.tuxes.insert(
            "tux-1".into(),
            TuxRecord {
                tux_id: "tux-1".into(),
                pubkey: "ed25519:tux-1".into(),
                tx,
            },
        );

        handle_claim_ack(
            &mut state,
            &keypair,
            &kennel_id,
            "tux-1",
            &["lease-1".into()],
        );

        assert!(matches!(
            state.targets.get("target-1").map(|target| &target.control_state.lease),
            Some(LeaseState::Claimed {
                target_id,
                lease_id,
                tux_id,
                ..
            }) if target_id == "target-1" && lease_id == "lease-1" && tux_id == "tux-1"
        ));
    }
}
