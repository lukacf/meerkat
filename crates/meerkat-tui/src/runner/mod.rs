//! The runner: joins the live session and the terminal UI.
//!
//! The UI loop ([`crate::ui::run_ui`]) owns the terminal and emits
//! [`Effect`]s; the runner turns them into session and runtime calls and
//! feeds the session's events, steer acknowledgements, turn outcomes and
//! file diffs back as [`Msg`]s. Nothing here touches the terminal.
//!
//! A *turn*, as the UI sees it, is everything the runtime does for the
//! inputs the runner admitted while the UI was busy: the started prompt
//! (`Queue`), messages steered into it (`Steer`), and the follow-up runs the
//! runtime adds for steers that found no boundary. The turn ends when every
//! one of those inputs has reached its terminal and the session's last run
//! has published its terminal event; [`Msg::TurnFinished`] then carries the
//! result and the steered messages the runtime never delivered.
//!
//! The approval gate's side lives in [`approval`]: escalation dialogs,
//! `/rules`, `/auto`, decision notices and classifier usage.
//!
//! `/model` and the picker live in [`models`], the price table and the cost
//! sidecar in [`cost`].
//!
//! Helpers ([`crate::mobs`]): their scoped events go to the UI as
//! [`Msg::Scoped`] (nested blocks, per-helper usage) and the running set as
//! [`StatusUpdate::Helpers`]; an interrupt cancels the session's helpers
//! (see [`SessionHandle::interrupt`]).
//!
//! Effects of later phases are answered honestly instead of being stubbed:
//! `/remember` (P6) and manual `/compact` (the runtime compacts on its own; a
//! manual trigger needs an upstream primitive).

mod approval;
mod cost;
mod lifecycle;
mod models;
mod quit;
#[doc(hidden)]
pub mod testing;

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures::{FutureExt, StreamExt};
use meerkat::ToolGatewayBuilder;
use meerkat_core::comms::EventStream;
use meerkat_core::{AgentEvent, AgentToolDispatcher, EventEnvelope, InputId, RunResult, SessionId};
use meerkat_runtime::completion::{CompletionOutcome, CompletionWaitError};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::mpsc;

use crate::approval::ApprovalEngine;
use crate::bootstrap::{self, BootstrapOptions, ModelChoice, Runtime};
use crate::cli::Args;
use crate::delivery::{PendingId, SteerFailure};
use crate::error::TuiError;
use crate::memory::{MemoryToolDispatcher, NoteStore, Scope, prompt_index};
use crate::mobs::HelperUpdate;
use crate::session::{Admission, SessionHandle, SessionOptions};
use crate::tools::{FileDiff, FileToolDispatcher};
use crate::ui::{
    App, CompactFailure, Effect, History, Msg, NoticeLevel, StatusInfo, StatusUpdate, TurnOutcome,
    TurnSummary, UiChannels, run_ui,
};

pub use lifecycle::{Start, blocks_from_session, open_start, session_endpoint, session_messages};
pub use models::DEFAULT_CONTEXT_WINDOW;
pub use quit::SavedUnsent;

/// Capacity of the primary agent-event channel.
const AGENT_EVENT_CAPACITY: usize = 256;

/// How long a finished turn waits for its last run's terminal event before
/// reporting anyway (a stopped run may publish none).
const RUN_EVENT_GRACE: Duration = Duration::from_secs(1);

