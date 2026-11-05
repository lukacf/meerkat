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
//! runtime adds for steers that found no boundary. Its end is
//! completion-driven: the turn ends when every one of those inputs has
//! reached its terminal (each input's completion handle, awaited by a task
//! in the runner's [`JoinSet`]). Run events are tracked by run id only so a
//! finished turn's last text is forwarded before its summary: once every
//! input is terminal the runner waits briefly for the runs it saw start to
//! publish their terminal events (a run that starts over under the same id
//! counts once). [`Msg::TurnFinished`] then carries the typed end
//! ([`TurnFailure`] or the summary) and the steered messages the runtime
//! never delivered. An input ends *interrupted* when its completion is
//! `Cancelled` or the runtime reports this interrupt cancelled it (the stop
//! receipt's committed `Abandoned { reason: Cancelled }`, or a cancel that
//! found it pending); after an interrupt the turn waits for that report.
//!
//! A steered message is delivered when the runtime says so in typed terms:
//! `BoundaryAppendApplied` for its input id, or its input's completion
//! (consumed by a follow-up run). Text is never matched.
//!
//! The approval gate's side lives in [`approval`]: escalation dialogs,
//! `/rules`, `/auto`, decision notices and classifier usage.
//!
//! `/model` and the picker live in [`models`], the price table and the cost
//! record in [`cost`] and [`cost_store`].
//!
//! Helpers ([`crate::mobs`]): their scoped events go to the UI as
//! [`Msg::Scoped`] (nested blocks, per-helper usage) and the running set as
//! [`StatusUpdate::Helpers`]; an interrupt cancels the session's helpers
//! (see [`SessionHandle::interrupt`]).
//!
//! `/remember` and the compaction trigger (`/compact now`, model switches)
//! live in [`context`].

mod approval;
mod context;
mod cost;
pub mod cost_store;
mod lifecycle;
mod models;
mod quit;
#[doc(hidden)]
pub mod testing;

use std::collections::HashSet;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures::{FutureExt, StreamExt};
use meerkat::ToolGatewayBuilder;
use meerkat_core::comms::EventStream;
use meerkat_core::lifecycle::RunId;
use meerkat_core::{
    AgentEvent, AgentToolDispatcher, EventEnvelope, InputId, RunResult, SessionId,
    TurnTerminalOutcome,
};
use meerkat_runtime::completion::{CompletionOutcome, CompletionWaitError};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::approval::ApprovalEngine;
use crate::bootstrap::{self, BootstrapOptions, InstructionSources, ModelChoice, Runtime};
use crate::cli::Args;
use crate::delivery::{PendingId, SteerFailure};
use crate::endpoints::EndpointName;
use crate::error::TuiError;
use crate::memory::{NoteStore, NoteToolDispatcher, Scope, prompt_index};
use crate::mobs::HelperUpdate;
use crate::session::{Admission, Completion, SessionHandle, SessionIdentity, SessionOptions};
use crate::tools::{FileDiff, FileToolDispatcher};
use crate::ui::{
    App, CompactionPolicy, CompactionTrigger, Effect, History, Msg, NoticeLevel, StatusInfo,
    StatusUpdate, TurnFailure, TurnOutcome, TurnSummary, UiChannels, run_ui,
};

pub use lifecycle::{Start, blocks_from_session, open_start, session_messages};
pub use models::DEFAULT_CONTEXT_WINDOW;
pub use quit::SavedUnsent;

/// Capacity of the primary agent-event channel.
const AGENT_EVENT_CAPACITY: usize = 256;

