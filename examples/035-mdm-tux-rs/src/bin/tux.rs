//! # 035 - MDM TUX: RPC Client TUI
//!
//! Ratatui-based terminal UI that controls remote Meerkat agents via JSON-RPC.
//!
//! ```text
//! mdm-tux --target HOST:PORT [--target HOST2:PORT2]   # direct mode
//! mdm-tux --kennel HOST:PORT                           # kennel mode
//! ```
//!
//! ## Architecture
//! TUX has no Meerkat comms runtime and no agent runtime. It is a pure RPC
//! client, but kennel mode persists an Ed25519 identity for signed kennel
//! protocol messages:
//! - Connects to target agents via `RpcClient` (JSON-RPC over TCP)
//! - Streams `session/stream_event` notifications for live display
//! - In kennel mode, the kennel is just a broker that gives TUX the target's
//!   RPC address; all subsequent interaction goes direct to the target
//! - Target and hive RPC are unauthenticated plaintext and must remain inside
//!   an authenticated, encrypted network boundary
//!
//! ## Keys
//! Tab=mode  Up/Down=target  PgUp/PgDn=scroll  End=auto-scroll  Esc=quit

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind, KeyModifiers,
};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use serde_json::Value;
use tokio::io::BufReader;
use tokio::net::TcpStream;
use tokio::sync::mpsc;

use mdm_tux::machines::tux_claim::{self, State as ClaimState};
use mdm_tux::machines::tux_kennel_control::{
    self, BrokerEpoch, Effect as TkcEffect, Event as TkcEvent, State as TuxKennelState,
};
use mdm_tux::rpc_client::RpcClient;
use mdm_tux::{
    ClaimGrant, KennelPayload, LeaseRef, LeaseTerminationReason, ListScope, TargetListEntry,
    build_signed_envelope, load_or_generate_keypair, read_envelope, verify_envelope,
    write_envelope,
};

// ── Target model ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum TargetPhase {
    Disconnected,
    Idle,
    Running,
}

struct TargetView {
    stream_binding: Option<mdm_tux::rpc_client::StreamBinding>,
    connection: Option<mdm_tux::rpc_client::RpcConnection>,
    name: String,
    target_id: String,
    rpc_addr: String,
    session_id: Option<String>,
    history: VecDeque<String>,
    streaming_text: String,
    phase: TargetPhase,
    /// Channel to cancel background notification listener.
    _cancel_tx: Option<mpsc::Sender<()>>,
    /// Cached model IDs from the last `/models` catalog fetch. Used to
    /// validate a `/model <name>` next-turn override.
    known_models: HashSet<String>,
}

impl TargetView {
    fn new(name: String, target_id: String, rpc_addr: String) -> Self {
        Self {
            stream_binding: None,
            connection: None,
            name,
            target_id,
            rpc_addr,
            session_id: None,
            history: VecDeque::new(),
            streaming_text: String::new(),
            phase: TargetPhase::Disconnected,
            _cancel_tx: None,
            known_models: HashSet::new(),
        }
    }

    fn push_line(&mut self, line: String) {
        for l in line.split('\n') {
            self.history.push_back(l.to_string());
            if self.history.len() > 1000 {
                self.history.pop_front();
            }
        }
    }

    fn ensure_section_gap(&mut self) {
        if self.history.back().is_some_and(|line| !line.is_empty()) {
            self.history.push_back(String::new());
        }
    }

    fn push_section<I>(&mut self, lines: I)
    where
        I: IntoIterator<Item = String>,
    {
        self.ensure_section_gap();
        for line in lines {
            self.push_line(line);
        }
    }

    fn push_user_turn(&mut self, body: &str) {
        self.push_section([
            "**You**".to_string(),
            String::new(),
            body.trim().to_string(),
        ]);
    }

    fn push_remote_turn(&mut self, body: &str) {
        self.push_section([
            format!("**{}**", self.name),
            String::new(),
            body.trim().to_string(),
        ]);
    }

    fn push_notice(&mut self, title: &str, body: &str) {
        self.push_section([format!("_{title}_"), body.trim().to_string()]);
    }

    fn flush_streaming(&mut self) {
        if !self.streaming_text.is_empty() {
            let text = std::mem::take(&mut self.streaming_text);
            self.push_remote_turn(&text);
        }
    }
}

// ── Application state ────────────────────────────────────────────────────────

