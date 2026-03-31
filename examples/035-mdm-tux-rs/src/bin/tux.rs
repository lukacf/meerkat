//! # 035 — MDM TUX: Controller TUI
//!
//! Ratatui-based terminal UI for managing remote Meerkat agents.
//!
//! ```text
//! mcm-tux <PORT> [--model MODEL]
//! ```
//!
//! ## Modes
//! - **Direct** (default): true interactive control of a target agent. Messages
//!   are turns on the target's session (history accumulates). Streaming events
//!   (text deltas, tool calls) display in real-time. No API key needed on the
//!   TUX side.
//! - **Hive** (Tab): a local LLM agent fans commands out to any/all targets.
//!   Requires an API key.
//!
//! ## Keys
//! Tab=mode  ↑/↓=target  PgUp/PgDn=scroll  End=auto-scroll  Esc=quit

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind, KeyModifiers,
};
use meerkat::{AgentBuilder, AgentEvent, AgentFactory, DynAgent};
use meerkat_comms::MessageKind;
use meerkat_comms::agent::CommsToolDispatcher;
use meerkat_comms::agent::types::CommsContent;
use meerkat_core::{AgentSessionStore, AgentToolDispatcher};
use meerkat_store::{JsonlStore, StoreAdapter};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use tokio::sync::{mpsc, oneshot};

use mdm_tux::machines::tux_claim::{
    self, Effect as ClaimEffect, Event as ClaimEvent, State as ClaimState,
};
use mdm_tux::{
    ClaimGrant, CommsNode, KennelPayload, KennelTargetState, ListScope, STREAM_PREFIX,
    TargetListEntry, auto_detect, build_llm_client, build_signed_envelope, detect_provider,
    load_or_generate_keypair, read_envelope, run_registration_server, verify_envelope,
    write_envelope,
};
use tokio::io::BufReader;
use tokio::net::TcpStream;

/// Timeout for router.send() calls. Prevents a black-holed target from wedging
/// the command loop (TCP connect can hang for minutes without this).
const SEND_TIMEOUT: Duration = Duration::from_secs(10);
const LOCAL_ATTACH_TIMEOUT: Duration = Duration::from_secs(20);

// ── Event / command types ─────────────────────────────────────────────────────

enum AppCommand {
    Send {
        target: String,
        body: String,
    },
    RunHive {
        prompt: String,
    },
    /// Slash command sent to a target (no busy indicator set).
    SlashCmd {
        target: String,
        cmd: String,
    },
    /// Drop the cached hive agent so the next RunHive starts fresh.
    ResetHive,
    ClaimTarget {
        target_id: String,
    },
    ReleaseTarget {
        lease_id: String,
    },
}

enum KennelClientCommand {
    ClaimTarget { target_id: String },
    ClaimAck { lease_id: String },
    ReleaseTarget { lease_id: String },
    AttachConfirmed { lease_id: String },
    Shutdown { reply: oneshot::Sender<()> },
}

