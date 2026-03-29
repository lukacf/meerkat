//! # 035 — MDM TUX: Controller TUI (Rust)
//!
//! Ratatui-based terminal UI for managing remote Meerkat agents.
//!
//! ## Modes
//! - **Direct** (default): send a command to a single selected target — no API key needed
//! - **Hive** (Tab): a local LLM agent fans commands out to any/all targets using the
//!   `send` comms tool; the hive agent is built lazily on first use
//!
//! ## API key requirements
//! - Direct mode: none — messages are dispatched via `router.send()` only
//! - Hive mode: `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, or `GEMINI_API_KEY` depending
//!   on the model configured in `tux.toml`; detected from the model name prefix
//!
//! ## Supported providers
//! Detected from the model name prefix (see `detect_provider` in lib.rs).
//! Provider examples: `claude-sonnet-4-6` → Anthropic, `gpt-5.2` → OpenAI,
//! `gemini-3.1-flash-lite` → Gemini.
//!
//! ## Ownership model
//! TUX's single `CommsManager` is split at startup:
//! - `Arc<Router>` → command task (outbound sends) + hive agent
//! - `CommsManager` (inbox) → background drain task (inbound replies)
//! The hive agent uses the shared router for sending only — it never owns
//! the inbox — so there is no listener conflict or duplicate identity.
//!
//! ## Async/sync bridge
//! The ratatui loop runs inside `tokio::task::spawn_blocking` (no `.await`).
//! Async comms events flow in via `mpsc::try_recv`. Commands flow out via
//! an unbounded `mpsc::send` (never drops silently).
//!
//! ## Hive semantics
//! `hive_planning` gates the UI while the LLM planning call is in-flight.
//! It clears when `hive_agent.run()` returns ("dispatch phase done").
//! Target replies arrive later and independently via the drain task —
//! no correlation mechanism exists for this example.
//!
//! ## Run
//! ```bash
//! # Direct mode only (no API key needed):
//! cargo run --bin tux -- tux.toml.example
//!
//! # Hive mode (API key required for model in tux.toml):
//! ANTHROPIC_API_KEY=... cargo run --bin tux -- tux.toml.example
//! ```

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use crossterm::event::{Event, KeyCode, KeyEventKind};
use meerkat::{AgentBuilder, AgentFactory, DynAgent};
use meerkat_comms::MessageKind;
use meerkat_comms::agent::{
    CommsManager, CommsManagerConfig, CommsToolDispatcher, spawn_tcp_listener,
};
use meerkat_core::{AgentSessionStore, AgentToolDispatcher};
use meerkat_store::{JsonlStore, StoreAdapter};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use tokio::sync::mpsc;

use mdm_tux::{
    TuxConfig, TargetEntry, build_llm_client, detect_provider, load_or_generate_keypair,
    targets_to_trusted_peers,
};

// ── Event / command types ─────────────────────────────────────────────────────

enum AppCommand {
    Send { target: String, body: String },
    RunHive { prompt: String },
}

enum TuiEvent {
    /// Message received from a target via comms.
    CommsMessage(String),
    /// Hive planner finished dispatching; replies still in flight via drain task.
    HivePlanDone(String),
    HiveError(String),
    /// A direct `router.send()` call failed (target offline, wrong pubkey, etc.).
    SendError(String),
}

// ── TUI application state ─────────────────────────────────────────────────────

#[derive(PartialEq, Clone, Copy)]
enum Mode {
    Direct,
    Hive,
}

struct App {
    mode: Mode,
    targets: Vec<TargetEntry>,
    selected: usize,
    input: String,
    /// Output ring-buffer (max 500 lines).
    output: VecDeque<String>,
    /// True only while the hive LLM planning call is in-flight.
    hive_planning: bool,
    quit: bool,
}

