//! # 035 — MDM TUX: Controller TUI
//!
//! Ratatui-based terminal UI for managing remote Meerkat agents.
//!
//! ```text
//! mdm-tux <PORT> [--model MODEL]
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

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind, KeyModifiers,
};
use meerkat::{AgentBuildConfig, AgentEvent, AgentFactory, DynAgent, LlmClient};
use meerkat_comms::MessageKind;
use meerkat_comms::agent::CommsToolDispatcher;
use meerkat_comms::agent::types::CommsContent;
use meerkat_core::{AgentSessionStore, AgentToolDispatcher};
use meerkat_mob_mcp::{AgentMobToolSurfaceFactory, MobMcpState};
use meerkat_store::{JsonlStore, StoreAdapter};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use tokio::sync::{mpsc, oneshot};

use mdm_tux::machines::tux_attach::{self, ClaimFacts as AttachClaimFacts};
use mdm_tux::machines::tux_claim::{Effect as ClaimEffect, State as ClaimState};
use mdm_tux::machines::tux_kennel_control::{
    self, Effect as TkcEffect, Event as TkcEvent, State as TuxKennelState,
};
use mdm_tux::machines::tux_runtime::{Event as RuntimeEvent, Phase as RuntimePhase};
use mdm_tux::machines::tux_runtime_registry as runtime_registry;
use mdm_tux::{
    ClaimGrant, CommsNode, DirectControlPayload, KennelPayload, LeaseRef, LeaseTerminationReason,
    ListScope, ProviderKind, TargetListEntry, auto_detect, build_signed_envelope,
    direct_control_request, load_or_generate_keypair, parse_direct_control_message, read_envelope,
    run_registration_server, verify_envelope, write_envelope,
};
use tokio::io::BufReader;
use tokio::net::TcpStream;

/// Timeout for router.send() calls. Prevents a black-holed target from wedging
/// the command loop (TCP connect can hang for minutes without this).
const SEND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

struct HiveAgentState {
    agent: DynAgent,
    _mob_state: Arc<MobMcpState>,
}

// ── Event / command types ─────────────────────────────────────────────────────

