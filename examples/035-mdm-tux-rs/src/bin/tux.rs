//! # 035 — MDM TUX: Controller TUI
//!
//! Ratatui-based terminal UI for managing remote Meerkat agents.
//!
//! ```text
//! tux <PORT> [--model MODEL]
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
use tokio::sync::mpsc;

use mdm_tux::{
    CMD_PREFIX, CommsNode, HEARTBEAT_PREFIX, STREAM_PREFIX, auto_detect, build_llm_client,
    detect_provider, load_or_generate_keypair, run_registration_server,
};

// ── Event / command types ─────────────────────────────────────────────────────

enum AppCommand {
    Send { target: String, body: String },
    RunHive { prompt: String },
    /// Slash command sent to a target (no busy indicator set).
    SlashCmd { target: String, cmd: String },
    /// Drop the cached hive agent so the next RunHive starts fresh.
    ResetHive,
}

enum TuiEvent {
    CommsMessage(String),
    Heartbeat { from: String },
    HivePlanDone(String),
    HiveError(String),
    SendError(String),
    TargetRegistered { name: String },
    /// Streaming agent event from a target (text delta, tool call, etc.).
    StreamEvent { from: String, event: AgentEvent },
}

// ── TUI application state ─────────────────────────────────────────────────────

#[derive(PartialEq, Clone, Copy)]
enum Mode {
    Direct,
    Hive,
}