/// Run the TUI end to end.
///
/// # Errors
///
/// [`TuiError::NotATerminal`] without a terminal (checked first, so a
/// script fails fast), otherwise when bootstrap fails, the session cannot be
/// opened, or the terminal cannot be driven.
pub async fn run(args: Args) -> Result<(), TuiError> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(TuiError::NotATerminal);
    }
    let (scope, config) = bootstrap::prepare(&args).await?;
    init_logging(&scope.realm_root())?;
    let runtime =
        bootstrap::bootstrap_with(scope, config, &args, BootstrapOptions::default()).await?;
    let (template, diff_rx) = Template::new(&runtime)?;
    let start = Start::from_args(&args);
    let (session, transcript) = open_start(&runtime, template.options(None), &start).await?;

    let mut status = StatusInfo::default();
    status.set_model(&runtime.choice.endpoint, session.model());
    status.set_session(&session.id().to_string());
    let realm_root = runtime.scope.realm_root();
    let history = History::load(realm_root.join("tui_history"));
    let global_history = runtime
        .scope
        .global
        .as_ref()
        .map(|global| History::load(global.history_file()));
    let mut app = App::new(status)
        .with_history(history.entries().to_vec())
        .with_global_history(
            global_history
                .as_ref()
                .map(|h| h.entries().to_vec())
                .unwrap_or_default(),
        );
    let histories: Vec<History> = std::iter::once(history).chain(global_history).collect();

    let (agent_tx, agent_rx) = mpsc::channel(AGENT_EVENT_CAPACITY);
    let (_scoped_tx, scoped_rx) = mpsc::channel(AGENT_EVENT_CAPACITY);
    let (ui_tx, ui_rx) = mpsc::unbounded_channel::<Msg>();
    let (effect_tx, effect_rx) = mpsc::unbounded_channel::<Effect>();
    if let Some(text) = quit::unsent_notice(&realm_root) {
        let _ = ui_tx.send(Msg::Notice {
            level: NoticeLevel::Warn,
            text,
        });
    }
    // The prompt argument is typed into the editor and submitted, so the UI
    // shows it and knows a turn runs.
    if let Some(prompt) = args.prompt.clone() {
        let mut first = app.update(Msg::Paste(prompt));
        first.extend(app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))));
        for effect in first {
            let _ = effect_tx.send(effect);
        }
    }

    let runner = Runner::new(RunnerParts {
        runtime,
        template,
        session,
        transcript,
        agent_tx,
        ui_tx,
        effect_rx,
        diff_rx,
    })
    .await;
    let runner_task = tokio::spawn(runner.run());
    let ui_result = run_ui(
        app,
        UiChannels {
            agent_rx,
            scoped_rx,
            ui_rx,
        },
        effect_tx,
        args.mouse_capture(),
        histories,
    )
    .await;
    // The UI has gone (and closed the effect channel): the runner interrupts
    // the turn, saves unsent messages and takes the session off the runtime.
    match tokio::time::timeout(quit::SHUTDOWN_TIMEOUT, runner_task).await {
        Ok(Ok(Some(saved))) => eprintln!(
            "rkat-tui: {} unsent message(s) saved in {}",
            saved.count,
            saved.path.display()
        ),
        Ok(Ok(None)) => {}
        Ok(Err(err)) => tracing::warn!(error = %err, "runner task failed"),
        Err(_) => tracing::warn!("runner did not shut down in time"),
    }
    ui_result.map_err(TuiError::from)
}

/// Route tracing to a log file inside the realm so nothing writes to the
/// terminal while the UI owns it.
fn init_logging(realm_root: &Path) -> Result<(), TuiError> {
    let log_dir = realm_root.join("logs");
    std::fs::create_dir_all(&log_dir).map_err(|e| TuiError::io("create log directory", e))?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_dir.join("rkat-tui.log"))
        .map_err(|e| TuiError::io("open log file", e))?;
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    // A second init (tests) is not an error worth failing the run for.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_writer(std::sync::Mutex::new(file))
        .try_init();
    Ok(())
}

/// Directory of the workspace's memory notes.
fn workspace_notes_dir(workspace: &Path) -> PathBuf {
    workspace.join(".rkat").join("tui").join("notes")
}

/// What every session build shares: the external tools (file and note
/// tools) and where the notes index comes from.
pub(crate) struct Template {
    external_tools: Arc<dyn AgentToolDispatcher>,
    notes: Arc<MemoryToolDispatcher>,
    approval: Arc<ApprovalEngine>,
}

