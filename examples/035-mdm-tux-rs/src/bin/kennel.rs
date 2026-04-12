use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;
use mdm_tux::machines::kennel_lease;
use mdm_tux::machines::kennel_target_control::{
    self, Effect as ControlEffect, Event as ControlEvent, State as ControlState,
};
use mdm_tux::{
    ClaimGrant, KennelPayload, KennelTargetState, LeaseTerminationReason, LeaseView, ListScope,
    SignedKennelEnvelope, TargetListEntry, TargetRegistrationRejectReason, build_signed_envelope,
    load_or_generate_keypair, read_envelope, verify_envelope, write_envelope,
};
use meerkat_mob::{
    MeerkatId, MobBackendKind, MobDefinition, MobId, MobRuntimeMode, Profile, ProfileBinding,
    ProfileName, RuntimeBinding, SpawnMemberSpec, ToolConfig,
};
use meerkat_mob::definition::{BackendConfig, ExternalBackendConfig, SessionCleanupPolicy, WiringRules};
use meerkat_mob_mcp::{AgentMobToolSurfaceFactory, MobMcpState};
use meerkat_store::{JsonlStore, MemoryBlobStore, SessionStore};
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

#[derive(Default)]
struct KennelState {
    targets: HashMap<String, TargetRecord>,
    tuxes: HashMap<String, TuxRecord>,
}