struct App {
    mode: Mode,
    targets: Vec<String>,
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
                if self.busy_targets.contains(t) {
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
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "warn,tui_markdown=error".into()),
        )
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        eprintln!("Usage: tux <PORT> [--model MODEL]");
        eprintln!("Direct mode: no API key needed");
        eprintln!("Hive mode: set ANTHROPIC_API_KEY, OPENAI_API_KEY, or GEMINI_API_KEY");
        std::process::exit(1);
    }
    let port: u16 = args[0].parse().context("PORT must be a number")?;
    anyhow::ensure!(port < 65535, "PORT must be < 65535 (registration uses PORT+1)");
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
    node.listen(&comms_addr).await?;

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
    println!("\nStart targets with: target <THIS_IP>:{port}\n");

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
                    meerkat_comms::agent::types::CommsMessage::from_inbox_item(
                        &item, &peers, true,
                    )
                };
                let Some(msg) = msg else {
                    continue;
                };

                let tui_event = match &msg.content {
                    CommsContent::Message { body, .. }
                        if body.starts_with(HEARTBEAT_PREFIX) =>
                    {
                        TuiEvent::Heartbeat { from: msg.from_peer.clone() }
                    }
                    CommsContent::Message { body, .. }
                        if body.starts_with(STREAM_PREFIX) =>
                    {
                        let json = &body[STREAM_PREFIX.len()..];
                        match serde_json::from_str::<AgentEvent>(json) {
                            Ok(event) => {
                                TuiEvent::StreamEvent { from: msg.from_peer.clone(), event }
                            }
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
    let hive_model = model_override.clone().or_else(|| auto_detect().map(|(m, _, _)| m));
    let hive_session_dir = session_dir.clone();

    tokio::spawn({
        let event_tx = event_tx.clone();
        let router = router.clone();
        let trusted_shared = trusted_shared.clone();
        async move {
            let mut hive_agent: Option<DynAgent> = None;

            while let Some(cmd) = command_rx.recv().await {
                match cmd {
                    AppCommand::Send { target, body } => {
                        let r = router.clone();
                        let ev = event_tx.clone();
                        tokio::spawn(async move {
                            if let Err(e) = r
                                .send(&target, MessageKind::Message { body, blocks: None })
                                .await
                            {
                                let _ = ev
                                    .send(TuiEvent::SendError(format!(
                                        "[{target}] send failed: {e}"
                                    )))
                                    .await;
                            }
                        });
                    }
                    AppCommand::SlashCmd { target, cmd } => {
                        // Send __CMD__-prefixed message. No busy indicator.
                        let r = router.clone();
                        let body = format!("{CMD_PREFIX}{cmd}");
                        let ev = event_tx.clone();
                        tokio::spawn(async move {
                            if let Err(e) = r
                                .send(&target, MessageKind::Message { body, blocks: None })
                                .await
                            {
                                let _ = ev
                                    .send(TuiEvent::SendError(format!(
                                        "[{target}] cmd failed: {e}"
                                    )))
                                    .await;
                            }
                        });
                    }
                    AppCommand::ResetHive => {
                        hive_agent = None;
                    }
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
                                    let _ =
                                        event_tx.send(TuiEvent::HiveError(e.to_string())).await;
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
        mode: Mode::Direct,
        targets: Vec::new(),
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

    tokio::task::spawn_blocking(move || tui_loop(app, command_tx, event_rx))
        .await?
        .context("TUI loop")?;

    Ok(())
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
    let tools: Arc<dyn AgentToolDispatcher> =
        Arc::new(CommsToolDispatcher::new(router, trusted));
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

// ── TUI loop (sync — no .await) ──────────────────────────────────────────────

fn tui_loop(
    mut app: App,
    command_tx: mpsc::UnboundedSender<AppCommand>,
    mut event_rx: mpsc::Receiver<TuiEvent>,
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
                    handle_key(&mut app, key.code, key.modifiers, &command_tx);
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
                    if let Some(name) = e.strip_prefix('[').and_then(|s| s.split_once(']').map(|(n, _)| n)) {
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
            }
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
                let bg = args.get("background").and_then(|v| v.as_bool()).unwrap_or(false);
                if bg { format!("$ {cmd} &") } else { format!("$ {cmd}") }
            } else {
                let s = args.to_string();
                let short: String = s.chars().take(80).collect();
                if s.len() > 80 { format!("{short}...") } else { short }
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
) {
    match code {
        KeyCode::Tab => {
            app.mode = if app.mode == Mode::Direct { Mode::Hive } else { Mode::Direct };
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
        KeyCode::Enter
            if modifiers.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) =>
        {
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
                    app.input.split_once(' ').map(|(c, _)| c).unwrap_or(&app.input),
                    "/new" | "/resume" | "/help"
                );
            // Check if submission is possible before consuming input.
            let selected_busy = !app.targets.is_empty()
                && app.busy_targets.contains(&app.targets[app.selected]);
            match app.mode {
                Mode::Direct if app.targets.is_empty() => return,
                // Slash commands blocked while target is busy (can't reset mid-run)
                Mode::Direct if selected_busy && is_slash => return,
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
                        if app.targets.is_empty() { return; }
                        let target = app.targets[app.selected].clone();
                        app.output.clear();
                        app.streaming_text.clear();
                        app.push(format!("> /new ({target})"));
                        let _ = command_tx.send(AppCommand::SlashCmd {
                            target,
                            cmd: "NEW_SESSION".into(),
                        });
                    }
                    "/resume" if arg.is_empty() => {
                        if app.targets.is_empty() { return; }
                        let target = app.targets[app.selected].clone();
                        app.push(format!("> /resume ({target})"));
                        let _ = command_tx.send(AppCommand::SlashCmd {
                            target,
                            cmd: "LIST_SESSIONS".into(),
                        });
                    }
                    "/resume" => {
                        if app.targets.is_empty() { return; }
                        let target = app.targets[app.selected].clone();
                        app.push(format!("> /resume {arg} ({target})"));
                        let _ = command_tx.send(AppCommand::SlashCmd {
                            target,
                            cmd: format!("RESUME {arg}"),
                        });
                    }
                    "/help" => {
                        app.push("**Commands:**".into());
                        app.push("  `/new`          — start a fresh session".into());
                        app.push("  `/resume`       — list past sessions".into());
                        app.push("  `/resume <N>`   — resume session N".into());
                        app.push("  `/help`         — show this help".into());
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
            if avail == 0 || line.is_empty() { 1 } else { line.len().div_ceil(avail) }
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
            let suffix = if busy { " [...]" } else { "" };

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
        last.spans.push(Span::styled("_", Style::default().fg(Color::DarkGray)));
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