enum TuiEvent {
    CommsMessage(String),
    Heartbeat {
        from: String,
    },
    HivePlanDone(String),
    HiveError(String),
    SendError(String),
    TargetRegistered {
        name: String,
    },
    /// Streaming agent event from a target (text delta, tool call, etc.).
    StreamEvent {
        from: String,
        event: AgentEvent,
    },
    KennelAvailableTargets(Vec<TargetListEntry>),
    KennelMineTargets(Vec<TargetListEntry>),
    KennelClaimGranted(Vec<ClaimGrant>),
    KennelClaimReleased {
        target_id: String,
        lease_id: String,
        reason: String,
    },
    KennelAttached {
        target_name: String,
        lease_id: String,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TransportMode {
    Direct,
    Kennel,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum KennelUiState {
    Available,
    Attaching,
    ClaimedByMe,
    ClaimUncertain,
}

type ClaimRegistry = parking_lot::RwLock<HashMap<String, ClaimState>>;

// ── TUI application state ─────────────────────────────────────────────────────

#[derive(PartialEq, Clone, Copy)]
enum Mode {
    Direct,
    Hive,
}

struct App {
    transport: TransportMode,
    mode: Mode,
    targets: Vec<String>,
    target_ids: HashMap<String, String>,
    target_states: HashMap<String, KennelUiState>,
    target_leases: HashMap<String, String>,
    attaching_since: HashMap<String, Instant>,
    selected: usize,
    input: String,
    /// Completed output lines (ring buffer, max 1000).
    output: VecDeque<String>,
    hive_planning: bool,
    quit: bool,
    /// Targets with an in-flight agent run.
    busy_targets: HashSet<String>,
    /// Live streaming text per target (not yet in `output`).
    /// BTreeMap for deterministic render order across frames.
    streaming_text: BTreeMap<String, String>,
    /// Last time we received any activity from each target.
    last_seen: HashMap<String, Instant>,
    /// Scroll state for the output panel.
    scroll_offset: u16,
    auto_scroll: bool,
}

impl App {
    fn push(&mut self, line: String) {
        // Split on newlines so each visual line is a separate entry.
        for l in line.split('\n') {
            self.output.push_back(l.to_string());
            if self.output.len() > 1000 {
                self.output.pop_front();
            }
        }
    }

    fn set_busy(&mut self, target: &str, busy: bool) {
        if busy {
            self.busy_targets.insert(target.to_string());
        } else {
            self.busy_targets.remove(target);
        }
    }

    /// Flush streaming text for a target into the output buffer.
    fn flush_streaming(&mut self, target: &str) {
        if let Some(text) = self.streaming_text.remove(target)
            && !text.is_empty()
        {
            self.push(format!("[{target}] {text}"));
        }
    }

    /// The label prefix shown before the input text (e.g. `[target] > `).
    fn input_label(&self) -> String {
        match self.mode {
            Mode::Direct if !self.targets.is_empty() => {
                let t = &self.targets[self.selected];
                if self.transport == TransportMode::Kennel
                    && !matches!(self.target_states.get(t), Some(KennelUiState::ClaimedByMe))
                {
                    format!("[{t} unavailable] > ")
                } else if self.busy_targets.contains(t) {
                    format!("[{t} ...processing] > ")
                } else {
                    format!("[{t}] > ")
                }
            }
            Mode::Hive if self.hive_planning => "[hive: planning...] > ".into(),
            Mode::Hive => "[hive] > ".into(),
            _ => "[waiting for targets...] > ".into(),
        }
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "warn,tui_markdown=error".into()),
        )
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        eprintln!("Usage: mcm-tux <PORT> [--model MODEL]");
        eprintln!(
            "   or: mcm-tux --kennel HOST:PORT --listen PORT [--advertise IP] [--model MODEL]"
        );
        eprintln!("Direct mode: no API key needed");
        eprintln!("Hive mode: set ANTHROPIC_API_KEY, OPENAI_API_KEY, or GEMINI_API_KEY");
        std::process::exit(1);
    }
    if find_flag(&args, "--kennel").is_some() {
        return run_kennel_tux(&args).await;
    }
    let port: u16 = args[0].parse().context("PORT must be a number")?;
    anyhow::ensure!(
        port < 65535,
        "PORT must be < 65535 (registration uses PORT+1)"
    );
    let model_override = find_flag(&args, "--model");

    let data_dir = find_flag(&args, "--data-dir")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".rkat/mdm/tux")
        });

    // ── 1. Load or generate TUX identity ─────────────────────────────────────
    let keypair = load_or_generate_keypair(&data_dir.join("identity")).await?;

    // ── 2. Create comms node ──────────────────────────────────────────────────
    let mut node = CommsNode::new(keypair);
    let comms_addr = format!("0.0.0.0:{port}");
    let _ = node.listen(&comms_addr).await?;

    println!("=== TUX — Meerkat Device Manager ===");
    println!("comms     : tcp://0.0.0.0:{port}");
    println!("register  : tcp://0.0.0.0:{}", port + 1);
    if let Some(ref m) = model_override {
        println!("hive model: {m} ({})", detect_provider(m));
    } else if let Some((ref m, ref p, _)) = auto_detect() {
        println!("hive model: {m} ({p})");
    } else {
        println!("hive model: (none — Direct mode only)");
    }
    println!("\nStart targets with: mcm-target <THIS_IP>:{port}\n");

    // ── 3. Channels ───────────────────────────────────────────────────────────
    let (command_tx, mut command_rx) = mpsc::unbounded_channel::<AppCommand>();
    let (event_tx, event_rx) = mpsc::channel::<TuiEvent>(256);
    let (reg_target_tx, mut reg_target_rx) = mpsc::channel::<(String, String)>(32);

    // ── 4. Spawn registration server (port + 1) ──────────────────────────────
    let reg_addr = format!("0.0.0.0:{}", port + 1);
    let reg_pubkey = node.pubkey_string();
    let reg_trusted = node.trusted.clone();
    tokio::spawn(async move {
        if let Err(e) =
            run_registration_server(reg_addr, reg_pubkey, reg_trusted, reg_target_tx).await
        {
            eprintln!("[tux] registration server error: {e}");
        }
    });

    // ── 5. Spawn comms drain task ─────────────────────────────────────────────
    // Use recv_raw() + require_peer_auth=false to accept all signed messages
    // from dynamically registered targets.
    let router = node.router.clone();
    let trusted_shared = node.trusted.clone();
    tokio::spawn({
        let event_tx = event_tx.clone();
        let trusted = trusted_shared.clone();
        async move {
            loop {
                let Some(item) = node.recv_raw().await else {
                    break;
                };
                let msg = {
                    let peers = trusted.read();
                    meerkat_comms::agent::types::CommsMessage::from_inbox_item(&item, &peers, true)
                };
                let Some(msg) = msg else {
                    continue;
                };

                let tui_event = match &msg.content {
                    // Typed heartbeat request from target
                    CommsContent::Request { intent, .. } if intent.as_str() == "heartbeat" => {
                        TuiEvent::Heartbeat {
                            from: msg.from_peer.clone(),
                        }
                    }
                    // Streaming events (acknowledged tech debt — Phase 3 eliminates this)
                    CommsContent::Message { body, .. } if body.starts_with(STREAM_PREFIX) => {
                        let json = &body[STREAM_PREFIX.len()..];
                        match serde_json::from_str::<AgentEvent>(json) {
                            Ok(event) => TuiEvent::StreamEvent {
                                from: msg.from_peer.clone(),
                                event,
                            },
                            Err(_) => TuiEvent::CommsMessage(msg.to_user_message_text()),
                        }
                    }
                    _ => TuiEvent::CommsMessage(msg.to_user_message_text()),
                };
                if event_tx.send(tui_event).await.is_err() {
                    break;
                }
            }
        }
    });

    // ── 6. Spawn command handler ──────────────────────────────────────────────
    let session_dir = data_dir.join("sessions");
    tokio::fs::create_dir_all(&session_dir).await?;
    let hive_model = model_override
        .clone()
        .or_else(|| auto_detect().map(|(m, _, _)| m));
    let hive_session_dir = session_dir.clone();

    tokio::spawn({
        let event_tx = event_tx.clone();
        let router = router.clone();
        let trusted_shared = trusted_shared.clone();
        async move {
            let mut hive_agent: Option<DynAgent> = None;
            // Per-target send queues: preserves FIFO within each target while
            // preventing one slow/unreachable target from blocking others.
            let mut target_senders: HashMap<String, mpsc::Sender<MessageKind>> = HashMap::new();

            while let Some(cmd) = command_rx.recv().await {
                match cmd {
                    AppCommand::Send { target, body } => {
                        let tx = target_senders.entry(target.clone()).or_insert_with(|| {
                            spawn_target_sender(target.clone(), router.clone(), event_tx.clone())
                        });
                        let _ = tx.send(MessageKind::Message { body, blocks: None }).await;
                    }
                    AppCommand::SlashCmd { target, cmd } => {
                        let params = serde_json::json!({ "cmd": cmd });
                        let tx = target_senders.entry(target.clone()).or_insert_with(|| {
                            spawn_target_sender(target.clone(), router.clone(), event_tx.clone())
                        });
                        let _ = tx
                            .send(MessageKind::Request {
                                intent: "command".into(),
                                params,
                            })
                            .await;
                    }
                    AppCommand::ResetHive => {
                        hive_agent = None;
                    }
                    AppCommand::ClaimTarget { .. } | AppCommand::ReleaseTarget { .. } => {}
                    AppCommand::RunHive { prompt } => {
                        if hive_agent.is_none() {
                            let Some(ref model) = hive_model else {
                                let _ = event_tx
                                    .send(TuiEvent::HiveError(
                                        "no API key — set ANTHROPIC_API_KEY, OPENAI_API_KEY, \
                                         or GEMINI_API_KEY"
                                            .into(),
                                    ))
                                    .await;
                                continue;
                            };
                            match build_hive(
                                &hive_session_dir,
                                model,
                                router.clone(),
                                trusted_shared.clone(),
                            )
                            .await
                            {
                                Ok(a) => hive_agent = Some(a),
                                Err(e) => {
                                    let _ = event_tx.send(TuiEvent::HiveError(e.to_string())).await;
                                    continue;
                                }
                            }
                        }
                        let agent = hive_agent.as_mut().expect("just built");
                        let ev = match agent.run(prompt.into()).await {
                            Ok(r) => TuiEvent::HivePlanDone(r.text),
                            Err(e) => TuiEvent::HiveError(e.to_string()),
                        };
                        let _ = event_tx.send(ev).await;
                    }
                }
            }
        }
    });

    // ── 7. Forward registrations to TUI ───────────────────────────────────────
    tokio::spawn({
        let event_tx = event_tx.clone();
        async move {
            while let Some((name, _addr)) = reg_target_rx.recv().await {
                let _ = event_tx.send(TuiEvent::TargetRegistered { name }).await;
            }
        }
    });

    // ── 8. Run TUI ────────────────────────────────────────────────────────────
    let app = App {
        transport: TransportMode::Direct,
        mode: Mode::Direct,
        targets: Vec::new(),
        target_ids: HashMap::new(),
        target_states: HashMap::new(),
        target_leases: HashMap::new(),
        attaching_since: HashMap::new(),
        selected: 0,
        input: String::new(),
        output: VecDeque::new(),
        hive_planning: false,
        quit: false,
        busy_targets: HashSet::new(),
        streaming_text: BTreeMap::new(),
        last_seen: HashMap::new(),
        scroll_offset: 0,
        auto_scroll: true,
    };

    tokio::task::spawn_blocking(move || tui_loop(app, command_tx, event_rx, None))
        .await?
        .context("TUI loop")?;

    Ok(())
}

/// Spawn a dedicated sender task for one target. Messages are drained FIFO
/// with a per-send timeout, so one unreachable target cannot stall sends to
/// other healthy targets.
fn spawn_target_sender(
    target: String,
    router: Arc<meerkat_comms::Router>,
    event_tx: mpsc::Sender<TuiEvent>,
) -> mpsc::Sender<MessageKind> {
    let (tx, mut rx) = mpsc::channel::<MessageKind>(32);
    tokio::spawn(async move {
        while let Some(kind) = rx.recv().await {
            let send_fut = router.send(&target, kind);
            match tokio::time::timeout(SEND_TIMEOUT, send_fut).await {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => {
                    let _ = event_tx
                        .send(TuiEvent::SendError(format!("[{target}] send failed: {e}")))
                        .await;
                }
                Err(_) => {
                    let _ = event_tx
                        .send(TuiEvent::SendError(format!("[{target}] send timed out")))
                        .await;
                }
            }
        }
    });
    tx
}

async fn build_hive(
    session_dir: &std::path::Path,
    model: &str,
    router: Arc<meerkat_comms::Router>,
    trusted: Arc<parking_lot::RwLock<meerkat_comms::TrustedPeers>>,
) -> anyhow::Result<DynAgent> {
    let factory = AgentFactory::new(session_dir);
    let provider = detect_provider(model);
    let llm = build_llm_client(&factory, model, provider).await?;
    let tools: Arc<dyn AgentToolDispatcher> = Arc::new(CommsToolDispatcher::new(router, trusted));
    let store = Arc::new(JsonlStore::new(session_dir.to_path_buf()));
    store.init().await?;
    let hive_store: Arc<dyn AgentSessionStore> = Arc::new(StoreAdapter::new(store));

    let agent: DynAgent = AgentBuilder::new()
        .model(model)
        .system_prompt(
            "You are a hive orchestrator for a fleet of remote machines. \
             Use the 'peers' tool to discover targets, then 'send' to dispatch \
             commands. Target replies appear in the TUX output panel; the user \
             will relay results if needed. Keep dispatches concise.",
        )
        .build(llm, tools, hive_store)
        .await;
    Ok(agent)
}

fn current_attached_target_ids(claims: &ClaimRegistry) -> Vec<String> {
    claims
        .read()
        .values()
        .filter(|state| state.is_attached_for_rebind())
        .map(|state| state.target_id().to_string())
        .collect()
}

fn claim_state_for_name(
    app: &App,
    claims: &ClaimRegistry,
    target_name: &str,
) -> Option<ClaimState> {
    let target_id = app.target_ids.get(target_name)?;
    claims.read().get(target_id).cloned()
}