impl Template {
    /// The file tools (diffs go to the returned receiver) and the notes,
    /// behind one gateway.
    pub(crate) fn new(
        runtime: &Runtime,
    ) -> Result<(Self, mpsc::UnboundedReceiver<FileDiff>), TuiError> {
        let (diff_tx, diff_rx) = mpsc::unbounded_channel();
        let file_tools = Arc::new(FileToolDispatcher::new(
            runtime.scope.project_root.clone(),
            Some(diff_tx),
        ));
        let notes = Arc::new(MemoryToolDispatcher::new(
            runtime
                .scope
                .global
                .as_ref()
                .map(|global| NoteStore::open(global.notes_dir(), Scope::User)),
            NoteStore::open(
                workspace_notes_dir(&runtime.scope.context_root),
                Scope::Workspace,
            ),
        ));
        let gateway = ToolGatewayBuilder::new()
            .add_dispatcher(file_tools as Arc<dyn AgentToolDispatcher>)
            .add_dispatcher(Arc::clone(&notes) as Arc<dyn AgentToolDispatcher>)
            .build()
            .map_err(|e| TuiError::Mcp(format!("compose tool dispatchers: {e}")))?;
        Ok((
            Self {
                external_tools: Arc::new(gateway),
                notes,
                approval: Arc::clone(&runtime.approval.engine),
            },
            diff_rx,
        ))
    }

    /// Options for a session build on `choice` (`None`: the start-up
    /// choice). The notes index is re-read every time, so `/new` and
    /// `/resume` pick up notes saved meanwhile.
    pub(crate) fn options(&self, choice: Option<ModelChoice>) -> SessionOptions {
        SessionOptions {
            external_tools: Some(Arc::clone(&self.external_tools)),
            additional_instructions: prompt_index(self.notes.user(), self.notes.workspace())
                .into_iter()
                .collect(),
            choice,
            approval: Some(Arc::clone(&self.approval)),
        }
    }
}

/// What a runtime input of the current turn is.
#[derive(Debug)]
enum InputKind {
    /// The prompt that started the turn (or a prompt queued behind it).
    Prompt,
    /// A message steered into the turn.
    Steer {
        id: PendingId,
        text: String,
        /// Shown to the UI as written into the conversation.
        delivered: bool,
    },
}

/// One input the runner admitted for the current turn.
#[derive(Debug)]
struct Tracked {
    input_id: InputId,
    kind: InputKind,
    done: bool,
}

/// The UI's current turn: its inputs, in admission order, and what they
/// came to.
#[derive(Debug, Default)]
struct Turn {
    inputs: Vec<Tracked>,
    /// The last run result, or the first failure.
    result: Option<Result<TurnSummary, String>>,
    /// The user interrupted it.
    interrupted: bool,
    /// Steered messages the runtime ended without delivering.
    undelivered: Vec<PendingId>,
    /// Runs started minus runs that published their terminal event.
    open_runs: usize,
    /// Every input is done; the turn ends when the open runs close or this
    /// passes.
    finish_by: Option<tokio::time::Instant>,
}

impl Turn {
    fn is_active(&self) -> bool {
        !self.inputs.is_empty()
    }

    fn all_done(&self) -> bool {
        self.inputs.iter().all(|input| input.done)
    }

    fn outstanding(&self) -> Vec<InputId> {
        self.inputs
            .iter()
            .filter(|input| !input.done)
            .map(|input| input.input_id.clone())
            .collect()
    }

    fn record(&mut self, result: Result<TurnSummary, String>) {
        match (&self.result, &result) {
            (Some(Err(_)), _) => {}
            _ => self.result = Some(result),
        }
    }
}

/// A completion waiter's report.
struct Completed {
    session: SessionId,
    input_id: InputId,
    outcome: Result<CompletionOutcome, CompletionWaitError>,
}

/// What an input's terminal means for the turn.
enum Settled {
    Done(Option<Box<RunResult>>),
    Interrupted,
    Failed(String),
}