#[derive(PartialEq, Clone, Copy)]
enum Mode {
    Direct,
    Hive,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TransportMode {
    Direct,
    Kennel,
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum KennelUiState {
    Available,
    ClaimedByMe,
}

type ClaimRegistry = parking_lot::RwLock<TuxKennelState>;

struct App {
    transport: TransportMode,
    mode: Mode,
    targets: Vec<TargetView>,
    selected: usize,
    input: String,
    scroll_offset: u16,
    auto_scroll: bool,
    hive_planning: bool,
    quit: bool,
    /// Pending model override for next turn/start.
    pending_model: Option<String>,
    /// Kennel-mode target states (keyed by target_id).
    target_states: HashMap<String, KennelUiState>,
    target_leases: HashMap<String, String>,
    /// Hive agent view (displayed when in Hive mode).
    hive_view: TargetView,
}

impl App {
    fn active_view(&self) -> Option<&TargetView> {
        if self.mode == Mode::Hive {
            Some(
                self.find_target_by_id("hive")
                    .and_then(|idx| self.targets.get(idx))
                    .unwrap_or(&self.hive_view),
            )
        } else {
            self.selected_target()
        }
    }

    fn active_view_mut(&mut self) -> Option<&mut TargetView> {
        if self.mode == Mode::Hive {
            match self.find_target_by_id("hive") {
                Some(idx) => self.targets.get_mut(idx),
                None => Some(&mut self.hive_view),
            }
        } else {
            self.selected_target_mut()
        }
    }

    fn selected_target(&self) -> Option<&TargetView> {
        self.targets.get(self.selected)
    }

    fn selected_target_mut(&mut self) -> Option<&mut TargetView> {
        self.targets.get_mut(self.selected)
    }

    fn selected_target_name(&self) -> Option<&str> {
        self.selected_target().map(|t| t.name.as_str())
    }

    fn find_target_by_id(&self, target_id: &str) -> Option<usize> {
        self.targets.iter().position(|t| t.target_id == target_id)
    }

    fn input_label(&self) -> String {
        match self.mode {
            Mode::Direct if !self.targets.is_empty() => {
                let t = &self.targets[self.selected];
                if self.transport == TransportMode::Kennel
                    && t.target_id != "hive"
                    && !matches!(
                        self.target_states.get(&t.target_id),
                        Some(KennelUiState::ClaimedByMe)
                    )
                {
                    format!("[{} unavailable] > ", t.name)
                } else {
                    match t.phase {
                        TargetPhase::Disconnected => format!("[{} disconnected] > ", t.name),
                        TargetPhase::Running => format!("[{} ...running] > ", t.name),
                        TargetPhase::Idle => format!("[{}] > ", t.name),
                    }
                }
            }
            Mode::Hive if self.hive_planning => "[hive: planning...] > ".into(),
            Mode::Hive => "[hive] > ".into(),
            _ => "[waiting for targets...] > ".into(),
        }
    }
}

// ── Events ───────────────────────────────────────────────────────────────────

enum TuiEvent {
    /// A target's RPC client connected.
    TargetConnected {
        target_id: String,
        connection: mdm_tux::rpc_client::RpcConnection,
    },
    /// A target's RPC client disconnected.
    TargetDisconnected {
        target_id: String,
        connection: mdm_tux::rpc_client::RpcConnection,
    },
    /// RPC response to display.
    RpcResponse {
        target_id: String,
        body: String,
    },
    RpcError {
        target_id: String,
        message: String,
    },
    /// Session created or resumed.
    SessionBound {
        target_id: String,
        binding: mdm_tux::rpc_client::StreamBinding,
    },
    SessionBindingFailed {
        target_id: String,
        selection: mdm_tux::rpc_client::BindingSelection,
        message: String,
    },
    /// Streaming event from a target.
    StreamEvent {
        target_id: String,
        event: Value,
        binding: mdm_tux::rpc_client::StreamBinding,
    },
    StreamEnded {
        target_id: String,
        binding: mdm_tux::rpc_client::StreamBinding,
    },
    Kennel {
        epoch: BrokerEpoch,
        event: KennelUiEvent,
    },
    /// Diagnostics before a signed broker registration exists.
    KennelError(String),
    /// Model catalog response.
    ModelCatalog {
        target_id: String,
        catalog: Value,
    },
    /// Session list response.
    SessionList {
        target_id: String,
        sessions: Value,
    },
}

enum KennelUiEvent {
    AvailableTargets(Vec<TargetListEntry>),
    MineTargets(Vec<TargetListEntry>),
    ClaimGranted(Vec<ClaimGrant>),
    ClaimReleased {
        target_id: String,
        lease_ref: LeaseRef,
        reason: LeaseTerminationReason,
    },
    Error(String),
    Reconciled,
    HiveRegistered {
        rpc_addr: String,
        session_id: Option<String>,
    },
    /// Hive agent responses from the kennel.
    HiveStreamEvent {
        event: Value,
    },
    HiveComplete {
        text: String,
    },
    HiveError {
        message: String,
    },
}

impl TuiEvent {
    fn kennel(epoch: &BrokerEpoch, event: KennelUiEvent) -> Self {
        Self::Kennel {
            epoch: epoch.clone(),
            event,
        }
    }
}

// ── Commands to background RPC tasks ─────────────────────────────────────────

enum RpcCommand {
    /// Start a turn on the current session.
    StartTurn {
        target_id: String,
        session_id: String,
        prompt: String,
        model: Option<String>,
    },
    /// List sessions.
    ListSessions {
        target_id: String,
    },
    /// Resume a session by ID.
    ResumeSession {
        target_id: String,
        session_id: String,
    },
    /// List models.
    ListModels {
        target_id: String,
    },
    /// Interrupt a running turn.
    Interrupt {
        target_id: String,
        session_id: String,
    },
    /// Resume the most recent session. Session creation is not implemented
    /// for a fresh direct target.
    ResumeLatestOrCreate {
        target_id: String,
    },
    /// Connect to a target's RPC address.
    Connect {
        target_id: String,
        rpc_addr: String,
    },
    Disconnect {
        target_id: String,
    },
}

enum KennelClientCommand {
    ClaimTarget {
        target_id: String,
    },
    ClaimAck {
        lease_id: String,
    },
    ReleaseTarget {
        lease_id: String,
    },
    Shutdown {
        reply: tokio::sync::oneshot::Sender<()>,
    },
}

// ── RPC command processor ────────────────────────────────────────────────────

struct RpcTargetConnection {
    attempt: u64,
    client: Option<Arc<RpcClient>>,
}

async fn rpc_command_loop(
    mut command_rx: mpsc::UnboundedReceiver<RpcCommand>,
    event_tx: mpsc::Sender<TuiEvent>,
) {
    let clients: Arc<tokio::sync::Mutex<HashMap<String, RpcTargetConnection>>> =
        Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let mut next_connection_attempt = 0u64;

    while let Some(cmd) = command_rx.recv().await {
        match cmd {
            RpcCommand::Disconnect { target_id } => {
                let client = clients
                    .lock()
                    .await
                    .remove(&target_id)
                    .and_then(|slot| slot.client);
                if let Some(client) = client {
                    client.shutdown().await;
                    let _ = event_tx
                        .send(TuiEvent::TargetDisconnected {
                            target_id,
                            connection: client.connection(),
                        })
                        .await;
                }
            }
            RpcCommand::Connect {
                target_id,
                rpc_addr,
            } => {
                next_connection_attempt += 1;
                let attempt = next_connection_attempt;
                let previous = clients
                    .lock()
                    .await
                    .insert(
                        target_id.clone(),
                        RpcTargetConnection {
                            attempt,
                            client: None,
                        },
                    )
                    .and_then(|slot| slot.client);
                if let Some(previous) = previous {
                    previous.shutdown().await;
                    let _ = event_tx
                        .send(TuiEvent::TargetDisconnected {
                            target_id: target_id.clone(),
                            connection: previous.connection(),
                        })
                        .await;
                }
                let event_tx = event_tx.clone();
                let clients = clients.clone();
                let tid = target_id.clone();
                tokio::spawn(async move {
                    let (ntf_tx, mut ntf_rx) = mpsc::unbounded_channel::<Value>();
                    match RpcClient::connect(&rpc_addr, ntf_tx).await {
                        Ok(mut client) => {
                            // Set a generous timeout for turns that involve
                            // multiple tool calls + LLM reasoning.
                            client.set_request_timeout(Duration::from_secs(300));
                            if let Err(e) =
                                client.request("initialize", serde_json::json!({})).await
                            {
                                let map = clients.lock().await;
                                if map.get(&tid).is_some_and(|slot| slot.attempt == attempt) {
                                    let _ = event_tx
                                        .send(TuiEvent::RpcError {
                                            target_id: tid,
                                            message: format!("initialize failed: {e}"),
                                        })
                                        .await;
                                }
                                return;
                            }
                            let client = Arc::new(client);
                            {
                                let mut map = clients.lock().await;
                                let Some(slot) =
                                    map.get_mut(&tid).filter(|slot| slot.attempt == attempt)
                                else {
                                    drop(map);
                                    client.shutdown().await;
                                    return;
                                };
                                slot.client = Some(client.clone());
                                let _ = event_tx
                                    .send(TuiEvent::TargetConnected {
                                        target_id: tid.clone(),
                                        connection: client.connection(),
                                    })
                                    .await;
                            }
                            // Drain notifications
                            let event_tx2 = event_tx.clone();
                            let tid2 = tid.clone();
                            let clients2 = clients.clone();
                            let this_client = client.clone();
                            tokio::spawn(async move {
                                while let Some(notification) = ntf_rx.recv().await {
                                    let current = clients2
                                        .lock()
                                        .await
                                        .get(&tid2)
                                        .and_then(|slot| slot.client.as_ref())
                                        .is_some_and(|active| Arc::ptr_eq(active, &this_client));
                                    if !current {
                                        break;
                                    }
                                    if let Some((binding, params)) =
                                        this_client.stream_notification(&notification).await
                                    {
                                        let _ = event_tx2
                                            .send(TuiEvent::StreamEvent {
                                                target_id: tid2.clone(),
                                                event: params,
                                                binding,
                                            })
                                            .await;
                                    } else if let Some(binding) =
                                        this_client.ended_stream(&notification).await
                                    {
                                        let _ = event_tx2
                                            .send(TuiEvent::StreamEnded {
                                                target_id: tid2.clone(),
                                                binding,
                                            })
                                            .await;
                                    }
                                }
                                // Remove dead client, but only if no newer
                                // connection has already replaced it.
                                let was_current = {
                                    let mut map = clients2.lock().await;
                                    if let Some(existing) =
                                        map.get(&tid2).and_then(|slot| slot.client.as_ref())
                                    {
                                        if Arc::ptr_eq(existing, &this_client) {
                                            map.remove(&tid2);
                                            true
                                        } else {
                                            false // newer connection already replaced us
                                        }
                                    } else {
                                        false
                                    }
                                };
                                // Only fire TargetDisconnected if we were still the
                                // active client — a newer connection should not be
                                // overridden by a stale disconnect event.
                                if was_current {
                                    let _ = event_tx2
                                        .send(TuiEvent::TargetDisconnected {
                                            target_id: tid2,
                                            connection: this_client.connection(),
                                        })
                                        .await;
                                }
                            });
                        }
                        Err(e) => {
                            let map = clients.lock().await;
                            if map.get(&tid).is_some_and(|slot| slot.attempt == attempt) {
                                let _ = event_tx
                                    .send(TuiEvent::RpcError {
                                        target_id: tid,
                                        message: format!("connect failed: {e}"),
                                    })
                                    .await;
                            }
                        }
                    }
                });
            }
            RpcCommand::StartTurn {
                target_id,
                session_id,
                prompt,
                model,
            } => {
                let event_tx = event_tx.clone();
                let clients = clients.clone();
                tokio::spawn(async move {
                    let client = {
                        let map = clients.lock().await;
                        map.get(&target_id).and_then(|slot| slot.client.clone())
                    };
                    let Some(client) = client else {
                        let _ = event_tx
                            .send(TuiEvent::RpcError {
                                target_id,
                                message: "not connected".into(),
                            })
                            .await;
                        return;
                    };
                    let mut params = mdm_tux::rpc_client::turn_params(&session_id, &prompt);
                    if let Some(m) = model {
                        // Infer provider from model name prefix
                        let provider = if m.starts_with("claude-") {
                            "anthropic"
                        } else if m.starts_with("gpt-")
                            || m.starts_with("o1-")
                            || m.starts_with("o3-")
                        {
                            "openai"
                        } else if m.starts_with("gemini-") {
                            "gemini"
                        } else {
                            "self_hosted"
                        };
                        params["model"] = Value::String(m);
                        params["provider"] = Value::String(provider.into());
                    }
                    match client.request("turn/start", params).await {
                        Ok(_) => {
                            // Turn started; events arrive via notifications.
                        }
                        Err(e) => {
                            let _ = event_tx
                                .send(TuiEvent::RpcError {
                                    target_id,
                                    message: format!("turn/start failed: {e}"),
                                })
                                .await;
                        }
                    }
                });
            }
            RpcCommand::ListSessions { target_id } => {
                let event_tx = event_tx.clone();
                let clients = clients.clone();
                tokio::spawn(async move {
                    let client = {
                        let map = clients.lock().await;
                        map.get(&target_id).and_then(|slot| slot.client.clone())
                    };
                    let Some(client) = client else {
                        let _ = event_tx
                            .send(TuiEvent::RpcError {
                                target_id,
                                message: "not connected".into(),
                            })
                            .await;
                        return;
                    };
                    match client.request("session/list", serde_json::json!({})).await {
                        Ok(result) => {
                            let _ = event_tx
                                .send(TuiEvent::SessionList {
                                    target_id,
                                    sessions: result,
                                })
                                .await;
                        }
                        Err(e) => {
                            let _ = event_tx
                                .send(TuiEvent::RpcError {
                                    target_id,
                                    message: format!("session/list failed: {e}"),
                                })
                                .await;
                        }
                    }
                });
            }
            RpcCommand::ResumeSession {
                target_id,
                session_id,
            } => {
                let event_tx = event_tx.clone();
                let clients = clients.clone();
                let selected = clients
                    .lock()
                    .await
                    .get(&target_id)
                    .and_then(|slot| slot.client.as_ref())
                    .map(|client| (client.clone(), client.begin_binding()));
                tokio::spawn(async move {
                    let Some((client, pending)) = selected else {
                        let _ = event_tx
                            .send(TuiEvent::RpcError {
                                target_id,
                                message: "not connected".into(),
                            })
                            .await;
                        return;
                    };
                    let selection = pending.selection();
                    // Bind only after the server confirms that the session
                    // exists and its stream was opened.
                    match client.bind_selected(&session_id, pending).await {
                        Ok(binding) => {
                            if !clients
                                .lock()
                                .await
                                .get(&target_id)
                                .and_then(|slot| slot.client.as_ref())
                                .is_some_and(|active| Arc::ptr_eq(active, &client))
                            {
                                return;
                            }
                            let _ = event_tx
                                .send(TuiEvent::SessionBound { target_id, binding })
                                .await;
                        }
                        Err(e) => {
                            let _ = event_tx
                                .send(TuiEvent::SessionBindingFailed {
                                    target_id,
                                    selection,
                                    message: format!("session/stream_open failed: {e}"),
                                })
                                .await;
                        }
                    }
                });
            }
            RpcCommand::ListModels { target_id } => {
                let event_tx = event_tx.clone();
                let clients = clients.clone();
                tokio::spawn(async move {
                    let client = {
                        let map = clients.lock().await;
                        map.get(&target_id).and_then(|slot| slot.client.clone())
                    };
                    let Some(client) = client else {
                        let _ = event_tx
                            .send(TuiEvent::RpcError {
                                target_id,
                                message: "not connected".into(),
                            })
                            .await;
                        return;
                    };
                    match client
                        .request("models/catalog", serde_json::json!({}))
                        .await
                    {
                        Ok(result) => {
                            let _ = event_tx
                                .send(TuiEvent::ModelCatalog {
                                    target_id,
                                    catalog: result,
                                })
                                .await;
                        }
                        Err(e) => {
                            let _ = event_tx
                                .send(TuiEvent::RpcError {
                                    target_id,
                                    message: format!("models/catalog failed: {e}"),
                                })
                                .await;
                        }
                    }
                });
            }
            RpcCommand::Interrupt {
                target_id,
                session_id,
            } => {
                let event_tx = event_tx.clone();
                let clients = clients.clone();
                tokio::spawn(async move {
                    let client = {
                        let map = clients.lock().await;
                        map.get(&target_id).and_then(|slot| slot.client.clone())
                    };
                    let Some(client) = client else {
                        let _ = event_tx
                            .send(TuiEvent::RpcError {
                                target_id,
                                message: "not connected".into(),
                            })
                            .await;
                        return;
                    };
                    match client
                        .request(
                            "turn/interrupt",
                            serde_json::json!({ "session_id": session_id }),
                        )
                        .await
                    {
                        Ok(_) => {
                            let _ = event_tx
                                .send(TuiEvent::RpcResponse {
                                    target_id,
                                    body: "turn interrupted".into(),
                                })
                                .await;
                        }
                        Err(e) => {
                            let _ = event_tx
                                .send(TuiEvent::RpcError {
                                    target_id,
                                    message: format!("turn/interrupt failed: {e}"),
                                })
                                .await;
                        }
                    }
                });
            }
            RpcCommand::ResumeLatestOrCreate { target_id } => {
                let event_tx = event_tx.clone();
                let clients = clients.clone();
                let selected = clients
                    .lock()
                    .await
                    .get(&target_id)
                    .and_then(|slot| slot.client.as_ref())
                    .map(|client| (client.clone(), client.begin_binding()));
                tokio::spawn(async move {
                    let Some((client, pending)) = selected else {
                        return;
                    };
                    let selection = pending.selection();
                    // Try to find an existing session to resume.
                    if let Ok(list_result) =
                        client.request("session/list", serde_json::json!({})).await
                        && let Some(sessions) =
                            list_result.get("sessions").and_then(|v| v.as_array())
                        && let Some(latest) = sessions.first()
                        && let Some(sid) = latest.get("session_id").and_then(|v| v.as_str())
                    {
                        match client.bind_selected(sid, pending).await {
                            Ok(binding) => {
                                if !clients
                                    .lock()
                                    .await
                                    .get(&target_id)
                                    .and_then(|slot| slot.client.as_ref())
                                    .is_some_and(|active| Arc::ptr_eq(active, &client))
                                {
                                    return;
                                }
                                let _ = event_tx
                                    .send(TuiEvent::SessionBound {
                                        target_id: target_id.clone(),
                                        binding,
                                    })
                                    .await;
                            }
                            Err(e) => {
                                let _ = event_tx
                                    .send(TuiEvent::SessionBindingFailed {
                                        target_id: target_id.clone(),
                                        selection,
                                        message: format!("session/stream_open failed: {e}"),
                                    })
                                    .await;
                            }
                        }
                        return;
                    }
                    // No existing session. Kennel mode normally creates one
                    // before TUX connects, but fresh direct mode does not.
                    let _ = event_tx
                        .send(TuiEvent::SessionBindingFailed {
                            target_id,
                            selection,
                            message: "no persisted session found; fresh direct-session creation is not implemented"
                                .into(),
                        })
                        .await;
                });
            }
        }
    }
}

// ── Stream event handling ────────────────────────────────────────────────────

fn handle_stream_event(target: &mut TargetView, params: &Value) {
    // Notifications arrive in two shapes:
    // 1. session/stream_event: { stream_id, session_id, event: { payload: { type, ... } } }
    // 2. session/event:        { event: { payload: { type, ... } } }
    // Unwrap to the payload level where the event type lives.
    let event = params
        .get("event")
        .and_then(|e| e.get("payload"))
        .or_else(|| params.get("event"))
        .unwrap_or(params);
    let kind = event
        .get("kind")
        .or_else(|| event.get("type"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    match kind {
        "run_started" | "RunStarted" => {
            target.phase = TargetPhase::Running;
            target.streaming_text.clear();
        }
        "text_delta" | "TextDelta" => {
            target.phase = TargetPhase::Running;
            if let Some(delta) = event.get("delta").and_then(|v| v.as_str()) {
                target.streaming_text.push_str(delta);
            }
        }
        "text_complete" | "TextComplete" => {
            if let Some(content) = event.get("content").and_then(|v| v.as_str()) {
                target.streaming_text.clear();
                if !content.is_empty() {
                    target.push_remote_turn(content);
                }
            }
        }
        "tool_call_requested" | "ToolCallRequested" => {
            target.flush_streaming();
            let name = event.get("name").and_then(|v| v.as_str()).unwrap_or("?");
            let args = event.get("args").cloned().unwrap_or(Value::Null);
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
            target.push_section([
                format!("**tool** `{name}`"),
                "```text".into(),
                display,
                "```".into(),
            ]);
        }
        "tool_execution_completed" | "ToolExecutionCompleted" => {
            let name = event.get("name").and_then(|v| v.as_str()).unwrap_or("?");
            let result = event
                .get("content")
                .and_then(|v| v.as_array())
                .map(|blocks| {
                    blocks
                        .iter()
                        .filter(|&b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                        .map(|b| b.get("text").and_then(|t| t.as_str()).unwrap_or(""))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            let is_error = event
                .get("is_error")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let duration_ms = event
                .get("duration_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let lines = format_tool_completion(name, &result, is_error, duration_ms);
            target.push_section(lines);
        }
        "run_completed" | "RunCompleted" => {
            target.flush_streaming();
            target.phase = TargetPhase::Idle;
        }
        "run_failed" | "RunFailed" => {
            target.flush_streaming();
            let error = event
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            target.push_notice("run failed", error);
            target.phase = TargetPhase::Idle;
        }
        // Silent events — no display needed
        "turn_started"
        | "TurnStarted"
        | "turn_completed"
        | "TurnCompleted"
        | "tool_config_changed"
        | "ToolConfigChanged"
        | "reasoning_started"
        | "ReasoningStarted"
        | "reasoning_complete"
        | "ReasoningComplete"
        | "mcp_pending_notice"
        | "McpPendingNotice"
        | "system_context_appended"
        | "SystemContextAppended"
        | "compaction_started"
        | "CompactionStarted"
        | "compaction_completed"
        | "CompactionCompleted" => {}
        _ => {
            // Truly unknown event — show only if non-trivial
            if !kind.is_empty() {
                tracing::debug!("unhandled stream event: {kind}");
            }
        }
    }
}

// ── Tool output formatting ───────────────────────────────────────────────────

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
            format!("{clipped}...")
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

    if let Ok(json) = serde_json::from_str::<Value>(result) {
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
                    lines.push(summary.join("  |  "));
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

// ── Kennel helpers ───────────────────────────────────────────────────────────

fn current_attached_target_ids(claims: &ClaimRegistry) -> Vec<String> {
    claims.read().current_attached_target_ids()
}

fn claim_state_for_id(claims: &ClaimRegistry, target_id: &str) -> Option<ClaimState> {
    claims.read().claims.get(target_id).cloned()
}

fn apply_claim_effects(
    _claims: &ClaimRegistry,
    kennel_tx: Option<&mpsc::UnboundedSender<KennelClientCommand>>,
    effects: &[TkcEffect],
) {
    for effect in effects {
        match effect {
            TkcEffect::Claim {
                effect: tux_claim::Effect::SendClaimToKennel { target_id },
                ..
            } => {
                if let Some(kennel_tx) = kennel_tx {
                    let _ = kennel_tx.send(KennelClientCommand::ClaimTarget {
                        target_id: target_id.clone(),
                    });
                }
            }
            TkcEffect::Claim {
                effect: tux_claim::Effect::SendClaimAck { lease_id },
                ..
            } => {
                if let Some(kennel_tx) = kennel_tx {
                    let _ = kennel_tx.send(KennelClientCommand::ClaimAck {
                        lease_id: lease_id.clone(),
                    });
                }
            }
            TkcEffect::Claim {
                effect: tux_claim::Effect::SendReleaseToKennel { lease_id },
                ..
            } => {
                if let Some(kennel_tx) = kennel_tx {
                    let _ = kennel_tx.send(KennelClientCommand::ReleaseTarget {
                        lease_id: lease_id.clone(),
                    });
                }
            }
            TkcEffect::Claim {
                effect: tux_claim::Effect::RpcConnect { .. },
                ..
            } => {
                // RPC connect is handled by the TUI event loop via
                // KennelTargetDiscovered, not via the kennel client command
                // channel.
            }
        }
    }
}

fn apply_tkc_event<R, F>(
    claims: &ClaimRegistry,
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
            return None;
        };
        let result = inspect(&new_state);
        *guard = new_state;
        (effects, result)
    };
    apply_claim_effects(claims, kennel_tx, &effects);
    Some(result)
}

async fn send_kennel_message(
    write_half: &mut tokio::net::tcp::OwnedWriteHalf,
    keypair: &meerkat_comms::identity::Keypair,
    tux_id: &str,
    payload: KennelPayload,
) -> bool {
    let Ok(env) = build_signed_envelope(keypair, tux_id, payload) else {
        return false;
    };
    write_envelope(write_half, &env).await.is_ok()
}

#[allow(clippy::too_many_arguments)]
async fn spawn_kennel_client(
    mut kennel_rx: mpsc::UnboundedReceiver<KennelClientCommand>,
    kennel_cmd_tx: mpsc::UnboundedSender<KennelClientCommand>,
    event_tx: mpsc::Sender<TuiEvent>,
    _rpc_command_tx: mpsc::UnboundedSender<RpcCommand>,
    keypair: Arc<meerkat_comms::identity::Keypair>,
    claims: Arc<ClaimRegistry>,
    kennel_addr: String,
    tux_id: String,
    _direct_addr: String,
) {
    let mut broker_signer: Option<String> = None;
    'outer: loop {
        let stream =
            match tokio::time::timeout(Duration::from_secs(10), TcpStream::connect(&kennel_addr))
                .await
            {
                Ok(Ok(stream)) => stream,
                Ok(Err(e)) => {
                    let _ = event_tx
                        .send(TuiEvent::KennelError(format!(
                            "[kennel] connect failed: {e}"
                        )))
                        .await;
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
                Err(_) => {
                    let _ = event_tx
                        .send(TuiEvent::KennelError("[kennel] connect timed out".into()))
                        .await;
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
            };
        let (read_half, mut write_half) = stream.into_split();
        let mut reader = BufReader::new(read_half);
        if !send_kennel_message(
            &mut write_half,
            &keypair,
            &tux_id,
            KennelPayload::TuxRegister {
                tux_id: tux_id.clone(),
                pubkey: keypair.public_key().to_pubkey_string(),
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
        if verify_envelope(&env).is_err()
            || broker_signer
                .as_ref()
                .is_some_and(|signer| signer != &env.signer_id)
        {
            let _ = event_tx
                .send(TuiEvent::KennelError(
                    "[kennel] invalid signed register reply".into(),
                ))
                .await;
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        }
        broker_signer = Some(env.signer_id.clone());
        let (hive_rpc_addr, hive_session_id, epoch) = match &env.payload {
            KennelPayload::TuxRegistered {
                hive_rpc_addr,
                hive_session_id,
                broker_incarnation,
            } => {
                let epoch = BrokerEpoch::new(broker_incarnation.clone());
                let _ = apply_tkc_event(
                    &claims,
                    Some(&kennel_cmd_tx),
                    TkcEvent::BrokerRegistered {
                        epoch: epoch.clone(),
                    },
                    |_| (),
                );
                let _ = event_tx
                    .send(TuiEvent::kennel(&epoch, KennelUiEvent::Reconciled))
                    .await;
                (hive_rpc_addr.clone(), hive_session_id.clone(), epoch)
            }
            _ => {
                let _ = event_tx
                    .send(TuiEvent::KennelError(format!(
                        "[kennel] unexpected register reply: {:?}",
                        env.payload
                    )))
                    .await;
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
        };
        // Advertise and connect only after the UI validates this broker epoch.
        if let Some(addr) = hive_rpc_addr {
            let _ = event_tx
                .send(TuiEvent::kennel(
                    &epoch,
                    KennelUiEvent::HiveRegistered {
                        rpc_addr: addr,
                        session_id: hive_session_id,
                    },
                ))
                .await;
        }
        let _ = apply_tkc_event(
            &claims,
            Some(&kennel_cmd_tx),
            TkcEvent::KennelConnected,
            |_| (),
        );

        let mut heartbeat = tokio::time::interval(Duration::from_secs(10));
        let mut refresh = tokio::time::interval(Duration::from_secs(5));
        let mut renew = tokio::time::interval(Duration::from_secs(15));
        let mut claim_requests: HashMap<String, Vec<String>> = HashMap::new();

        loop {
            tokio::select! {
                _ = heartbeat.tick() => {
                    if !send_kennel_message(&mut write_half, &keypair, &tux_id, KennelPayload::TuxHeartbeat).await {
                        break;
                    }
                }
                _ = refresh.tick() => {
                    let mut refresh_ok = true;
                    for scope in [ListScope::Available, ListScope::Mine] {
                        if !send_kennel_message(&mut write_half, &keypair, &tux_id, KennelPayload::ListTargets { scope }).await {
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
                            &keypair,
                            &tux_id,
                            KennelPayload::RenewLeases { lease_ids, lease_ttl_sec: None },
                        ).await
                    {
                        break;
                    }
                }
                maybe = read_envelope(&mut reader) => {
                    let Some(env) = maybe.ok().flatten() else { break; };
                    if verify_envelope(&env).is_err()
                        || broker_signer.as_ref() != Some(&env.signer_id) {
                        break;
                    }
                    match env.payload {
                        KennelPayload::ClaimRequestCompleted { in_reply_to, refused_target_ids } => {
                            if let Some(requested) = claim_requests.remove(&in_reply_to) {
                                for target_id in refused_target_ids {
                                    if requested.contains(&target_id) {
                                        let _ = apply_tkc_event(&claims, Some(&kennel_cmd_tx),
                                            TkcEvent::ClaimRefused { target_id }, |_| ());
                                    }
                                }
                                let _ = event_tx.send(TuiEvent::kennel(&epoch, KennelUiEvent::Reconciled)).await;
                            }
                        }
                        KennelPayload::TargetList { scope: ListScope::Available, targets } => {
                            let _ = apply_tkc_event(&claims, None,
                                TkcEvent::SeenAvailableList { targets: targets.clone() }, |_| ());
                            let _ = event_tx.send(TuiEvent::kennel(&epoch, KennelUiEvent::AvailableTargets(targets))).await;
                        }
                        KennelPayload::TargetList { scope: ListScope::Mine, targets } => {
                            let _ = apply_tkc_event(&claims, None,
                                TkcEvent::SeenMineList { targets: targets.clone() }, |_| ());
                            let _ = event_tx.send(TuiEvent::kennel(&epoch, KennelUiEvent::MineTargets(targets))).await;
                        }
                        KennelPayload::ClaimGranted { claims: grants } => {
                            let _ = apply_tkc_event(
                                &claims,
                                Some(&kennel_cmd_tx),
                                TkcEvent::ClaimGranted {
                                    claims: grants.clone(),
                                    now_ms: chrono::Utc::now().timestamp_millis(),
                                },
                                |_| (),
                            );
                            let _ = event_tx.send(TuiEvent::kennel(&epoch, KennelUiEvent::ClaimGranted(grants))).await;
                        }
                        KennelPayload::ClaimReleased {
                            target_id,
                            lease_ref,
                            reason,
                        } => {
                            let released = apply_tkc_event(
                                &claims,
                                Some(&kennel_cmd_tx),
                                TkcEvent::ClaimReleased {
                                    target_id: target_id.clone(),
                                    lease_ref: lease_ref.clone(),
                                },
                                |_| (),
                            );
                            if released.is_none() { continue; }
                            let _ = event_tx
                                .send(TuiEvent::kennel(&epoch, KennelUiEvent::ClaimReleased {
                                    target_id,
                                    lease_ref,
                                    reason,
                                }))
                                .await;
                        }
                        KennelPayload::HiveStreamEvent { event } => {
                            let _ = event_tx.send(TuiEvent::kennel(&epoch, KennelUiEvent::HiveStreamEvent { event })).await;
                        }
                        KennelPayload::HiveComplete { text } => {
                            let _ = event_tx.send(TuiEvent::kennel(&epoch, KennelUiEvent::HiveComplete { text })).await;
                        }
                        KennelPayload::HiveError { message } => {
                            let _ = event_tx.send(TuiEvent::kennel(&epoch, KennelUiEvent::HiveError { message })).await;
                        }
                        _ => {}
                    }
                }
                Some(cmd) = kennel_rx.recv() => {
                    match cmd {
                        KennelClientCommand::ClaimTarget { target_id } => {
                            if !matches!(claim_state_for_id(&claims, &target_id), Some(ClaimState::ClaimRequested { .. })) {
                                continue;
                            }
                            let envelope = build_signed_envelope(&keypair, &tux_id,
                                KennelPayload::ClaimTargets {
                                    target_ids: vec![target_id.clone()],
                                    lease_ttl_sec: None,
                                },
                            );
                            let Ok(envelope) = envelope else { break; };
                            claim_requests.insert(envelope.message_id.clone(), vec![target_id]);
                            if write_envelope(&mut write_half, &envelope).await.is_err() {
                                break;
                            }
                        }
                        KennelClientCommand::ClaimAck { lease_id } => {
                            if !send_kennel_message(
                                &mut write_half,
                                &keypair,
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
                            if !send_kennel_message(
                                &mut write_half,
                                &keypair,
                                &tux_id,
                                KennelPayload::ReleaseTargets { lease_ids: vec![lease_id] },
                            )
                            .await
                            {
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
                                let _ = send_kennel_message(&mut write_half, &keypair, &tux_id, KennelPayload::ReleaseTargets { lease_ids }).await;
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
            Some(&kennel_cmd_tx),
            TkcEvent::KennelDisconnected,
            |_| (),
        );
        let _ = event_tx
            .send(TuiEvent::kennel(
                &epoch,
                KennelUiEvent::Error("[kennel] disconnected".into()),
            ))
            .await;
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

// ── Entry point ──────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut rust_log = std::env::var("RUST_LOG").unwrap_or_else(|_| "warn".into());
    if !rust_log
        .split(',')
        .any(|directive| directive.trim_start().starts_with("tui_markdown"))
    {
        rust_log.push_str(",tui_markdown=error");
    }
    tracing_subscriber::fmt().with_env_filter(rust_log).init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        eprintln!("Usage: mdm-tux --target HOST:PORT [--target HOST2:PORT2]");
        eprintln!("   or: mdm-tux --kennel HOST:PORT [--data-dir PATH]");
        eprintln!("\nDirect mode: connect to target RPC servers directly");
        eprintln!("Kennel mode: broker discovers targets and provides their RPC addresses");
        eprintln!(
            "Fresh direct targets have no session; direct mode can only resume persisted sessions."
        );
        eprintln!(
            "WARNING: target and hive RPC are unauthenticated plaintext; use only inside an authenticated, encrypted network boundary."
        );
        std::process::exit(1);
    }

    if find_flag(&args, "--kennel").is_some() {
        return run_kennel_tux(&args).await;
    }

    // Direct mode: --target HOST:PORT [--target HOST2:PORT2 ...]
    let targets = find_all_flags(&args, "--target");
    if targets.is_empty() {
        anyhow::bail!("at least one --target HOST:PORT is required");
    }

    let (event_tx, event_rx) = mpsc::channel::<TuiEvent>(256);
    let (rpc_tx, rpc_rx) = mpsc::unbounded_channel::<RpcCommand>();

    // Spawn RPC command loop
    tokio::spawn({
        let event_tx = event_tx.clone();
        async move {
            rpc_command_loop(rpc_rx, event_tx).await;
        }
    });

    // Build initial target list and connect
    let mut initial_targets = Vec::new();
    for (i, addr) in targets.iter().enumerate() {
        let name = format!("target-{}", i + 1);
        let target_id = format!("direct-{}", i + 1);
        initial_targets.push(TargetView::new(name, target_id.clone(), addr.clone()));
        let _ = rpc_tx.send(RpcCommand::Connect {
            target_id,
            rpc_addr: addr.clone(),
        });
    }

    println!("=== TUX -- Meerkat Device Manager (RPC Client) ===");
    for t in &initial_targets {
        println!("  target: {} @ {}", t.name, t.rpc_addr);
    }
    println!();

    let app = App {
        transport: TransportMode::Direct,
        mode: Mode::Direct,
        targets: initial_targets,
        selected: 0,
        input: String::new(),
        scroll_offset: 0,
        auto_scroll: true,
        hive_planning: false,
        quit: false,
        pending_model: None,
        target_states: HashMap::new(),
        target_leases: HashMap::new(),
        hive_view: TargetView::new("hive".into(), "hive".into(), String::new()),
    };

    tokio::task::spawn_blocking(move || tui_loop(app, rpc_tx, event_rx, None, None))
        .await?
        .context("TUI loop")?;

    Ok(())
}

async fn run_kennel_tux(args: &[String]) -> anyhow::Result<()> {
    let kennel_addr = find_flag(args, "--kennel").context("--kennel HOST:PORT is required")?;
    let listen_port: u16 = find_flag(args, "--listen")
        .unwrap_or_else(|| "0".to_string())
        .parse()
        .context("--listen PORT must be a number")?;
    let advertise = find_flag(args, "--advertise");

    let data_dir = find_flag(args, "--data-dir")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join(".rkat/mdm/tux")
        });

    // Load or generate keypair (needed for kennel signed envelopes)
    let keypair = Arc::new(load_or_generate_keypair(&data_dir.join("identity")).await?);
    let tux_id = keypair.public_key().to_peer_id().to_string();
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
    let direct_addr = format!("tcp://{direct_ip}:{listen_port}");

    println!("=== TUX -- Meerkat Device Manager (Kennel Mode) ===");
    println!("kennel    : {kennel_addr}");
    println!("legacy_hint: {direct_addr} (not used for RPC or kennel registration)");
    println!("warning   : discovered RPC endpoints are unauthenticated plaintext");
    println!();

    let (event_tx, event_rx) = mpsc::channel::<TuiEvent>(256);
    let (rpc_tx, rpc_rx) = mpsc::unbounded_channel::<RpcCommand>();
    let (kennel_tx, kennel_rx) = mpsc::unbounded_channel::<KennelClientCommand>();

    let claims: Arc<ClaimRegistry> = Arc::new(parking_lot::RwLock::new(TuxKennelState::default()));

    // Spawn RPC command loop
    tokio::spawn({
        let event_tx = event_tx.clone();
        async move {
            rpc_command_loop(rpc_rx, event_tx).await;
        }
    });

    // Spawn kennel client
    tokio::spawn({
        let event_tx = event_tx.clone();
        let kennel_cmd_tx = kennel_tx.clone();
        let rpc_command_tx = rpc_tx.clone();
        let keypair = keypair.clone();
        let claims = claims.clone();
        let kennel_addr = kennel_addr.clone();
        let tux_id = tux_id.clone();
        let direct_addr = direct_addr.clone();
        async move {
            spawn_kennel_client(
                kennel_rx,
                kennel_cmd_tx,
                event_tx,
                rpc_command_tx,
                keypair,
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
        selected: 0,
        input: String::new(),
        scroll_offset: 0,
        auto_scroll: true,
        hive_planning: false,
        quit: false,
        pending_model: None,
        target_states: HashMap::new(),
        target_leases: HashMap::new(),
        hive_view: TargetView::new("hive".into(), "hive".into(), String::new()),
    };

    let claims_for_tui = Some(claims.clone());
    let kennel_tx_for_tui = Some(kennel_tx.clone());
    tokio::task::spawn_blocking(move || {
        tui_loop(app, rpc_tx, event_rx, claims_for_tui, kennel_tx_for_tui)
    })
    .await?
    .context("TUI loop")?;

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let _ = kennel_tx.send(KennelClientCommand::Shutdown { reply: shutdown_tx });
    let _ = tokio::time::timeout(Duration::from_secs(3), shutdown_rx).await;

    Ok(())
}

// ── TUI loop (sync) ─────────────────────────────────────────────────────────

fn tui_loop(
    mut app: App,
    rpc_tx: mpsc::UnboundedSender<RpcCommand>,
    mut event_rx: mpsc::Receiver<TuiEvent>,
    claims: Option<Arc<ClaimRegistry>>,
    kennel_tx: Option<mpsc::UnboundedSender<KennelClientCommand>>,
) -> anyhow::Result<()> {
    let mut terminal = ratatui::init();

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
        let mut dirty = false;
        if crossterm::event::poll(Duration::from_millis(100))? {
            match crossterm::event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    handle_key(
                        &mut app,
                        key.code,
                        key.modifiers,
                        &rpc_tx,
                        claims.as_deref(),
                        kennel_tx.as_ref(),
                    );
                    dirty = true;
                }
                Event::Paste(text) => {
                    let line_count = text.lines().count();
                    app.input.push_str(&text);
                    if line_count > 1
                        && let Some(t) = app.selected_target_mut()
                    {
                        t.push_notice("pasted input", &format!("{line_count} lines"));
                    }
                    dirty = true;
                }
                _ => {}
            }
        }

        while let Ok(ev) = event_rx.try_recv() {
            dirty = true;
            process_event(&mut app, ev, &rpc_tx, claims.as_deref(), kennel_tx.as_ref());
        }

        if dirty {
            terminal.draw(|f| render(f, &mut app))?;
        }
        if app.quit {
            break;
        }
    }

    Ok(())
}

fn process_event(
    app: &mut App,
    ev: TuiEvent,
    rpc_tx: &mpsc::UnboundedSender<RpcCommand>,
    claims: Option<&ClaimRegistry>,
    _kennel_tx: Option<&mpsc::UnboundedSender<KennelClientCommand>>,
) {
    match ev {
        TuiEvent::TargetConnected {
            target_id,
            connection,
        } => {
            if !connection.is_live() {
                return;
            }
            if let Some(idx) = app.find_target_by_id(&target_id) {
                app.targets[idx].connection = Some(connection);
                let was_disconnected = app.targets[idx].phase == TargetPhase::Disconnected;
                app.targets[idx].phase = TargetPhase::Idle;
                let name = app.targets[idx].name.clone();
                app.targets[idx].push_notice(
                    "connected",
                    &format!("RPC connection to {name} established"),
                );
                // Clear stale session on reconnect — the target may have
                // restarted with a different session.
                app.targets[idx].stream_binding = None;
                if was_disconnected && target_id != "hive" {
                    app.targets[idx].session_id = None;
                }
                // Resume the latest persisted session. Fresh direct targets
                // currently have no session-creation path.
                if app.targets[idx].session_id.is_none() {
                    let _ = rpc_tx.send(RpcCommand::ResumeLatestOrCreate { target_id });
                } else if let Some(session_id) = app.targets[idx].session_id.clone() {
                    let _ = rpc_tx.send(RpcCommand::ResumeSession {
                        target_id,
                        session_id,
                    });
                }
            }
        }
        TuiEvent::TargetDisconnected {
            target_id,
            connection,
        } => {
            if let Some(idx) = app.find_target_by_id(&target_id) {
                if app.targets[idx].connection.as_ref() != Some(&connection) {
                    return;
                }
                app.targets[idx].connection = None;
                app.targets[idx].phase = TargetPhase::Disconnected;
                app.targets[idx].stream_binding = None;
                app.targets[idx].flush_streaming();
                app.targets[idx].push_notice("disconnected", "RPC connection lost");
            }
        }
        TuiEvent::RpcResponse { target_id, body } => {
            if let Some(idx) = app.find_target_by_id(&target_id) {
                app.targets[idx].push_notice("rpc", &body);
            }
        }
        TuiEvent::RpcError { target_id, message } => {
            if let Some(idx) = app.find_target_by_id(&target_id) {
                if app.targets[idx].phase == TargetPhase::Running {
                    app.targets[idx].phase = TargetPhase::Idle;
                    app.targets[idx].flush_streaming();
                }
                app.targets[idx].push_notice("error", &message);
            }
        }
        TuiEvent::SessionBound { target_id, binding } => {
            if let Some(idx) = app.find_target_by_id(&target_id) {
                if !binding.is_current()
                    || app.targets[idx].connection.as_ref() != Some(binding.connection())
                {
                    return;
                }
                let session_id = binding.session_id.clone();
                let short = short_id(&session_id);
                app.targets[idx].session_id = Some(session_id);
                app.targets[idx].stream_binding = Some(binding);
                app.targets[idx].phase = TargetPhase::Idle;
                app.targets[idx].push_notice("session", &format!("bound to {short}"));
            }
        }
        TuiEvent::SessionBindingFailed {
            target_id,
            selection,
            message,
        } => {
            if let Some(idx) = app.find_target_by_id(&target_id)
                && selection.is_current()
                && app.targets[idx].connection.as_ref() == Some(selection.connection())
            {
                app.targets[idx].stream_binding = None;
                app.targets[idx].phase = TargetPhase::Idle;
                app.targets[idx].flush_streaming();
                app.targets[idx].push_notice("error", &message);
            }
        }
        TuiEvent::StreamEnded { target_id, binding } => {
            if let Some(idx) = app.find_target_by_id(&target_id)
                && app.targets[idx].stream_binding.as_ref() == Some(&binding)
            {
                app.targets[idx].stream_binding = None;
                app.targets[idx].flush_streaming();
                app.targets[idx].phase = TargetPhase::Idle;
                app.targets[idx]
                    .push_notice("stream ended", "use /resume to rebind before sending");
            }
        }
        TuiEvent::StreamEvent {
            target_id,
            event,
            binding,
        } => {
            if let Some(idx) = app.find_target_by_id(&target_id)
                && binding.is_current()
                && app.targets[idx].stream_binding.as_ref() == Some(&binding)
            {
                handle_stream_event(&mut app.targets[idx], &event);
            }
        }
        TuiEvent::ModelCatalog { target_id, catalog } => {
            if let Some(idx) = app.find_target_by_id(&target_id) {
                let mut lines = vec!["**Available models:**".to_string(), String::new()];
                let mut model_ids = HashSet::new();
                // The catalog response has { providers: [{ provider, default_model_id, models: [...] }] }
                if let Some(providers) = catalog.get("providers").and_then(|v| v.as_array()) {
                    for provider_entry in providers {
                        let provider_name = provider_entry
                            .get("provider")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown");
                        let default_model = provider_entry
                            .get("default_model_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        lines.push(format!("*{provider_name}* (default: {default_model})"));
                        if let Some(models) =
                            provider_entry.get("models").and_then(|v| v.as_array())
                        {
                            for model in models {
                                let id = model.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                                let name = model
                                    .get("display_name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(id);
                                lines.push(format!("  `{id}` -- {name}"));
                                model_ids.insert(id.to_string());
                            }
                        }
                        lines.push(String::new());
                    }
                } else {
                    lines.push("(unexpected catalog format)".into());
                }
                app.targets[idx].known_models = model_ids;
                app.targets[idx].push_section(lines);
            }
        }
        TuiEvent::SessionList {
            target_id,
            sessions,
        } => {
            if let Some(idx) = app.find_target_by_id(&target_id) {
                let session_rows = sessions
                    .get("sessions")
                    .and_then(|value| value.as_array())
                    .or_else(|| sessions.as_array());
                let display = if let Some(arr) = session_rows {
                    if arr.is_empty() {
                        "  (no persisted sessions)".to_string()
                    } else {
                        arr.iter()
                            .map(|s| {
                                let id =
                                    s.get("session_id").and_then(|v| v.as_str()).unwrap_or("?");
                                let message_count =
                                    s.get("message_count").and_then(|v| v.as_u64()).unwrap_or(0);
                                let active = s
                                    .get("is_active")
                                    .and_then(|v| v.as_bool())
                                    .unwrap_or(false);
                                let state = if active { ", active" } else { "" };
                                format!("  {id} ({message_count} messages{state})")
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    }
                } else {
                    serde_json::to_string_pretty(&sessions).unwrap_or_else(|_| sessions.to_string())
                };
                app.targets[idx].push_section([
                    "**Sessions**".to_string(),
                    display,
                    String::new(),
                    "Use `/resume <ID>` with the full session ID.".to_string(),
                ]);
            }
        }
        TuiEvent::Kennel { epoch, event } => {
            let Some(claims) = claims else {
                return;
            };
            let owner = claims.read();
            if owner.broker_epoch.as_ref() != Some(&epoch) {
                return;
            }
            // Keep the owner observation stable through projection and command enqueue.
            process_kennel_event(app, event, rpc_tx, &owner);
        }
        TuiEvent::KennelError(msg) => {
            if let Some(target) = app.selected_target_mut() {
                target.push_notice("kennel", &msg);
            }
        }
    }
}

fn process_kennel_event(
    app: &mut App,
    event: KennelUiEvent,
    rpc_tx: &mpsc::UnboundedSender<RpcCommand>,
    owner: &TuxKennelState,
) {
    match event {
        KennelUiEvent::AvailableTargets(targets) => {
            // Discovery is not ownership intent.
            for entry in &targets {
                if let Some(rpc_addr) = &entry.rpc_addr {
                    if app.find_target_by_id(&entry.target_id).is_none() {
                        app.targets.push(TargetView::new(
                            entry.name.clone(),
                            entry.target_id.clone(),
                            rpc_addr.clone(),
                        ));
                    } else if let Some(idx) = app.find_target_by_id(&entry.target_id) {
                        app.targets[idx].rpc_addr = rpc_addr.clone();
                    }
                }
                app.target_states
                    .entry(entry.target_id.clone())
                    .or_insert(KennelUiState::Available);
            }
        }
        KennelUiEvent::MineTargets(targets) => {
            for entry in &targets {
                let Some(ClaimState::Claimed { lease_id, .. }) = owner.claims.get(&entry.target_id)
                else {
                    continue;
                };
                if entry.lease_id.as_ref() != Some(lease_id) {
                    continue;
                }
                app.target_states
                    .insert(entry.target_id.clone(), KennelUiState::ClaimedByMe);
                app.target_leases
                    .insert(entry.target_id.clone(), lease_id.clone());
                if let Some(rpc_addr) = &entry.rpc_addr
                    && let Some(idx) = app.find_target_by_id(&entry.target_id)
                {
                    let addr_changed = app.targets[idx].rpc_addr != *rpc_addr;
                    app.targets[idx].rpc_addr = rpc_addr.clone();
                    if addr_changed || app.targets[idx].phase == TargetPhase::Disconnected {
                        let _ = rpc_tx.send(RpcCommand::Connect {
                            target_id: entry.target_id.clone(),
                            rpc_addr: rpc_addr.clone(),
                        });
                    }
                }
            }
        }
        KennelUiEvent::ClaimGranted(grants) => {
            for grant in &grants {
                if owner
                    .claims
                    .get(&grant.target_id)
                    .and_then(ClaimState::lease_id)
                    != Some(grant.lease_id.as_str())
                {
                    continue;
                }
                app.target_states
                    .insert(grant.target_id.clone(), KennelUiState::ClaimedByMe);
                app.target_leases
                    .insert(grant.target_id.clone(), grant.lease_id.clone());
                // Add target if not already present
                if app.find_target_by_id(&grant.target_id).is_none() {
                    let rpc_addr = grant.rpc_addr.clone().unwrap_or_default();
                    app.targets.push(TargetView::new(
                        grant.target_name.clone(),
                        grant.target_id.clone(),
                        rpc_addr,
                    ));
                }
                // Connect RPC immediately (no attach step)
                if let Some(rpc_addr) = &grant.rpc_addr
                    && let Some(idx) = app.find_target_by_id(&grant.target_id)
                    && !rpc_addr.is_empty()
                {
                    app.targets[idx].rpc_addr = rpc_addr.clone();
                    let _ = rpc_tx.send(RpcCommand::Connect {
                        target_id: grant.target_id.clone(),
                        rpc_addr: rpc_addr.clone(),
                    });
                    app.targets[idx].push_notice("claimed", &grant.target_name);
                }
            }
        }
        KennelUiEvent::ClaimReleased {
            target_id,
            lease_ref,
            reason,
        } => {
            if !matches!(
                owner.claims.get(&target_id),
                Some(ClaimState::Available { .. })
            ) {
                return;
            }
            if let LeaseRef::Known { lease_id } = &lease_ref
                && app
                    .target_leases
                    .get(&target_id)
                    .is_some_and(|current| current != lease_id)
            {
                return;
            }
            app.target_states
                .insert(target_id.clone(), KennelUiState::Available);
            app.target_leases.remove(&target_id);
            let _ = rpc_tx.send(RpcCommand::Disconnect {
                target_id: target_id.clone(),
            });
            if let Some(idx) = app.find_target_by_id(&target_id) {
                app.targets[idx].stream_binding = None;
                app.targets[idx].flush_streaming();
                app.targets[idx].phase = TargetPhase::Disconnected;
                let lease_text = match &lease_ref {
                    LeaseRef::Known { lease_id } => lease_id.clone(),
                    LeaseRef::PendingRebind => "pending rebind".into(),
                };
                app.targets[idx].push_notice(
                    "kennel released claim",
                    &format!("({lease_text}): {reason}"),
                );
            }
        }
        KennelUiEvent::HiveRegistered {
            rpc_addr,
            session_id,
        } => {
            let index = app.find_target_by_id("hive").unwrap_or_else(|| {
                app.targets.push(TargetView::new(
                    "hive".into(),
                    "hive".into(),
                    rpc_addr.clone(),
                ));
                app.targets.len() - 1
            });
            app.targets[index].rpc_addr = rpc_addr.clone();
            app.targets[index].session_id = session_id;
            app.targets[index].stream_binding = None;
            app.targets[index].connection = None;
            app.targets[index].phase = TargetPhase::Disconnected;
            let _ = rpc_tx.send(RpcCommand::Connect {
                target_id: "hive".into(),
                rpc_addr,
            });
        }
        KennelUiEvent::Error(msg) => {
            if let Some(t) = app.selected_target_mut() {
                t.push_notice("kennel", &msg);
            }
        }
        KennelUiEvent::Reconciled => {
            for (id, claim) in &owner.claims {
                if matches!(claim, ClaimState::Available { .. }) {
                    app.target_states
                        .insert(id.clone(), KennelUiState::Available);
                    app.target_leases.remove(id);
                }
            }
        }
        KennelUiEvent::HiveStreamEvent { event } => {
            handle_stream_event(&mut app.hive_view, &event);
        }
        KennelUiEvent::HiveComplete { text } => {
            app.hive_view.flush_streaming();
            if !text.is_empty() {
                app.hive_view.push_remote_turn(&text);
            }
            app.hive_planning = false;
        }
        KennelUiEvent::HiveError { message } => {
            app.hive_view.flush_streaming();
            app.hive_view.push_notice("hive error", &message);
            app.hive_planning = false;
        }
    }
}

// ── Slash command handling ────────────────────────────────────────────────────

fn is_known_slash_command(input: &str) -> bool {
    input.starts_with('/')
        && matches!(
            input.split_once(' ').map(|(cmd, _)| cmd).unwrap_or(input),
            "/new"
                | "/resume"
                | "/help"
                | "/claim"
                | "/release"
                | "/model"
                | "/models"
                | "/steer"
                | "/queue"
                | "/interrupt"
        )
}

fn short_id(id: &str) -> String {
    if id.len() <= 18 {
        id.to_string()
    } else {
        format!("{}...{}", &id[..8], &id[id.len() - 6..])
    }
}

// ── Key handling ─────────────────────────────────────────────────────────────

fn timeline_max_scroll(content_height: u16, inner_height: u16) -> u16 {
    content_height.saturating_sub(inner_height)
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
    rpc_tx: &mpsc::UnboundedSender<RpcCommand>,
    claims: Option<&ClaimRegistry>,
    kennel_tx: Option<&mpsc::UnboundedSender<KennelClientCommand>>,
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
        KeyCode::PageUp => scroll_timeline_up(app, 10),
        KeyCode::PageDown => scroll_timeline_down(app, 10),
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
            if let Some(t) = app.active_view_mut() {
                t.history.clear();
                t.streaming_text.clear();
            }
            app.scroll_offset = 0;
            app.auto_scroll = true;
        }
        KeyCode::Enter if modifiers.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) => {
            app.input.push('\n');
        }
        KeyCode::Char('j') if modifiers.contains(KeyModifiers::CONTROL) => {
            app.input.push('\n');
        }
        KeyCode::Enter if !app.input.is_empty() => {
            let is_slash = is_known_slash_command(&app.input);
            match app.mode {
                Mode::Direct if app.targets.is_empty() && !is_slash => return,
                Mode::Hive if app.hive_planning => return,
                _ => {}
            }

            let body = std::mem::take(&mut app.input);

            if is_slash {
                handle_slash_command(app, &body, rpc_tx, claims, kennel_tx);
                return;
            }

            // Regular message
            if app.mode == Mode::Direct && !app.targets.is_empty() {
                let idx = app.selected;
                let target = &app.targets[idx];
                if target.phase == TargetPhase::Disconnected {
                    app.targets[idx].push_notice("error", "target not connected");
                    return;
                }
                if target.phase == TargetPhase::Running {
                    app.targets[idx].push_notice("busy", "wait for completion or /interrupt");
                    return;
                }
                if target.session_id.is_none()
                    || !target
                        .stream_binding
                        .as_ref()
                        .is_some_and(|binding| binding.is_current())
                {
                    let message = if app.transport == TransportMode::Direct {
                        "no persisted session is available; fresh direct-session creation is not implemented"
                    } else {
                        "no session is bound; wait for kennel registration"
                    };
                    app.targets[idx].push_notice("error", message);
                    return;
                }
                // Kennel claim check (skip for hive — it's directly connected
                // via RPC, not brokered through the kennel claim protocol).
                if app.transport == TransportMode::Kennel && target.target_id != "hive" {
                    let tid = &target.target_id;
                    if !matches!(app.target_states.get(tid), Some(KennelUiState::ClaimedByMe)) {
                        app.targets[idx].push_notice("error", "use /claim first");
                        return;
                    }
                }

                let target_id = app.targets[idx].target_id.clone();
                let session_id = app.targets[idx].session_id.clone().unwrap_or_default();
                let model = app.pending_model.take();
                app.targets[idx].push_user_turn(&body);
                app.targets[idx].phase = TargetPhase::Running;
                let _ = rpc_tx.send(RpcCommand::StartTurn {
                    target_id,
                    session_id,
                    prompt: body,
                    model,
                });
            }
            // Hive mode: dispatch to the "hive" target via RPC (same as Direct)
            else if app.mode == Mode::Hive {
                if let Some(hive_idx) = app.find_target_by_id("hive") {
                    let hive = &app.targets[hive_idx];
                    if hive.phase == TargetPhase::Disconnected {
                        app.targets[hive_idx].push_notice("error", "hive not connected");
                        return;
                    }
                    if hive.phase == TargetPhase::Running {
                        app.targets[hive_idx]
                            .push_notice("busy", "wait for completion or /interrupt");
                        return;
                    }
                    if hive.session_id.is_none()
                        || !hive
                            .stream_binding
                            .as_ref()
                            .is_some_and(|binding| binding.is_current())
                    {
                        app.targets[hive_idx]
                            .push_notice("error", "hive has no session; waiting for connection");
                        return;
                    }
                    let target_id = hive.target_id.clone();
                    let session_id = hive.session_id.clone().unwrap_or_default();
                    let model = app.pending_model.take();
                    app.targets[hive_idx].push_user_turn(&body);
                    app.targets[hive_idx].phase = TargetPhase::Running;
                    let _ = rpc_tx.send(RpcCommand::StartTurn {
                        target_id,
                        session_id,
                        prompt: body,
                        model,
                    });
                } else {
                    if let Some(t) = app.targets.get_mut(0) {
                        t.push_notice("error", "hive not available (kennel mode required)")
                    }
                }
            }
        }
        KeyCode::Char(c) => app.input.push(c),
        KeyCode::Backspace => {
            app.input.pop();
        }
        KeyCode::Esc => app.quit = true,
        _ => {}
    }
}

fn handle_slash_command(
    app: &mut App,
    body: &str,
    rpc_tx: &mpsc::UnboundedSender<RpcCommand>,
    claims: Option<&ClaimRegistry>,
    kennel_tx: Option<&mpsc::UnboundedSender<KennelClientCommand>>,
) {
    let parts: Vec<&str> = body.splitn(2, ' ').collect();
    let slash = parts[0];
    let arg = parts.get(1).unwrap_or(&"").trim();
    let idx = if app.mode == Mode::Hive {
        app.find_target_by_id("hive").unwrap_or(app.selected)
    } else {
        app.selected
    };

    match slash {
        "/new" => {
            if app.targets.is_empty() {
                return;
            }
            app.targets[idx].push_user_turn("/new");
            app.targets[idx].push_notice("unsupported",
                "/new is disabled: managed sessions require an owner-coordinated reset; no session was archived or replaced");
        }
        "/resume" if arg.is_empty() => {
            if app.targets.is_empty() {
                return;
            }
            let target_id = app.targets[idx].target_id.clone();
            app.targets[idx].push_user_turn("/resume");
            let _ = rpc_tx.send(RpcCommand::ListSessions { target_id });
        }
        "/resume" => {
            if app.targets.is_empty() {
                return;
            }
            let target_id = app.targets[idx].target_id.clone();
            let session_id = arg.to_string();
            app.targets[idx].push_user_turn(&format!("/resume {arg}"));
            app.targets[idx].history.clear();
            app.targets[idx].streaming_text.clear();
            let _ = rpc_tx.send(RpcCommand::ResumeSession {
                target_id,
                session_id,
            });
        }
        "/help" => {
            if let Some(t) = app.targets.get_mut(idx) {
                t.push_section(["**Commands**".into()]);
                if app.transport == TransportMode::Kennel {
                    t.push_line("  `/claim`        -- claim selected target from kennel".into());
                    t.push_line(
                        "  `/release`      -- release selected target back to kennel".into(),
                    );
                }
                t.push_line(
                    "  `/new`          -- unsupported (no managed reset transaction)".into(),
                );
                t.push_line("  `/resume`       -- list past sessions".into());
                t.push_line("  `/resume <ID>`  -- resume session by ID".into());
                t.push_line("  `/model <name>` -- set model for next turn".into());
                t.push_line("  `/models`       -- list available models".into());
                t.push_line("  `/steer`, `/queue` -- unsupported on this RPC client".into());
                t.push_line("  `/interrupt`    -- interrupt current turn".into());
                t.push_line("  `/help`         -- show this help".into());
            }
        }
        "/model" if !arg.is_empty() => {
            if let Some(t) = app.targets.get_mut(idx) {
                // Validate against cached model catalog (if available).
                if !t.known_models.is_empty() && !t.known_models.contains(arg) {
                    t.push_notice(
                        "error",
                        &format!("unknown model '{arg}'. Run /models to see available models."),
                    );
                    return;
                }
                app.pending_model = Some(arg.to_string());
                t.push_notice("model", &format!("next turn will use model: {arg}"));
            }
        }
        "/model" => {
            if let Some(t) = app.targets.get_mut(idx) {
                t.push_line("Usage: /model <model-name>".into());
            }
        }
        "/models" => {
            if app.targets.is_empty() {
                return;
            }
            let target_id = app.targets[idx].target_id.clone();
            app.targets[idx].push_user_turn("/models");
            let _ = rpc_tx.send(RpcCommand::ListModels { target_id });
        }
        "/steer" | "/queue" => {
            if let Some(t) = app.targets.get_mut(idx) {
                t.push_notice(
                    "unsupported",
                    "turn/start has no steering/queue mode; wait for completion or /interrupt",
                );
            }
        }
        "/interrupt" => {
            if app.targets.is_empty() {
                return;
            }
            let target_id = app.targets[idx].target_id.clone();
            if let Some(session_id) = app.targets[idx].session_id.clone() {
                app.targets[idx].push_user_turn("/interrupt");
                let _ = rpc_tx.send(RpcCommand::Interrupt {
                    target_id,
                    session_id,
                });
            }
        }
        "/claim" => {
            if app.transport != TransportMode::Kennel || app.targets.is_empty() {
                return;
            }
            let target_id = app.targets[idx].target_id.clone();
            app.targets[idx].push_user_turn("/claim");
            if let Some(claims) = claims {
                let _ = apply_tkc_event(
                    claims,
                    kennel_tx,
                    TkcEvent::UserClaimTarget { target_id },
                    |_| (),
                );
            }
        }
        "/release" => {
            if app.transport != TransportMode::Kennel || app.targets.is_empty() {
                return;
            }
            let target_id = app.targets[idx].target_id.clone();
            if let Some(claims) = claims
                && let Some(state) = claim_state_for_id(claims, &target_id)
                && let Some(lease_id) = state.lease_id()
            {
                app.targets[idx].push_user_turn("/release");
                let _ = apply_tkc_event(
                    claims,
                    kennel_tx,
                    TkcEvent::UserReleaseLease {
                        lease_id: lease_id.to_string(),
                    },
                    |_| (),
                );
            }
        }
        _ => {}
    }
}

// ── Rendering ────────────────────────────────────────────────────────────────

fn render(f: &mut ratatui::Frame, app: &mut App) {
    let label = app.input_label();
    let label_len = label.len();
    let inner_width = f.area().width.saturating_sub(2) as usize;
    let input_lines: usize = app
        .input
        .split('\n')
        .enumerate()
        .map(|(i, line)| {
            let avail = if i == 0 {
                inner_width.saturating_sub(label_len)
            } else {
                inner_width.saturating_sub(2)
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

    render_header(f, outer[0], app);

    let main = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(25), Constraint::Percentage(75)])
        .split(outer[1]);

    render_targets(f, main[0], app);
    render_timeline(f, main[1], app);
    render_input(f, outer[2], app);
    render_footer(f, outer[3], app);
}

fn render_header(f: &mut ratatui::Frame, area: ratatui::layout::Rect, app: &App) {
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
        .map(|name| format!("  selected: {name}"))
        .unwrap_or_else(|| "  selected: none".into());
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " TUX -- Meerkat Device Manager ",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                match app.transport {
                    TransportMode::Direct => " RPC Direct ",
                    TransportMode::Kennel => " Kennel ",
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

fn render_targets(f: &mut ratatui::Frame, area: ratatui::layout::Rect, app: &App) {
    let items: Vec<ListItem> = app
        .targets
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let sel = app.mode == Mode::Direct && i == app.selected;
            let prefix = if sel { "> " } else { "  " };
            let (icon, suffix) = match t.phase {
                TargetPhase::Disconnected => ("x", " disconnected"),
                TargetPhase::Running => ("*", " running"),
                TargetPhase::Idle => {
                    if app.transport == TransportMode::Kennel {
                        match app.target_states.get(&t.target_id).copied() {
                            Some(KennelUiState::ClaimedByMe) => ("o", " mine"),
                            _ => ("o", ""),
                        }
                    } else {
                        ("o", " idle")
                    }
                }
            };
            let style = if sel {
                Style::default()
                    .fg(Color::Green)
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
            } else {
                match t.phase {
                    TargetPhase::Disconnected => Style::default().fg(Color::DarkGray),
                    TargetPhase::Running => Style::default().fg(Color::Yellow),
                    TargetPhase::Idle => Style::default(),
                }
            };
            ListItem::new(format!("{prefix}{icon} {}{suffix}", t.name)).style(style)
        })
        .collect();
    let title = format!("TARGETS  {}", app.targets.len());
    f.render_widget(
        List::new(items).block(Block::default().borders(Borders::ALL).title(title)),
        area,
    );
}

fn render_timeline(f: &mut ratatui::Frame, area: ratatui::layout::Rect, app: &mut App) {
    let inner_width = area.width.saturating_sub(2);
    let inner_height = area.height.saturating_sub(2);

    // In Hive mode, show the hive target's view; in Direct mode, show the selected target.
    let Some(target) = app.active_view() else {
        let placeholder =
            "No targets configured.\n\nUse --target HOST:PORT to connect to an agent.";
        f.render_widget(
            Paragraph::new(placeholder)
                .block(Block::default().borders(Borders::ALL).title("TIMELINE"))
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(Color::DarkGray)),
            area,
        );
        return;
    };

    // Build markdown from the selected target's history + streaming text
    let mut md = String::new();
    for line in &target.history {
        md.push_str(line);
        md.push('\n');
    }
    if !target.streaming_text.is_empty() {
        md.push_str(&format!("[{}] {}...\n", target.name, target.streaming_text));
    }

    if md.trim().is_empty() {
        let placeholder = format!(
            "No activity yet for {}.\n\nType a command or use /help.",
            target.name
        );
        f.render_widget(
            Paragraph::new(placeholder)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(format!("TIMELINE  {}  idle", target.name)),
                )
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(Color::DarkGray)),
            area,
        );
        return;
    }

    let text = tui_markdown::from_str(&md);
    let paragraph = Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL).title(""))
        .wrap(Wrap { trim: false });

    let content_height = paragraph.line_count(inner_width) as u16;
    let max_scroll = timeline_max_scroll(content_height, inner_height);
    let phase = target.phase;
    let name = target.name.clone();
    let history_len = target.history.len();
    if app.auto_scroll {
        app.scroll_offset = max_scroll;
    } else {
        app.scroll_offset = app.scroll_offset.min(max_scroll);
        if app.scroll_offset >= max_scroll {
            app.auto_scroll = true;
        }
    }

    let phase_label = match phase {
        TargetPhase::Disconnected => "disconnected",
        TargetPhase::Running => "running",
        TargetPhase::Idle => "idle",
    };
    let title = if app.auto_scroll {
        format!("TIMELINE  {}  {}  {} lines", name, phase_label, history_len)
    } else {
        format!(
            "TIMELINE  {}  scrolled  line {} / {}",
            name,
            app.scroll_offset.saturating_add(1),
            max_scroll.saturating_add(1)
        )
    };

    let paragraph = Paragraph::new(tui_markdown::from_str(&md))
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: false })
        .scroll((app.scroll_offset, 0));

    f.render_widget(paragraph, area);
}

fn render_input(f: &mut ratatui::Frame, area: ratatui::layout::Rect, app: &App) {
    let label = app.input_label();
    let mut lines: Vec<Line> = Vec::new();
    if app.input.is_empty() {
        lines.push(Line::from(vec![
            Span::raw(label.clone()),
            Span::styled(
                "Type a command or /help",
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
    if let Some(last) = lines.last_mut() {
        last.spans
            .push(Span::styled("_", Style::default().fg(Color::DarkGray)));
    }

    // Status hint line
    let hint = if app.mode == Mode::Hive {
        if let Some(hive_idx) = app.find_target_by_id("hive") {
            let hive = &app.targets[hive_idx];
            match hive.phase {
                TargetPhase::Disconnected => {
                    "Hive disconnected -- waiting for RPC connection".into()
                }
                TargetPhase::Running => "Hive is working; wait or /interrupt.".into(),
                TargetPhase::Idle => "Ready to send to hive.".into(),
            }
        } else {
            "Hive not available.".into()
        }
    } else if let Some(t) = app.selected_target() {
        match t.phase {
            TargetPhase::Disconnected => "Target disconnected -- waiting for RPC connection".into(),
            TargetPhase::Running => format!("{} is working; wait or /interrupt.", t.name),
            TargetPhase::Idle => format!("Ready to send to {}.", t.name),
        }
    } else {
        "No target selected.".into()
    };
    lines.push(Line::from(Span::styled(
        hint,
        Style::default().fg(Color::DarkGray),
    )));

    let status = if let Some(t) = app.selected_target() {
        match t.phase {
            TargetPhase::Running => ("Running", Color::Yellow),
            TargetPhase::Idle => ("Ready", Color::Green),
            TargetPhase::Disconnected => ("Disconnected", Color::Red),
        }
    } else {
        ("No target", Color::Yellow)
    };

    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(Line::from(vec![
                        Span::raw("COMMAND  "),
                        Span::styled(
                            status.0,
                            Style::default().fg(status.1).add_modifier(Modifier::BOLD),
                        ),
                    ])),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_footer(f: &mut ratatui::Frame, area: ratatui::layout::Rect, app: &App) {
    let selected_hint = app
        .selected_target_name()
        .map(|name| format!("selected: {name}"))
        .unwrap_or_else(|| "no target selected".into());
    let scroll_hint = if app.auto_scroll {
        "timeline live"
    } else {
        "timeline paused"
    };
    let lines = vec![
        Line::from(vec![Span::styled(
            format!("{selected_hint}  |  {scroll_hint}"),
            Style::default().fg(Color::DarkGray),
        )]),
        Line::from(vec![Span::styled(
            "[Tab] mode  [Up/Down] select  [Enter] send  [Shift+Enter] newline  \
             [Ctrl+U] clear input  [Ctrl+L] clear timeline  [PgUp/PgDn] scroll  [Esc] quit",
            Style::default().fg(Color::DarkGray),
        )]),
    ];
    f.render_widget(Paragraph::new(lines), area);
}

// ── Utility ──────────────────────────────────────────────────────────────────

fn find_flag(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

fn find_all_flags(args: &[String], flag: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == flag
            && let Some(val) = args.get(i + 1)
        {
            values.push(val.clone());
            i += 2;
            continue;
        }
        i += 1;
    }
    values
}

fn discover_local_ip(host: &str, port: u16) -> anyhow::Result<String> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").context("bind UDP probe socket")?;
    sock.connect(format!("{host}:{port}"))
        .with_context(|| format!("UDP probe to {host}:{port}"))?;
    Ok(sock.local_addr()?.ip().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn slash_command_guard_recognizes_supported_commands() {
        assert!(is_known_slash_command("/help"));
        assert!(is_known_slash_command("/resume 3"));
        assert!(is_known_slash_command("/claim"));
        assert!(is_known_slash_command("/steer"));
        assert!(is_known_slash_command("/queue"));
        assert!(is_known_slash_command("/interrupt"));
        assert!(!is_known_slash_command("/wat"));
        assert!(!is_known_slash_command("ls /tmp"));
    }

    #[test]
    fn target_view_push_respects_capacity() {
        let mut target = TargetView::new("t".into(), "id".into(), "addr".into());
        for i in 0..1100 {
            target.push_line(format!("line {i}"));
        }
        assert!(target.history.len() <= 1000);
    }

    #[test]
    fn timeline_scroll_helpers_disable_auto_follow() {
        let mut app = App {
            transport: TransportMode::Direct,
            mode: Mode::Direct,
            targets: Vec::new(),
            selected: 0,
            input: String::new(),
            scroll_offset: 20,
            auto_scroll: true,
            hive_planning: false,
            quit: false,
            pending_model: None,
            target_states: HashMap::new(),
            target_leases: HashMap::new(),
            hive_view: TargetView::new("hive".into(), "hive".into(), String::new()),
        };
        scroll_timeline_up(&mut app, 5);
        assert!(!app.auto_scroll);
        assert_eq!(app.scroll_offset, 15);
        scroll_timeline_down(&mut app, 3);
        assert_eq!(app.scroll_offset, 18);
    }

    #[test]
    fn timeline_max_scroll_exact_bottom() {
        assert_eq!(timeline_max_scroll(40, 10), 30);
        assert_eq!(timeline_max_scroll(10, 10), 0);
        assert_eq!(timeline_max_scroll(8, 10), 0);
    }

    #[test]
    fn short_id_truncates_long_ids() {
        assert_eq!(short_id("short"), "short");
        assert_eq!(short_id("abcdefghijklmnopqrstuvwxyz"), "abcdefgh...uvwxyz");
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
    }

    #[test]
    fn find_all_flags_collects_multiple_values() {
        let args: Vec<String> = vec![
            "--target",
            "host1:8000",
            "--target",
            "host2:9000",
            "--other",
            "val",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        let targets = find_all_flags(&args, "--target");
        assert_eq!(targets, vec!["host1:8000", "host2:9000"]);
    }

    fn app() -> App {
        App {
            transport: TransportMode::Kennel,
            mode: Mode::Hive,
            targets: vec![
                TargetView::new("target".into(), "t".into(), "addr".into()),
                TargetView::new("hive".into(), "hive".into(), "addr".into()),
            ],
            selected: 0,
            input: String::new(),
            scroll_offset: 0,
            auto_scroll: true,
            hive_planning: false,
            quit: false,
            pending_model: None,
            target_states: HashMap::new(),
            target_leases: HashMap::new(),
            hive_view: TargetView::new("fallback".into(), "hive".into(), String::new()),
        }
    }

    fn register_broker(claims: &ClaimRegistry, incarnation: &str) -> BrokerEpoch {
        let epoch = BrokerEpoch::new(incarnation);
        apply_tkc_event(
            claims,
            None,
            TkcEvent::BrokerRegistered {
                epoch: epoch.clone(),
            },
            |_| (),
        )
        .unwrap();
        epoch
    }

    async fn stream_server_fixture() -> (String, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = stream.into_split();
            let mut lines = BufReader::new(reader).lines();
            let mut next = 0u64;
            while let Some(line) = lines.next_line().await.unwrap() {
                let request: Value = serde_json::from_str(&line).unwrap();
                let result = match request["method"].as_str().unwrap() {
                    "initialize" => serde_json::json!({}),
                    "session/stream_open" => {
                        next += 1;
                        serde_json::json!({"stream_id": format!("stream-{next}")})
                    }
                    "session/stream_close" => serde_json::json!({"closed": true}),
                    method => panic!("unexpected fixture request {method}"),
                };
                let response = serde_json::json!({
                    "jsonrpc": "2.0", "id": request["id"], "result": result,
                });
                writer
                    .write_all(format!("{response}\n").as_bytes())
                    .await
                    .unwrap();
            }
        });
        (addr.to_string(), server)
    }

    async fn stream_client_fixture() -> (RpcClient, tokio::task::JoinHandle<()>) {
        let (addr, server) = stream_server_fixture().await;
        let (events, _) = mpsc::unbounded_channel();
        let client = RpcClient::connect(&addr, events).await.unwrap();
        (client, server)
    }

    #[test]
    fn unsupported_reset_and_modes_never_send_rpc_commands() {
        let mut app = app();
        let (tx, mut rx) = mpsc::unbounded_channel();
        for mode in [Mode::Hive, Mode::Direct] {
            app.mode = mode;
            for command in ["/new", "/steer", "/queue"] {
                handle_slash_command(&mut app, command, &tx, None, None);
                assert!(rx.try_recv().is_err());
            }
        }
        assert!(
            app.targets
                .iter()
                .all(|view| view.history.iter().any(|line| line.contains("unsupported")))
        );
    }

    #[tokio::test]
    async fn initial_hive_binding_subscribes_and_obsolete_streams_cannot_paint() {
        let (client, server) = stream_client_fixture().await;
        let mut app = app();
        let claims = ClaimRegistry::default();
        let epoch = register_broker(&claims, "broker");
        let (rpc, mut commands) = mpsc::unbounded_channel();
        process_event(
            &mut app,
            TuiEvent::kennel(
                &epoch,
                KennelUiEvent::HiveRegistered {
                    rpc_addr: "fixture-rpc-address".into(),
                    session_id: Some("managed-hive".into()),
                },
            ),
            &rpc,
            Some(&claims),
            None,
        );
        assert!(
            matches!(commands.try_recv().unwrap(), RpcCommand::Connect { target_id, .. }
            if target_id == "hive")
        );
        process_event(
            &mut app,
            TuiEvent::TargetConnected {
                target_id: "hive".into(),
                connection: client.connection(),
            },
            &rpc,
            None,
            None,
        );
        assert!(
            matches!(commands.try_recv().unwrap(), RpcCommand::ResumeSession {
            target_id, session_id,
        } if target_id == "hive" && session_id == "managed-hive")
        );
        let stale = client.bind_session("old-hive").await.unwrap();
        let binding = client.bind_session("managed-hive").await.unwrap();
        process_event(
            &mut app,
            TuiEvent::SessionBound {
                target_id: "hive".into(),
                binding: binding.clone(),
            },
            &rpc,
            None,
            None,
        );
        let event = serde_json::json!({"event":{"payload":{"type":"text_delta","delta":"fresh"}}});
        process_event(
            &mut app,
            TuiEvent::StreamEvent {
                target_id: "hive".into(),
                binding: stale.clone(),
                event: event.clone(),
            },
            &rpc,
            None,
            None,
        );
        assert!(app.active_view().unwrap().streaming_text.is_empty());
        process_event(
            &mut app,
            TuiEvent::StreamEnded {
                target_id: "hive".into(),
                binding: stale,
            },
            &rpc,
            None,
            None,
        );
        assert_eq!(
            app.active_view().unwrap().stream_binding.as_ref(),
            Some(&binding)
        );
        process_event(
            &mut app,
            TuiEvent::StreamEvent {
                target_id: "hive".into(),
                binding: binding.clone(),
                event,
            },
            &rpc,
            None,
            None,
        );
        assert_eq!(app.active_view().unwrap().streaming_text, "fresh");
        client.close().await;
        server.await.unwrap();
    }

    #[tokio::test]
    async fn reversed_bound_publication_and_late_end_cannot_replace_latest_selection() {
        let (client, server) = stream_client_fixture().await;
        let old = client.bind_session("session-a").await.unwrap();
        let newest = client.bind_session("session-b").await.unwrap();
        let mut app = app();
        let (rpc, _commands) = mpsc::unbounded_channel();
        process_event(
            &mut app,
            TuiEvent::TargetConnected {
                target_id: "hive".into(),
                connection: client.connection(),
            },
            &rpc,
            None,
            None,
        );
        process_event(
            &mut app,
            TuiEvent::SessionBound {
                target_id: "hive".into(),
                binding: newest.clone(),
            },
            &rpc,
            None,
            None,
        );
        app.targets[1].phase = TargetPhase::Running;
        let history_count = app.targets[1].history.len();
        process_event(
            &mut app,
            TuiEvent::SessionBound {
                target_id: "hive".into(),
                binding: old.clone(),
            },
            &rpc,
            None,
            None,
        );
        process_event(
            &mut app,
            TuiEvent::StreamEnded {
                target_id: "hive".into(),
                binding: old,
            },
            &rpc,
            None,
            None,
        );
        assert_eq!(app.targets[1].session_id.as_deref(), Some("session-b"));
        assert_eq!(app.targets[1].stream_binding.as_ref(), Some(&newest));
        assert!(app.targets[1].phase == TargetPhase::Running);
        assert_eq!(app.targets[1].history.len(), history_count);
        let ended = client
            .ended_stream(&serde_json::json!({
                "method": "session/stream_end",
                "params": {"session_id": newest.session_id, "stream_id": newest.stream_id},
            }))
            .await
            .unwrap();
        process_event(
            &mut app,
            TuiEvent::StreamEnded {
                target_id: "hive".into(),
                binding: ended.clone(),
            },
            &rpc,
            None,
            None,
        );
        process_event(
            &mut app,
            TuiEvent::SessionBound {
                target_id: "hive".into(),
                binding: ended,
            },
            &rpc,
            None,
            None,
        );
        assert!(app.targets[1].stream_binding.is_none());
        client.close().await;
        server.await.unwrap();
    }

    #[tokio::test]
    async fn obsolete_connection_bound_and_lifecycle_events_cannot_replace_current_connection() {
        let (old_client, old_server) = stream_client_fixture().await;
        let old = old_client.bind_session("old-session").await.unwrap();
        let (new_client, new_server) = stream_client_fixture().await;
        let new = new_client.bind_session("new-session").await.unwrap();
        let mut app = app();
        let (rpc, _commands) = mpsc::unbounded_channel();
        process_event(
            &mut app,
            TuiEvent::TargetConnected {
                target_id: "hive".into(),
                connection: new_client.connection(),
            },
            &rpc,
            None,
            None,
        );
        process_event(
            &mut app,
            TuiEvent::SessionBound {
                target_id: "hive".into(),
                binding: new.clone(),
            },
            &rpc,
            None,
            None,
        );
        // Even before transport shutdown, the view only accepts its own connection.
        assert!(old.is_current());
        process_event(
            &mut app,
            TuiEvent::SessionBound {
                target_id: "hive".into(),
                binding: old,
            },
            &rpc,
            None,
            None,
        );
        old_client.shutdown().await;
        process_event(
            &mut app,
            TuiEvent::TargetDisconnected {
                target_id: "hive".into(),
                connection: old_client.connection(),
            },
            &rpc,
            None,
            None,
        );
        process_event(
            &mut app,
            TuiEvent::TargetConnected {
                target_id: "hive".into(),
                connection: old_client.connection(),
            },
            &rpc,
            None,
            None,
        );
        assert_eq!(app.targets[1].session_id.as_deref(), Some("new-session"));
        assert_eq!(app.targets[1].stream_binding.as_ref(), Some(&new));
        old_client.close().await;
        new_client.close().await;
        old_server.await.unwrap();
        new_server.await.unwrap();
    }

    #[tokio::test]
    async fn delayed_latest_discovery_cannot_override_a_newer_explicit_resume() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (listed_tx, listed_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (reader, writer) = stream.into_split();
            let writer = Arc::new(tokio::sync::Mutex::new(writer));
            let mut lines = BufReader::new(reader).lines();
            let mut listed = Some(listed_tx);
            let mut release = Some(release_rx);
            let mut delayed = None;
            let mut opened = Vec::new();
            while let Some(line) = lines.next_line().await.unwrap() {
                let request: Value = serde_json::from_str(&line).unwrap();
                if request["method"] == "session/list" {
                    listed.take().unwrap().send(()).unwrap();
                    let release = release.take().unwrap();
                    let writer = writer.clone();
                    delayed = Some(tokio::spawn(async move {
                        tokio::time::timeout(Duration::from_secs(5), release)
                            .await
                            .unwrap()
                            .unwrap();
                        let response = serde_json::json!({
                            "jsonrpc": "2.0", "id": request["id"],
                            "result": {"sessions": [{"session_id": "older-latest"}]},
                        });
                        writer
                            .lock()
                            .await
                            .write_all(format!("{response}\n").as_bytes())
                            .await
                            .unwrap();
                    }));
                    continue;
                }
                let result = match request["method"].as_str().unwrap() {
                    "initialize" => serde_json::json!({}),
                    "session/stream_open" => {
                        let session = request["params"]["session_id"].as_str().unwrap();
                        opened.push(session.to_string());
                        serde_json::json!({"stream_id": format!("stream-{session}")})
                    }
                    "session/stream_close" => serde_json::json!({"closed": true}),
                    method => panic!("unexpected request {method}"),
                };
                let response = serde_json::json!({
                    "jsonrpc": "2.0", "id": request["id"], "result": result,
                });
                writer
                    .lock()
                    .await
                    .write_all(format!("{response}\n").as_bytes())
                    .await
                    .unwrap();
            }
            if let Some(delayed) = delayed {
                delayed.await.unwrap();
            }
            opened
        });
        let (commands, command_rx) = mpsc::unbounded_channel();
        let (events, mut event_rx) = mpsc::channel(16);
        let worker = tokio::spawn(rpc_command_loop(command_rx, events));
        commands
            .send(RpcCommand::Connect {
                target_id: "hive".into(),
                rpc_addr: addr.to_string(),
            })
            .unwrap();
        let connected = tokio::time::timeout(Duration::from_secs(5), event_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(&connected, TuiEvent::TargetConnected { .. }));
        let mut app = app();
        let (ui_commands, _unused) = mpsc::unbounded_channel();
        process_event(&mut app, connected, &ui_commands, None, None);
        commands
            .send(RpcCommand::ResumeLatestOrCreate {
                target_id: "hive".into(),
            })
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), listed_rx)
            .await
            .unwrap()
            .unwrap();
        commands
            .send(RpcCommand::ResumeSession {
                target_id: "hive".into(),
                session_id: "newer-explicit".into(),
            })
            .unwrap();
        let bound = tokio::time::timeout(Duration::from_secs(5), event_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(&bound, TuiEvent::SessionBound { binding, .. }
            if binding.session_id == "newer-explicit"));
        process_event(&mut app, bound, &ui_commands, None, None);
        let history_count = app.targets[1].history.len();
        release_tx.send(()).unwrap();
        let obsolete = tokio::time::timeout(Duration::from_secs(5), event_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(&obsolete, TuiEvent::SessionBindingFailed { selection, .. }
            if !selection.is_current())
        );
        process_event(&mut app, obsolete, &ui_commands, None, None);
        assert_eq!(app.targets[1].session_id.as_deref(), Some("newer-explicit"));
        assert_eq!(app.targets[1].history.len(), history_count);
        commands
            .send(RpcCommand::Disconnect {
                target_id: "hive".into(),
            })
            .unwrap();
        drop(commands);
        worker.await.unwrap();
        assert_eq!(server.await.unwrap(), vec!["newer-explicit"]);
    }

    #[tokio::test]
    async fn late_connection_completion_cannot_replace_newer_connection() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let old_addr = listener.local_addr().unwrap().to_string();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let old_server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = stream.into_split();
            let mut reader = BufReader::new(reader);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["method"], "initialize");
            entered_tx.send(()).unwrap();
            tokio::time::timeout(Duration::from_secs(5), release_rx)
                .await
                .unwrap()
                .unwrap();
            let response = serde_json::json!({
                "jsonrpc": "2.0", "id": request["id"], "result": {},
            });
            writer
                .write_all(format!("{response}\n").as_bytes())
                .await
                .unwrap();
            line.clear();
            assert_eq!(
                reader.read_line(&mut line).await.unwrap(),
                0,
                "obsolete connection must be closed, not installed"
            );
        });
        let (new_addr, new_server) = stream_server_fixture().await;
        let (commands, command_rx) = mpsc::unbounded_channel();
        let (events, mut event_rx) = mpsc::channel(16);
        let worker = tokio::spawn(rpc_command_loop(command_rx, events));
        commands
            .send(RpcCommand::Connect {
                target_id: "hive".into(),
                rpc_addr: old_addr,
            })
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), entered_rx)
            .await
            .unwrap()
            .unwrap();
        commands
            .send(RpcCommand::Connect {
                target_id: "hive".into(),
                rpc_addr: new_addr,
            })
            .unwrap();
        let connected = tokio::time::timeout(Duration::from_secs(5), event_rx.recv())
            .await
            .unwrap()
            .unwrap();
        let TuiEvent::TargetConnected { connection, .. } = connected else {
            panic!("new connection should become ready first");
        };
        release_tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), old_server)
            .await
            .unwrap()
            .unwrap();
        assert!(event_rx.try_recv().is_err());
        commands
            .send(RpcCommand::ResumeSession {
                target_id: "hive".into(),
                session_id: "new-connection-session".into(),
            })
            .unwrap();
        let bound = tokio::time::timeout(Duration::from_secs(5), event_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(bound, TuiEvent::SessionBound { binding, .. }
            if binding.connection() == &connection && binding.is_current()));
        commands
            .send(RpcCommand::Disconnect {
                target_id: "hive".into(),
            })
            .unwrap();
        drop(commands);
        worker.await.unwrap();
        new_server.await.unwrap();
    }

    #[test]
    fn clear_uses_rendered_hive_and_preserves_other_targets() {
        let mut app = app();
        app.targets[0].push_line("other".into());
        app.targets[1].push_line("rendered".into());
        app.targets[1].streaming_text = "delta".into();
        app.hive_view.push_line("fallback".into());
        let (tx, _) = mpsc::unbounded_channel();
        handle_key(
            &mut app,
            KeyCode::Char('l'),
            KeyModifiers::CONTROL,
            &tx,
            None,
            None,
        );
        assert!(app.active_view().unwrap().history.is_empty());
        assert!(app.active_view().unwrap().streaming_text.is_empty());
        assert_eq!(app.targets[0].history.len(), 1);
        assert_eq!(app.hive_view.history.len(), 1);
        app.targets.pop();
        handle_key(
            &mut app,
            KeyCode::Char('l'),
            KeyModifiers::CONTROL,
            &tx,
            None,
            None,
        );
        assert!(app.hive_view.history.is_empty());
        app.mode = Mode::Direct;
        app.selected = 0;
        app.targets[0].streaming_text = "direct delta".into();
        app.hive_view.push_line("unselected fallback".into());
        handle_key(
            &mut app,
            KeyCode::Char('l'),
            KeyModifiers::CONTROL,
            &tx,
            None,
            None,
        );
        assert!(app.targets[0].history.is_empty());
        assert!(app.targets[0].streaming_text.is_empty());
        assert_eq!(app.hive_view.history.len(), 1);
    }

    #[test]
    fn discovery_and_release_do_not_send_claim_requests() {
        let mut app = app();
        let claims = ClaimRegistry::default();
        let epoch = register_broker(&claims, "broker");
        let (rpc, _) = mpsc::unbounded_channel();
        let (kennel, mut requests) = mpsc::unbounded_channel();
        let entry = TargetListEntry {
            target_id: "t".into(),
            name: "target".into(),
            state: mdm_tux::KennelTargetState::Available,
            lease_id: None,
            rpc_addr: Some("addr".into()),
        };
        for _ in 0..3 {
            apply_tkc_event(
                &claims,
                None,
                TkcEvent::SeenAvailableList {
                    targets: vec![entry.clone()],
                },
                |_| (),
            )
            .unwrap();
            process_event(
                &mut app,
                TuiEvent::kennel(&epoch, KennelUiEvent::AvailableTargets(vec![entry.clone()])),
                &rpc,
                Some(&claims),
                Some(&kennel),
            );
            assert!(requests.try_recv().is_err());
        }
        apply_tkc_event(
            &claims,
            Some(&kennel),
            TkcEvent::UserClaimTarget {
                target_id: "t".into(),
            },
            |_| (),
        )
        .unwrap();
        assert!(matches!(
            requests.try_recv().unwrap(),
            KennelClientCommand::ClaimTarget { .. }
        ));
        let grant = ClaimGrant {
            target_id: "t".into(),
            target_name: "target".into(),
            lease_id: "lease".into(),
            target_pubkey: "synthetic".into(),
            target_direct_addr: "inproc://synthetic".into(),
            rpc_addr: Some("addr".into()),
            expires_at_ms: 10_000,
        };
        apply_tkc_event(
            &claims,
            Some(&kennel),
            TkcEvent::ClaimGranted {
                claims: vec![grant.clone()],
                now_ms: 100,
            },
            |_| (),
        )
        .unwrap();
        assert!(matches!(
            requests.try_recv().unwrap(),
            KennelClientCommand::ClaimAck { .. }
        ));
        process_event(
            &mut app,
            TuiEvent::kennel(&epoch, KennelUiEvent::ClaimGranted(vec![grant])),
            &rpc,
            Some(&claims),
            Some(&kennel),
        );
        apply_tkc_event(
            &claims,
            Some(&kennel),
            TkcEvent::UserReleaseLease {
                lease_id: "lease".into(),
            },
            |_| (),
        )
        .unwrap();
        assert!(matches!(
            requests.try_recv().unwrap(),
            KennelClientCommand::ReleaseTarget { .. }
        ));
        let lease_ref = LeaseRef::Known {
            lease_id: "lease".into(),
        };
        apply_tkc_event(
            &claims,
            Some(&kennel),
            TkcEvent::ClaimReleased {
                target_id: "t".into(),
                lease_ref: lease_ref.clone(),
            },
            |_| (),
        )
        .unwrap();
        process_event(
            &mut app,
            TuiEvent::kennel(
                &epoch,
                KennelUiEvent::ClaimReleased {
                    target_id: "t".into(),
                    lease_ref,
                    reason: LeaseTerminationReason::ReleasedByTux,
                },
            ),
            &rpc,
            Some(&claims),
            Some(&kennel),
        );
        for _ in 0..2 {
            process_event(
                &mut app,
                TuiEvent::kennel(&epoch, KennelUiEvent::AvailableTargets(vec![entry.clone()])),
                &rpc,
                Some(&claims),
                Some(&kennel),
            );
            assert!(requests.try_recv().is_err());
        }
    }

    fn projection_snapshot(app: &App) -> Value {
        fn view(view: &TargetView) -> Value {
            serde_json::json!({
                "id": view.target_id, "name": view.name, "rpc": view.rpc_addr,
                "session": view.session_id, "history": view.history,
                "streaming": view.streaming_text,
                "phase": match view.phase {
                    TargetPhase::Disconnected => "disconnected",
                    TargetPhase::Idle => "idle",
                    TargetPhase::Running => "running",
                },
            })
        }
        let states: std::collections::BTreeMap<_, _> = app
            .target_states
            .iter()
            .map(|(id, state)| (id.clone(), *state == KennelUiState::ClaimedByMe))
            .collect();
        serde_json::json!({
            "targets": app.targets.iter().map(view).collect::<Vec<_>>(),
            "fallback": view(&app.hive_view), "planning": app.hive_planning,
            "states": states, "leases": app.target_leases,
        })
    }

    #[test]
    fn every_old_broker_projection_is_rejected_after_restart_or_reconnect() {
        for next_incarnation in ["first", "second"] {
            let entry = TargetListEntry {
                target_id: "t".into(),
                name: "obsolete-target".into(),
                state: mdm_tux::KennelTargetState::Claimed,
                lease_id: Some("lease".into()),
                rpc_addr: Some("obsolete-address".into()),
            };
            let grant = ClaimGrant {
                target_id: "t".into(),
                target_name: "obsolete-target".into(),
                lease_id: "lease".into(),
                target_pubkey: "synthetic".into(),
                target_direct_addr: "obsolete-direct".into(),
                rpc_addr: Some("obsolete-address".into()),
                expires_at_ms: 10_000,
            };
            let events = vec![
                KennelUiEvent::AvailableTargets(vec![entry.clone()]),
                KennelUiEvent::MineTargets(vec![entry]),
                KennelUiEvent::ClaimGranted(vec![grant]),
                KennelUiEvent::ClaimReleased {
                    target_id: "t".into(),
                    lease_ref: LeaseRef::Known {
                        lease_id: "lease".into(),
                    },
                    reason: LeaseTerminationReason::ReleasedByTux,
                },
                KennelUiEvent::HiveRegistered {
                    rpc_addr: "obsolete-hive".into(),
                    session_id: Some("obsolete-session".into()),
                },
                KennelUiEvent::Reconciled,
                KennelUiEvent::Error("obsolete error".into()),
                KennelUiEvent::HiveStreamEvent {
                    event: serde_json::json!({
                        "event": {"payload": {"type": "text_delta", "delta": "obsolete delta"}}
                    }),
                },
                KennelUiEvent::HiveComplete {
                    text: "obsolete completion".into(),
                },
                KennelUiEvent::HiveError {
                    message: "obsolete hive error".into(),
                },
            ];
            for event in events {
                let claims = ClaimRegistry::default();
                let old_epoch = register_broker(&claims, "first");
                let current_epoch = register_broker(&claims, next_incarnation);
                assert_ne!(old_epoch, current_epoch);
                let available = matches!(
                    &event,
                    KennelUiEvent::ClaimReleased { .. } | KennelUiEvent::Reconciled
                );
                claims.write().claims.insert(
                    "t".into(),
                    if available {
                        ClaimState::Available {
                            target_id: "t".into(),
                            target_name: "target".into(),
                        }
                    } else {
                        ClaimState::Claimed {
                            target_id: "t".into(),
                            target_name: "target".into(),
                            lease_id: "lease".into(),
                            rpc_addr: Some("current-target".into()),
                        }
                    },
                );
                let owner_before = claims.read().clone();
                let mut app = app();
                app.targets[0].rpc_addr = "current-target".into();
                app.targets[0].session_id = Some("current-target-session".into());
                app.targets[0].phase = TargetPhase::Running;
                app.targets[0].streaming_text = "current target delta".into();
                app.targets[1].rpc_addr = "current-hive".into();
                app.targets[1].session_id = Some("current-hive-session".into());
                app.target_states
                    .insert("t".into(), KennelUiState::ClaimedByMe);
                app.target_leases.insert("t".into(), "lease".into());
                app.hive_view.push_line("current fallback".into());
                app.hive_view.streaming_text = "current hive delta".into();
                app.hive_planning = true;
                let before = projection_snapshot(&app);
                let (rpc, mut commands) = mpsc::unbounded_channel();
                process_event(
                    &mut app,
                    TuiEvent::kennel(&old_epoch, event),
                    &rpc,
                    Some(&claims),
                    None,
                );
                assert_eq!(projection_snapshot(&app), before);
                assert_eq!(*claims.read(), owner_before);
                assert!(
                    commands.try_recv().is_err(),
                    "old epoch emitted an RPC command"
                );
            }
        }
    }

    #[test]
    fn same_epoch_release_cannot_disconnect_a_newer_claim_and_empty_mine_preserves_it() {
        let claims = ClaimRegistry::default();
        let epoch = register_broker(&claims, "broker");
        claims.write().claims.insert(
            "t".into(),
            ClaimState::Claimed {
                target_id: "t".into(),
                target_name: "target".into(),
                lease_id: "new-lease".into(),
                rpc_addr: Some("current-address".into()),
            },
        );
        let mut app = app();
        app.target_states
            .insert("t".into(), KennelUiState::ClaimedByMe);
        app.target_leases.insert("t".into(), "new-lease".into());
        let before = projection_snapshot(&app);
        let owner_before = claims.read().clone();
        let (rpc, mut commands) = mpsc::unbounded_channel();
        process_event(
            &mut app,
            TuiEvent::kennel(
                &epoch,
                KennelUiEvent::ClaimReleased {
                    target_id: "t".into(),
                    lease_ref: LeaseRef::Known {
                        lease_id: "old-lease".into(),
                    },
                    reason: LeaseTerminationReason::ReleasedByTux,
                },
            ),
            &rpc,
            Some(&claims),
            None,
        );
        process_event(
            &mut app,
            TuiEvent::kennel(&epoch, KennelUiEvent::MineTargets(vec![])),
            &rpc,
            Some(&claims),
            None,
        );
        assert_eq!(projection_snapshot(&app), before);
        assert_eq!(*claims.read(), owner_before);
        assert!(commands.try_recv().is_err());
        process_event(
            &mut app,
            TuiEvent::kennel(
                &epoch,
                KennelUiEvent::ClaimGranted(vec![ClaimGrant {
                    target_id: "t".into(),
                    target_name: "target".into(),
                    lease_id: "new-lease".into(),
                    target_pubkey: "synthetic".into(),
                    target_direct_addr: "inproc://target".into(),
                    rpc_addr: Some("current-address".into()),
                    expires_at_ms: 10_000,
                }]),
            ),
            &rpc,
            Some(&claims),
            None,
        );
        assert!(
            matches!(commands.try_recv().unwrap(), RpcCommand::Connect { target_id, rpc_addr }
            if target_id == "t" && rpc_addr == "current-address")
        );
    }

    #[test]
    fn delayed_ui_snapshots_cannot_resurrect_a_released_or_restarted_lease() {
        let mut app = app();
        let claims = ClaimRegistry::default();
        let old_epoch = register_broker(&claims, "first");
        let (rpc, mut commands) = mpsc::unbounded_channel();
        let entry = TargetListEntry {
            target_id: "t".into(),
            name: "target".into(),
            state: mdm_tux::KennelTargetState::Claimed,
            lease_id: Some("obsolete".into()),
            rpc_addr: Some("old-address".into()),
        };
        apply_tkc_event(
            &claims,
            None,
            TkcEvent::SeenMineList {
                targets: vec![entry.clone()],
            },
            |_| (),
        )
        .unwrap();
        apply_tkc_event(
            &claims,
            None,
            TkcEvent::ClaimReleased {
                target_id: "t".into(),
                lease_ref: LeaseRef::Known {
                    lease_id: "obsolete".into(),
                },
            },
            |_| (),
        )
        .unwrap();
        let expected = claims.read().clone();
        process_event(
            &mut app,
            TuiEvent::kennel(&old_epoch, KennelUiEvent::MineTargets(vec![entry.clone()])),
            &rpc,
            Some(&claims),
            None,
        );
        assert_eq!(*claims.read(), expected);
        assert!(commands.try_recv().is_err());
        apply_tkc_event(
            &claims,
            None,
            TkcEvent::BrokerRegistered {
                epoch: BrokerEpoch::new("first"),
            },
            |_| (),
        )
        .unwrap();
        apply_tkc_event(
            &claims,
            None,
            TkcEvent::SeenMineList {
                targets: vec![entry.clone()],
            },
            |_| (),
        )
        .unwrap();
        apply_tkc_event(
            &claims,
            None,
            TkcEvent::BrokerRegistered {
                epoch: BrokerEpoch::new("second"),
            },
            |_| (),
        )
        .unwrap();
        let expected = claims.read().clone();
        process_event(
            &mut app,
            TuiEvent::kennel(&old_epoch, KennelUiEvent::MineTargets(vec![entry])),
            &rpc,
            Some(&claims),
            None,
        );
        assert_eq!(*claims.read(), expected);
        assert!(commands.try_recv().is_err());
    }
}