fn rebuild_kennel_projection(app: &mut App, claims: &ClaimRegistry) {
    app.target_states.clear();
    app.target_leases.clear();
    app.attaching_since.clear();

    let now_ms = chrono::Utc::now().timestamp_millis();
    for state in claims.read().values() {
        let target_name = state.target_name().to_string();
        match state {
            ClaimState::Available { .. } => {
                app.target_states
                    .insert(target_name, KennelUiState::Available);
            }
            ClaimState::ClaimRequested { .. } => {
                app.target_states
                    .insert(target_name.clone(), KennelUiState::Attaching);
                app.attaching_since.insert(target_name, Instant::now());
            }
            ClaimState::Attaching {
                lease_id,
                started_at_ms,
                ..
            } => {
                app.target_states
                    .insert(target_name.clone(), KennelUiState::Attaching);
                app.target_leases
                    .insert(target_name.clone(), lease_id.clone());
                let elapsed_ms = (now_ms - *started_at_ms).max(0) as u64;
                let started = Instant::now()
                    .checked_sub(Duration::from_millis(elapsed_ms))
                    .unwrap_or_else(Instant::now);
                app.attaching_since.insert(target_name, started);
            }
            ClaimState::Claimed { lease_id, .. } => {
                app.target_states
                    .insert(target_name.clone(), KennelUiState::ClaimedByMe);
                app.target_leases.insert(target_name, lease_id.clone());
            }
            ClaimState::ClaimUncertain { lease_id, .. } => {
                app.target_states
                    .insert(target_name.clone(), KennelUiState::ClaimUncertain);
                app.target_leases.insert(target_name, lease_id.clone());
            }
            ClaimState::ReleasePending { lease_id, .. } => {
                app.target_states
                    .insert(target_name.clone(), KennelUiState::ClaimedByMe);
                app.target_leases.insert(target_name, lease_id.clone());
            }
        }
    }
}

fn claim_lookup_by_lease(claims: &ClaimRegistry, lease_id: &str) -> Option<ClaimState> {
    claims
        .read()
        .values()
        .find(|state| state.lease_id() == Some(lease_id))
        .cloned()
}

fn apply_claim_effects(
    claims: &ClaimRegistry,
    router: &Arc<meerkat_comms::Router>,
    kennel_tx: Option<&mpsc::UnboundedSender<KennelClientCommand>>,
    target_id: &str,
    effects: &[ClaimEffect],
) {
    for effect in effects {
        match effect {
            ClaimEffect::EnsureTrustedPeer {
                target_pubkey,
                target_direct_addr,
            } => {
                if let Ok(pubkey) = meerkat_comms::identity::PubKey::from_peer_id(target_pubkey) {
                    let target_name = claims
                        .read()
                        .get(target_id)
                        .map(|state| state.target_name().to_string())
                        .unwrap_or_else(|| target_id.to_string());
                    router.add_trusted_peer(meerkat_comms::TrustedPeer {
                        name: target_name,
                        pubkey,
                        addr: target_direct_addr.clone(),
                        meta: meerkat_comms::PeerMeta::default(),
                    });
                }
            }
            ClaimEffect::RemoveTrustedPeer { target_pubkey } => {
                if let Ok(pubkey) = meerkat_comms::identity::PubKey::from_peer_id(target_pubkey) {
                    router.remove_trusted_peer(&pubkey);
                }
            }
            ClaimEffect::SendClaimToKennel { target_id } => {
                if let Some(kennel_tx) = kennel_tx {
                    let _ = kennel_tx.send(KennelClientCommand::ClaimTarget {
                        target_id: target_id.clone(),
                    });
                }
            }
            ClaimEffect::SendClaimAck { lease_id } => {
                if let Some(kennel_tx) = kennel_tx {
                    let _ = kennel_tx.send(KennelClientCommand::ClaimAck {
                        lease_id: lease_id.clone(),
                    });
                }
            }
            ClaimEffect::SendAttachConfirmed { lease_id } => {
                if let Some(kennel_tx) = kennel_tx {
                    let _ = kennel_tx.send(KennelClientCommand::AttachConfirmed {
                        lease_id: lease_id.clone(),
                    });
                }
            }
            ClaimEffect::SendReleaseToKennel { lease_id } => {
                if let Some(kennel_tx) = kennel_tx {
                    let _ = kennel_tx.send(KennelClientCommand::ReleaseTarget {
                        lease_id: lease_id.clone(),
                    });
                }
            }
            ClaimEffect::ClearUiProjection => {}
        }
    }
}

async fn send_kennel_message(
    write_half: &mut tokio::net::tcp::OwnedWriteHalf,
    router: &Arc<meerkat_comms::Router>,
    tux_id: &str,
    payload: KennelPayload,
) -> bool {
    let Ok(env) = build_signed_envelope(router.keypair_arc().as_ref(), tux_id, payload) else {
        return false;
    };
    write_envelope(write_half, &env).await.is_ok()
}