impl App {
    fn push(&mut self, line: String) {
        self.output.push_back(line);
        if self.output.len() > 500 {
            self.output.pop_front();
        }
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "warn".into()))
        .init();

    let config_path = std::env::args().nth(1).unwrap_or_else(|| "tux.toml".into());
    let raw = tokio::fs::read_to_string(&config_path)
        .await
        .with_context(|| format!("read config '{config_path}'"))?;
    let config: TuxConfig = toml::from_str(&raw)
        .with_context(|| format!("parse config '{config_path}'"))?;

    let data_dir = PathBuf::from(&config.data_dir);

    // ── 1. Load or generate TUX identity ─────────────────────────────────────
    let keypair = load_or_generate_keypair(&data_dir.join("identity")).await?;
    let pubkey = keypair.public_key();
    println!("=== TUX — Meerkat Device Manager ===");
    println!("my pubkey : {}", pubkey.to_peer_id());
    println!("listening : tcp://{}", config.listen_addr);
    println!("targets   : {}", config.targets.len());
    println!("hive model: {} ({})", config.model, detect_provider(&config.model));
    println!("(add pubkey + addr to target.toml on each managed machine)\n");
    println!("Direct mode needs no API key. Hive mode requires the matching key on first use.\n");

    // ── 2. Build trusted peers from targets ───────────────────────────────────
    let trusted = targets_to_trusted_peers(&config.targets)?;

    // ── 3. Create CommsManager ────────────────────────────────────────────────
    let comms_cfg = CommsManagerConfig::with_keypair(keypair).trusted_peers(trusted);
    let mut comms = CommsManager::new(comms_cfg)?;

    // ── 4. Start TCP listener ─────────────────────────────────────────────────
    let trusted_shared = comms.router().shared_trusted_peers();
    let _listener = spawn_tcp_listener(
        &config.listen_addr,
        comms.keypair_arc(),
        trusted_shared.clone(),
        comms.inbox_sender().clone(),
    )
    .await
    .context("spawn TCP listener")?;

    tokio::time::sleep(Duration::from_millis(50)).await;

    // ── 5. Split ownership ────────────────────────────────────────────────────
    // router → used by command handler (outbound sends) and hive agent
    // comms (inbox) → moved into drain task
    let router = comms.router().clone();

    let (command_tx, mut command_rx) = mpsc::unbounded_channel::<AppCommand>();
    let (event_tx, event_rx) = mpsc::channel::<TuiEvent>(256);

    // ── 6. Spawn comms drain task ─────────────────────────────────────────────
    tokio::spawn({
        let event_tx = event_tx.clone();
        async move {
            loop {
                let Some(msg) = comms.recv_message().await else { break };
                if event_tx
                    .send(TuiEvent::CommsMessage(msg.to_user_message_text()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        }
    });

    // ── 7. Spawn command handler ──────────────────────────────────────────────
    // The hive agent is built lazily on first RunHive — no API key required
    // at startup. Direct sends are spawned independently so they are never
    // serialised behind a hive LLM call.
    let session_dir = data_dir.join("sessions");
    tokio::fs::create_dir_all(&session_dir).await?;
    let hive_model = config.model.clone();
    let hive_session_dir = session_dir.clone();

    tokio::spawn(async move {
        let mut hive_agent: Option<DynAgent> = None;

        while let Some(cmd) = command_rx.recv().await {
            match cmd {
                AppCommand::Send { target, body } => {
                    // Spawn independently so Direct sends are never blocked by a hive run.
                    let r = router.clone();
                    let ev_tx = event_tx.clone();
                    tokio::spawn(async move {
                        if let Err(e) =
                            r.send(&target, MessageKind::Message { body, blocks: None }).await
                        {
                            let _ = ev_tx
                                .send(TuiEvent::SendError(format!("[{target}] send failed: {e}")))
                                .await;
                        }
                    });
                }
                AppCommand::RunHive { prompt } => {
                    // Build agent on first use — reads API key from env at this point.
                    if hive_agent.is_none() {
                        let provider = detect_provider(&hive_model);
                        let factory = AgentFactory::new(&hive_session_dir);

                        let llm = match build_llm_client(&factory, &hive_model, provider).await {
                            Ok(l) => l,
                            Err(e) => {
                                let _ = event_tx.send(TuiEvent::HiveError(e.to_string())).await;
                                continue;
                            }
                        };

                        let hive_tools: Arc<dyn AgentToolDispatcher> = Arc::new(
                            CommsToolDispatcher::new(router.clone(), trusted_shared.clone()),
                        );

                        let store = Arc::new(JsonlStore::new(hive_session_dir.clone()));
                        if let Err(e) = store.init().await {
                            let _ = event_tx
                                .send(TuiEvent::HiveError(format!("store init: {e}")))
                                .await;
                            continue;
                        }
                        let hive_store: Arc<dyn AgentSessionStore> =
                            Arc::new(StoreAdapter::new(store));

                        let agent: DynAgent = AgentBuilder::new()
                            .model(&hive_model)
                            .system_prompt(
                                "You are a hive orchestrator for a fleet of remote machines. \
                                 Use the 'peers' tool to discover available targets, then 'send' \
                                 to dispatch commands to them. Target responses arrive in the TUX \
                                 output panel; the user will relay results if needed. \
                                 Keep dispatches concise — each target will reply via comms.",
                            )
                            .build(llm, hive_tools, hive_store)
                            .await;

                        hive_agent = Some(agent);
                    }

                    // Safe: we either just built it or it was already Some.
                    let agent = hive_agent.as_mut().expect("hive agent was just built");
                    let ev = match agent.run(prompt.into()).await {
                        Ok(result) => TuiEvent::HivePlanDone(result.text),
                        Err(e) => TuiEvent::HiveError(e.to_string()),
                    };
                    let _ = event_tx.send(ev).await;
                }
            }
        }
    });

    // ── 8. Run TUI in blocking thread ─────────────────────────────────────────
    let app = App {
        mode: Mode::Direct,
        targets: config.targets.clone(),
        selected: 0,
        input: String::new(),
        output: VecDeque::new(),
        hive_planning: false,
        quit: false,
    };

    tokio::task::spawn_blocking(move || tui_loop(app, command_tx, event_rx))
        .await?
        .context("TUI loop")?;

    Ok(())
}

// ── TUI loop (sync — runs inside spawn_blocking, no .await) ──────────────────

fn tui_loop(
    mut app: App,
    command_tx: mpsc::UnboundedSender<AppCommand>,
    mut event_rx: mpsc::Receiver<TuiEvent>,
) -> anyhow::Result<()> {
    let mut terminal = ratatui::init();

    loop {
        // 1. Poll keyboard (16 ms ≈ 60 fps)
        if crossterm::event::poll(Duration::from_millis(16))? {
            if let Event::Key(key) = crossterm::event::read()? {
                if key.kind == KeyEventKind::Press {
                    handle_key(&mut app, key.code, &command_tx);
                }
            }
        }

        // 2. Drain async events (try_recv is non-blocking, no .await)
        while let Ok(ev) = event_rx.try_recv() {
            match ev {
                TuiEvent::CommsMessage(s) => app.push(s),
                TuiEvent::HivePlanDone(summary) => {
                    // Planner finished; target replies still in flight via drain task.
                    app.push(format!("[hive dispatched] {summary}"));
                    app.hive_planning = false;
                }
                TuiEvent::HiveError(e) => {
                    app.push(format!("[hive error] {e}"));
                    app.hive_planning = false;
                }
                TuiEvent::SendError(e) => {
                    app.push(format!("[send error] {e}"));
                }
            }
        }

        // 3. Render
        terminal.draw(|f| render(f, &app))?;

        if app.quit {
            break;
        }
    }

    ratatui::restore();
    Ok(())
}

fn handle_key(app: &mut App, code: KeyCode, command_tx: &mpsc::UnboundedSender<AppCommand>) {
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
        KeyCode::Char(c) => {
            app.input.push(c);
        }
        KeyCode::Backspace => {
            app.input.pop();
        }
        KeyCode::Enter if !app.input.is_empty() => {
            let body = std::mem::take(&mut app.input);
            let cmd = match app.mode {
                Mode::Direct if !app.targets.is_empty() => {
                    let target = app.targets[app.selected].name.clone();
                    app.push(format!("> [{target}] {body}"));
                    AppCommand::Send { target, body }
                }
                Mode::Hive if !app.hive_planning => {
                    app.hive_planning = true;
                    app.push(format!("> [hive] {body}"));
                    AppCommand::RunHive { prompt: body }
                }
                _ => return,
            };
            // Unbounded sender: never fails (no silent drops).
            let _ = command_tx.send(cmd);
        }
        KeyCode::Esc => {
            app.quit = true;
        }
        _ => {}
    }
}

// ── Rendering ─────────────────────────────────────────────────────────────────

fn render(f: &mut ratatui::Frame, app: &App) {
    let area = f.area();

    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0), Constraint::Length(3)])
        .split(area);

    render_title(f, outer[0], app);
    render_main(f, outer[1], app);
    render_input(f, outer[2], app);
}