fn settle(outcome: Result<CompletionOutcome, CompletionWaitError>) -> Settled {
    match outcome {
        Ok(CompletionOutcome::Completed(result)) => Settled::Done(Some(result)),
        Ok(CompletionOutcome::CompletedWithoutResult) => Settled::Done(None),
        Ok(CompletionOutcome::Cancelled) => Settled::Interrupted,
        Ok(
            CompletionOutcome::Abandoned { reason, .. }
            | CompletionOutcome::AbandonedWithError { reason, .. }
            | CompletionOutcome::RuntimeTerminated { reason, .. },
        ) => Settled::Failed(reason),
        Ok(CompletionOutcome::CompletedWithFinalizationFailure { error }) => Settled::Failed(
            error
                .detail
                .unwrap_or_else(|| "the turn could not be committed".to_string()),
        ),
        Ok(
            CompletionOutcome::CallbackPending { .. }
            | CompletionOutcome::CallbackBatchPending { .. },
        ) => Settled::Failed(
            "the turn waits for an external tool result rkat-tui does not provide".to_string(),
        ),
        Ok(CompletionOutcome::OutcomeUnknown { run_id }) => {
            Settled::Failed(format!("the outcome of run {run_id} is unknown"))
        }
        Err(err) => Settled::Failed(err.to_string()),
    }
}

fn summary_of(result: &RunResult) -> TurnSummary {
    let usage = result.run_usage.as_ref().unwrap_or(&result.usage);
    TurnSummary {
        text: result.text.clone(),
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
    }
}

/// The already-built pieces a [`Runner`] drives.
pub(crate) struct RunnerParts {
    pub(crate) runtime: Runtime,
    pub(crate) template: Template,
    pub(crate) session: SessionHandle,
    /// The start session's committed transcript when it was resumed.
    pub(crate) transcript: Option<meerkat_core::Session>,
    pub(crate) agent_tx: mpsc::Sender<EventEnvelope<AgentEvent>>,
    pub(crate) ui_tx: mpsc::UnboundedSender<Msg>,
    pub(crate) effect_rx: mpsc::UnboundedReceiver<Effect>,
    pub(crate) diff_rx: mpsc::UnboundedReceiver<FileDiff>,
}

/// The event loop joining the session and the UI.
pub(crate) struct Runner {
    runtime: Runtime,
    template: Template,
    session: SessionHandle,
    events: Option<EventStream>,
    agent_tx: mpsc::Sender<EventEnvelope<AgentEvent>>,
    ui_tx: mpsc::UnboundedSender<Msg>,
    effect_rx: mpsc::UnboundedReceiver<Effect>,
    diff_rx: mpsc::UnboundedReceiver<FileDiff>,
    completion_tx: mpsc::UnboundedSender<Completed>,
    completion_rx: mpsc::UnboundedReceiver<Completed>,
    turn: Turn,
    /// Steered messages delivered in finished turns (quitting never saves
    /// them as unsent).
    delivered: Vec<PendingId>,
    /// Messages the UI still held when the user quit ([`Effect::SaveUnsent`]).
    held_unsent: Vec<(Option<PendingId>, String)>,
    /// The approval engine's channels and open escalations.
    approval: approval::ApprovalDesk,
    /// Endpoint of the current session's model.
    current_endpoint: String,
    /// Model lists of every endpoint, once fetched.
    models: Option<crate::models::ModelCache>,
    /// A model-list fetch running in the background.
    model_fetch: Option<models::ModelFetch>,
    /// The picker opens when the running fetch finishes.
    picker_waiting: bool,
    /// A `/model <id>` waiting for the lists.
    pending_set: Option<String>,
    /// Prices for the cost readout.
    prices: Arc<crate::pricing::PriceTable>,
    /// A community price-list refresh running in the background.
    price_fetch: Option<cost::PriceFetch>,
    /// Helper events and the running set.
    helper_updates: Option<mpsc::UnboundedReceiver<HelperUpdate>>,
}