#[allow(clippy::too_many_arguments)]
async fn spawn_command_processor(
    mut command_rx: mpsc::UnboundedReceiver<AppCommand>,
    kennel_tx: mpsc::UnboundedSender<KennelClientCommand>,
    event_tx: mpsc::Sender<TuiEvent>,
    router: Arc<meerkat_comms::Router>,
    trusted_shared: Arc<parking_lot::RwLock<meerkat_comms::TrustedPeers>>,
    claims: Arc<ClaimRegistry>,
    hive_model: Option<String>,
    hive_session_dir: PathBuf,
) {
    let mut hive_agent: Option<DynAgent> = None;
    let mut target_senders: HashMap<String, mpsc::Sender<MessageKind>> = HashMap::new();

    while let Some(cmd) = command_rx.recv().await {
        match cmd {
            AppCommand::Send { target, body } => {
                let tx = target_senders.entry(target.clone()).or_insert_with(|| {
                    spawn_target_sender(target.clone(), router.clone(), event_tx.clone())
                });
                let _ = tx.send(MessageKind::Message { body, blocks: None }).await;
            }
            AppCommand::SlashCmd { target, cmd } => {
                let params = serde_json::json!({ "cmd": cmd });
                let tx = target_senders.entry(target.clone()).or_insert_with(|| {
                    spawn_target_sender(target.clone(), router.clone(), event_tx.clone())
                });
                let _ = tx
                    .send(MessageKind::Request {
                        intent: "command".into(),
                        params,
                    })
                    .await;
            }
            AppCommand::ClaimTarget { target_id } => {
                if let Some(state) = claims.read().get(&target_id).cloned()
                    && let Ok((new_state, effects)) =
                        tux_claim::transition(state, ClaimEvent::ClaimRequested)
                {
                    claims.write().insert(target_id.clone(), new_state);
                    apply_claim_effects(&claims, &router, Some(&kennel_tx), &target_id, &effects);
                }
            }
            AppCommand::ReleaseTarget { lease_id } => {
                if let Some(target_id) = claims
                    .read()
                    .iter()
                    .find(|(_, state)| state.lease_id() == Some(lease_id.as_str()))
                    .map(|(target_id, _)| target_id.clone())
                    && let Some(state) = claims.read().get(&target_id).cloned()
                    && let Ok((new_state, effects)) =
                        tux_claim::transition(state, ClaimEvent::ReleaseRequested)
                {
                    claims.write().insert(target_id.clone(), new_state);
                    apply_claim_effects(&claims, &router, Some(&kennel_tx), &target_id, &effects);
                }
            }
            AppCommand::ResetHive => {
                hive_agent = None;
            }
            AppCommand::RunHive { prompt } => {
                if hive_agent.is_none() {
                    let Some(ref model) = hive_model else {
                        let _ = event_tx.send(TuiEvent::HiveError(
                            "no API key — set ANTHROPIC_API_KEY, OPENAI_API_KEY, or GEMINI_API_KEY".into(),
                        )).await;
                        continue;
                    };
                    match build_hive(
                        &hive_session_dir,
                        model,
                        router.clone(),
                        trusted_shared.clone(),
                    )
                    .await
                    {
                        Ok(a) => hive_agent = Some(a),
                        Err(e) => {
                            let _ = event_tx.send(TuiEvent::HiveError(e.to_string())).await;
                            continue;
                        }
                    }
                }
                let agent = hive_agent.as_mut().expect("just built");
                let ev = match agent.run(prompt.into()).await {
                    Ok(r) => TuiEvent::HivePlanDone(r.text),
                    Err(e) => TuiEvent::HiveError(e.to_string()),
                };
                let _ = event_tx.send(ev).await;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn spawn_kennel_client(
    mut kennel_rx: mpsc::UnboundedReceiver<KennelClientCommand>,
    kennel_cmd_tx: mpsc::UnboundedSender<KennelClientCommand>,
    event_tx: mpsc::Sender<TuiEvent>,
    router: Arc<meerkat_comms::Router>,
    claims: Arc<ClaimRegistry>,
    kennel_addr: String,
    tux_id: String,
    direct_addr: String,
) {
    'outer: loop {
        let stream =
            match tokio::time::timeout(Duration::from_secs(10), TcpStream::connect(&kennel_addr))
                .await
            {
                Ok(Ok(stream)) => stream,
                Ok(Err(e)) => {
                    let _ = event_tx
                        .send(TuiEvent::HiveError(format!("[kennel] connect failed: {e}")))
                        .await;
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
                Err(_) => {
                    let _ = event_tx
                        .send(TuiEvent::HiveError("[kennel] connect timed out".into()))
                        .await;
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
            };
        let (read_half, mut write_half) = stream.into_split();
        let mut reader = BufReader::new(read_half);
        // On reconnect, drop in-flight claims (attached=false) that can't
        // complete because the old kennel session is gone. Without this,
        // a target's mcm.attach for a stale claim creates a ghost session
        // that the kennel knows nothing about.
        {
            claims
                .write()
                .retain(|_, state| state.is_attached_for_rebind());
        }
        if !send_kennel_message(
            &mut write_half,
            &router,
            &tux_id,
            KennelPayload::TuxRegister {
                tux_id: tux_id.clone(),
                pubkey: tux_id.clone(),
                direct_addr: direct_addr.clone(),
                attached_target_ids: current_attached_target_ids(&claims),
            },
        )
        .await
        {
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        }
        let Some(env) = read_envelope(&mut reader).await.ok().flatten() else {
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        };
        let _ = verify_envelope(&env);

        let rebound_targets = current_attached_target_ids(&claims);
        if !rebound_targets.is_empty()
            && !send_kennel_message(
                &mut write_half,
                &router,
                &tux_id,
                KennelPayload::RebindTargets {
                    target_ids: rebound_targets,
                },
            )
            .await
        {
            let _ = event_tx
                .send(TuiEvent::SendError("[kennel] disconnected".into()))
                .await;
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        }

        let mut heartbeat = tokio::time::interval(Duration::from_secs(10));
        let mut refresh = tokio::time::interval(Duration::from_secs(5));
        let mut renew = tokio::time::interval(Duration::from_secs(15));

        loop {
            tokio::select! {
                _ = heartbeat.tick() => {
                    if !send_kennel_message(&mut write_half, &router, &tux_id, KennelPayload::TuxHeartbeat).await {
                        break;
                    }
                }
                _ = refresh.tick() => {
                    let mut refresh_ok = true;
                    for scope in [ListScope::Available, ListScope::Mine] {
                        if !send_kennel_message(&mut write_half, &router, &tux_id, KennelPayload::ListTargets { scope }).await {
                            refresh_ok = false;
                            break;
                        }
                    }
                    if !refresh_ok {
                        break;
                    }
                }
                _ = renew.tick() => {
                    let lease_ids: Vec<String> = claims.read().values().filter_map(|state| {
                        state.is_attached_for_rebind().then(|| state.lease_id().map(ToString::to_string)).flatten()
                    }).collect();
                    if !lease_ids.is_empty()
                        && !send_kennel_message(
                            &mut write_half,
                            &router,
                            &tux_id,
                            KennelPayload::RenewLeases { lease_ids, lease_ttl_sec: None },
                        ).await
                    {
                        break;
                    }
                }
                maybe = read_envelope(&mut reader) => {
                    let Some(env) = maybe.ok().flatten() else { break; };
                    if verify_envelope(&env).is_err() { continue; }
                    match env.payload {
                        KennelPayload::TargetList { scope: ListScope::Available, targets } => {
                            let _ = event_tx.send(TuiEvent::KennelAvailableTargets(targets)).await;
                        }
                        KennelPayload::TargetList { scope: ListScope::Mine, targets } => {
                            let _ = event_tx.send(TuiEvent::KennelMineTargets(targets)).await;
                        }
                        KennelPayload::ClaimGranted { claims: grants } => {
                            {
                                let mut guard = claims.write();
                                for claim in &grants {
                                    let state = guard
                                        .get(&claim.target_id)
                                        .cloned()
                                        .unwrap_or(ClaimState::Available {
                                            target_id: claim.target_id.clone(),
                                            target_name: claim.target_name.clone(),
                                        });
                                    if let Ok((new_state, effects)) = tux_claim::transition(
                                        state,
                                        ClaimEvent::ClaimGranted {
                                            lease_id: claim.lease_id.clone(),
                                            target_pubkey: claim.target_pubkey.clone(),
                                            target_direct_addr: claim.target_direct_addr.clone(),
                                            now_ms: chrono::Utc::now().timestamp_millis(),
                                        },
                                    ) {
                                        guard.insert(claim.target_id.clone(), new_state);
                                        drop(guard);
                                        apply_claim_effects(&claims, &router, Some(&kennel_cmd_tx), &claim.target_id, &effects);
                                        guard = claims.write();
                                    }
                                }
                            }
                            let _ = event_tx.send(TuiEvent::KennelClaimGranted(grants)).await;
                        }
                        KennelPayload::ClaimReleased { target_id, lease_id, reason } => {
                            let claim_target_id = if !lease_id.is_empty() {
                                claim_lookup_by_lease(&claims, &lease_id)
                                    .map(|state| state.target_id().to_string())
                            } else {
                                Some(target_id.clone()).filter(|id| claims.read().contains_key(id))
                            };
                            if let Some(claim_target_id) = claim_target_id
                                && let Some(state) = claims.read().get(&claim_target_id).cloned()
                                && let Ok((new_state, effects)) =
                                    tux_claim::transition(state, ClaimEvent::ClaimReleased)
                            {
                                claims.write().insert(claim_target_id.clone(), new_state);
                                apply_claim_effects(
                                    &claims,
                                    &router,
                                    Some(&kennel_cmd_tx),
                                    &claim_target_id,
                                    &effects,
                                );
                            }
                            let _ = event_tx.send(TuiEvent::KennelClaimReleased { target_id, lease_id, reason }).await;
                        }
                        KennelPayload::TargetLost { target_id, lease_id: Some(lease_id) } => {
                            if let Some(claim_target_id) = claim_lookup_by_lease(&claims, &lease_id)
                                .map(|state| state.target_id().to_string())
                                && let Some(state) = claims.read().get(&claim_target_id).cloned()
                                && let Ok((new_state, effects)) =
                                    tux_claim::transition(state, ClaimEvent::ClaimReleased)
                            {
                                claims.write().insert(claim_target_id.clone(), new_state);
                                apply_claim_effects(
                                    &claims,
                                    &router,
                                    Some(&kennel_cmd_tx),
                                    &claim_target_id,
                                    &effects,
                                );
                            }
                            let _ = event_tx
                                .send(TuiEvent::KennelClaimReleased {
                                    target_id,
                                    lease_id,
                                    reason: "target_lost".into(),
                                })
                                .await;
                        }
                        KennelPayload::TargetLost { .. } => {}
                        KennelPayload::LeaseRebound { lease_id, target_id, target_pubkey, target_direct_addr, .. } => {
                            let target_name = if let Some(state) = claims.read().get(&target_id).cloned()
                                && let Ok((new_state, effects)) = tux_claim::transition(
                                    state,
                                    ClaimEvent::LeaseRebound {
                                        new_lease_id: lease_id.clone(),
                                        target_pubkey,
                                        target_direct_addr,
                                    },
                                )
                            {
                                let target_name = new_state.target_name().to_string();
                                claims.write().insert(target_id.clone(), new_state);
                                apply_claim_effects(
                                    &claims,
                                    &router,
                                    Some(&kennel_cmd_tx),
                                    &target_id,
                                    &effects,
                                );
                                Some(target_name)
                            } else {
                                None
                            };
                            if let Some(target_name) = target_name {
                                let _ = event_tx.send(TuiEvent::KennelAttached { target_name, lease_id }).await;
                            }
                        }
                        _ => {}
                    }
                }
                Some(cmd) = kennel_rx.recv() => {
                    match cmd {
                        KennelClientCommand::ClaimTarget { target_id } => {
                            if !send_kennel_message(
                                &mut write_half,
                                &router,
                                &tux_id,
                                KennelPayload::ClaimTargets {
                                    target_ids: vec![target_id],
                                    lease_ttl_sec: None,
                                },
                            )
                            .await
                            {
                                break;
                            }
                        }
                        KennelClientCommand::ClaimAck { lease_id } => {
                            if !send_kennel_message(
                                &mut write_half,
                                &router,
                                &tux_id,
                                KennelPayload::ClaimAck {
                                    lease_ids: vec![lease_id],
                                },
                            )
                            .await
                            {
                                break;
                            }
                        }
                        KennelClientCommand::ReleaseTarget { lease_id } => {
                            if !send_kennel_message(&mut write_half, &router, &tux_id, KennelPayload::ReleaseTargets { lease_ids: vec![lease_id] }).await {
                                break;
                            }
                        }
                        KennelClientCommand::AttachConfirmed { lease_id } => {
                            if !send_kennel_message(&mut write_half, &router, &tux_id, KennelPayload::AttachConfirmed { lease_id }).await {
                                break;
                            }
                        }
                        KennelClientCommand::Shutdown { reply } => {
                            let lease_ids: Vec<String> = claims
                                .read()
                                .values()
                                .filter_map(|state| state.lease_id().map(ToString::to_string))
                                .collect();
                            if !lease_ids.is_empty() {
                                let _ = send_kennel_message(&mut write_half, &router, &tux_id, KennelPayload::ReleaseTargets { lease_ids }).await;
                            }
                            let _ = reply.send(());
                            break 'outer;
                        }
                    }
                }
                else => break,
            }
        }

        let _ = event_tx
            .send(TuiEvent::SendError("[kennel] disconnected".into()))
            .await;
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn run_kennel_tux(args: &[String]) -> anyhow::Result<()> {
    let kennel_addr = find_flag(args, "--kennel").context("--kennel HOST:PORT is required")?;
    let port: u16 = find_flag(args, "--listen")
        .context("--listen PORT is required in kennel mode")?
        .parse()
        .context("--listen PORT must be a number")?;
    let model_override = find_flag(args, "--model");
    let advertise = find_flag(args, "--advertise");
    let data_dir = find_flag(args, "--data-dir")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".rkat/mdm/tux")
        });

    let keypair = load_or_generate_keypair(&data_dir.join("identity")).await?;
    let mut node = CommsNode::new(keypair);
    let comms_addr = format!("0.0.0.0:{port}");
    let _ = node.listen(&comms_addr).await?;
    let tux_id = node.pubkey_string();
    let direct_ip = match advertise {
        Some(ip) => ip,
        None => {
            let (host, kennel_port) = kennel_addr
                .rsplit_once(':')
                .context("invalid --kennel HOST:PORT")?;
            let kennel_port: u16 = kennel_port.parse().context("invalid kennel port")?;
            discover_local_ip(host, kennel_port).with_context(|| {
                format!("failed to detect advertise address toward {kennel_addr}; use --advertise")
            })?
        }
    };
    let direct_addr = format!("tcp://{direct_ip}:{port}");

    println!("=== MCM TUX — Kennel Mode ===");
    println!("listen    : tcp://0.0.0.0:{port}");
    println!("kennel    : {kennel_addr}");
    println!("direct    : {direct_addr}");

    let (command_tx, command_rx) = mpsc::unbounded_channel::<AppCommand>();
    let (event_tx, event_rx) = mpsc::channel::<TuiEvent>(256);
    let (kennel_tx, kennel_rx) = mpsc::unbounded_channel::<KennelClientCommand>();

    let router = node.router.clone();
    let trusted_shared = node.trusted.clone();
    let claims: Arc<ClaimRegistry> = Arc::new(parking_lot::RwLock::new(HashMap::new()));

    tokio::spawn({
        let event_tx = event_tx.clone();
        let trusted = trusted_shared.clone();
        let kennel_tx = kennel_tx.clone();
        let claims = claims.clone();
        let router = router.clone();
        async move {
            loop {
                let Some(item) = node.recv_raw().await else {
                    break;
                };
                let msg = {
                    let peers = trusted.read();
                    meerkat_comms::agent::types::CommsMessage::from_inbox_item(&item, &peers, true)
                };
                let Some(msg) = msg else {
                    continue;
                };

                match &msg.content {
                    CommsContent::Request { intent, params, .. }
                        if intent.as_str() == "heartbeat" =>
                    {
                        let _ = event_tx
                            .send(TuiEvent::Heartbeat {
                                from: msg.from_peer.clone(),
                            })
                            .await;
                    }
                    CommsContent::Request { intent, params, .. }
                        if intent.as_str() == "mcm.attach" =>
                    {
                        let lease_id = params
                            .get("lease_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let target_id = params
                            .get("target_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let matched = {
                            let claims = claims.read();
                            claims
                                .values()
                                .find(|state| state.lease_id() == Some(lease_id))
                                .filter(|claim| {
                                    claim.target_id() == target_id
                                        && claim.target_pubkey()
                                            == Some(msg.from_pubkey.to_peer_id().as_str())
                                })
                                .cloned()
                        };
                        if let Some(state) = matched {
                            let _ = tokio::time::timeout(
                                SEND_TIMEOUT,
                                router.send(
                                    &msg.from_peer,
                                    MessageKind::Request {
                                        intent: "mcm.attach_ok".into(),
                                        params: serde_json::json!({ "lease_id": lease_id }),
                                    },
                                ),
                            )
                            .await;
                            if let Ok((new_state, effects)) = tux_claim::transition(
                                state,
                                ClaimEvent::AttachConfirmed {
                                    lease_id: lease_id.to_string(),
                                },
                            ) {
                                let target_id = new_state.target_id().to_string();
                                let target_name = new_state.target_name().to_string();
                                claims.write().insert(target_id.clone(), new_state);
                                apply_claim_effects(
                                    &claims,
                                    &router,
                                    Some(&kennel_tx),
                                    &target_id,
                                    &effects,
                                );
                                let _ = event_tx
                                    .send(TuiEvent::KennelAttached {
                                        target_name,
                                        lease_id: lease_id.to_string(),
                                    })
                                    .await;
                            }
                        }
                    }
                    CommsContent::Message { body, .. } if body.starts_with(STREAM_PREFIX) => {
                        let json = &body[STREAM_PREFIX.len()..];
                        match serde_json::from_str::<AgentEvent>(json) {
                            Ok(event) => {
                                let _ = event_tx
                                    .send(TuiEvent::StreamEvent {
                                        from: msg.from_peer.clone(),
                                        event,
                                    })
                                    .await;
                            }
                            Err(_) => {
                                let _ = event_tx
                                    .send(TuiEvent::CommsMessage(msg.to_user_message_text()))
                                    .await;
                            }
                        }
                    }
                    _ => {
                        let _ = event_tx
                            .send(TuiEvent::CommsMessage(msg.to_user_message_text()))
                            .await;
                    }
                }
            }
        }
    });

    let session_dir = data_dir.join("sessions");
    tokio::fs::create_dir_all(&session_dir).await?;
    let hive_model = model_override
        .clone()
        .or_else(|| auto_detect().map(|(m, _, _)| m));
    let hive_session_dir = session_dir.clone();

    tokio::spawn({
        let router = router.clone();
        let trusted_shared = trusted_shared.clone();
        let kennel_tx = kennel_tx.clone();
        let event_tx = event_tx.clone();
        let claims = claims.clone();
        async move {
            spawn_command_processor(
                command_rx,
                kennel_tx,
                event_tx,
                router,
                trusted_shared,
                claims,
                hive_model,
                hive_session_dir,
            )
            .await;
        }
    });

    tokio::spawn({
        let event_tx = event_tx.clone();
        let kennel_cmd_tx = kennel_tx.clone();
        let router = router.clone();
        let claims = claims.clone();
        let kennel_addr = kennel_addr.clone();
        let tux_id = tux_id.clone();
        let direct_addr = direct_addr.clone();
        async move {
            spawn_kennel_client(
                kennel_rx,
                kennel_cmd_tx,
                event_tx,
                router,
                claims,
                kennel_addr,
                tux_id,
                direct_addr,
            )
            .await;
        }
    });

    let app = App {
        transport: TransportMode::Kennel,
        mode: Mode::Direct,
        targets: Vec::new(),
        target_ids: HashMap::new(),
        target_states: HashMap::new(),
        target_leases: HashMap::new(),
        attaching_since: HashMap::new(),
        selected: 0,
        input: String::new(),
        output: VecDeque::new(),
        hive_planning: false,
        quit: false,
        busy_targets: HashSet::new(),
        streaming_text: BTreeMap::new(),
        last_seen: HashMap::new(),
        scroll_offset: 0,
        auto_scroll: true,
    };

    let claims_for_tui = claims.clone();
    tokio::task::spawn_blocking(move || tui_loop(app, command_tx, event_rx, Some(claims_for_tui)))
        .await?
        .context("TUI loop")?;

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let _ = kennel_tx.send(KennelClientCommand::Shutdown { reply: shutdown_tx });
    let _ = tokio::time::timeout(Duration::from_secs(3), shutdown_rx).await;

    Ok(())
}

// ── TUI loop (sync — no .await) ──────────────────────────────────────────────

fn tui_loop(
    mut app: App,
    command_tx: mpsc::UnboundedSender<AppCommand>,
    mut event_rx: mpsc::Receiver<TuiEvent>,
    claims: Option<Arc<ClaimRegistry>>,
) -> anyhow::Result<()> {
    let mut terminal = ratatui::init();

    // Enable bracketed paste so multiline pastes arrive as Event::Paste
    // instead of individual Enter key events.
    crossterm::execute!(std::io::stdout(), EnableBracketedPaste)?;

    struct TermGuard;
    impl Drop for TermGuard {
        fn drop(&mut self) {
            let _ = crossterm::execute!(std::io::stdout(), DisableBracketedPaste);
            ratatui::restore();
        }
    }
    let _guard = TermGuard;

    loop {
        if crossterm::event::poll(Duration::from_millis(16))? {
            match crossterm::event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    handle_key(
                        &mut app,
                        key.code,
                        key.modifiers,
                        &command_tx,
                        claims.as_deref(),
                    );
                }
                Event::Paste(text) => {
                    let line_count = text.lines().count();
                    app.input.push_str(&text);
                    if line_count > 1 {
                        app.push(format!("[pasted {line_count} lines]"));
                    }
                }
                _ => {}
            }
        }

        while let Ok(ev) = event_rx.try_recv() {
            match ev {
                TuiEvent::CommsMessage(s) => {
                    // Update last_seen for the sender
                    if let Some(rest) = s.strip_prefix("[COMMS MESSAGE from ")
                        && let Some((from, _body)) = rest.split_once("]\n")
                    {
                        app.last_seen.insert(from.to_string(), Instant::now());
                    }
                    // Strip the raw prefix for clean display
                    let clean = s
                        .strip_prefix("[COMMS MESSAGE from ")
                        .and_then(|rest| rest.split_once("]\n"))
                        .map(|(from, body)| format!("[{from}] {body}"))
                        .unwrap_or(s);
                    app.push(clean);
                }
                TuiEvent::Heartbeat { from } => {
                    app.last_seen.insert(from, Instant::now());
                }
                TuiEvent::HivePlanDone(s) => {
                    app.push(format!("[hive dispatched] {s}"));
                    app.hive_planning = false;
                }
                TuiEvent::HiveError(e) => {
                    app.push(format!("[hive error] {e}"));
                    app.hive_planning = false;
                }
                TuiEvent::SendError(e) => {
                    // Extract target name from "[target] send failed: ..."
                    // and clear its busy state so it's not stuck processing.
                    if let Some(name) = e
                        .strip_prefix('[')
                        .and_then(|s| s.split_once(']').map(|(n, _)| n))
                    {
                        app.set_busy(name, false);
                    }
                    app.push(format!("[send error] {e}"));
                }
                TuiEvent::TargetRegistered { name } => {
                    if !app.targets.contains(&name) {
                        app.targets.push(name.clone());
                    }
                    // Clear stale transient state from a previous connection.
                    // Without this, busy_targets/streaming_text survive a
                    // disconnect+reconnect and wedge the UI (stuck [...],
                    // CommsMessage dedupe drops replies from "busy" target).
                    app.set_busy(&name, false);
                    app.streaming_text.remove(&name);
                    app.last_seen.insert(name.clone(), Instant::now());
                    app.push(format!("[registered] target '{name}' connected"));
                }
                TuiEvent::StreamEvent { from, event } => {
                    app.last_seen.insert(from.clone(), Instant::now());
                    handle_stream_event(&mut app, &from, &event);
                }
                TuiEvent::KennelAvailableTargets(targets) => {
                    let available_names: HashSet<String> =
                        targets.iter().map(|t| t.name.clone()).collect();
                    for target in targets {
                        app.target_ids
                            .insert(target.name.clone(), target.target_id.clone());
                        app.target_states
                            .entry(target.name.clone())
                            .or_insert(KennelUiState::Available);
                        if !matches!(
                            app.target_states.get(&target.name),
                            Some(KennelUiState::Attaching)
                                | Some(KennelUiState::ClaimedByMe)
                                | Some(KennelUiState::ClaimUncertain)
                        ) {
                            app.target_states
                                .insert(target.name.clone(), KennelUiState::Available);
                        }
                        if !app.targets.contains(&target.name) {
                            app.targets.push(target.name);
                        }
                    }
                    app.targets.retain(|name| {
                        if matches!(app.target_states.get(name), Some(KennelUiState::Available)) {
                            available_names.contains(name)
                        } else {
                            true
                        }
                    });
                    app.targets.sort();
                    // Clamp selected index to prevent out-of-bounds panic
                    // after targets are pruned during fleet churn.
                    if !app.targets.is_empty() {
                        app.selected = app.selected.min(app.targets.len() - 1);
                    } else {
                        app.selected = 0;
                    }
                }
                TuiEvent::KennelMineTargets(targets) => {
                    // The mine list is authoritative. Reconcile local claim
                    // state: anything the kennel stopped reporting is no longer
                    // owned by this TUX.
                    let mine_names: HashSet<String> =
                        targets.iter().map(|t| t.name.clone()).collect();

                    // Clear stale claims not in the authoritative list.
                    let stale_mine: Vec<String> = app
                        .target_states
                        .iter()
                        .filter(|(_, state)| {
                            matches!(
                                state,
                                KennelUiState::ClaimedByMe
                                    | KennelUiState::Attaching
                                    | KennelUiState::ClaimUncertain
                            )
                        })
                        .map(|(name, _)| name.clone())
                        .filter(|name| !mine_names.contains(name))
                        .collect();
                    for name in &stale_mine {
                        app.target_states
                            .insert(name.clone(), KennelUiState::Available);
                        app.target_leases.remove(name);
                        app.attaching_since.remove(name);
                        app.set_busy(name, false);
                        app.flush_streaming(name);
                        app.push(format!(
                            "[{name}] claim no longer held — returned to available"
                        ));
                    }

                    // Upsert entries from the authoritative list.
                    for target in targets {
                        app.target_ids
                            .insert(target.name.clone(), target.target_id.clone());
                        if let Some(lease_id) = target.lease_id.clone() {
                            app.target_leases.insert(target.name.clone(), lease_id);
                        }
                        let state = match target.state {
                            KennelTargetState::PendingAttach => KennelUiState::Attaching,
                            KennelTargetState::Claimed | KennelTargetState::RecoveringClaim => {
                                KennelUiState::ClaimedByMe
                            }
                            KennelTargetState::Available => KennelUiState::Available,
                        };
                        app.target_states.insert(target.name.clone(), state);
                        if !app.targets.contains(&target.name) {
                            app.targets.push(target.name);
                        }
                    }
                    app.targets.sort();
                }
                TuiEvent::KennelClaimGranted(claims) => {
                    for claim in claims {
                        app.target_ids
                            .insert(claim.target_name.clone(), claim.target_id.clone());
                        app.target_leases
                            .insert(claim.target_name.clone(), claim.lease_id.clone());
                        app.target_states
                            .insert(claim.target_name.clone(), KennelUiState::Attaching);
                        app.attaching_since
                            .insert(claim.target_name.clone(), Instant::now());
                        if !app.targets.contains(&claim.target_name) {
                            app.targets.push(claim.target_name);
                        }
                    }
                    app.targets.sort();
                }
                TuiEvent::KennelClaimReleased {
                    target_id,
                    lease_id,
                    reason,
                } => {
                    let target_name = app.target_ids.iter().find_map(|(name, id)| {
                        if id == &target_id {
                            Some(name.clone())
                        } else {
                            None
                        }
                    });
                    if let Some(name) = target_name {
                        app.target_states
                            .insert(name.clone(), KennelUiState::Available);
                        app.target_leases.remove(&name);
                        app.attaching_since.remove(&name);
                        app.set_busy(&name, false);
                        app.flush_streaming(&name);
                        app.push(format!("[{name}] kennel released claim: {reason}"));
                    } else {
                        app.push(format!("[kennel] claim released {lease_id}: {reason}"));
                    }
                }
                TuiEvent::KennelAttached {
                    target_name,
                    lease_id,
                } => {
                    app.target_states
                        .insert(target_name.clone(), KennelUiState::ClaimedByMe);
                    app.target_leases.insert(target_name.clone(), lease_id);
                    app.attaching_since.remove(&target_name);
                    app.push(format!("[{target_name}] attached"));
                }
            }
        }

        if app.transport == TransportMode::Kennel
            && let Some(claims) = claims.as_deref()
        {
            let now_ms = chrono::Utc::now().timestamp_millis();
            let timeout_ms = LOCAL_ATTACH_TIMEOUT.as_millis() as i64;
            let timed_out: Vec<String> = claims
                .read()
                .iter()
                .filter_map(|(target_id, state)| match state {
                    ClaimState::Attaching { started_at_ms, .. }
                        if now_ms - *started_at_ms >= timeout_ms =>
                    {
                        Some(target_id.clone())
                    }
                    _ => None,
                })
                .collect();
            for target_id in timed_out {
                if let Some(state) = claims.read().get(&target_id).cloned()
                    && let Ok((new_state, _effects)) = tux_claim::transition(
                        state,
                        ClaimEvent::AttachTimeout { now_ms, timeout_ms },
                    )
                {
                    claims.write().insert(target_id, new_state);
                }
            }
            rebuild_kennel_projection(&mut app, claims);
        }

        // Clear busy/streaming state for targets that stopped heartbeating.
        // Without this, a target that disconnects mid-run (after RunStarted
        // but before RunCompleted) shows "...processing" forever.
        let stale_threshold = Duration::from_secs(30);
        let stale_targets: Vec<String> = app
            .busy_targets
            .iter()
            .filter(|t| {
                app.last_seen
                    .get(*t)
                    .is_some_and(|seen| seen.elapsed() > stale_threshold)
            })
            .cloned()
            .collect();
        for t in &stale_targets {
            app.flush_streaming(t);
            app.set_busy(t, false);
            app.push(format!("[{t}] target unresponsive — cleared busy state"));
        }

        terminal.draw(|f| render(f, &mut app))?;
        if app.quit {
            break;
        }
    }

    Ok(())
}

fn handle_stream_event(app: &mut App, from: &str, event: &AgentEvent) {
    match event {
        AgentEvent::RunStarted { .. } => {
            app.set_busy(from, true);
            app.streaming_text.insert(from.to_string(), String::new());
        }
        AgentEvent::TextDelta { delta, .. } => {
            app.streaming_text
                .entry(from.to_string())
                .or_default()
                .push_str(delta);
        }
        AgentEvent::TextComplete { content, .. } => {
            // TextComplete carries the authoritative final text.
            // Preserve source attribution so multi-target output is distinguishable.
            app.streaming_text.remove(from);
            if !content.is_empty() {
                app.push(format!("[{from}] {content}"));
            }
        }
        AgentEvent::ToolCallRequested { name, args, .. } => {
            app.flush_streaming(from);
            // For shell: show just the command. For others: compact args.
            let display = if name == "shell" {
                let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("?");
                let bg = args
                    .get("background")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if bg {
                    format!("$ {cmd} &")
                } else {
                    format!("$ {cmd}")
                }
            } else {
                let s = args.to_string();
                let short: String = s.chars().take(80).collect();
                if s.len() > 80 {
                    format!("{short}...")
                } else {
                    short
                }
            };
            app.push(format!("  [{name}] {display}"));
        }
        AgentEvent::ToolExecutionCompleted {
            name,
            result,
            is_error,
            duration_ms,
            ..
        } => {
            if *is_error {
                app.push(format!("  [{name}] error ({duration_ms}ms):"));
                for line in result.lines().take(10) {
                    app.push(format!("  {line}"));
                }
            } else if result.trim().is_empty() {
                // Silent success (e.g. background job started with no output)
            } else if let Ok(json) = serde_json::from_str::<serde_json::Value>(result) {
                // Structured result — prefer "message" field, fall back to compact JSON
                if let Some(msg) = json.get("message").and_then(|v| v.as_str()) {
                    app.push(format!("  {msg}"));
                } else {
                    let pretty = serde_json::to_string_pretty(&json).unwrap_or_default();
                    for line in pretty.lines().take(15) {
                        app.push(format!("  {line}"));
                    }
                }
            } else {
                // Plain text output — show first 15 lines
                let line_count = result.lines().count();
                for (i, line) in result.lines().enumerate() {
                    if i >= 15 {
                        app.push(format!("  ... ({} more lines)", line_count - 15));
                        break;
                    }
                    app.push(format!("  {line}"));
                }
            }
        }
        AgentEvent::RunCompleted { .. } => {
            app.flush_streaming(from);
            app.set_busy(from, false);
            app.push(String::new()); // blank line between turns
        }
        AgentEvent::RunFailed { error, .. } => {
            app.flush_streaming(from);
            app.push(format!("[{from} error] {error}"));
            app.set_busy(from, false);
        }
        _ => {}
    }
}

fn handle_key(
    app: &mut App,
    code: KeyCode,
    modifiers: KeyModifiers,
    command_tx: &mpsc::UnboundedSender<AppCommand>,
    claims: Option<&ClaimRegistry>,
) {
    match code {
        KeyCode::Tab => {
            app.mode = if app.mode == Mode::Direct {
                Mode::Hive
            } else {
                Mode::Direct
            };
        }
        KeyCode::Up if app.mode == Mode::Direct => {
            app.selected = app.selected.saturating_sub(1);
        }
        KeyCode::Down if app.mode == Mode::Direct => {
            app.selected = (app.selected + 1).min(app.targets.len().saturating_sub(1));
        }
        KeyCode::PageUp => {
            app.auto_scroll = false;
            app.scroll_offset = app.scroll_offset.saturating_sub(10);
        }
        KeyCode::PageDown => {
            app.scroll_offset = app.scroll_offset.saturating_add(10);
        }
        KeyCode::Home => {
            app.auto_scroll = false;
            app.scroll_offset = 0;
        }
        KeyCode::End => {
            app.auto_scroll = true;
        }
        // Shift+Enter, Alt+Enter, or Ctrl+J → insert newline
        KeyCode::Enter if modifiers.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) => {
            app.input.push('\n');
        }
        KeyCode::Char('j') if modifiers.contains(KeyModifiers::CONTROL) => {
            app.input.push('\n');
        }
        // Plain Enter → submit (slash commands or regular message)
        KeyCode::Enter if !app.input.is_empty() => {
            // Check if submission is possible BEFORE consuming the input.
            // Slash commands are always submittable. Regular messages need
            // a valid target (Direct) or no in-flight hive plan (Hive).
            let is_slash = app.input.starts_with('/')
                && matches!(
                    app.input
                        .split_once(' ')
                        .map(|(c, _)| c)
                        .unwrap_or(&app.input),
                    "/new" | "/resume" | "/help" | "/claim" | "/release"
                );
            // Check if submission is possible before consuming input.
            let selected_busy =
                !app.targets.is_empty() && app.busy_targets.contains(&app.targets[app.selected]);
            match app.mode {
                Mode::Direct if app.targets.is_empty() && !is_slash => return,
                // Slash commands blocked while target is busy (can't reset mid-run)
                Mode::Direct if selected_busy && is_slash => return,
                Mode::Direct
                    if app.transport == TransportMode::Kennel
                        && !is_slash
                        && !app.targets.is_empty()
                        && !matches!(
                            claims.and_then(|claims| claim_state_for_name(
                                app,
                                claims,
                                &app.targets[app.selected]
                            )),
                            Some(
                                ClaimState::Claimed { .. }
                                    | ClaimState::ClaimUncertain { .. }
                                    | ClaimState::ReleasePending { .. }
                            )
                        ) =>
                {
                    return;
                }
                // Regular messages while busy: send immediately via comms.
                // The target's inbox loop is sequential — the message queues
                // in the comms inbox and runs after the current turn finishes.
                Mode::Direct if selected_busy => {}
                Mode::Hive if app.hive_planning => return,
                _ => {}
            }

            let body = std::mem::take(&mut app.input);

            // ── Slash commands ────────────────────────────────────────────
            if is_slash {
                let parts: Vec<&str> = body.splitn(2, ' ').collect();
                let slash = parts[0];
                let arg = parts.get(1).unwrap_or(&"").trim();

                match slash {
                    "/new" => {
                        if app.mode == Mode::Hive {
                            app.output.clear();
                            app.streaming_text.clear();
                            app.hive_planning = false;
                            app.push("Session reset (hive).".into());
                            let _ = command_tx.send(AppCommand::ResetHive);
                            return;
                        }
                        if app.targets.is_empty() {
                            return;
                        }
                        let target = app.targets[app.selected].clone();
                        // Don't clear output yet — wait for the target to confirm.
                        // Clearing before the command succeeds would lose unrelated
                        // history if the send fails or other targets are connected.
                        app.push(format!("> /new ({target})"));
                        let _ = command_tx.send(AppCommand::SlashCmd {
                            target,
                            cmd: "NEW_SESSION".into(),
                        });
                    }
                    "/resume" if arg.is_empty() => {
                        if app.targets.is_empty() {
                            return;
                        }
                        let target = app.targets[app.selected].clone();
                        app.push(format!("> /resume ({target})"));
                        let _ = command_tx.send(AppCommand::SlashCmd {
                            target,
                            cmd: "LIST_SESSIONS".into(),
                        });
                    }
                    "/resume" => {
                        if app.targets.is_empty() {
                            return;
                        }
                        let target = app.targets[app.selected].clone();
                        app.push(format!("> /resume {arg} ({target})"));
                        let _ = command_tx.send(AppCommand::SlashCmd {
                            target,
                            cmd: format!("RESUME {arg}"),
                        });
                    }
                    "/help" => {
                        app.push("**Commands:**".into());
                        if app.transport == TransportMode::Kennel {
                            app.push(
                                "  `/claim`        — claim selected target from kennel".into(),
                            );
                            app.push(
                                "  `/release`      — release selected target back to kennel".into(),
                            );
                        }
                        app.push("  `/new`          — start a fresh session".into());
                        app.push("  `/resume`       — list past sessions".into());
                        app.push("  `/resume <N>`   — resume session N".into());
                        app.push("  `/help`         — show this help".into());
                    }
                    "/claim" => {
                        if app.transport != TransportMode::Kennel || app.targets.is_empty() {
                            return;
                        }
                        let target = app.targets[app.selected].clone();
                        let Some(target_id) = app.target_ids.get(&target).cloned() else {
                            return;
                        };
                        app.push(format!("> /claim ({target})"));
                        let _ = command_tx.send(AppCommand::ClaimTarget { target_id });
                    }
                    "/release" => {
                        if app.transport != TransportMode::Kennel || app.targets.is_empty() {
                            return;
                        }
                        let target = app.targets[app.selected].clone();
                        let Some(lease_id) = claims
                            .and_then(|claims| claim_state_for_name(app, claims, &target))
                            .and_then(|state| state.lease_id().map(ToString::to_string))
                        else {
                            return;
                        };
                        app.push(format!("> /release ({target})"));
                        let _ = command_tx.send(AppCommand::ReleaseTarget { lease_id });
                    }
                    _ => unreachable!("guard above only admits known commands"),
                }
                return;
            }

            // ── Regular message ───────────────────────────────────────────
            let cmd = match app.mode {
                Mode::Direct if !app.targets.is_empty() => {
                    let target = app.targets[app.selected].clone();
                    let display = body.replace('\n', " ");
                    if app.busy_targets.contains(&target) {
                        // Target is busy — message sent immediately via comms,
                        // queued in the target's inbox for after the current run.
                        app.push(format!("  ↳ [{target}] {display}"));
                    } else {
                        app.push(format!("> [{target}] {display}"));
                        app.set_busy(&target, true);
                    }
                    AppCommand::Send { target, body }
                }
                Mode::Hive => {
                    app.hive_planning = true;
                    let display = body.replace('\n', " ");
                    app.push(format!("> [hive] {display}"));
                    AppCommand::RunHive { prompt: body }
                }
                _ => unreachable!("checked before taking input"),
            };
            let _ = command_tx.send(cmd);
        }
        KeyCode::Char(c) => app.input.push(c),
        KeyCode::Backspace => {
            app.input.pop();
        }
        KeyCode::Esc => app.quit = true,
        _ => {}
    }
}

// ── Rendering ─────────────────────────────────────────────────────────────────

fn render(f: &mut ratatui::Frame, app: &mut App) {
    // Input box height: 2 (borders) + estimated visual lines (min 1, max 8).
    // Derive the label length from the actual label string so the
    // first-line available width is always correct.
    let label = app.input_label();
    let label_len = label.len();
    let inner_width = f.area().width.saturating_sub(2) as usize; // minus left+right border
    let input_lines: usize = app
        .input
        .split('\n')
        .enumerate()
        .map(|(i, line)| {
            let avail = if i == 0 {
                inner_width.saturating_sub(label_len)
            } else {
                inner_width.saturating_sub(2) // "  " continuation indent
            };
            if avail == 0 || line.is_empty() {
                1
            } else {
                line.len().div_ceil(avail)
            }
        })
        .sum();
    let input_height = (input_lines.clamp(1, 8) as u16) + 2;

    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(input_height),
        ])
        .split(f.area());

    // Title bar
    let d = if app.mode == Mode::Direct {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let h = if app.mode == Mode::Hive {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " TUX — Meerkat Device Manager   ",
                Style::default().fg(Color::White),
            ),
            Span::styled(" Direct ", d),
            Span::raw(" "),
            Span::styled(" Hive ", h),
            Span::styled(
                "  [Tab] toggle  [PgUp/Dn] scroll  [Esc] quit",
                Style::default().fg(Color::DarkGray),
            ),
        ])),
        outer[0],
    );

    // Main: targets + output
    let main = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(22), Constraint::Percentage(78)])
        .split(outer[1]);

    render_targets(f, main[0], app);
    render_output(f, main[1], app);
    render_input(f, outer[2], app);
}