/// How long a turn whose inputs are all terminal waits for the runs it saw
/// start to publish their terminal events, so their last text reaches the
/// UI before the summary (a stopped run may publish none).
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
    let trigger = template.startup_trigger(&runtime);
    let (session, transcript) =
        open_start(&runtime, template.options(None, trigger), &start).await?;

    let mut status = StatusInfo::default();
    status.set_model(runtime.choice.endpoint.as_str(), &runtime.choice.model);
    status.set_session(&session.id().to_string());
    let realm_root = runtime.scope.realm_root();
    let history = History::load(realm_root.join("tui_history"));
    let global_history = runtime
        .scope
        .global
        .as_ref()
        .map(|global| History::load(global.history_file()));
    let mut app = App::new(status)
        .with_compaction_policy(template.policy)
        .with_history(history.entries().to_vec())
        .with_global_history(
            global_history
                .as_ref()
                .map(|h| h.entries().to_vec())
                .unwrap_or_default(),
        );
    let histories: Vec<History> = std::iter::once(history).chain(global_history).collect();

    let (agent_tx, agent_rx) = mpsc::channel(AGENT_EVENT_CAPACITY);
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
        UiChannels { agent_rx, ui_rx },
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
/// tools), where the instruction files and the notes index come from, and
/// the compaction policy.
pub(crate) struct Template {
    external_tools: Arc<dyn AgentToolDispatcher>,
    notes: Arc<NoteToolDispatcher>,
    approval: Arc<ApprovalEngine>,
    instructions: InstructionSources,
    policy: CompactionPolicy,
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
        let notes = Arc::new(NoteToolDispatcher::new(
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
            .map_err(TuiError::ToolGateway)?;
        let policy = runtime
            .approval
            .engine
            .settings()
            .compaction_policy(None)
            .normalized()
            .0;
        Ok((
            Self {
                external_tools: Arc::new(gateway),
                notes,
                approval: Arc::clone(&runtime.approval.engine),
                instructions: runtime.instructions.clone(),
                policy,
            },
            diff_rx,
        ))
    }

    /// Options for a session build on `choice` (`None`: the start-up
    /// choice) with the native auto-compaction `trigger`. The instruction
    /// files and the notes index are re-read every time, so `/new` picks up
    /// what changed meanwhile (a resumed session keeps the instructions it
    /// was created with, see [`SessionHandle::resume`]).
    pub(crate) fn options(
        &self,
        choice: Option<ModelChoice>,
        trigger: CompactionTrigger,
    ) -> SessionOptions {
        let mut additional_instructions = self.instructions.render();
        // Last, and labelled by the note store as untrusted, model-written
        // context: the notes are what an earlier session wrote, not
        // instructions.
        additional_instructions.extend(prompt_index(self.notes.user(), self.notes.workspace()));
        SessionOptions {
            external_tools: Some(Arc::clone(&self.external_tools)),
            additional_instructions,
            choice,
            approval: Some(Arc::clone(&self.approval)),
            compaction: trigger,
        }
    }

    /// The trigger of the start session: the policy's for the start-up
    /// model (a resumed session on another model gets its own once the
    /// runner runs, see [`Runner::ensure_trigger`]).
    pub(crate) fn startup_trigger(&self, runtime: &Runtime) -> CompactionTrigger {
        let choice = &runtime.choice;
        let (window, estimated) = models::window_of(runtime, choice.provider, &choice.model);
        self.policy.trigger((!estimated).then_some(window))
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
        /// Shown to the UI as written into the conversation.
        delivered: bool,
    },
}

/// One input the runner admitted for the current turn.
#[derive(Debug)]
struct Tracked {
    input_id: InputId,
    /// The caller idempotency key it was admitted under.
    key: String,
    kind: InputKind,
    done: bool,
    /// What its terminal contributes to the turn's result (`None`: nothing,
    /// a steer consumed without a run result).
    end: Option<Result<TurnSummary, TurnFailure>>,
}

/// The UI's current turn: its inputs, in admission order, and what they
/// came to.
#[derive(Debug, Default)]
struct Turn {
    inputs: Vec<Tracked>,
    /// An interrupt was sent and its report (the inputs it cancelled) has
    /// not arrived yet: the turn does not end before it.
    stop_pending: bool,
    /// Inputs the runtime reports the interrupt cancelled.
    cancelled: HashSet<InputId>,
    /// Steered messages the runtime ended without delivering.
    undelivered: Vec<PendingId>,
    /// Runs that published `RunStarted` and no terminal event yet, by run
    /// id (a run starting over under the same id is one run).
    open_runs: HashSet<RunId>,
    /// Every input is terminal; the turn ends when the open runs close or
    /// this passes.
    finish_by: Option<tokio::time::Instant>,
}

impl Turn {
    fn is_active(&self) -> bool {
        !self.inputs.is_empty()
    }