impl Runner {
    pub(crate) async fn new(parts: RunnerParts) -> Self {
        let RunnerParts {
            mut runtime,
            template,
            session,
            transcript,
            agent_tx,
            ui_tx,
            effect_rx,
            diff_rx,
        } = parts;
        let (completion_tx, completion_rx) = mpsc::unbounded_channel();
        let desk = approval::ApprovalDesk::new(
            Arc::clone(&runtime.approval.engine),
            runtime.approval.escalations.take(),
            runtime.approval.events.take(),
        );
        let notices = std::mem::take(&mut runtime.approval.notices);
        let models = runtime.models.take();
        let prices = Arc::new(runtime.pricing.table.clone());
        let price_fetch = runtime.pricing.refresh_url().map(|url| {
            cost::spawn_price_fetch(url.to_string(), runtime.pricing.cache_path.clone())
        });
        let current_endpoint = runtime.choice.endpoint.clone();
        let helper_updates = runtime.helpers.as_ref().and_then(|h| h.take_updates());
        let mut runner = Self {
            runtime,
            template,
            session,
            events: None,
            agent_tx,
            ui_tx,
            effect_rx,
            diff_rx,
            completion_tx,
            completion_rx,
            turn: Turn::default(),
            delivered: Vec::new(),
            held_unsent: Vec::new(),
            approval: desk,
            current_endpoint,
            models,
            model_fetch: None,
            picker_waiting: false,
            pending_set: None,
            prices,
            price_fetch,
            helper_updates,
        };
        runner.push_auto_status();
        for notice in runner.runtime.confinement.notices.clone() {
            runner.notice(NoticeLevel::Info, notice);
        }
        for notice in notices {
            runner.notice(NoticeLevel::Warn, notice);
        }
        runner.push_prices();
        runner.show_session(transcript.as_ref(), false);
        // Fetch the lists in the background, for the picker, bare-id
        // lookups, the gauge and endpoint prices.
        if runner.models.is_none() && runner.runtime.endpoints.iter().any(|e| e.has_key()) {
            runner.ensure_fetch(false);
        }
        runner.follow_events().await;
        runner
    }

    /// Serve effects until the UI quits (or drops the effect channel), then
    /// shut down (see [`Runner::shutdown`]).
    pub(crate) async fn run(mut self) -> Option<SavedUnsent> {
        loop {
            tokio::select! {
                effect = self.effect_rx.recv() => match effect {
                    None | Some(Effect::Quit) => break,
                    Some(effect) => self.handle_effect(effect).await,
                },
                event = next_event(&mut self.events) => match event {
                    Some(env) => self.on_event(env).await,
                    None => {
                        tracing::warn!(session = %self.session.id(), "the session's event stream ended");
                        self.events = None;
                    }
                },
                Some(done) = self.completion_rx.recv() => self.on_completed(done).await,
                Some(request) = recv(&mut self.approval.escalations) => self.on_escalation(request),
                Some(event) = recv_unbounded(&mut self.approval.events) => self.on_approval_event(event),
                Some(update) = recv_unbounded(&mut self.helper_updates) => self.on_helper_update(update),
                joined = join(&mut self.model_fetch) => self.model_fetch_finished(joined).await,
                joined = join(&mut self.price_fetch) => self.price_fetch_finished(joined),
                Some(diff) = self.diff_rx.recv() => self.send(Msg::Diff {
                    tool_use_id: diff.tool_use_id,
                    path: diff.path.display().to_string(),
                    unified: diff.unified,
                    added: diff.added,
                    removed: diff.removed,
                }),
                () = sleep_until(self.turn.finish_by) => self.finish_turn().await,
            }
        }
        self.shutdown().await
    }

    /// Follow the current session's events.
    async fn follow_events(&mut self) {
        self.events = match self.session.events().await {
            Ok(stream) => Some(stream),
            Err(err) => {
                tracing::warn!(error = %err, "cannot follow the session's events");
                self.notice(
                    NoticeLevel::Error,
                    format!("live output unavailable: {err}"),
                );
                None
            }
        };
    }

    fn send(&self, msg: Msg) {
        let _ = self.ui_tx.send(msg);
    }

    /// Forward a helper's event, or the running set, to the UI.
    fn on_helper_update(&self, update: HelperUpdate) {
        self.send(match update {
            HelperUpdate::Event(scoped) => Msg::Scoped(scoped),
            HelperUpdate::Live(helpers) => Msg::Status(StatusUpdate::Helpers(helpers)),
        });
    }

    fn notice(&self, level: NoticeLevel, text: impl Into<String>) {
        self.send(Msg::Notice {
            level,
            text: text.into(),
        });
    }