fn render_targets(f: &mut ratatui::Frame, area: ratatui::layout::Rect, app: &App) {
    let items: Vec<ListItem> = app
        .targets
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let sel = app.mode == Mode::Direct && i == app.selected;
            let busy = app.busy_targets.contains(t);
            let prefix = if sel { "> " } else { "  " };
            let suffix = if app.transport == TransportMode::Kennel {
                match app.target_states.get(t).copied() {
                    Some(KennelUiState::Attaching) => " [attaching...]",
                    Some(KennelUiState::ClaimedByMe) if busy => " [mine ...]",
                    Some(KennelUiState::ClaimedByMe) => " [mine]",
                    Some(KennelUiState::ClaimUncertain) => " [claim?]",
                    _ if busy => " [...]",
                    _ => "",
                }
            } else if busy {
                " [...]"
            } else {
                ""
            };

            // Status color based on last_seen age
            let age = app
                .last_seen
                .get(t)
                .map(|ts| ts.elapsed())
                .unwrap_or(Duration::MAX);
            let style = if age > Duration::from_secs(60) {
                // Offline — override all other styling
                Style::default().fg(Color::DarkGray)
            } else if age >= Duration::from_secs(35) {
                // Stale
                Style::default().fg(Color::Yellow)
            } else if app.transport == TransportMode::Kennel
                && matches!(app.target_states.get(t), Some(KennelUiState::ClaimedByMe))
            {
                Style::default().fg(Color::Green)
            } else if sel {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else if busy {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            };
            ListItem::new(format!("{prefix}{t}{suffix}")).style(style)
        })
        .collect();
    f.render_widget(
        List::new(items).block(Block::default().borders(Borders::ALL).title("TARGETS")),
        area,
    );
}