    fn all_done(&self) -> bool {
        self.inputs.iter().all(|input| input.done)
    }

    /// Every input is terminal and no interrupt report is outstanding.
    fn settled(&self) -> bool {
        self.all_done() && !self.stop_pending
    }

    fn outstanding(&self) -> Vec<InputId> {
        self.inputs
            .iter()
            .filter(|input| !input.done)
            .map(|input| input.input_id.clone())
            .collect()
    }

    /// The turn's result from its inputs' ends, in admission order: an input
    /// the runtime reports this interrupt cancelled ended interrupted. A
    /// failure is never replaced; an interruption only by a failure; an
    /// unknown or pending end only by those two; a completion only by a
    /// later completion. No end at all is an empty completion.
    fn result(&self) -> Result<TurnSummary, TurnFailure> {
        let mut result: Option<Result<TurnSummary, TurnFailure>> = None;
        for input in &self.inputs {
            let Some(end) = &input.end else {
                continue;
            };
            let end = match end {
                Err(_) if self.cancelled.contains(&input.input_id) => {
                    Err(TurnFailure::Interrupted)
                }
                other => other.clone(),
            };
            result = Some(merge(result, end));
        }
        result.unwrap_or_else(|| Ok(TurnSummary::default()))
    }
}

/// Fold one input's end into the turn's (see [`Turn::result`]).
fn merge(
    current: Option<Result<TurnSummary, TurnFailure>>,
    next: Result<TurnSummary, TurnFailure>,
) -> Result<TurnSummary, TurnFailure> {
    let rank = |r: &Result<TurnSummary, TurnFailure>| match r {
        Ok(_) => 0,
        Err(TurnFailure::Unknown(_) | TurnFailure::Pending { .. }) => 1,
        Err(TurnFailure::Interrupted) => 2,
        Err(TurnFailure::Failed(_)) => 3,
    };
    match current {
        Some(current) if rank(&current) > rank(&next) => current,
        Some(current) if rank(&current) == rank(&next) && current.is_err() => current,
        _ => next,
    }
}

/// The report of an interrupt task: the inputs the runtime says it
/// cancelled.
struct Stopped {
    session: SessionId,
    cancelled: Vec<InputId>,
}

/// A completion waiter's report.
struct Completed {
    session: SessionId,
    input_id: InputId,
    outcome: Settled,
}

/// What an input's terminal means for the turn.
#[derive(Debug)]
enum Settled {
    /// Consumed, with the run's result when it produced one.
    Done(Option<Box<RunResult>>),
    /// The ends a turn can have without a result.
    Ended(TurnFailure),
}

/// The typed end of one input's completion: no failure is rewritten into
/// an interruption; a cancellation is one only when the runtime says so
/// (`Cancelled`, or failure metadata whose terminal outcome is
/// `Cancelled`).
fn settle(outcome: Result<CompletionOutcome, CompletionWaitError>) -> Settled {
    let ended = |failure: TurnFailure| Settled::Ended(failure);
    match outcome {
        Ok(CompletionOutcome::Completed(result)) => Settled::Done(Some(result)),
        Ok(CompletionOutcome::CompletedWithoutResult) => Settled::Done(None),
        Ok(CompletionOutcome::Cancelled) => ended(TurnFailure::Interrupted),
        Ok(
            CompletionOutcome::Abandoned { reason, error }
            | CompletionOutcome::AbandonedWithError { reason, error }
            | CompletionOutcome::RuntimeTerminated { reason, error },
        ) => {
            if error.outcome == Some(TurnTerminalOutcome::Cancelled) {
                ended(TurnFailure::Interrupted)
            } else {
                ended(TurnFailure::Failed(reason))
            }
        }
        Ok(CompletionOutcome::CompletedWithFinalizationFailure { error }) => {
            ended(TurnFailure::Failed(error.detail.unwrap_or_else(|| {
                "the turn could not be committed".to_string()
            })))
        }
        Ok(CompletionOutcome::CallbackPending { tool_name, .. }) => {
            ended(TurnFailure::Pending { tool: tool_name })
        }
        Ok(CompletionOutcome::CallbackBatchPending { pending_tool_calls }) => {
            ended(TurnFailure::Pending {
                tool: pending_tool_calls
                    .first()
                    .map(|call| call.tool_name.clone())
                    .unwrap_or_default(),
            })
        }
        Ok(CompletionOutcome::OutcomeUnknown { run_id }) => ended(TurnFailure::Unknown(format!(
            "run {run_id} committed no receipt for it"
        ))),
        Err(err) => ended(TurnFailure::Unknown(err.to_string())),
    }
}

/// What an admission says about its input's end before any waiting.
fn settle_admission(completion: &Completion) -> Option<Settled> {
    match completion {
        Completion::Pending(_) => None,
        Completion::TerminalAtAdmission => Some(Settled::Ended(TurnFailure::Unknown(
            "the runtime reported the input terminal at admission without an outcome".to_string(),
        ))),
        Completion::AlreadyTerminal => Some(Settled::Done(None)),
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
    /// The current session's model, provider and binding as its
    /// authoritative metadata recorded them when last read (after opening,
    /// switching or rebuilding it).
    identity: SessionIdentity,
    events: Option<EventStream>,
    agent_tx: mpsc::Sender<EventEnvelope<AgentEvent>>,
    ui_tx: mpsc::UnboundedSender<Msg>,
    effect_rx: mpsc::UnboundedReceiver<Effect>,
    diff_rx: mpsc::UnboundedReceiver<FileDiff>,
    completion_tx: mpsc::UnboundedSender<Completed>,
    completion_rx: mpsc::UnboundedReceiver<Completed>,
    stopped_tx: mpsc::UnboundedSender<Stopped>,
    stopped_rx: mpsc::UnboundedReceiver<Stopped>,
    /// Completion waiters and interrupt tasks; aborted when the session is
    /// switched or the runner shuts down.
    tasks: JoinSet<()>,
    turn: Turn,
    /// Steered messages delivered in finished turns (quitting never saves
    /// them as unsent).
    delivered: Vec<PendingId>,
    /// Messages the UI still held when the user quit ([`Effect::SaveUnsent`]).
    held_unsent: Vec<(Option<PendingId>, String)>,
    /// The approval engine's channels and open escalations.
    approval: approval::ApprovalDesk,
    /// The endpoint the current session runs on, derived from its identity
    /// (`None`: no endpoint of this process serves its binding).
    current_endpoint: Option<EndpointName>,
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
    /// The session runs on `/compact now`'s one-turn trigger.
    compact_now_pending: bool,
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
        let (stopped_tx, stopped_rx) = mpsc::unbounded_channel();
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
        let helper_updates = runtime.helpers.as_ref().and_then(|h| h.take_updates());
        let identity = SessionIdentity {
            model: runtime.choice.model.clone(),
            provider: runtime.choice.provider,
            auth_binding: runtime.choice.auth_binding.clone(),
        };
        let mut runner = Self {
            runtime,
            template,
            session,
            identity,
            events: None,
            agent_tx,
            ui_tx,
            effect_rx,
            diff_rx,
            completion_tx,
            completion_rx,
            stopped_tx,
            stopped_rx,
            tasks: JoinSet::new(),
            turn: Turn::default(),
            delivered: Vec::new(),
            held_unsent: Vec::new(),
            approval: desk,
            current_endpoint: None,
            models,
            model_fetch: None,
            picker_waiting: false,
            pending_set: None,
            prices,
            price_fetch,
            helper_updates,
            compact_now_pending: false,
        };
        runner.refresh_identity().await;
        runner.push_auto_status();
        for notice in runner.runtime.confinement.notices.clone() {
            runner.notice(NoticeLevel::Info, notice);
        }
        for notice in notices
            .into_iter()
            .chain(std::mem::take(&mut runner.runtime.notices))
        {
            runner.notice(NoticeLevel::Warn, notice);
        }
        runner.push_prices();
        runner.show_session(transcript.as_ref(), false);
        // Fetch the lists in the background, for the picker, bare-id
        // lookups and endpoint prices.
        if runner.models.is_none() && runner.runtime.endpoints.iter().any(|e| e.has_key()) {
            runner.ensure_fetch(false);
        }
        runner.follow_events().await;
        // A resumed start session on another model than the start-up one.
        runner.ensure_trigger().await;
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
                Some(stopped) = self.stopped_rx.recv() => self.on_stopped(stopped).await,
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
                Some(joined) = self.tasks.join_next(), if !self.tasks.is_empty() => {
                    if let Err(err) = joined
                        && !err.is_cancelled()
                    {
                        tracing::warn!(error = %err, "a runner task failed");
                    }
                }
                () = sleep_until(self.turn.finish_by) => self.finish_turn().await,
            }
        }
        self.shutdown().await
    }

    /// Re-read the current session's identity from its authoritative
    /// metadata and derive its endpoint.
    async fn refresh_identity(&mut self) {
        match self.session.identity().await {
            Ok(identity) => self.identity = identity,
            Err(err) => {
                tracing::warn!(error = %err, "cannot read the session's identity");
            }
        }
        self.current_endpoint = crate::endpoints::endpoint_of(
            self.identity.provider,
            self.identity.auth_binding.as_ref(),
            &self.runtime.endpoints,
        );
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
            HelperUpdate::Unmeasured(label) => Msg::HelperUnmeasured(label),
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
            Effect::Remember(text) => self.remember(&text).await,
            Effect::CompactNow => self.compact_now().await,
            Effect::CostSnapshot(snapshot) => self.save_cost_snapshot(&snapshot),
            Effect::TrustRealmRules => self.trust_realm_rules(),
            Effect::TrustProjectMcp => self.trust_project_mcp().await,
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
            Err(err) => self.send(Msg::TurnFinished(TurnOutcome::plain(Err(
                TurnFailure::Failed(err.to_string()),
            )))),
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
            key,
            completion,
        } = admission;
        self.turn.finish_by = None;
        self.turn.inputs.push(Tracked {
            input_id: input_id.clone(),
            key,
            kind,
            done: false,
            end: None,
        });
        let tx = self.completion_tx.clone();
        let session = self.session.id().clone();
        if let Some(outcome) = settle_admission(&completion) {
            let _ = tx.send(Completed {
                session,
                input_id,
                outcome,
            });
            return;
        }
        if let Completion::Pending(handle) = completion {
            self.tasks.spawn(async move {
                let outcome = settle(handle.wait().await);
                let _ = tx.send(Completed {
                    session,
                    input_id,
                    outcome,
                });
            });
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
        self.turn.stop_pending = true;
        let session = self.session.clone();
        let outstanding = self.turn.outstanding();
        let ui_tx = self.ui_tx.clone();
        let stopped_tx = self.stopped_tx.clone();
        self.tasks.spawn(async move {
            let cancelled = match session.interrupt(&outstanding).await {
                Ok(cancelled) => cancelled,
                Err(err) => {
                    tracing::warn!(error = %err, "interrupt failed");
                    let _ = ui_tx.send(Msg::Notice {
                        level: NoticeLevel::Error,
                        text: format!("interrupt failed: {err}"),
                    });
                    Vec::new()
                }
            };
            let _ = stopped_tx.send(Stopped {
                session: session.id().clone(),
                cancelled,
            });
        });
    }

    /// The interrupt's report arrived: record what it cancelled and end
    /// the turn when every input is terminal.
    async fn on_stopped(&mut self, stopped: Stopped) {
        if &stopped.session != self.session.id() || !self.turn.is_active() {
            return;
        }
        self.turn.stop_pending = false;
        self.turn.cancelled.extend(stopped.cancelled);
        self.maybe_finish().await;
    }

    /// End the turn once it is settled (after a short grace for run events
    /// still open).
    async fn maybe_finish(&mut self) {
        if self.turn.settled() {
            if self.turn.open_runs.is_empty() {
                self.finish_turn().await;
            } else {
                self.turn.finish_by = Some(tokio::time::Instant::now() + RUN_EVENT_GRACE);
            }
        }
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
        if self.turn.finish_by.is_some() && self.turn.open_runs.is_empty() {
            self.finish_turn().await;
        }
    }

    /// Steer acknowledgements an event implies, and run bookkeeping by run
    /// id. Events without a run id never associate with a run.
    fn observe(&mut self, event: &AgentEvent) -> Vec<Msg> {
        let mut msgs = Vec::new();
        match event {
            AgentEvent::RunStarted { identity, .. } => {
                self.approval
                    .engine
                    .run_started(self.session.id(), identity.run_id.as_ref());
                // A repeated start of a run id is that run starting over.
                if let Some(run_id) = &identity.run_id {
                    self.turn.open_runs.insert(run_id.clone());
                }
            }
            AgentEvent::RunCompleted { identity, .. } | AgentEvent::RunFailed { identity, .. } => {
                if let Some(run_id) = &identity.run_id {
                    self.turn.open_runs.remove(run_id);
                }
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

    /// Flip a steer's delivered flag; returns its id when it changed.
    fn set_delivered(&mut self, input_id: &InputId, value: bool) -> Option<PendingId> {
        self.turn
            .inputs
            .iter_mut()
            .find_map(|input| match &mut input.kind {
                InputKind::Steer { id, delivered }
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
        let Some(input) = self
            .turn
            .inputs
            .iter_mut()
            .find(|input| input.input_id == done.input_id && !input.done)
        else {
            return;
        };
        input.done = true;
        let mut msgs = Vec::new();
        let mut undelivered = None;
        let end = match (&mut input.kind, done.outcome) {
            (InputKind::Prompt, Settled::Done(result)) => {
                Some(Ok(result.as_deref().map(summary_of).unwrap_or_default()))
            }
            (InputKind::Steer { id, delivered }, Settled::Done(result)) => {
                // Consumed without a boundary event: a follow-up run
                // delivered it (its completion says so).
                if !*delivered {
                    *delivered = true;
                    msgs.push(Msg::SteerDelivered(*id));
                }
                result.as_deref().map(|r| Ok(summary_of(r)))
            }
            (InputKind::Prompt, Settled::Ended(end)) => Some(Err(end)),
            (InputKind::Steer { id, .. }, Settled::Ended(end)) => {
                undelivered = Some(*id);
                Some(Err(end))
            }
        };
        input.end = end;
        self.turn.undelivered.extend(undelivered);
        for msg in msgs {
            self.send(msg);
        }
        self.maybe_finish().await;
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
            } = input.kind
                && !turn.undelivered.contains(&id)
            {
                self.delivered.push(id);
            }
        }
        // Steered messages the model never received are not the user's
        // words to it: they leave the classifier transcript again.
        for input in &turn.inputs {
            if let InputKind::Steer { id, .. } = &input.kind
                && turn.undelivered.contains(id)
            {
                self.session.forget_input(&input.key);
            }
        }
        let result = turn.result();
        let mut undelivered = turn.undelivered;
        undelivered.sort();
        self.send(Msg::TurnFinished(TurnOutcome {
            result,
            undelivered,
        }));
        // Effects the UI sends on the outcome wait for this rebuild.
        self.end_compact_now().await;
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
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_core::TurnErrorMetadata;

    fn failed(reason: &str) -> Result<TurnSummary, TurnFailure> {
        Err(TurnFailure::Failed(reason.into()))
    }

    fn input(end: Result<TurnSummary, TurnFailure>) -> Tracked {
        Tracked {
            input_id: InputId::new(),
            key: String::new(),
            kind: InputKind::Prompt,
            done: true,
            end: Some(end),
        }
    }

    #[test]
    fn a_failure_is_not_overwritten_by_a_later_end() {
        let turn = Turn {
            inputs: vec![
                input(Ok(TurnSummary::default())),
                input(failed("boom")),
                input(Ok(TurnSummary {
                    text: "later".into(),
                    ..TurnSummary::default()
                })),
                input(Err(TurnFailure::Interrupted)),
                input(failed("second")),
            ],
            ..Turn::default()
        };
        assert_eq!(turn.result(), failed("boom"));
        // An interruption outranks an unknown end; a completion outranks
        // nothing.
        let turn = Turn {
            inputs: vec![
                input(Err(TurnFailure::Unknown("?".into()))),
                input(Err(TurnFailure::Interrupted)),
                input(Ok(TurnSummary::default())),
            ],
            ..Turn::default()
        };
        assert_eq!(turn.result(), Err(TurnFailure::Interrupted));
    }

    #[test]
    fn an_input_the_interrupt_cancelled_ended_interrupted_and_nothing_else_did() {
        let stopped = input(failed("rkat-tui: interrupted by the user"));
        let other = input(failed("provider error"));
        let mut turn = Turn {
            cancelled: HashSet::from([stopped.input_id.clone()]),
            inputs: vec![stopped],
            ..Turn::default()
        };
        assert_eq!(turn.result(), Err(TurnFailure::Interrupted));
        // A failure the interrupt did not cancel stays a failure.
        turn.inputs.push(other);
        assert_eq!(turn.result(), failed("provider error"));
        // The turn waits for the interrupt's report.
        turn.stop_pending = true;
        assert!(turn.all_done() && !turn.settled());
    }

    fn metadata(outcome: Option<TurnTerminalOutcome>) -> TurnErrorMetadata {
        let mut error = TurnErrorMetadata::runtime_apply_failure("why");
        error.outcome = outcome;
        error
    }

    #[test]
    fn terminals_settle_to_typed_ends() {
        let end = |outcome| match settle(outcome) {
            Settled::Ended(end) => end,
            Settled::Done(_) => panic!("expected an end"),
        };
        assert_eq!(
            end(Ok(CompletionOutcome::Cancelled)),
            TurnFailure::Interrupted
        );
        // A failure stays a failure; only a typed cancellation interrupts.
        assert_eq!(
            end(Ok(CompletionOutcome::Abandoned {
                reason: "boom".into(),
                error: metadata(None),
            })),
            TurnFailure::Failed("boom".into())
        );
        assert_eq!(
            end(Ok(CompletionOutcome::AbandonedWithError {
                reason: "stopped".into(),
                error: metadata(Some(TurnTerminalOutcome::Cancelled)),
            })),
            TurnFailure::Interrupted
        );
        assert!(matches!(
            end(Ok(CompletionOutcome::CallbackPending {
                tool_use_id: "t".into(),
                tool_name: "ext".into(),
                args: serde_json::Value::Null,
                dedupe_key: None,
            })),
            TurnFailure::Pending { tool } if tool == "ext"
        ));
        assert!(matches!(
            end(Err(CompletionWaitError::ChannelClosed)),
            TurnFailure::Unknown(_)
        ));
        assert!(matches!(
            settle(Ok(CompletionOutcome::CompletedWithoutResult)),
            Settled::Done(None)
        ));
    }

    #[test]
    fn an_admission_without_a_waiter_is_never_a_fabricated_completion() {
        assert!(matches!(
            settle_admission(&Completion::TerminalAtAdmission),
            Some(Settled::Ended(TurnFailure::Unknown(_)))
        ));
        assert!(matches!(
            settle_admission(&Completion::AlreadyTerminal),
            Some(Settled::Done(None))
        ));
    }

    #[test]
    fn runs_are_tracked_by_id_and_a_restart_counts_once() {
        let mut turn = Turn::default();
        let run = RunId::new();
        turn.open_runs.insert(run.clone());
        // `RunStarted` again for the same id: the run starting over.
        turn.open_runs.insert(run.clone());
        assert_eq!(turn.open_runs.len(), 1);
        turn.open_runs.remove(&run);
        assert!(turn.open_runs.is_empty());
    }
}