    async fn handle_effect(&mut self, effect: Effect) {
        match effect {
            Effect::StartTurn(text) => self.start_turn(&text).await,
            Effect::Steer { id, text } => self.steer(id, text).await,
            // The UI keeps the text of `InterruptAndSend` and starts the next
            // turn itself once `TurnFinished` arrives.
            Effect::Interrupt | Effect::InterruptAndSend(_) => self.interrupt(),
            Effect::Quit => {}
            Effect::SaveUnsent(held) => self.held_unsent.extend(held),
            Effect::NewSession => self.new_session().await,
            Effect::ResumeSession(reference) => self.resume_session(&reference).await,
            Effect::RequestResumePicker => self.show_resume_picker().await,
            effect @ (Effect::RespondEscalation(..)
            | Effect::AddRule { .. }
            | Effect::RemoveRule(_)
            | Effect::ListRules
            | Effect::SetAutoMode(_)
            | Effect::ResumeAutoMode) => self.handle_approval_effect(effect),
            Effect::SetModel(reference) => self.set_model(reference).await,
            Effect::RequestModelPicker { refresh } => self.request_model_picker(refresh),
            Effect::Remember(_) => self.notice(
                NoticeLevel::Warn,
                "/remember is not available in this build yet; edit AGENTS.md instead",
            ),
            Effect::Compact => self.send(Msg::CompactFinished(Err(CompactFailure::Other(
                "manual compaction is not available: the runtime compacts the context automatically"
                    .to_string(),
            )))),
            Effect::CostSnapshot(snapshot) => self.save_cost_snapshot(&snapshot),
        }
    }

    /// [`Effect::StartTurn`]: admit a `Queue` input.
    async fn start_turn(&mut self, text: &str) {
        match self.session.start_turn(text).await {
            Ok(admission) => self.track(admission, InputKind::Prompt),
            Err(err) if self.turn.is_active() => {
                self.notice(
                    NoticeLevel::Error,
                    format!("could not queue the prompt: {err}"),
                );
            }
            Err(err) => self.send(Msg::TurnFinished(TurnOutcome::plain(Err(err.to_string())))),
        }
    }

    /// [`Effect::Steer`]: admit a durable `Steer` input into the running
    /// turn. Without a running turn the UI sends it as a new turn instead.
    async fn steer(&mut self, id: PendingId, text: String) {
        if !self.turn.is_active() {
            self.send(Msg::SteerAck {
                id,
                result: Err(SteerFailure::NotRunning),
            });
            return;
        }
        match self.session.steer(&text).await {
            Ok(admission) => {
                self.track(
                    admission,
                    InputKind::Steer {
                        id,
                        text,
                        delivered: false,
                    },
                );
                self.send(Msg::SteerAck { id, result: Ok(()) });
            }
            Err(err) => self.send(Msg::SteerAck {
                id,
                result: Err(SteerFailure::Rejected(err.to_string())),
            }),
        }
    }

    /// Add an admitted input to the turn and wait for its terminal.
    fn track(&mut self, admission: Admission, kind: InputKind) {
        let Admission {
            input_id,
            completion,
        } = admission;
        self.turn.finish_by = None;
        self.turn.inputs.push(Tracked {
            input_id: input_id.clone(),
            kind,
            done: false,
        });
        let tx = self.completion_tx.clone();
        let session = self.session.id().clone();
        match completion {
            Some(handle) => {
                tokio::spawn(async move {
                    let outcome = handle.wait().await;
                    let _ = tx.send(Completed {
                        session,
                        input_id,
                        outcome,
                    });
                });
            }
            // Terminal during admission (a duplicate of a finished input).
            None => {
                let _ = tx.send(Completed {
                    session,
                    input_id,
                    outcome: Ok(CompletionOutcome::CompletedWithoutResult),
                });
            }
        }
    }

    /// Interrupt the turn without blocking the loop: the completions of the
    /// stopped and cancelled inputs end it.
    fn interrupt(&mut self) {
        let session_id = self.session.id().clone();
        self.stop_approvals(&session_id);
        if !self.turn.is_active() {
            return;
        }
        self.turn.interrupted = true;
        let session = self.session.clone();
        let outstanding = self.turn.outstanding();
        let ui_tx = self.ui_tx.clone();
        tokio::spawn(async move {
            if let Err(err) = session.interrupt(&outstanding).await {
                tracing::warn!(error = %err, "interrupt failed");
                let _ = ui_tx.send(Msg::Notice {
                    level: NoticeLevel::Error,
                    text: format!("interrupt failed: {err}"),
                });
            }
        });
    }