fn render_title(f: &mut ratatui::Frame, area: ratatui::layout::Rect, app: &App) {
    let direct_style = if app.mode == Mode::Direct {
        Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let hive_style = if app.mode == Mode::Hive {
        Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let title = Line::from(vec![
        Span::styled(" TUX — Meerkat Device Manager   ", Style::default().fg(Color::White)),
        Span::styled(" Direct ", direct_style),
        Span::raw(" "),
        Span::styled(" Hive ", hive_style),
        Span::styled("  [Tab] toggle  [Esc] quit", Style::default().fg(Color::DarkGray)),
    ]);
    f.render_widget(Paragraph::new(title), area);
}

fn render_main(f: &mut ratatui::Frame, area: ratatui::layout::Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(22), Constraint::Percentage(78)])
        .split(area);

    // Target list
    let items: Vec<ListItem> = app
        .targets
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let prefix = if app.mode == Mode::Direct && i == app.selected { "> " } else { "  " };
            let style = if app.mode == Mode::Direct && i == app.selected {
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(format!("{prefix}{}", t.name)).style(style)
        })
        .collect();

    let target_list =
        List::new(items).block(Block::default().borders(Borders::ALL).title("TARGETS"));
    f.render_widget(target_list, chunks[0]);

    // Output panel — show most recent lines that fit
    let inner_height = chunks[1].height.saturating_sub(2) as usize;
    let start = app.output.len().saturating_sub(inner_height);
    let lines: Vec<Line> =
        app.output.iter().skip(start).map(|s| Line::from(s.as_str())).collect();

    let output = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("OUTPUT"))
        .wrap(Wrap { trim: false });
    f.render_widget(output, chunks[1]);
}

fn render_input(f: &mut ratatui::Frame, area: ratatui::layout::Rect, app: &App) {
    let mode_label: String = match app.mode {
        Mode::Direct if !app.targets.is_empty() => {
            format!("[{}] ", app.targets[app.selected].name)
        }
        Mode::Hive if app.hive_planning => "[hive: planning...] ".into(),
        Mode::Hive => "[hive] ".into(),
        _ => "[no targets] ".into(),
    };
    let prompt = format!("{mode_label}> {}_", app.input);
    let input_widget =
        Paragraph::new(prompt).block(Block::default().borders(Borders::ALL).title("COMMAND"));
    f.render_widget(input_widget, area);
}