// ── Main ─────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "warn".into()))
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        eprintln!("Usage: mdm-kennel --listen HOST:PORT [--data-dir PATH] [--hive-rpc-port PORT]");
        std::process::exit(1);
    }
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
    let kennel_id = keypair.public_key().to_peer_id();

    let listener = TcpListener::bind(&listen)
        .await
        .with_context(|| format!("bind kennel listener at {listen}"))?;
    let state = Arc::new(Mutex::new(KennelState::default()));

    // Resolve the externally-reachable IP for addresses advertised to TUX
    // and targets. If --listen is 0.0.0.0:PORT, probe the default route.
    let kennel_host = listen.rsplit_once(':').map(|(h, _)| h).unwrap_or(&listen);
    let advertise_ip = find_flag(&args, "--advertise")
        .unwrap_or_else(|| resolve_advertise_ip(kennel_host)
            .unwrap_or_else(|_| kennel_host.to_string()));

    // ── Hive agent: CommsRuntime ────────────────────────────────────────────
    let hive_comms_config = meerkat_comms::ResolvedCommsConfig {
        enabled: true,
        name: "hive".to_string(),
        inproc_namespace: None,
        listen_tcp: None,
        listen_uds: None,
        event_listen_tcp: None,
        #[cfg(unix)]
        event_listen_uds: None,
        identity_dir: data_dir.join("hive_identity"),
        trusted_peers_path: data_dir.join("hive_trusted_peers.json"),
        comms_config: Default::default(),
        auth: Default::default(),
        require_peer_auth: true,
        allow_external_unauthenticated: false,
    };
    let mut hive_comms_runtime = meerkat_comms::CommsRuntime::new(hive_comms_config)
        .await
        .map_err(|e| anyhow::anyhow!("hive comms runtime: {e}"))?;
    hive_comms_runtime.set_blob_store(Arc::new(MemoryBlobStore::new()));
    let hive_comms_runtime = Arc::new(hive_comms_runtime);
    let hive_comms_port = {
        let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
        let local_addr = listener.local_addr()?;
        let kp = hive_comms_runtime.router_arc().keypair_arc();
        let tp = hive_comms_runtime.trusted_peers_shared();
        let inbox = hive_comms_runtime.router_arc().inbox_sender().clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let (kp, tp, sender) = (kp.clone(), tp.clone(), inbox.clone());
                tokio::spawn(async move {
                    let snapshot = tp.read().clone();
                    let _ = meerkat_comms::handle_connection(
                        stream, true, &kp, &snapshot, &sender,
                    )
                    .await;
                });
            }
        });
        local_addr.port()
    };

    // ── Hive agent: session directory & persistence ─────────────────────────
    let hive_dir = data_dir.join("hive");
    tokio::fs::create_dir_all(&hive_dir).await?;
    let session_dir = hive_dir.join("sessions");
    tokio::fs::create_dir_all(&session_dir).await?;

    // Add a sentinel peer so comms tools are never gated out at build time.
    // Real peers are added dynamically when targets register.
    hive_comms_runtime.upsert_trusted_peer(meerkat_comms::TrustedPeer {
        name: "__sentinel__".into(),
        pubkey: hive_comms_runtime.public_key(),
        addr: "inproc://sentinel".into(),
        meta: meerkat_comms::PeerMeta::default(),
    });

    // ── Hive agent: AgentFactory + Config ───────────────────────────────────
    let hive_factory = meerkat::AgentFactory::new(&session_dir)
        .shell(true)
        .builtins(true)
        .comms(true)
        .schedule(true)
        .mob(true)
        .with_comms_runtime(Arc::clone(&hive_comms_runtime));
    let home = dirs::home_dir();
    let hive_config = meerkat_core::Config::load_from(&session_dir, home.as_deref())
        .await
        .unwrap_or_default();

    // ── Hive agent: persistence stores ──────────────────────────────────────
    let hive_schedule_store = Arc::new(meerkat::SqliteScheduleStore::open(
        session_dir.join("hive_schedule.sqlite"),
    )?) as Arc<dyn meerkat::ScheduleStore>;
    let hive_jsonl = Arc::new(JsonlStore::new(session_dir.to_path_buf()));
    hive_jsonl.init().await?;
    let hive_persistence = meerkat::PersistenceBundle::new_with_schedule_store(
        hive_jsonl as Arc<dyn SessionStore>,
        None,
        Arc::new(MemoryBlobStore::new()),
        hive_schedule_store,
    );

    // ── Hive agent: SessionRuntime with mob tools ───────────────────────────
    let hive_config_store: Arc<dyn meerkat_core::ConfigStore> =
        Arc::new(meerkat_core::MemoryConfigStore::new(hive_config.clone()));
    let mut hive_runtime = meerkat_rpc::session_runtime::SessionRuntime::new(
        hive_factory,
        hive_config,
        1024,
        hive_persistence,
        meerkat_rpc::router::NotificationSink::noop(),
    );
    let hive_mob_state = Arc::new(MobMcpState::new_with_runtime_adapter(
        hive_runtime.session_service(),
        Some(hive_runtime.runtime_adapter()),
    ));
    let hive_mob_state_for_kennel = Arc::clone(&hive_mob_state);
    hive_runtime.set_mob_tools(Arc::new(AgentMobToolSurfaceFactory::new(
        Arc::clone(&hive_mob_state),
    )));
    hive_runtime.set_mob_state(hive_mob_state);
    hive_runtime.set_config_runtime(Arc::new(meerkat_core::ConfigRuntime::new(
        Arc::clone(&hive_config_store),
        session_dir.join("hive_config_state.json"),
    )));
    let hive_runtime = Arc::new(hive_runtime);

    // ── Hive agent: RPC TCP server ──────────────────────────────────────────
    let hive_rpc_port: u16 = find_flag(&args, "--hive-rpc-port")
        .and_then(|p| p.parse().ok())
        .unwrap_or(4801);
    {
        let hive_runtime_clone = Arc::clone(&hive_runtime);
        let hive_config_store_clone = Arc::clone(&hive_config_store);
        tokio::spawn(async move {
            let addr = format!("0.0.0.0:{hive_rpc_port}");
            if let Err(e) = meerkat_rpc::serve_tcp(
                &addr,
                hive_runtime_clone,
                hive_config_store_clone,
                None,
            )
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
        let existing = hive_runtime.list_sessions(Default::default()).await;
        let resumed_id = existing
            .into_iter()
            .next()
            .map(|s| s.session_id.to_string());
        if let Some(ref sid) = resumed_id {
            eprintln!("[kennel] hive session resumed: {sid}");
            resumed_id
        } else {
            let mut build = meerkat::AgentBuildConfig::new("gpt-5.4".to_string());
            build.system_prompt = Some(
                "You are the hive orchestrator for a fleet of managed target agents.\n\
                 Use the 'peers' tool to discover which targets are connected.\n\
                 Use 'send_request' to dispatch tasks to targets and collect responses.\n\
                 Use 'send_message' for fire-and-forget notifications.\n\
                 Always check peers first to see who is available before dispatching work."
                    .to_string(),
            );
            build.override_builtins = meerkat_core::ToolCategoryOverride::Enable;
            build.override_shell = meerkat_core::ToolCategoryOverride::Enable;
            build.override_mob = meerkat_core::ToolCategoryOverride::Enable;
            match hive_runtime.create_session(build, None, None).await {
                Ok(sid) => {
                    let sid_str = sid.to_string();
                    eprintln!("[kennel] hive session created: {sid_str}");
                    // Verify the session is accessible
                    let sessions = hive_runtime.list_sessions(Default::default()).await;
                    eprintln!("[kennel] sessions after create: {}", sessions.len());
                    for s in &sessions {
                        eprintln!("[kennel]   session: {} state={:?}", s.session_id, s.state);
                    }
                    Some(sid_str)
                }
                Err(e) => {
                    eprintln!("[kennel] failed to create hive session: {e:?}");
                    None
                }
            }
        }
    };

    // ── Enable comms drain on hive session ────────────────────────────────
    // The hive session uses the factory's shared comms runtime (no comms_name),
    // so keep_alive / update_peer_ingress_context can't be set through the
    // normal turn/start path. Wire the drain explicitly so the hive receives
    // incoming peer requests/responses between turns.
    if let Some(ref sid) = hive_session_id {
        let uuid = uuid::Uuid::parse_str(sid).expect("hive session id is valid uuid");
        let session_id = meerkat_core::types::SessionId(uuid);
        hive_runtime
            .enable_comms_drain(
                &session_id,
                Arc::clone(&hive_comms_runtime) as Arc<dyn meerkat_core::agent::CommsRuntime>,
            )
            .await;
        eprintln!("[kennel] hive comms drain enabled for {sid}");
    }

    // ── Create hive mob (external backend) ────────────────────────────────
    let hive_mob_id: Option<MobId> = {
        let mut profiles = BTreeMap::new();
        profiles.insert(
            ProfileName::from("target"),
            ProfileBinding::Inline(Profile {
                model: "gpt-5.4".to_string(),
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
            }),
        );

        let definition = MobDefinition {
            id: MobId::from("hive-fleet"),
            orchestrator: None,
            profiles,
            mcp_servers: BTreeMap::new(),
            wiring: WiringRules {
                auto_wire_orchestrator: true,
                role_wiring: Vec::new(),
            },
            skills: BTreeMap::new(),
            backend: BackendConfig {
                default: MobBackendKind::External,
                external: Some(ExternalBackendConfig {
                    address_base: format!("tcp://{advertise_ip}:{hive_comms_port}"),
                }),
            },
            flows: BTreeMap::new(),
            topology: None,
            supervisor: None,
            limits: None,
            spawn_policy: None,
            event_router: None,
            owner_session_id: None,
            session_cleanup_policy: SessionCleanupPolicy::Manual,
            is_implicit: false,
        };

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
    };

    let hive_rpc_addr = format!("tcp://{advertise_ip}:{hive_rpc_port}");
    let hive_comms_addr = format!("tcp://{advertise_ip}:{hive_comms_port}");

    println!("=== MCM Kennel ===");
    println!("listen    : {listen}");
    println!("kennel_id : {kennel_id}");
    println!("advertise : {advertise_ip}");
    println!("hive_rpc  : {hive_rpc_addr}");
    println!("hive_comms: {hive_comms_addr}");
    if let Some(mob_id) = &hive_mob_id {
        println!("hive_mob  : {mob_id}");
    }

    tokio::spawn(run_janitor(
        state.clone(),
        keypair.clone(),
        kennel_id.clone(),
    ));

    loop {
        let (stream, _) = listener.accept().await?;
        let state = state.clone();
        let keypair = keypair.clone();
        let kennel_id = kennel_id.clone();
        let hive_rpc_addr = hive_rpc_addr.clone();
        let hive_comms_addr_c = hive_comms_addr.clone();
        let hive_sid = hive_session_id.clone();
        let mob_state = Arc::clone(&hive_mob_state_for_kennel);
        let mob_id = hive_mob_id.clone();
        let hive_comms = Arc::clone(&hive_comms_runtime);
        tokio::spawn(async move {
            if let Err(e) = handle_connection(
                stream,
                state,
                keypair,
                kennel_id,
                hive_rpc_addr,
                hive_comms_addr_c,
                hive_sid,
                mob_state,
                mob_id,
                hive_comms,
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

async fn handle_connection(
    stream: TcpStream,
    state: Arc<Mutex<KennelState>>,
    keypair: Arc<meerkat_comms::identity::Keypair>,
    kennel_id: String,
    hive_rpc_addr: String,
    hive_comms_addr: String,
    hive_session_id: Option<String>,
    hive_mob_state: Arc<MobMcpState>,
    hive_mob_id: Option<MobId>,
    hive_comms_runtime: Arc<meerkat_comms::CommsRuntime>,
) -> anyhow::Result<()> {
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

    loop {
        let Some(env) = read_envelope(&mut reader).await? else {
            break;
        };
        let signer = verify_envelope(&env)?;
        let signer_id = signer.to_peer_id();

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
                anyhow::ensure!(target_id == &signer_id, "target signer_id mismatch");
                anyhow::ensure!(pubkey == &signer_id, "target pubkey mismatch");
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
                        let reply = build_signed_envelope(
                            &keypair,
                            &kennel_id,
                            KennelPayload::TargetRegistered {
                                hive_pubkey: Some(hive_comms_runtime.public_key().to_peer_id()),
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
                        session_kind = Some(SessionKind::Target(target_id.clone()));

                        // Always update trusted peer on (re-)registration so the
                        // hive has the target's current comms address.
                        if let Ok(pk) = meerkat_comms::identity::PubKey::from_peer_id(pubkey) {
                            hive_comms_runtime.upsert_trusted_peer(
                                meerkat_comms::TrustedPeer {
                                    name: name.clone(),
                                    pubkey: pk,
                                    addr: direct_addr.clone(),
                                    meta: meerkat_comms::PeerMeta::default(),
                                },
                            );
                        }

                        // Spawn target as external mob member in the hive fleet.
                        // RuntimeBinding::External carries the real target identity
                        // so the mob roster has the correct peer_id and address.
                        if let Some(mob_id) = &hive_mob_id {
                            let mut spec = SpawnMemberSpec::new(
                                ProfileName::from("target"),
                                MeerkatId::from(name.clone()),
                            );
                            spec.binding = Some(RuntimeBinding::External {
                                peer_id: pubkey.clone(),
                                address: direct_addr.clone(),
                            });
                            match hive_mob_state
                                .mob_spawn_spec(mob_id, spec)
                                .await
                            {
                                Ok(_) => {
                                    eprintln!(
                                        "[kennel] spawned {name} as hive mob member"
                                    );
                                }
                                Err(e) => {
                                    eprintln!(
                                        "[kennel] mob spawn {name}: {e} (peer still updated)"
                                    );
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

            KennelPayload::TuxRegister {
                tux_id,
                pubkey,
                ..
            } => {
                anyhow::ensure!(tux_id == &signer_id, "tux signer_id mismatch");
                anyhow::ensure!(pubkey == &signer_id, "tux pubkey mismatch");
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
                    KennelPayload::ClaimGranted { claims },
                )?;
                let _ = tx.send(reply);
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
                eprintln!("[kennel] hive prompt via control channel (redirecting to RPC): {prompt}");
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

    writer_task.abort();

    if let Some(kind) = session_kind {
        // Look up the target name before taking the lock for mob retire.
        let target_name_for_retire = match &kind {
            SessionKind::Target(target_id) => {
                let guard = state.lock();
                guard.targets.get(target_id).map(|t| t.name.clone())
            }
            _ => None,
        };

        {
            let mut guard = state.lock();
            match &kind {
                SessionKind::Target(target_id) => {
                    handle_target_disconnect(&mut guard, &keypair, &kennel_id, target_id);
                }
                SessionKind::Tux(tux_id) => {
                    handle_tux_disconnect(&mut guard, &keypair, &kennel_id, tux_id);
                }
            }
        }

        // Retire the target from the hive mob (async, outside the lock).
        if let (SessionKind::Target(_), Some(mob_id), Some(name)) =
            (&kind, &hive_mob_id, target_name_for_retire)
        {
            let member_id = MeerkatId::from(name.clone());
            match hive_mob_state.mob_retire(mob_id, member_id).await {
                Ok(()) => eprintln!("[kennel] retired {name} from hive mob"),
                Err(e) => eprintln!("[kennel] failed to retire {name} from hive mob: {e}"),
            }
        }
    }

    Ok(())
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
            existing.name = name;
            existing.pubkey = pubkey;
            existing.direct_addr = direct_addr;
            existing.rpc_addr = rpc_addr;
            existing.labels = labels;
            existing.capabilities = capabilities;
            existing.tx = tx;
            let (new_state, effects) = kennel_target_control::transition(
                existing.control_state.clone(),
                ControlEvent::Registered {
                    attached_tux_id,
                    now_ms,
                    recovery_window_ms: RECOVERY_WINDOW_MS,
                },
            )
            .map_err(|e| anyhow::anyhow!("target re-register transition: {e}"))?;
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
                state.targets.remove(effect_tid);
            }
        }
    }
}

// ── Janitor ──────────────────────────────────────────────────────────────────

async fn run_janitor(
    state: Arc<Mutex<KennelState>>,
    keypair: Arc<meerkat_comms::identity::Keypair>,
    kennel_id: String,
) {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let now_ms = chrono::Utc::now().timestamp_millis();
        let mut guard = state.lock();

        // Collect target IDs that need tick processing (avoid borrow issues)
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

        // Remove phantom targets: disconnected records that expired back to
        // Available. These have a closed tx (dead TCP connection) and must
        // not appear in listings or accept new claims.
        guard.targets.retain(|_, target| {
            if matches!(
                target.control_state.lease,
                kennel_lease::State::Available { .. }
            ) && target.tx.is_closed()
            {
                false // remove phantom
            } else {
                true
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mdm_tux::machines::kennel_lease::State as LeaseState;
    use meerkat_comms::identity::Keypair;

    #[test]
    fn claim_ack_transitions_directly_to_claimed() {
        let keypair = Keypair::generate();
        let kennel_id = keypair.public_key().to_peer_id();
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
                        expires_at_ms: 10_000,
                        ack_deadline_ms: 5_000,
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

        handle_claim_ack(&mut state, &keypair, &kennel_id, "tux-1", &["lease-1".into()]);

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
        let sock = std::net::UdpSocket::bind("0.0.0.0:0")
            .context("bind UDP probe socket")?;
        // Connect to a well-known external address (doesn't send any data).
        sock.connect("8.8.8.8:80")
            .context("UDP probe to discover local IP")?;
        Ok(sock.local_addr()?.ip().to_string())
    } else {
        Ok(listen_host.to_string())
    }
}