    /// Forward a session event to the UI, after mapping what it means for
    /// steered messages.
    async fn on_event(&mut self, env: EventEnvelope<AgentEvent>) {
        let msgs = self.observe(&env.payload);
        if self.agent_tx.send(env).await.is_err() {
            tracing::debug!("UI event receiver closed");
        }
        for msg in msgs {
            self.send(msg);
        }
        if self.turn.finish_by.is_some() && self.turn.open_runs == 0 {
            self.finish_turn().await;
        }
    }

    /// Steer acknowledgements an event implies, and run bookkeeping.
    fn observe(&mut self, event: &AgentEvent) -> Vec<Msg> {
        let mut msgs = Vec::new();
        match event {
            AgentEvent::RunStarted { input, .. } => {
                self.turn.open_runs += 1;
                self.approval.engine.on_run_started(self.session.id());
                // A follow-up run the runtime started for a steer that found
                // no boundary: its prompt is the steered text.
                if let Some(prompt) = input.prompt_text() {
                    msgs.extend(self.mark_delivered(|text| text.trim() == prompt.trim()));
                }
            }
            AgentEvent::RunCompleted { .. } | AgentEvent::RunFailed { .. } => {
                self.turn.open_runs = self.turn.open_runs.saturating_sub(1);
            }
            AgentEvent::BoundaryAppendApplied { input_id, .. } => {
                if let Some(id) = self.set_delivered(input_id, true) {
                    msgs.push(Msg::SteerDelivered(id));
                }
            }
            AgentEvent::BoundaryAppendsDiscarded(discarded) => {
                for input_id in &discarded.input_ids {
                    if let Some(id) = self.set_delivered(input_id, false) {
                        msgs.push(Msg::SteerRequeued(id));
                    }
                }
            }
            _ => {}
        }
        msgs
    }

    /// Mark the first undelivered steer whose text matches as delivered.
    fn mark_delivered(&mut self, matches: impl Fn(&str) -> bool) -> Option<Msg> {
        self.turn
            .inputs
            .iter_mut()
            .find_map(|input| match &mut input.kind {
                InputKind::Steer {
                    id,
                    text,
                    delivered,
                } if !*delivered && !input.done && matches(text) => {
                    *delivered = true;
                    Some(Msg::SteerDelivered(*id))
                }
                _ => None,
            })
    }

    /// Flip a steer's delivered flag; returns its id when it changed.
    fn set_delivered(&mut self, input_id: &InputId, value: bool) -> Option<PendingId> {
        self.turn
            .inputs
            .iter_mut()
            .find_map(|input| match &mut input.kind {
                InputKind::Steer { id, delivered, .. }
                    if &input.input_id == input_id && *delivered != value =>
                {
                    *delivered = value;
                    Some(*id)
                }
                _ => None,
            })
    }

    async fn on_completed(&mut self, done: Completed) {
        if &done.session != self.session.id() {
            return;
        }
        let interrupted = self.turn.interrupted;
        let Some(input) = self
            .turn
            .inputs
            .iter_mut()
            .find(|input| input.input_id == done.input_id && !input.done)
        else {
            return;
        };
        input.done = true;
        let settled = match settle(done.outcome) {
            Settled::Failed(_) if interrupted => Settled::Interrupted,
            other => other,
        };
        let mut msgs = Vec::new();
        let mut undelivered = None;
        let result = match (&mut input.kind, settled) {
            (InputKind::Prompt, Settled::Done(result)) => {
                Some(Ok(result.as_deref().map(summary_of).unwrap_or_default()))
            }
            (InputKind::Steer { id, delivered, .. }, Settled::Done(result)) => {
                // Consumed without a boundary event: a follow-up run
                // delivered it.
                if !*delivered {
                    *delivered = true;
                    msgs.push(Msg::SteerDelivered(*id));
                }
                result.as_deref().map(|r| Ok(summary_of(r)))
            }
            (InputKind::Prompt, Settled::Interrupted) => Some(Err("interrupted".to_string())),
            (InputKind::Prompt, Settled::Failed(reason)) => Some(Err(reason)),
            (InputKind::Steer { id, .. }, Settled::Interrupted) => {
                undelivered = Some(*id);
                None
            }
            (InputKind::Steer { id, .. }, Settled::Failed(reason)) => {
                undelivered = Some(*id);
                Some(Err(reason))
            }
        };
        self.turn.undelivered.extend(undelivered);
        if let Some(result) = result {
            self.turn.record(result);
        }
        for msg in msgs {
            self.send(msg);
        }
        if self.turn.all_done() {
            if self.turn.open_runs == 0 {
                self.finish_turn().await;
            } else {
                self.turn.finish_by = Some(tokio::time::Instant::now() + RUN_EVENT_GRACE);
            }
        }
    }