fn render_output(f: &mut ratatui::Frame, area: ratatui::layout::Rect, app: &mut App) {
    let inner_width = area.width.saturating_sub(2);
    let inner_height = area.height.saturating_sub(2);

    // Join all output lines + live streaming text (BTreeMap = deterministic order)
    let mut md = String::new();
    for line in &app.output {
        md.push_str(line);
        md.push('\n');
    }
    for (target, text) in &app.streaming_text {
        if !text.is_empty() {
            md.push_str(&format!("[{target}] {text}▌\n"));
        }
    }

    // Render markdown → styled ratatui Text
    let text = tui_markdown::from_str(&md);

    // Compute actual rendered height by counting lines after wrapping.
    // This is the real content height that Paragraph will render.
    let content_height: u16 = text
        .lines
        .iter()
        .map(|line| {
            let line_width: u16 = line.width() as u16;
            if inner_width == 0 {
                1
            } else {
                line_width.div_ceil(inner_width).max(1)
            }
        })
        .sum();

    // Auto-scroll: set offset so the bottom of content is visible
    if app.auto_scroll {
        app.scroll_offset = content_height.saturating_sub(inner_height);
    }

    f.render_widget(
        Paragraph::new(text)
            .block(Block::default().borders(Borders::ALL).title("OUTPUT"))
            .wrap(Wrap { trim: false })
            .scroll((app.scroll_offset, 0)),
        area,
    );
}

fn render_input(f: &mut ratatui::Frame, area: ratatui::layout::Rect, app: &App) {
    let label = app.input_label();
    // Build multiline display: first line gets the label, rest are continuation
    let mut lines: Vec<Line> = Vec::new();
    for (i, part) in app.input.split('\n').enumerate() {
        if i == 0 {
            lines.push(Line::from(format!("{label}{part}")));
        } else {
            lines.push(Line::from(format!("  {part}")));
        }
    }
    // Cursor on the last line
    if let Some(last) = lines.last_mut() {
        last.spans
            .push(Span::styled("_", Style::default().fg(Color::DarkGray)));
    }
    let title = "COMMAND  [Ctrl+J: newline]".to_string();
    f.render_widget(
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title(title))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn find_flag(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

fn discover_local_ip(host: &str, port: u16) -> anyhow::Result<String> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").context("bind UDP probe socket")?;
    sock.connect(format!("{host}:{port}"))
        .with_context(|| format!("UDP probe to {host}:{port}"))?;
    Ok(sock.local_addr()?.ip().to_string())
}