enum AppCommand {
    Send {
        target_peer: String,
        target_id: String,
        body: String,
    },
    RunHive {
        prompt: String,
    },
    /// Slash command sent to a target (no busy indicator set).
    SlashCmd {
        target_peer: String,
        target_id: String,
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
    RebindTargets { target_ids: Vec<String> },
    ReleaseTarget { lease_id: String },
    AttachConfirmed { lease_id: String },
    Shutdown { reply: oneshot::Sender<()> },
}

enum TuiEvent {
    CommsMessage {
        from_target_id: Option<String>,
        from_name: Option<String>,
        body: String,
    },
    Heartbeat {
        from_target_id: String,
    },
    HivePlanDone(String),
    HiveError(String),
    SendFailed {
        target_id: Option<String>,
        message: String,
    },
    TargetRegistered {
        target_id: String,
        name: String,
    },
    /// Streaming agent event from a target (text delta, tool call, etc.).
    StreamEvent {
        from_target_id: String,
        from_name: String,
        event: AgentEvent,
    },
    KennelAvailableTargets(Vec<TargetListEntry>),
    KennelMineTargets(Vec<TargetListEntry>),
    KennelClaimGranted(Vec<ClaimGrant>),
    KennelClaimReleased {
        target_id: String,
        lease_ref: LeaseRef,
        reason: LeaseTerminationReason,
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

type ClaimRegistry = parking_lot::RwLock<TuxKennelState>;
type RuntimeRegistry = parking_lot::RwLock<runtime_registry::State>;

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
    /// Live streaming text per target (not yet in `output`).
    /// BTreeMap for deterministic render order across frames.
    streaming_text: BTreeMap<String, String>,
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

    fn ensure_section_gap(&mut self) {
        if self.output.back().is_some_and(|line| !line.is_empty()) {
            self.output.push_back(String::new());
        }
    }

    fn push_markdown_section<I>(&mut self, lines: I)
    where
        I: IntoIterator<Item = String>,
    {
        self.ensure_section_gap();
        for line in lines {
            self.push(line);
        }
    }

    fn push_user_turn(&mut self, target: &str, body: &str) {
        self.push_markdown_section([
            format!("**You -> {target}**"),
            String::new(),
            body.trim().to_string(),
        ]);
    }

    fn push_remote_turn(&mut self, target: &str, body: &str) {
        self.push_markdown_section([
            format!("**{target}**"),
            String::new(),
            body.trim().to_string(),
        ]);
    }

    fn push_notice(&mut self, title: &str, body: &str) {
        self.push_markdown_section([format!("_{}_", title), body.trim().to_string()]);
    }

    /// Flush streaming text for a target into the output buffer.
    fn flush_streaming(&mut self, target: &str) {
        if let Some(text) = self.streaming_text.remove(target)
            && !text.is_empty()
        {
            self.push_remote_turn(target, &text);
        }
    }

    fn selected_target_name(&self) -> Option<&str> {
        self.targets.get(self.selected).map(String::as_str)
    }

    fn selected_target_name_owned(&self) -> Option<String> {
        self.selected_target_name().map(ToString::to_string)
    }

    fn restore_selection(&mut self, previous_target: Option<String>) {
        if self.targets.is_empty() {
            self.selected = 0;
            return;
        }
        if let Some(previous_target) = previous_target
            && let Some(index) = self
                .targets
                .iter()
                .position(|target| target == &previous_target)
        {
            self.selected = index;
            return;
        }
        self.selected = self.selected.min(self.targets.len().saturating_sub(1));
    }

    /// The label prefix shown before the input text (e.g. `[target] > `).
    fn input_label(&self, runtime: &RuntimeRegistry) -> String {
        match self.mode {
            Mode::Direct if !self.targets.is_empty() => {
                let t = &self.targets[self.selected];
                let runtime_id = self.target_runtime_id(t);
                if self.transport == TransportMode::Kennel
                    && !matches!(self.target_states.get(t), Some(KennelUiState::ClaimedByMe))
                {
                    format!("[{t} unavailable] > ")
                } else if runtime.read().is_busy(runtime_id) {
                    format!("[{t} ...processing] > ")
                } else if runtime.read().phase(runtime_id) == RuntimePhase::Stalled {
                    format!("[{t} stalled] > ")
                } else {
                    format!("[{t}] > ")
                }
            }
            Mode::Hive if self.hive_planning => "[hive: planning...] > ".into(),
            Mode::Hive => "[hive] > ".into(),
            _ => "[waiting for targets...] > ".into(),
        }
    }

    fn target_runtime_id<'a>(&'a self, target_name: &'a str) -> &'a str {
        self.target_ids
            .get(target_name)
            .map(String::as_str)
            .unwrap_or(target_name)
    }

    fn target_name_for_id(&self, target_id: &str) -> Option<String> {
        self.target_ids.iter().find_map(|(name, id)| {
            if id == target_id {
                Some(name.clone())
            } else {
                None
            }
        })
    }

    fn display_name_for_target(&self, target_id: &str) -> String {
        self.target_name_for_id(target_id)
            .unwrap_or_else(|| target_id.to_string())
    }
}

fn is_known_slash_command(input: &str) -> bool {
    input.starts_with('/')
        && matches!(
            input.split_once(' ').map(|(cmd, _)| cmd).unwrap_or(input),
            "/new" | "/resume" | "/help" | "/claim" | "/release"
        )
}

fn short_id(id: &str) -> String {
    if id.len() <= 18 {
        id.to_string()
    } else {
        format!("{}...{}", &id[..8], &id[id.len() - 6..])
    }
}

fn format_age_ms(age_ms: i64) -> String {
    if age_ms < 0 {
        "just now".into()
    } else if age_ms < 1_000 {
        "just now".into()
    } else if age_ms < 60_000 {
        format!("{}s ago", age_ms / 1_000)
    } else if age_ms < 3_600_000 {
        format!("{}m ago", age_ms / 60_000)
    } else {
        format!("{}h ago", age_ms / 3_600_000)
    }
}

fn target_runtime_snapshot(
    app: &App,
    runtime: &RuntimeRegistry,
    target_name: &str,
) -> (RuntimePhase, Option<i64>, bool, bool) {
    let runtime = runtime.read();
    let runtime_id = app.target_runtime_id(target_name);
    let phase = runtime.phase(runtime_id);
    let age_ms = runtime
        .last_activity_ms(runtime_id)
        .map(|last| chrono::Utc::now().timestamp_millis() - last);
    let offline = age_ms.is_some_and(|age| age > Duration::from_secs(60).as_millis() as i64);
    let stale = phase == RuntimePhase::Stalled
        || age_ms.is_some_and(|age| age >= Duration::from_secs(35).as_millis() as i64);
    (phase, age_ms, stale, offline)
}

fn runtime_status_label(phase: RuntimePhase, stale: bool, offline: bool) -> &'static str {
    if offline {
        "offline"
    } else if stale {
        "stalled"
    } else {
        match phase {
            RuntimePhase::Idle => "idle",
            RuntimePhase::Dispatching => "dispatching",
            RuntimePhase::Running => "running",
            RuntimePhase::Stalled => "stalled",
        }
    }
}

fn control_status_label(app: &App, target_name: &str) -> String {
    match app.transport {
        TransportMode::Direct => "direct".into(),
        TransportMode::Kennel => match app.target_states.get(target_name).copied() {
            Some(KennelUiState::Available) => "available".into(),
            Some(KennelUiState::Attaching) => app
                .attaching_since
                .get(target_name)
                .map(|started| format!("attaching {}s", started.elapsed().as_secs()))
                .unwrap_or_else(|| "attaching".into()),
            Some(KennelUiState::ClaimedByMe) => "claimed by me".into(),
            Some(KennelUiState::ClaimUncertain) => "claim uncertain".into(),
            None => "unknown".into(),
        },
    }
}

fn composer_status(app: &App, runtime: &RuntimeRegistry) -> (String, String, Color) {
    match app.mode {
        Mode::Hive if app.hive_planning => (
            "Planner busy".into(),
            "Wait for the current hive plan to finish before sending another prompt.".into(),
            Color::Yellow,
        ),
        Mode::Hive => (
            "Hive ready".into(),
            "Prompt the planner and it will decide which targets to involve.".into(),
            Color::Green,
        ),
        Mode::Direct if app.targets.is_empty() => (
            "No targets".into(),
            if app.transport == TransportMode::Kennel {
                "Waiting for kennel inventory or claimed targets to appear.".into()
            } else {
                "Start mdm-target or check direct registration.".into()
            },
            Color::Yellow,
        ),
        Mode::Direct => {
            let target_name = app.selected_target_name().unwrap_or("target");
            let runtime_id = app.target_runtime_id(target_name);
            let runtime = runtime.read();
            let busy = runtime.is_busy(runtime_id);
            let is_slash = is_known_slash_command(&app.input);
            if app.transport == TransportMode::Kennel
                && !matches!(
                    app.target_states.get(target_name),
                    Some(KennelUiState::ClaimedByMe | KennelUiState::ClaimUncertain)
                )
            {
                (
                    "Claim required".into(),
                    format!("Use /claim before sending commands to {target_name}."),
                    Color::Yellow,
                )
            } else if busy && is_slash {
                (
                    "Run in progress".into(),
                    format!("{target_name} is busy; slash commands wait for the current run."),
                    Color::Yellow,
                )
            } else if runtime.phase(runtime_id) == RuntimePhase::Stalled {
                (
                    "Target stalled".into(),
                    format!("{target_name} stopped responding; messages still queue."),
                    Color::Yellow,
                )
            } else if busy {
                (
                    "Queued send".into(),
                    format!("{target_name} is already working; new messages queue behind it."),
                    Color::Cyan,
                )
            } else {
                (
                    "Ready".into(),
                    format!("Enter sends to {target_name}. /help shows session commands."),
                    Color::Green,
                )
            }
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
        eprintln!("Usage: mdm-tux <PORT> [--model MODEL --provider PROVIDER]");
        eprintln!(
            "   or: mdm-tux --kennel HOST:PORT --listen PORT [--advertise IP] [--model MODEL --provider PROVIDER]"
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
    let provider_override = parse_provider_override(&args)?;

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
        let provider = provider_override
            .as_ref()
            .context("--model requires --provider in override mode")?;
        println!("hive model: {m} ({provider})");
    } else if let Some((ref m, ref p, _)) = auto_detect() {
        println!("hive model: {m} ({p})");
    } else {
        println!("hive model: (none — Direct mode only)");
    }
    println!("\nStart targets with: mdm-target <THIS_IP>:{port}\n");

    // ── 3. Channels ───────────────────────────────────────────────────────────
    let (command_tx, mut command_rx) = mpsc::unbounded_channel::<AppCommand>();
    let (event_tx, event_rx) = mpsc::channel::<TuiEvent>(256);
    let (reg_target_tx, mut reg_target_rx) = mpsc::channel::<mdm_tux::DirectRegistrationNotice>(32);

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
                            from_target_id: msg.from_pubkey.to_peer_id(),
                        }
                    }
                    _ if matches!(
                        parse_direct_control_message(&msg),
                        Ok(Some(DirectControlPayload::StreamEvent { .. }))
                    ) =>
                    {
                        match parse_direct_control_message(&msg) {
                            Ok(Some(DirectControlPayload::StreamEvent { event })) => {
                                TuiEvent::StreamEvent {
                                    from_target_id: msg.from_pubkey.to_peer_id(),
                                    from_name: msg.from_peer.clone(),
                                    event,
                                }
                            }
                            _ => TuiEvent::CommsMessage {
                                from_target_id: Some(msg.from_pubkey.to_peer_id()),
                                from_name: Some(msg.from_peer.clone()),
                                body: msg.to_user_message_text(),
                            },
                        }
                    }
                    _ => TuiEvent::CommsMessage {
                        from_target_id: Some(msg.from_pubkey.to_peer_id()),
                        from_name: Some(msg.from_peer.clone()),
                        body: msg.to_user_message_text(),
                    },
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
    let hive_provider = provider_override.or_else(|| auto_detect().map(|(_, p, _)| p));
    let hive_session_dir = session_dir.clone();

    tokio::spawn({
        let event_tx = event_tx.clone();
        let router = router.clone();
        let trusted_shared = trusted_shared.clone();
        async move {
            let mut hive_agent: Option<HiveAgentState> = None;
            // Per-target send queues: preserves FIFO within each target while
            // preventing one slow/unreachable target from blocking others.
            let mut target_senders: HashMap<String, mpsc::Sender<MessageKind>> = HashMap::new();

            while let Some(cmd) = command_rx.recv().await {
                match cmd {
                    AppCommand::Send {
                        target_peer,
                        target_id,
                        body,
                    } => {
                        let tx = target_senders
                            .entry(target_peer.clone())
                            .or_insert_with(|| {
                                spawn_target_sender(
                                    target_peer.clone(),
                                    target_id.clone(),
                                    router.clone(),
                                    event_tx.clone(),
                                )
                            });
                        let _ = tx.send(MessageKind::Message { body, blocks: None }).await;
                    }
                    AppCommand::SlashCmd {
                        target_peer,
                        target_id,
                        cmd,
                    } => {
                        let params = serde_json::json!({ "cmd": cmd });
                        let tx = target_senders
                            .entry(target_peer.clone())
                            .or_insert_with(|| {
                                spawn_target_sender(
                                    target_peer.clone(),
                                    target_id.clone(),
                                    router.clone(),
                                    event_tx.clone(),
                                )
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
                            let Some(provider) = hive_provider else {
                                let _ = event_tx
                                    .send(TuiEvent::HiveError("missing hive provider".into()))
                                    .await;
                                continue;
                            };
                            match build_hive(
                                &hive_session_dir,
                                model,
                                provider,
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
                        let ev = match agent.agent.run(prompt.into()).await {
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
            while let Some(notice) = reg_target_rx.recv().await {
                let _ = event_tx
                    .send(TuiEvent::TargetRegistered {
                        target_id: notice.target_id,
                        name: notice.name,
                    })
                    .await;
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
        streaming_text: BTreeMap::new(),
        scroll_offset: 0,
        auto_scroll: true,
    };
    let runtime: Arc<RuntimeRegistry> = Arc::new(parking_lot::RwLock::new(Default::default()));

    tokio::task::spawn_blocking(move || tui_loop(app, command_tx, event_rx, None, None, runtime))
        .await?
        .context("TUI loop")?;

    Ok(())
}

/// Spawn a dedicated sender task for one target. Messages are drained FIFO
/// with a per-send timeout, so one unreachable target cannot stall sends to
/// other healthy targets.
fn spawn_target_sender(
    target_peer: String,
    target_id: String,
    router: Arc<meerkat_comms::Router>,
    event_tx: mpsc::Sender<TuiEvent>,
) -> mpsc::Sender<MessageKind> {
    let (tx, mut rx) = mpsc::channel::<MessageKind>(32);
    tokio::spawn(async move {
        while let Some(kind) = rx.recv().await {
            let send_fut = router.send(&target_peer, kind);
            match tokio::time::timeout(SEND_TIMEOUT, send_fut).await {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => {
                    let _ = event_tx
                        .send(TuiEvent::SendFailed {
                            target_id: Some(target_id.clone()),
                            message: format!("send failed: {e}"),
                        })
                        .await;
                }
                Err(_) => {
                    let _ = event_tx
                        .send(TuiEvent::SendFailed {
                            target_id: Some(target_id.clone()),
                            message: "send timed out".into(),
                        })
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
    provider: ProviderKind,
    router: Arc<meerkat_comms::Router>,
    trusted: Arc<parking_lot::RwLock<meerkat_comms::TrustedPeers>>,
) -> anyhow::Result<HiveAgentState> {
    build_hive_with_llm_override(session_dir, model, provider, router, trusted, None).await
}

async fn build_hive_with_llm_override(
    session_dir: &std::path::Path,
    model: &str,
    provider: ProviderKind,
    router: Arc<meerkat_comms::Router>,
    trusted: Arc<parking_lot::RwLock<meerkat_comms::TrustedPeers>>,
    llm_client_override: Option<Arc<dyn LlmClient>>,
) -> anyhow::Result<HiveAgentState> {
    let factory = AgentFactory::new(session_dir).mob(true);
    let tools: Arc<dyn AgentToolDispatcher> = Arc::new(CommsToolDispatcher::new(router, trusted));
    let store = Arc::new(JsonlStore::new(session_dir.to_path_buf()));
    store.init().await?;
    let hive_store: Arc<dyn AgentSessionStore> = Arc::new(StoreAdapter::new(store));
    let mob_state = MobMcpState::new_in_memory();

    let mut build = AgentBuildConfig::new(model.to_string());
    build.provider = Some(meerkat_core::Provider::from_name(&provider.to_string()));
    build.system_prompt = Some(
        "You are a hive orchestrator for a fleet of remote machines. \
         Use the 'peers' tool to discover targets, then 'send' to dispatch \
         commands. Target replies appear in the TUX output panel; the user \
         will relay results if needed. Keep dispatches concise."
            .to_string(),
    );
    build.external_tools = Some(tools);
    build.session_store_override = Some(hive_store);
    build.override_builtins = Some(false);
    build.override_shell = Some(false);
    build.override_mob = Some(true);
    build.mob_tools = Some(Arc::new(AgentMobToolSurfaceFactory::new(Arc::clone(
        &mob_state,
    ))));
    build.llm_client_override = llm_client_override;

    let agent = factory
        .build_agent(build, &meerkat_core::Config::default())
        .await?;
    Ok(HiveAgentState {
        agent,
        _mob_state: mob_state,
    })
}

fn current_attached_target_ids(claims: &ClaimRegistry) -> Vec<String> {
    claims.read().current_attached_target_ids()
}

fn claim_state_for_name(
    app: &App,
    claims: &ClaimRegistry,
    target_name: &str,
) -> Option<ClaimState> {
    let target_id = app.target_ids.get(target_name)?;
    claims.read().claims.get(target_id).cloned()
}

fn attach_claim_facts(claims: &ClaimRegistry, target_id: &str) -> Option<AttachClaimFacts> {
    let state = claims.read().claims.get(target_id).cloned()?;
    Some(AttachClaimFacts {
        target_id: state.target_id().to_string(),
        target_name: state.target_name().to_string(),
        lease_id: state.lease_id()?.to_string(),
        target_pubkey: state.target_pubkey()?.to_string(),
    })
}

fn rebuild_kennel_projection(app: &mut App, claims: &ClaimRegistry) {
    let selected_target = app.selected_target_name_owned();
    app.targets.clear();
    app.target_ids.clear();
    app.target_states.clear();
    app.target_leases.clear();
    app.attaching_since.clear();

    let now_ms = chrono::Utc::now().timestamp_millis();
    for state in claims.read().claims.values() {
        let target_name = state.target_name().to_string();
        app.targets.push(target_name.clone());
        app.target_ids
            .insert(target_name.clone(), state.target_id().to_string());
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
    app.targets.sort();
    app.targets.dedup();
    app.restore_selection(selected_target);
}

fn reconcile_available_targets(
    claims: &ClaimRegistry,
    router: &Arc<meerkat_comms::Router>,
    targets: &[TargetListEntry],
) {
    let _ = apply_tkc_event(
        claims,
        router,
        None,
        TkcEvent::SeenAvailableList {
            targets: targets.to_vec(),
        },
        |_| (),
    );
}

fn reconcile_mine_targets(
    claims: &ClaimRegistry,
    router: &Arc<meerkat_comms::Router>,
    targets: &[TargetListEntry],
) {
    let _ = apply_tkc_event(
        claims,
        router,
        None,
        TkcEvent::SeenMineList {
            targets: targets.to_vec(),
        },
        |_| (),
    );
}

fn apply_claim_effects(
    claims: &ClaimRegistry,
    router: &Arc<meerkat_comms::Router>,
    kennel_tx: Option<&mpsc::UnboundedSender<KennelClientCommand>>,
    effects: &[TkcEffect],
) {
    for effect in effects {
        match effect {
            TkcEffect::Claim {
                target_id,
                effect:
                    ClaimEffect::EnsureTrustedPeer {
                        target_pubkey,
                        target_direct_addr,
                    },
            } => {
                if let Ok(pubkey) = meerkat_comms::identity::PubKey::from_peer_id(target_pubkey) {
                    let target_name = claims
                        .read()
                        .claims
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
            TkcEffect::Claim {
                effect: ClaimEffect::RemoveTrustedPeer { target_pubkey },
                ..
            } => {
                if let Ok(pubkey) = meerkat_comms::identity::PubKey::from_peer_id(target_pubkey) {
                    router.remove_trusted_peer(&pubkey);
                }
            }
            TkcEffect::Claim {
                effect: ClaimEffect::SendClaimToKennel { target_id },
                ..
            } => {
                if let Some(kennel_tx) = kennel_tx {
                    let _ = kennel_tx.send(KennelClientCommand::ClaimTarget {
                        target_id: target_id.clone(),
                    });
                }
            }
            TkcEffect::Claim {
                effect: ClaimEffect::SendClaimAck { lease_id },
                ..
            } => {
                if let Some(kennel_tx) = kennel_tx {
                    let _ = kennel_tx.send(KennelClientCommand::ClaimAck {
                        lease_id: lease_id.clone(),
                    });
                }
            }
            TkcEffect::Claim {
                effect: ClaimEffect::SendAttachConfirmed { lease_id },
                ..
            } => {
                if let Some(kennel_tx) = kennel_tx {
                    let _ = kennel_tx.send(KennelClientCommand::AttachConfirmed {
                        lease_id: lease_id.clone(),
                    });
                }
            }
            TkcEffect::Claim {
                effect: ClaimEffect::SendReleaseToKennel { lease_id },
                ..
            } => {
                if let Some(kennel_tx) = kennel_tx {
                    let _ = kennel_tx.send(KennelClientCommand::ReleaseTarget {
                        lease_id: lease_id.clone(),
                    });
                }
            }
            TkcEffect::RebindTargets { target_ids } => {
                if let Some(kennel_tx) = kennel_tx
                    && !target_ids.is_empty()
                {
                    let _ = kennel_tx.send(KennelClientCommand::RebindTargets {
                        target_ids: target_ids.clone(),
                    });
                }
            }
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

fn apply_tkc_event<R, F>(
    claims: &ClaimRegistry,
    router: &Arc<meerkat_comms::Router>,
    kennel_tx: Option<&mpsc::UnboundedSender<KennelClientCommand>>,
    event: TkcEvent,
    inspect: F,
) -> Option<R>
where
    F: FnOnce(&TuxKennelState) -> R,
{
    let (effects, result) = {
        let mut guard = claims.write();
        let current = guard.clone();
        let Ok((new_state, effects)) = tux_kennel_control::transition(current, event.clone())
        else {
            eprintln!("[tux] invalid kennel-control transition: {event:?}");
            return None;
        };
        let result = inspect(&new_state);
        *guard = new_state;
        (effects, result)
    };
    apply_claim_effects(claims, router, kennel_tx, &effects);
    Some(result)
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
    hive_provider: Option<ProviderKind>,
    hive_session_dir: PathBuf,
) {
    let mut hive_agent: Option<HiveAgentState> = None;
    let mut target_senders: HashMap<String, mpsc::Sender<MessageKind>> = HashMap::new();

    while let Some(cmd) = command_rx.recv().await {
        match cmd {
            AppCommand::Send {
                target_peer,
                target_id,
                body,
            } => {
                let tx = target_senders
                    .entry(target_peer.clone())
                    .or_insert_with(|| {
                        spawn_target_sender(
                            target_peer.clone(),
                            target_id.clone(),
                            router.clone(),
                            event_tx.clone(),
                        )
                    });
                let _ = tx.send(MessageKind::Message { body, blocks: None }).await;
            }
            AppCommand::SlashCmd {
                target_peer,
                target_id,
                cmd,
            } => {
                let params = serde_json::json!({ "cmd": cmd });
                let tx = target_senders
                    .entry(target_peer.clone())
                    .or_insert_with(|| {
                        spawn_target_sender(
                            target_peer.clone(),
                            target_id.clone(),
                            router.clone(),
                            event_tx.clone(),
                        )
                    });
                let _ = tx
                    .send(MessageKind::Request {
                        intent: "command".into(),
                        params,
                    })
                    .await;
            }
            AppCommand::ClaimTarget { target_id } => {
                let _ = apply_tkc_event(
                    &claims,
                    &router,
                    Some(&kennel_tx),
                    TkcEvent::UserClaimTarget { target_id },
                    |_| (),
                );
            }
            AppCommand::ReleaseTarget { lease_id } => {
                let _ = apply_tkc_event(
                    &claims,
                    &router,
                    Some(&kennel_tx),
                    TkcEvent::UserReleaseLease { lease_id },
                    |_| (),
                );
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
                    let Some(provider) = hive_provider else {
                        let _ = event_tx
                            .send(TuiEvent::HiveError("missing hive provider".into()))
                            .await;
                        continue;
                    };
                    match build_hive(
                        &hive_session_dir,
                        model,
                        provider,
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
                let ev = match agent.agent.run(prompt.into()).await {
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
        if verify_envelope(&env).is_err() {
            let _ = event_tx
                .send(TuiEvent::HiveError(
                    "[kennel] invalid signed register reply".into(),
                ))
                .await;
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        }
        if !matches!(env.payload, KennelPayload::TuxRegistered) {
            let _ = event_tx
                .send(TuiEvent::HiveError(format!(
                    "[kennel] unexpected register reply: {:?}",
                    env.payload
                )))
                .await;
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        }
        let _ = apply_tkc_event(
            &claims,
            &router,
            Some(&kennel_cmd_tx),
            TkcEvent::KennelConnected,
            |_| (),
        );

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
            let _ = apply_tkc_event(
                &claims,
                &router,
                Some(&kennel_cmd_tx),
                TkcEvent::KennelDisconnected,
                |_| (),
            );
            let _ = event_tx
                .send(TuiEvent::SendFailed {
                    target_id: None,
                    message: "[kennel] disconnected".into(),
                })
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
                    let lease_ids: Vec<String> = claims.read().claims.values().filter_map(|state| {
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
                    if verify_envelope(&env).is_err() {
                        let _ = event_tx
                            .send(TuiEvent::SendFailed {
                                target_id: None,
                                message: "[kennel] invalid signed control frame".into(),
                            })
                            .await;
                        break;
                    }
                    match env.payload {
                        KennelPayload::TargetList { scope: ListScope::Available, targets } => {
                            let _ = event_tx.send(TuiEvent::KennelAvailableTargets(targets)).await;
                        }
                        KennelPayload::TargetList { scope: ListScope::Mine, targets } => {
                            let _ = event_tx.send(TuiEvent::KennelMineTargets(targets)).await;
                        }
                        KennelPayload::ClaimGranted { claims: grants } => {
                            let _ = apply_tkc_event(
                                &claims,
                                &router,
                                Some(&kennel_cmd_tx),
                                TkcEvent::ClaimGranted {
                                    claims: grants.clone(),
                                    now_ms: chrono::Utc::now().timestamp_millis(),
                                },
                                |_| (),
                            );
                            let _ = event_tx.send(TuiEvent::KennelClaimGranted(grants)).await;
                        }
                        KennelPayload::ClaimReleased {
                            target_id,
                            lease_ref,
                            reason,
                        } => {
                            let _ = apply_tkc_event(
                                &claims,
                                &router,
                                Some(&kennel_cmd_tx),
                                TkcEvent::ClaimReleased {
                                    target_id: target_id.clone(),
                                    lease_ref: lease_ref.clone(),
                                },
                                |_| (),
                            );
                            let _ = event_tx
                                .send(TuiEvent::KennelClaimReleased {
                                    target_id,
                                    lease_ref,
                                    reason,
                                })
                                .await;
                        }
                        KennelPayload::TargetLost {
                            target_id,
                            lease_ref,
                        } => {
                            let _ = apply_tkc_event(
                                &claims,
                                &router,
                                Some(&kennel_cmd_tx),
                                TkcEvent::TargetLost {
                                    target_id: target_id.clone(),
                                    lease_ref: lease_ref.clone(),
                                },
                                |_| (),
                            );
                            let lease_text = match lease_ref {
                                LeaseRef::Known { lease_id } => format!(" ({lease_id})"),
                                LeaseRef::PendingRebind => " (pending rebind)".to_string(),
                            };
                            let _ = event_tx
                                .send(TuiEvent::CommsMessage {
                                    from_target_id: Some(target_id.clone()),
                                    from_name: None,
                                    body: format!("[kennel] target lost: {target_id}{lease_text}"),
                                })
                                .await;
                        }
                        KennelPayload::LeaseRebound { lease_id, target_id, target_pubkey, target_direct_addr, .. } => {
                            let target_name = apply_tkc_event(
                                &claims,
                                &router,
                                Some(&kennel_cmd_tx),
                                TkcEvent::LeaseRebound {
                                    target_id: target_id.clone(),
                                    new_lease_id: lease_id.clone(),
                                    target_pubkey,
                                    target_direct_addr,
                                },
                                |new_state| {
                                    new_state
                                        .claims
                                        .get(&target_id)
                                        .map(|state| state.target_name().to_string())
                                },
                            )
                            .flatten();
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
                        KennelClientCommand::RebindTargets { target_ids } => {
                            if !send_kennel_message(
                                &mut write_half,
                                &router,
                                &tux_id,
                                KennelPayload::RebindTargets { target_ids },
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
                                .claims
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

        let _ = apply_tkc_event(
            &claims,
            &router,
            Some(&kennel_cmd_tx),
            TkcEvent::KennelDisconnected,
            |_| (),
        );
        let _ = event_tx
            .send(TuiEvent::SendFailed {
                target_id: None,
                message: "[kennel] disconnected".into(),
            })
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
    let provider_override = parse_provider_override(args)?;
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
    let claims: Arc<ClaimRegistry> = Arc::new(parking_lot::RwLock::new(TuxKennelState::default()));

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
                                from_target_id: msg.from_pubkey.to_peer_id(),
                            })
                            .await;
                    }
                    _ if matches!(
                        parse_direct_control_message(&msg),
                        Ok(Some(DirectControlPayload::AttachRequest { .. }))
                    ) =>
                    {
                        let Ok(Some(DirectControlPayload::AttachRequest {
                            lease_id,
                            target_id,
                        })) = parse_direct_control_message(&msg)
                        else {
                            continue;
                        };
                        let request_pubkey = msg.from_pubkey.to_peer_id();
                        let Ok((attach_state, effects)) = tux_attach::transition(
                            tux_attach::State::Idle,
                            tux_attach::Event::AttachRequested {
                                request_peer: msg.from_peer.clone(),
                                request_target_id: target_id.clone(),
                                request_lease_id: lease_id.clone(),
                                request_pubkey,
                                claim: attach_claim_facts(&claims, &target_id),
                            },
                        ) else {
                            eprintln!(
                                "[tux] invalid attach gate transition for target {target_id}"
                            );
                            continue;
                        };

                        for effect in effects {
                            match effect {
                                tux_attach::Effect::SendAttachAck {
                                    request_peer,
                                    lease_id,
                                } => {
                                    let Ok(kind) =
                                        direct_control_request(&DirectControlPayload::AttachAck {
                                            lease_id: lease_id.clone(),
                                        })
                                    else {
                                        continue;
                                    };
                                    let ack_sent = matches!(
                                        tokio::time::timeout(
                                            SEND_TIMEOUT,
                                            router.send(&request_peer, kind),
                                        )
                                        .await,
                                        Ok(Ok(_))
                                    );
                                    let followup = if ack_sent {
                                        tux_attach::Event::AckDelivered
                                    } else {
                                        tux_attach::Event::AckDeliveryFailed
                                    };
                                    let Ok((_, followup_effects)) =
                                        tux_attach::transition(attach_state.clone(), followup)
                                    else {
                                        eprintln!(
                                            "[tux] invalid attach followup transition for target {target_id}"
                                        );
                                        continue;
                                    };
                                    if !ack_sent {
                                        let _ = event_tx
                                            .send(TuiEvent::SendFailed {
                                                target_id: Some(target_id.clone()),
                                                message: "[kennel] failed to deliver attach ack"
                                                    .into(),
                                            })
                                            .await;
                                    }
                                    for followup_effect in followup_effects {
                                        if let tux_attach::Effect::ConfirmAttached {
                                            target_id,
                                            target_name,
                                            lease_id,
                                        } = followup_effect
                                        {
                                            let target_name = apply_tkc_event(
                                                &claims,
                                                &router,
                                                Some(&kennel_tx),
                                                TkcEvent::AttachConfirmed {
                                                    target_id: target_id.clone(),
                                                    lease_id: lease_id.clone(),
                                                },
                                                |new_state| {
                                                    new_state
                                                        .claims
                                                        .get(&target_id)
                                                        .map(|state| {
                                                            state.target_name().to_string()
                                                        })
                                                        .unwrap_or(target_name.clone())
                                                },
                                            );
                                            if let Some(target_name) = target_name {
                                                let _ = event_tx
                                                    .send(TuiEvent::KennelAttached {
                                                        target_name,
                                                        lease_id,
                                                    })
                                                    .await;
                                            }
                                        }
                                    }
                                }
                                tux_attach::Effect::ConfirmAttached { .. } => {}
                            }
                        }
                    }
                    _ if matches!(
                        parse_direct_control_message(&msg),
                        Ok(Some(DirectControlPayload::StreamEvent { .. }))
                    ) =>
                    {
                        if let Ok(Some(DirectControlPayload::StreamEvent { event })) =
                            parse_direct_control_message(&msg)
                        {
                            let _ = event_tx
                                .send(TuiEvent::StreamEvent {
                                    from_target_id: msg.from_pubkey.to_peer_id(),
                                    from_name: msg.from_peer.clone(),
                                    event,
                                })
                                .await;
                        }
                    }
                    _ => {
                        let _ = event_tx
                            .send(TuiEvent::CommsMessage {
                                from_target_id: Some(msg.from_pubkey.to_peer_id()),
                                from_name: Some(msg.from_peer.clone()),
                                body: msg.to_user_message_text(),
                            })
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
    let hive_provider = provider_override.or_else(|| auto_detect().map(|(_, p, _)| p));
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
                hive_provider,
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
        streaming_text: BTreeMap::new(),
        scroll_offset: 0,
        auto_scroll: true,
    };
    let runtime: Arc<RuntimeRegistry> = Arc::new(parking_lot::RwLock::new(Default::default()));

    let claims_for_tui = claims.clone();
    let router_for_tui = router.clone();
    let runtime_for_tui = runtime.clone();
    tokio::task::spawn_blocking(move || {
        tui_loop(
            app,
            command_tx,
            event_rx,
            Some(claims_for_tui),
            Some(router_for_tui),
            runtime_for_tui,
        )
    })
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
    router: Option<Arc<meerkat_comms::Router>>,
    runtime: Arc<RuntimeRegistry>,
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
                        &runtime,
                    );
                }
                Event::Paste(text) => {
                    let line_count = text.lines().count();
                    app.input.push_str(&text);
                    if line_count > 1 {
                        app.push_notice("pasted input", &format!("{line_count} lines"));
                    }
                }
                _ => {}
            }
        }

        while let Ok(ev) = event_rx.try_recv() {
            match ev {
                TuiEvent::CommsMessage {
                    from_target_id,
                    from_name,
                    body,
                } => {
                    if let Some(target_id) = from_target_id {
                        runtime.write().apply(
                            &target_id,
                            RuntimeEvent::LivenessObserved {
                                now_ms: chrono::Utc::now().timestamp_millis(),
                            },
                        );
                    }
                    let clean = body
                        .strip_prefix("[COMMS MESSAGE from ")
                        .and_then(|rest| rest.split_once("]\n"))
                        .map(|(from, body)| (from.to_string(), body.to_string()))
                        .or_else(|| from_name.map(|from| (from, body.clone())));
                    if let Some((from, body)) = clean {
                        app.push_remote_turn(&from, &body);
                    } else {
                        app.push_notice("message", &body);
                    }
                }
                TuiEvent::Heartbeat { from_target_id } => {
                    runtime.write().apply(
                        &from_target_id,
                        RuntimeEvent::LivenessObserved {
                            now_ms: chrono::Utc::now().timestamp_millis(),
                        },
                    );
                }
                TuiEvent::HivePlanDone(s) => {
                    app.push_notice("hive dispatched", &s);
                    app.hive_planning = false;
                }
                TuiEvent::HiveError(e) => {
                    app.push_notice("hive error", &e);
                    app.hive_planning = false;
                }
                TuiEvent::SendFailed { target_id, message } => {
                    if let Some(target_id) = target_id {
                        let display_name = app.display_name_for_target(&target_id);
                        runtime.write().apply(
                            &target_id,
                            RuntimeEvent::SendFailed {
                                now_ms: chrono::Utc::now().timestamp_millis(),
                            },
                        );
                        app.push_notice("send error", &format!("[{display_name}] {message}"));
                    } else {
                        app.push_notice("send error", &message);
                    }
                }
                TuiEvent::TargetRegistered { target_id, name } => {
                    let selected_target = app.selected_target_name_owned();
                    if !app.targets.contains(&name) {
                        app.targets.push(name.clone());
                        app.targets.sort();
                    }
                    app.restore_selection(selected_target);
                    app.target_ids.insert(name.clone(), target_id.clone());
                    app.streaming_text.remove(&name);
                    runtime.write().apply(
                        &target_id,
                        RuntimeEvent::TargetRegistered {
                            now_ms: chrono::Utc::now().timestamp_millis(),
                        },
                    );
                    app.push_notice("registered", &format!("target `{name}` connected"));
                }
                TuiEvent::StreamEvent {
                    from_target_id,
                    from_name,
                    event,
                } => {
                    handle_stream_event(&mut app, &runtime, &from_target_id, &from_name, &event);
                }
                TuiEvent::KennelAvailableTargets(targets) => {
                    if let (Some(claims), Some(router)) = (claims.as_deref(), router.as_ref()) {
                        reconcile_available_targets(claims, router, &targets);
                    }
                }
                TuiEvent::KennelMineTargets(targets) => {
                    if let (Some(claims), Some(router)) = (claims.as_deref(), router.as_ref()) {
                        reconcile_mine_targets(claims, router, &targets);
                    }
                }
                TuiEvent::KennelClaimGranted(claims) => {
                    let selected_target = app.selected_target_name_owned();
                    for claim in claims {
                        app.target_ids
                            .insert(claim.target_name.clone(), claim.target_id.clone());
                        app.target_leases
                            .insert(claim.target_name.clone(), claim.lease_id.clone());
                        app.target_states
                            .insert(claim.target_name.clone(), KennelUiState::Attaching);
                        app.attaching_since
                            .insert(claim.target_name.clone(), Instant::now());
                        runtime.write().apply(
                            &claim.target_id,
                            RuntimeEvent::LivenessObserved {
                                now_ms: chrono::Utc::now().timestamp_millis(),
                            },
                        );
                        if !app.targets.contains(&claim.target_name) {
                            app.targets.push(claim.target_name);
                        }
                    }
                    app.targets.sort();
                    app.restore_selection(selected_target);
                }
                TuiEvent::KennelClaimReleased {
                    target_id,
                    lease_ref,
                    reason,
                } => {
                    let lease_text = match &lease_ref {
                        LeaseRef::Known { lease_id } => lease_id.clone(),
                        LeaseRef::PendingRebind => "pending rebind".into(),
                    };
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
                        runtime.write().apply(
                            &target_id,
                            RuntimeEvent::TargetDisconnected {
                                now_ms: chrono::Utc::now().timestamp_millis(),
                            },
                        );
                        app.flush_streaming(&name);
                        app.push_notice(
                            "kennel released claim",
                            &format!("`{name}` ({lease_text}): {reason}"),
                        );
                    } else {
                        app.push_notice(
                            "kennel released claim",
                            &format!("`{target_id}` ({lease_text}): {reason}"),
                        );
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
                    app.push_notice("attached", &target_name);
                }
            }
        }

        if app.transport == TransportMode::Kennel
            && let Some(claims) = claims.as_deref()
        {
            rebuild_kennel_projection(&mut app, claims);
        }

        let stale_after_ms = Duration::from_secs(30).as_millis() as i64;
        let now_ms = chrono::Utc::now().timestamp_millis();
        let runtime_targets = runtime.read().target_ids();
        for target in runtime_targets {
            let before = runtime.read().phase(&target);
            runtime.write().apply(
                &target,
                RuntimeEvent::TickStale {
                    now_ms,
                    stale_after_ms,
                },
            );
            if before != RuntimePhase::Stalled
                && runtime.read().phase(&target) == RuntimePhase::Stalled
            {
                let display_name = app
                    .target_name_for_id(&target)
                    .unwrap_or_else(|| target.clone());
                app.flush_streaming(&display_name);
                app.push_notice("target unresponsive", &display_name);
            }
        }

        terminal.draw(|f| render(f, &mut app, &runtime))?;
        if app.quit {
            break;
        }
    }

    Ok(())
}

fn handle_stream_event(
    app: &mut App,
    runtime: &RuntimeRegistry,
    from_target_id: &str,
    from_name: &str,
    event: &AgentEvent,
) {
    let now_ms = chrono::Utc::now().timestamp_millis();
    match event {
        AgentEvent::RunStarted { .. } => {
            runtime
                .write()
                .apply(from_target_id, RuntimeEvent::RunStarted { now_ms });
            app.streaming_text
                .insert(from_name.to_string(), String::new());
        }
        AgentEvent::TextDelta { delta, .. } => {
            runtime
                .write()
                .apply(from_target_id, RuntimeEvent::ActivityObserved { now_ms });
            app.streaming_text
                .entry(from_name.to_string())
                .or_default()
                .push_str(delta);
        }
        AgentEvent::TextComplete { content, .. } => {
            runtime
                .write()
                .apply(from_target_id, RuntimeEvent::ActivityObserved { now_ms });
            // TextComplete carries the authoritative final text.
            // Preserve source attribution so multi-target output is distinguishable.
            app.streaming_text.remove(from_name);
            if !content.is_empty() {
                app.push_remote_turn(from_name, content);
            }
        }
        AgentEvent::ToolCallRequested { name, args, .. } => {
            runtime
                .write()
                .apply(from_target_id, RuntimeEvent::ActivityObserved { now_ms });
            app.flush_streaming(from_name);
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
            app.push_markdown_section([
                format!("**tool** `{name}`"),
                "```text".into(),
                display,
                "```".into(),
            ]);
        }
        AgentEvent::ToolExecutionCompleted {
            name,
            result,
            is_error,
            duration_ms,
            ..
        } => {
            runtime
                .write()
                .apply(from_target_id, RuntimeEvent::ActivityObserved { now_ms });
            app.push_markdown_section(format_tool_completion(
                name,
                result,
                *is_error,
                *duration_ms,
            ));
        }
        AgentEvent::RunCompleted { .. } => {
            app.flush_streaming(from_name);
            runtime
                .write()
                .apply(from_target_id, RuntimeEvent::RunCompleted { now_ms });
            app.push(String::new()); // blank line between turns
        }
        AgentEvent::RunFailed { error, .. } => {
            app.flush_streaming(from_name);
            app.push_notice(&format!("{from_name} error"), error);
            runtime
                .write()
                .apply(from_target_id, RuntimeEvent::RunFailed { now_ms });
        }
        _ => {}
    }
}

fn decode_common_escapes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

fn normalize_tool_text(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let mut normalized = if trimmed.matches("\\n").count() >= 2 && trimmed.lines().count() <= 2 {
        decode_common_escapes(trimmed)
    } else {
        trimmed.to_string()
    };
    normalized = normalized.replace("\r\n", "\n");

    let mut lines = Vec::new();
    let mut blank_run = 0;
    for raw in normalized.lines() {
        let line = raw.trim_end();
        if line.is_empty() {
            blank_run += 1;
            if blank_run <= 1 {
                lines.push(String::new());
            }
            continue;
        }
        blank_run = 0;
        lines.push(line.to_string());
    }
    lines.join("\n").trim().to_string()
}

fn truncate_tool_block(text: &str, max_lines: usize, max_cols: usize) -> Vec<String> {
    let normalized = normalize_tool_text(text);
    if normalized.is_empty() {
        return Vec::new();
    }
    let mut lines = Vec::new();
    let all_lines: Vec<&str> = normalized.lines().collect();
    let hidden = all_lines.len().saturating_sub(max_lines);
    for line in all_lines.into_iter().take(max_lines) {
        let trimmed = if line.chars().count() > max_cols {
            let clipped: String = line.chars().take(max_cols.saturating_sub(1)).collect();
            format!("{clipped}…")
        } else {
            line.to_string()
        };
        lines.push(trimmed);
    }
    if hidden > 0 {
        lines.push(format!("... ({hidden} more lines)"));
    }
    lines
}

fn emit_tool_block(lines: &mut Vec<String>, title: Option<String>, body: Vec<String>) {
    if body.is_empty() {
        return;
    }
    if let Some(title) = title {
        lines.push(title);
    }
    lines.push("```text".into());
    lines.extend(body);
    lines.push("```".into());
}

fn format_tool_completion(
    name: &str,
    result: &str,
    is_error: bool,
    duration_ms: u64,
) -> Vec<String> {
    let mut lines = Vec::new();
    let status = if is_error { "failed" } else { "completed" };
    lines.push(format!("**tool** `{name}` {status} in {duration_ms}ms"));

    if result.trim().is_empty() {
        if !is_error {
            lines.push("_no output_".into());
        }
        return lines;
    }

    if let Ok(json) = serde_json::from_str::<serde_json::Value>(result) {
        if let Some(obj) = json.as_object() {
            let stdout = obj
                .get("stdout")
                .and_then(|v| v.as_str())
                .map(|s| truncate_tool_block(s, 20, 140))
                .unwrap_or_default();
            let stderr = obj
                .get("stderr")
                .and_then(|v| v.as_str())
                .map(|s| truncate_tool_block(s, 12, 140))
                .unwrap_or_default();
            let message = obj
                .get("message")
                .and_then(|v| v.as_str())
                .map(|s| truncate_tool_block(s, 12, 140))
                .unwrap_or_default();
            let exit_code = obj.get("exit_code").and_then(|v| v.as_i64());
            let timed_out = obj
                .get("timed_out")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let has_message = !message.is_empty();
            let has_stdout = !stdout.is_empty();
            let has_stderr = !stderr.is_empty();

            if exit_code.is_some() || timed_out {
                let mut summary = Vec::new();
                if let Some(exit_code) = exit_code {
                    summary.push(format!("exit code: {exit_code}"));
                }
                if timed_out {
                    summary.push("timed out".into());
                }
                if !summary.is_empty() {
                    lines.push(summary.join("  •  "));
                }
            }

            if !message.is_empty() {
                emit_tool_block(&mut lines, Some("message".into()), message);
            }
            if !stdout.is_empty() {
                emit_tool_block(&mut lines, Some("stdout".into()), stdout);
            }
            if !stderr.is_empty() {
                emit_tool_block(&mut lines, Some("stderr".into()), stderr);
            }

            if !has_message && !has_stdout && !has_stderr {
                let pretty =
                    serde_json::to_string_pretty(&json).unwrap_or_else(|_| json.to_string());
                emit_tool_block(&mut lines, None, truncate_tool_block(&pretty, 18, 140));
            }
            return lines;
        }

        let pretty = serde_json::to_string_pretty(&json).unwrap_or_else(|_| json.to_string());
        emit_tool_block(&mut lines, None, truncate_tool_block(&pretty, 18, 140));
        return lines;
    }

    emit_tool_block(&mut lines, None, truncate_tool_block(result, 18, 140));
    lines
}

fn timeline_max_scroll(content_height: u16, inner_height: u16) -> u16 {
    if content_height <= inner_height {
        0
    } else {
        content_height
            .saturating_sub(inner_height)
            .saturating_add(2)
    }
}

fn scroll_timeline_up(app: &mut App, lines: u16) {
    app.auto_scroll = false;
    app.scroll_offset = app.scroll_offset.saturating_sub(lines);
}

fn scroll_timeline_down(app: &mut App, lines: u16) {
    app.auto_scroll = false;
    app.scroll_offset = app.scroll_offset.saturating_add(lines);
}

fn handle_key(
    app: &mut App,
    code: KeyCode,
    modifiers: KeyModifiers,
    command_tx: &mpsc::UnboundedSender<AppCommand>,
    claims: Option<&ClaimRegistry>,
    runtime: &RuntimeRegistry,
) {
    match code {
        KeyCode::Tab => {
            app.mode = if app.mode == Mode::Direct {
                Mode::Hive
            } else {
                Mode::Direct
            };
        }
        KeyCode::Char('p')
            if modifiers.contains(KeyModifiers::CONTROL) && app.mode == Mode::Direct =>
        {
            app.selected = app.selected.saturating_sub(1);
        }
        KeyCode::Char('n')
            if modifiers.contains(KeyModifiers::CONTROL) && app.mode == Mode::Direct =>
        {
            app.selected = (app.selected + 1).min(app.targets.len().saturating_sub(1));
        }
        KeyCode::Up if app.mode == Mode::Direct && modifiers.is_empty() => {
            app.selected = app.selected.saturating_sub(1);
        }
        KeyCode::Down if app.mode == Mode::Direct && modifiers.is_empty() => {
            app.selected = (app.selected + 1).min(app.targets.len().saturating_sub(1));
        }
        KeyCode::PageUp => {
            scroll_timeline_up(app, 10);
        }
        KeyCode::PageDown => {
            scroll_timeline_down(app, 10);
        }
        KeyCode::Char('b') if modifiers.contains(KeyModifiers::CONTROL) => {
            scroll_timeline_up(app, 10);
        }
        KeyCode::Char('f') if modifiers.contains(KeyModifiers::CONTROL) => {
            scroll_timeline_down(app, 10);
        }
        KeyCode::Up if modifiers.contains(KeyModifiers::ALT) => {
            scroll_timeline_up(app, 3);
        }
        KeyCode::Down if modifiers.contains(KeyModifiers::ALT) => {
            scroll_timeline_down(app, 3);
        }
        KeyCode::Home => {
            app.auto_scroll = false;
            app.scroll_offset = 0;
        }
        KeyCode::End => {
            app.auto_scroll = true;
        }
        KeyCode::Char('u') if modifiers.contains(KeyModifiers::CONTROL) => {
            app.input.clear();
        }
        KeyCode::Char('l') if modifiers.contains(KeyModifiers::CONTROL) => {
            app.output.clear();
            app.streaming_text.clear();
            app.scroll_offset = 0;
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
            let is_slash = is_known_slash_command(&app.input);
            // Check if submission is possible before consuming input.
            let selected_busy = !app.targets.is_empty()
                && runtime
                    .read()
                    .is_busy(app.target_runtime_id(&app.targets[app.selected]));
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
                            app.push_notice("session reset", "hive");
                            let _ = command_tx.send(AppCommand::ResetHive);
                            return;
                        }
                        if app.targets.is_empty() {
                            return;
                        }
                        let target = app.targets[app.selected].clone();
                        let target_id = app.target_runtime_id(&target).to_string();
                        // Don't clear output yet — wait for the target to confirm.
                        // Clearing before the command succeeds would lose unrelated
                        // history if the send fails or other targets are connected.
                        app.push_user_turn(&target, "/new");
                        let _ = command_tx.send(AppCommand::SlashCmd {
                            target_peer: target,
                            target_id,
                            cmd: "NEW_SESSION".into(),
                        });
                    }
                    "/resume" if arg.is_empty() => {
                        if app.targets.is_empty() {
                            return;
                        }
                        let target = app.targets[app.selected].clone();
                        let target_id = app.target_runtime_id(&target).to_string();
                        app.push_user_turn(&target, "/resume");
                        let _ = command_tx.send(AppCommand::SlashCmd {
                            target_peer: target,
                            target_id,
                            cmd: "LIST_SESSIONS".into(),
                        });
                    }
                    "/resume" => {
                        if app.targets.is_empty() {
                            return;
                        }
                        let target = app.targets[app.selected].clone();
                        let target_id = app.target_runtime_id(&target).to_string();
                        app.push_user_turn(&target, &format!("/resume {arg}"));
                        let _ = command_tx.send(AppCommand::SlashCmd {
                            target_peer: target,
                            target_id,
                            cmd: format!("RESUME {arg}"),
                        });
                    }
                    "/help" => {
                        app.push_markdown_section(["**Commands**".into()]);
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
                        app.push_user_turn(&target, "/claim");
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
                        app.push_user_turn(&target, "/release");
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
                    let target_id = app.target_runtime_id(&target).to_string();
                    if runtime.read().is_busy(&target_id) {
                        // Target is busy — message sent immediately via comms,
                        // queued in the target's inbox for after the current run.
                        app.push_user_turn(
                            &target,
                            &format!("{body}\n\n_queued behind current run_"),
                        );
                    } else {
                        app.push_user_turn(&target, &body);
                        runtime.write().apply(
                            &target_id,
                            RuntimeEvent::DispatchRequested {
                                now_ms: chrono::Utc::now().timestamp_millis(),
                            },
                        );
                    }
                    AppCommand::Send {
                        target_peer: target,
                        target_id,
                        body,
                    }
                }
                Mode::Hive => {
                    app.hive_planning = true;
                    app.push_user_turn("hive", &body);
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

fn render(f: &mut ratatui::Frame, app: &mut App, runtime: &RuntimeRegistry) {
    // Input box height: 2 (borders) + command lines + 1 contextual hint line.
    // Derive the label length from the actual label string so the
    // first-line available width is always correct.
    let label = app.input_label(runtime);
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
    let input_height = (input_lines.clamp(1, 7) as u16) + 3;

    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(input_height),
            Constraint::Length(2),
        ])
        .split(f.area());

    render_header(f, outer[0], app, runtime);

    // Main: targets + output
    let main = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(outer[1]);

    let sidebar = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(9)])
        .split(main[0]);

    render_targets(f, sidebar[0], app, runtime);
    render_target_inspector(f, sidebar[1], app, runtime);
    render_output(f, main[1], app);
    render_input(f, outer[2], app, runtime);
    render_footer(f, outer[3], app, runtime);
}

fn render_header(
    f: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    app: &App,
    runtime: &RuntimeRegistry,
) {
    let direct_style = if app.mode == Mode::Direct {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let hive_style = if app.mode == Mode::Hive {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let transport_style = match app.transport {
        TransportMode::Direct => Style::default()
            .fg(Color::Black)
            .bg(Color::Green)
            .add_modifier(Modifier::BOLD),
        TransportMode::Kennel => Style::default()
            .fg(Color::Black)
            .bg(Color::Magenta)
            .add_modifier(Modifier::BOLD),
    };
    let selected_text = app
        .selected_target_name()
        .map(|name| {
            let (phase, _, stale, offline) = target_runtime_snapshot(app, runtime, name);
            format!(
                "  selected: {} ({})",
                name,
                runtime_status_label(phase, stale, offline)
            )
        })
        .unwrap_or_else(|| "  selected: none".into());
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " TUX — Meerkat Device Manager ",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                match app.transport {
                    TransportMode::Direct => " Direct Transport ",
                    TransportMode::Kennel => " Kennel Transport ",
                },
                transport_style,
            ),
            Span::raw(" "),
            Span::styled(" Direct ", direct_style),
            Span::raw(" "),
            Span::styled(" Hive ", hive_style),
            Span::styled(
                format!("  {} targets{}", app.targets.len(), selected_text),
                Style::default().fg(Color::DarkGray),
            ),
        ])),
        area,
    );
}

fn render_targets(
    f: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    app: &App,
    runtime: &RuntimeRegistry,
) {
    let runtime = runtime.read();
    let claimed_count = app
        .target_states
        .values()
        .filter(|state| matches!(state, KennelUiState::ClaimedByMe))
        .count();
    let items: Vec<ListItem> = app
        .targets
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let sel = app.mode == Mode::Direct && i == app.selected;
            let target_id = app.target_runtime_id(t);
            let busy = runtime.is_busy(target_id);
            let phase = runtime.phase(target_id);
            let stalled = phase == RuntimePhase::Stalled;
            let prefix = if sel { "> " } else { "  " };
            let suffix = if app.transport == TransportMode::Kennel {
                match app.target_states.get(t).copied() {
                    Some(KennelUiState::Attaching) => " attaching",
                    Some(KennelUiState::ClaimedByMe) if busy => " mine + running",
                    Some(KennelUiState::ClaimedByMe) if stalled => " mine + stalled",
                    Some(KennelUiState::ClaimedByMe) => " mine",
                    Some(KennelUiState::ClaimUncertain) => " claim?",
                    _ if stalled => " stalled",
                    _ if busy => " running",
                    _ => "",
                }
            } else if stalled {
                " stalled"
            } else if busy {
                " running"
            } else {
                ""
            };

            let age_ms = runtime
                .last_activity_ms(target_id)
                .map(|last| chrono::Utc::now().timestamp_millis() - last)
                .unwrap_or(i64::MAX);
            let style = if age_ms > Duration::from_secs(60).as_millis() as i64 {
                // Offline — override all other styling
                Style::default().fg(Color::DarkGray)
            } else if age_ms >= Duration::from_secs(35).as_millis() as i64 || stalled {
                // Stale
                Style::default().fg(Color::Yellow)
            } else if sel {
                Style::default()
                    .fg(Color::Green)
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
            } else if app.transport == TransportMode::Kennel
                && matches!(app.target_states.get(t), Some(KennelUiState::ClaimedByMe))
            {
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD)
            } else if busy {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            };
            let icon = if age_ms > Duration::from_secs(60).as_millis() as i64 {
                "○"
            } else if stalled {
                "▲"
            } else if phase == RuntimePhase::Running || phase == RuntimePhase::Dispatching {
                "●"
            } else {
                "•"
            };
            ListItem::new(format!("{prefix}{icon} {t}{suffix}")).style(style)
        })
        .collect();
    let title = if app.transport == TransportMode::Kennel {
        format!(
            "TARGETS  {} total / {} mine",
            app.targets.len(),
            claimed_count
        )
    } else {
        format!("TARGETS  {}", app.targets.len())
    };
    f.render_widget(
        List::new(items).block(Block::default().borders(Borders::ALL).title(title)),
        area,
    );
}

fn render_target_inspector(
    f: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    app: &App,
    runtime: &RuntimeRegistry,
) {
    let lines = if let Some(target_name) = app.selected_target_name() {
        let target_id = app.target_runtime_id(target_name);
        let (phase, age_ms, stale, offline) = target_runtime_snapshot(app, runtime, target_name);
        let runtime_label = runtime_status_label(phase, stale, offline);
        let control_label = control_status_label(app, target_name);
        let lease = app
            .target_leases
            .get(target_name)
            .map(|lease| short_id(lease))
            .unwrap_or_else(|| "none".into());
        let activity = age_ms.map(format_age_ms).unwrap_or_else(|| "never".into());
        let action_hint: String = match app.mode {
            Mode::Hive => {
                "Enter sends to the hive planner; selected target is context only.".into()
            }
            Mode::Direct
                if app.transport == TransportMode::Kennel
                    && !matches!(
                        app.target_states.get(target_name),
                        Some(KennelUiState::ClaimedByMe | KennelUiState::ClaimUncertain)
                    ) =>
            {
                "Use /claim before sending commands.".into()
            }
            Mode::Direct if runtime.read().is_busy(target_id) => {
                "Messages queue while running; /new and /resume wait.".into()
            }
            Mode::Direct => "Ready for a command, /new, /resume, or /help.".into(),
        };
        vec![
            Line::from(Span::styled(
                target_name.to_string(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(format!("runtime:   {runtime_label}")),
            Line::from(format!("control:   {control_label}")),
            Line::from(format!("activity:  {activity}")),
            Line::from(format!("target id: {}", short_id(target_id))),
            Line::from(format!("lease:     {lease}")),
            Line::from(""),
            Line::from(action_hint),
        ]
    } else {
        vec![
            Line::from(Span::styled(
                "No target selected",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("Connect a target to start using Direct mode."),
            Line::from(""),
            Line::from("Helpful commands:"),
            Line::from("/help  /new  /resume"),
            Line::from(""),
            Line::from("Tab switches between Direct and Hive."),
        ]
    };
    f.render_widget(
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title("INSPECTOR"))
            .wrap(Wrap { trim: false }),
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

    if md.trim().is_empty() {
        let placeholder = match app.mode {
            Mode::Direct => {
                "No activity yet.\n\nPick a target, type a command, or use /help for session commands."
            }
            Mode::Hive => {
                "No hive activity yet.\n\nDescribe the outcome you want and the planner will fan work out to targets."
            }
        };
        f.render_widget(
            Paragraph::new(placeholder)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("TIMELINE  idle"),
                )
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(Color::DarkGray)),
            area,
        );
        return;
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

    let max_scroll = timeline_max_scroll(content_height, inner_height);
    if app.auto_scroll {
        app.scroll_offset = max_scroll;
    } else {
        app.scroll_offset = app.scroll_offset.min(max_scroll);
    }
    let title = if app.auto_scroll {
        format!("TIMELINE  live  {} lines", app.output.len())
    } else {
        format!(
            "TIMELINE  scrolled  line {} / {}",
            app.scroll_offset.saturating_add(1),
            max_scroll.saturating_add(1)
        )
    };

    f.render_widget(
        Paragraph::new(text)
            .block(Block::default().borders(Borders::ALL).title(title))
            .wrap(Wrap { trim: false })
            .scroll((app.scroll_offset, 0)),
        area,
    );
}

fn render_input(
    f: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    app: &App,
    runtime: &RuntimeRegistry,
) {
    let label = app.input_label(runtime);
    let (status, hint, tone) = composer_status(app, runtime);
    // Build multiline display: first line gets the label, rest are continuation
    let mut lines: Vec<Line> = Vec::new();
    if app.input.is_empty() {
        lines.push(Line::from(vec![
            Span::raw(label.clone()),
            Span::styled(
                match app.mode {
                    Mode::Direct => "Type a command or /help",
                    Mode::Hive => "Describe the task for the planner",
                },
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    } else {
        for (i, part) in app.input.split('\n').enumerate() {
            if i == 0 {
                lines.push(Line::from(format!("{label}{part}")));
            } else {
                lines.push(Line::from(format!("  {part}")));
            }
        }
    }
    // Cursor on the last line
    if let Some(last) = lines.last_mut() {
        last.spans
            .push(Span::styled("_", Style::default().fg(Color::DarkGray)));
    }
    lines.push(Line::from(Span::styled(
        hint,
        Style::default().fg(Color::DarkGray),
    )));
    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(Line::from(vec![
                        Span::raw("COMMAND  "),
                        Span::styled(
                            status,
                            Style::default().fg(tone).add_modifier(Modifier::BOLD),
                        ),
                    ])),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_footer(
    f: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    app: &App,
    runtime: &RuntimeRegistry,
) {
    let (_, hint, tone) = composer_status(app, runtime);
    let selected_hint = app
        .selected_target_name()
        .map(|target| format!("selected {target}"))
        .unwrap_or_else(|| "no target selected".into());
    let lines = vec![
        Line::from(vec![
            Span::styled("Status: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(hint, Style::default().fg(tone)),
            Span::styled(
                format!(
                    "  •  {}  •  {}",
                    selected_hint,
                    if app.auto_scroll {
                        "timeline live"
                    } else {
                        "timeline paused"
                    }
                ),
                Style::default().fg(Color::DarkGray),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                "[Tab] mode  [Up/Down or Ctrl+N/P] select  [Enter] send  [Shift+Enter/Ctrl+J] newline  ",
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                "[Ctrl+U] clear input  [Ctrl+L] clear timeline  [PgUp/PgDn] scroll  [Esc] quit",
                Style::default().fg(Color::DarkGray),
            ),
        ]),
    ];
    f.render_widget(Paragraph::new(lines), area);
}

fn find_flag(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

fn parse_provider_override(args: &[String]) -> anyhow::Result<Option<ProviderKind>> {
    let model = find_flag(args, "--model");
    let provider = find_flag(args, "--provider");
    match (model, provider) {
        (Some(_), Some(provider)) => Ok(Some(provider.parse()?)),
        (Some(_), None) => anyhow::bail!("--model requires --provider"),
        (None, Some(_)) => anyhow::bail!("--provider requires --model"),
        (None, None) => Ok(None),
    }
}

fn discover_local_ip(host: &str, port: u16) -> anyhow::Result<String> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").context("bind UDP probe socket")?;
    sock.connect(format!("{host}:{port}"))
        .with_context(|| format!("UDP probe to {host}:{port}"))?;
    Ok(sock.local_addr()?.ip().to_string())
}

#[cfg(test)]
mod tests {
    mod test_support {
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/test_support.rs"));
    }

    use super::{
        App, Mode, TransportMode, build_hive_with_llm_override, format_tool_completion,
        is_known_slash_command, normalize_tool_text, parse_provider_override, scroll_timeline_down,
        scroll_timeline_up, timeline_max_scroll,
    };
    use mdm_tux::ProviderKind;
    use meerkat::LlmClient;
    use meerkat_comms::identity::Keypair;
    use std::collections::{BTreeMap, HashMap, VecDeque};
    use std::sync::Arc;
    use test_support::CaptureClient;

    #[test]
    fn provider_override_requires_model_and_provider_together() {
        let err = parse_provider_override(&["--model".into(), "gpt-5.4".into()]).unwrap_err();
        assert!(err.to_string().contains("--model requires --provider"));

        let err = parse_provider_override(&["--provider".into(), "openai".into()]).unwrap_err();
        assert!(err.to_string().contains("--provider requires --model"));
    }

    #[test]
    fn provider_override_parses_explicit_provider() {
        let provider = parse_provider_override(&[
            "--model".into(),
            "gpt-5.4".into(),
            "--provider".into(),
            "openai".into(),
        ])
        .unwrap();
        assert_eq!(provider, Some(ProviderKind::Openai));
    }

    #[test]
    fn slash_command_guard_recognizes_supported_commands() {
        assert!(is_known_slash_command("/help"));
        assert!(is_known_slash_command("/resume 3"));
        assert!(is_known_slash_command("/claim"));
        assert!(!is_known_slash_command("/wat"));
        assert!(!is_known_slash_command("ls /tmp"));
    }

    #[test]
    fn restore_selection_prefers_same_target_after_resort() {
        let mut app = App {
            transport: TransportMode::Direct,
            mode: Mode::Direct,
            targets: vec!["beta".into(), "alpha".into(), "gamma".into()],
            target_ids: HashMap::new(),
            target_states: HashMap::new(),
            target_leases: HashMap::new(),
            attaching_since: HashMap::new(),
            selected: 0,
            input: String::new(),
            output: VecDeque::new(),
            hive_planning: false,
            quit: false,
            streaming_text: BTreeMap::new(),
            scroll_offset: 0,
            auto_scroll: true,
        };
        let previous = Some("beta".to_string());
        app.targets.sort();
        app.restore_selection(previous);
        assert_eq!(app.selected_target_name(), Some("beta"));
    }

    #[test]
    fn normalize_tool_text_decodes_escaped_newlines() {
        let text = "line one\\nline two\\nline three";
        assert_eq!(normalize_tool_text(text), "line one\nline two\nline three");
    }

    #[test]
    fn format_tool_completion_summarizes_shell_like_json() {
        let result = serde_json::json!({
            "stdout": "hello\\nworld\\nthird",
            "stderr": "",
            "exit_code": 0,
            "timed_out": false
        })
        .to_string();
        let lines = format_tool_completion("shell", &result, false, 42);
        let joined = lines.join("\n");
        assert!(joined.contains("**tool** `shell` completed in 42ms"));
        assert!(joined.contains("exit code: 0"));
        assert!(joined.contains("stdout"));
        assert!(joined.contains("hello"));
        assert!(joined.contains("world"));
    }

    #[test]
    fn timeline_scroll_helpers_disable_auto_follow() {
        let mut app = App {
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
            streaming_text: BTreeMap::new(),
            scroll_offset: 20,
            auto_scroll: true,
        };
        scroll_timeline_up(&mut app, 5);
        assert!(!app.auto_scroll);
        assert_eq!(app.scroll_offset, 15);
        scroll_timeline_down(&mut app, 3);
        assert_eq!(app.scroll_offset, 18);
    }

    #[test]
    fn timeline_max_scroll_keeps_bottom_slack() {
        assert_eq!(timeline_max_scroll(40, 10), 32);
        assert_eq!(timeline_max_scroll(8, 10), 0);
    }

    #[tokio::test]
    async fn hive_build_exposes_comms_and_mob_tools_without_shell() {
        let temp = tempfile::tempdir().unwrap();
        let node = mdm_tux::CommsNode::new(Keypair::generate());
        let capture: Arc<CaptureClient> = Arc::new(CaptureClient::default());

        let mut hive = build_hive_with_llm_override(
            temp.path(),
            "gpt-5.2",
            ProviderKind::Openai,
            node.router.clone(),
            node.trusted.clone(),
            Some(capture.clone() as Arc<dyn LlmClient>),
        )
        .await
        .unwrap();

        hive.agent
            .run("inspect hive tools".to_string().into())
            .await
            .unwrap();

        let tool_names = capture.tool_names();
        assert!(tool_names.iter().any(|name| name == "send"));
        assert!(tool_names.iter().any(|name| name == "peers"));
        assert!(tool_names.iter().any(|name| name == "delegate"));
        assert!(tool_names.iter().any(|name| name == "mob_list"));
        assert!(tool_names.iter().any(|name| name == "mob_check_member"));
        assert!(!tool_names.iter().any(|name| name == "shell"));
    }
}