    /// Report the finished turn. Events already published are forwarded
    /// first, so the turn's text never lands after its summary.
    async fn finish_turn(&mut self) {
        while let Some(next) = self
            .events
            .as_mut()
            .map(|stream| stream.next().now_or_never())
        {
            match next {
                Some(Some(env)) => {
                    let msgs = self.observe(&env.payload);
                    let _ = self.agent_tx.send(env).await;
                    for msg in msgs {
                        self.send(msg);
                    }
                }
                Some(None) => {
                    self.events = None;
                    break;
                }
                None => break,
            }
        }
        let turn = std::mem::take(&mut self.turn);
        if turn.inputs.is_empty() {
            return;
        }
        for input in &turn.inputs {
            if let InputKind::Steer {
                id,
                delivered: true,
                ..
            } = input.kind
                && !turn.undelivered.contains(&id)
            {
                self.delivered.push(id);
            }
        }
        let result = if turn.interrupted {
            Err("interrupted".to_string())
        } else {
            turn.result.unwrap_or_else(|| Ok(TurnSummary::default()))
        };
        // Steered messages the model never received are not the user's
        // words to it: they leave the classifier transcript again.
        for input in &turn.inputs {
            if let InputKind::Steer { id, text, .. } = &input.kind
                && turn.undelivered.contains(id)
            {
                self.approval.engine.forget_prompt(self.session.id(), text);
            }
        }
        let mut undelivered = turn.undelivered;
        undelivered.sort();
        self.send(Msg::TurnFinished(TurnOutcome {
            result,
            undelivered,
        }));
    }
}

/// The next event of `events`, or never when there is no stream.
async fn next_event(events: &mut Option<EventStream>) -> Option<EventEnvelope<AgentEvent>> {
    match events {
        Some(stream) => stream.next().await,
        None => std::future::pending().await,
    }
}

/// The result of an optional background task, or never.
async fn join<T>(
    task: &mut Option<tokio::task::JoinHandle<T>>,
) -> Result<T, tokio::task::JoinError> {
    match task {
        Some(handle) => handle.await,
        None => std::future::pending().await,
    }
}

/// The next item of an optional channel, or never.
async fn recv<T>(rx: &mut Option<mpsc::Receiver<T>>) -> Option<T> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

/// The next item of an optional unbounded channel, or never.
async fn recv_unbounded<T>(rx: &mut Option<mpsc::UnboundedReceiver<T>>) -> Option<T> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

/// Sleep until `deadline`, or never.
async fn sleep_until(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_is_not_overwritten_by_a_later_success() {
        let mut turn = Turn::default();
        turn.record(Ok(TurnSummary::default()));
        turn.record(Err("boom".into()));
        turn.record(Ok(TurnSummary {
            text: "later".into(),
            ..TurnSummary::default()
        }));
        assert_eq!(turn.result, Some(Err("boom".into())));
    }

    #[test]
    fn terminals_settle() {
        assert!(matches!(
            settle(Ok(CompletionOutcome::Cancelled)),
            Settled::Interrupted
        ));
        assert!(matches!(
            settle(Ok(CompletionOutcome::CompletedWithoutResult)),
            Settled::Done(None)
        ));
        assert!(matches!(
            settle(Err(CompletionWaitError::ChannelClosed)),
            Settled::Failed(_)
        ));
    }
}
