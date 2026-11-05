//! Shared harness for the headless runner tests: a temporary workspace, a
//! scripted LLM client that records every request, and a runner over the
//! real runtime-backed service (`PersistentSessionService` + `MeerkatMachine`).

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use clap::Parser;
use futures::StreamExt;
use meerkat::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::{
    AgentEvent, Message, Provider, Session, SessionId, StopReason, TurnUsage, Usage,
};
use meerkat_tui::Args;
use meerkat_tui::bootstrap::{self, BootstrapOptions, TuiService};
use meerkat_tui::runner::Start;
use meerkat_tui::runner::testing::{RunnerHarness, SavedUnsent, spawn_runner};
use meerkat_tui::ui::{Effect, Msg, TurnOutcome};
use parking_lot::Mutex;
use serde_json::Value;
use tempfile::TempDir;
use tokio::sync::{Notify, mpsc};

/// Model every test session runs on.
pub const MODEL: &str = "claude-sonnet-4-6";

/// Upper bound on any await that could hang on a wiring bug.
pub const TIMEOUT: Duration = Duration::from_secs(30);

pub async fn within<F: Future>(future: F) -> F::Output {
    tokio::time::timeout(TIMEOUT, future)
        .await
        .expect("timed out")
}

/// One scripted LLM response.
#[derive(Clone)]
pub enum Step {
    /// Text, then `Done(EndTurn)`.
    Text(&'static str),
    /// A single tool call, then `Done(ToolUse)`.
    Tool {
        id: &'static str,
        name: &'static str,
        args: Value,
    },
    /// The inner step, once the gate is notified.
    Gated(Arc<Notify>, Box<Step>),
}

impl Step {
    fn into_parts(self) -> (Option<Arc<Notify>>, Step) {
        match self {
            Self::Gated(gate, inner) => (Some(gate), *inner),
            other => (None, other),
        }
    }

    fn events(self, model: &str) -> Vec<Result<LlmEvent, LlmError>> {
        let usage = Ok(LlmEvent::UsageUpdate {
            usage: TurnUsage::host_declared(
                Provider::Anthropic,
                model,
                Usage {
                    input_tokens: 10,
                    output_tokens: 5,
                    ..Usage::default()
                },
            ),
        });
        let done = |stop_reason| {
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::success(stop_reason),
            })
        };
        match self {
            Self::Text(text) => vec![
                Ok(LlmEvent::TextDelta {
                    delta: text.to_string(),
                    meta: None,
                }),
                usage,
                done(StopReason::EndTurn),
            ],
            Self::Tool { id, name, args } => vec![
                Ok(LlmEvent::ToolCallComplete {
                    id: id.to_string(),
                    name: name.to_string(),
                    args,
                    meta: None,
                }),
                usage,
                done(StopReason::ToolUse),
            ],
            Self::Gated(_, inner) => inner.events(model),
        }
    }
}

/// Returns one scripted response per call and records every request.
pub struct ScriptedLlmClient {
    script: Vec<Step>,
    next: AtomicUsize,
    requests: Mutex<Vec<Vec<Message>>>,
    calls: mpsc::UnboundedSender<usize>,
    calls_rx: tokio::sync::Mutex<mpsc::UnboundedReceiver<usize>>,
}

impl ScriptedLlmClient {
    /// A client replaying `script` (then answering "script exhausted").
    pub fn new(script: Vec<Step>) -> Arc<Self> {
        let (calls, calls_rx) = mpsc::unbounded_channel();
        Arc::new(Self {
            script,
            next: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
            calls,
            calls_rx: tokio::sync::Mutex::new(calls_rx),
        })
    }

    /// The messages of every request so far.
    pub fn requests(&self) -> Vec<Vec<Message>> {
        self.requests.lock().clone()
    }

    /// Wait for call number `n` (0-based) to start streaming.
    pub async fn wait_call(&self, n: usize) {
        let mut rx = self.calls_rx.lock().await;
        within(async {
            while let Some(index) = rx.recv().await {
                if index >= n {
                    return;
                }
            }
            panic!("call channel closed");
        })
        .await;
    }
}

#[async_trait]
impl LlmClient for ScriptedLlmClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> std::pin::Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>>
    {
        let index = self.next.fetch_add(1, Ordering::SeqCst);
        self.requests.lock().push(request.messages.clone());
        let _ = self.calls.send(index);
        let step = self
            .script
            .get(index)
            .cloned()
            .unwrap_or(Step::Text("script exhausted"));
        let (gate, step) = step.into_parts();
        let events = step.events(&request.model);
        Box::pin(
            futures::stream::once(async move {
                if let Some(gate) = gate {
                    gate.notified().await;
                }
                futures::stream::iter(events)
            })
            .flatten(),
        )
    }

    fn provider(&self) -> Provider {
        Provider::Anthropic
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

/// A temporary workspace (the project root, user-config root and config dir
/// all live inside it, so nothing touches the user's home).
pub struct Workspace {
    pub dir: TempDir,
    pub root: PathBuf,
}

impl Workspace {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap().join("project");
        std::fs::create_dir_all(&root).unwrap();
        Self { dir, root }
    }

    /// A sibling directory under the same temporary root.
    pub fn sibling(&self, name: &str) -> PathBuf {
        self.root.parent().unwrap().join(name)
    }
}

/// Arguments for a workspace-local realm at `root`.
pub fn args_for(root: &Path) -> Args {
    let root = root.to_str().unwrap();
    let cfg = format!("{root}/.cfg");
    Args::try_parse_from([
        "rkat-tui",
        "-w",
        root,
        "--user-config-root",
        root,
        "--config-dir",
        &cfg,
        "--model",
        MODEL,
    ])
    .unwrap()
}

/// A runner over a real runtime-backed service, with its channels.
pub struct Rig {
    pub service: Arc<TuiService>,
    pub store: Arc<dyn meerkat::SessionStore>,
    pub effects: mpsc::UnboundedSender<Effect>,
    pub ui_rx: mpsc::UnboundedReceiver<Msg>,
    pub events: Arc<Mutex<Vec<AgentEvent>>>,
    pub first_session: SessionId,
    pub unsent_path: PathBuf,
    pub task: tokio::task::JoinHandle<Option<SavedUnsent>>,
    /// Every message received so far.
    pub seen: Vec<Msg>,
}

impl Rig {
    /// Bootstrap the realm at `root` with `llm` and start a runner on `start`.
    pub async fn start(root: &Path, llm: &Arc<ScriptedLlmClient>, start: Start) -> Self {
        let runtime = bootstrap::bootstrap(
            &args_for(root),
            BootstrapOptions {
                llm_override: Some(Arc::clone(llm) as Arc<dyn LlmClient>),
            },
        )
        .await
        .expect("bootstrap");
        let service = Arc::clone(&runtime.service);
        let store = Arc::clone(&runtime.store);
        let RunnerHarness {
            effects,
            ui_rx,
            mut agent_rx,
            first_session,
            unsent_path,
            task,
        } = within(spawn_runner(runtime, &start)).await.expect("runner");
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        // The UI would render these; nobody may let the runner block.
        tokio::spawn(async move {
            while let Some(env) = agent_rx.recv().await {
                sink.lock().push(env.payload);
            }
        });
        Self {
            service,
            store,
            effects,
            ui_rx,
            events,
            first_session,
            unsent_path,
            task,
            seen: Vec::new(),
        }
    }

    pub fn send(&self, effect: Effect) {
        self.effects.send(effect).unwrap();
    }

    /// The next message matching `pred` (earlier ones are kept in `seen`).
    pub async fn wait_for(&mut self, pred: impl Fn(&Msg) -> bool) -> Msg {
        within(async {
            loop {
                let msg = self.ui_rx.recv().await.expect("runner gone");
                self.seen.push(msg.clone());
                if pred(&msg) {
                    return msg;
                }
            }
        })
        .await
    }

    /// The next turn outcome.
    pub async fn turn_finished(&mut self) -> TurnOutcome {
        match self
            .wait_for(|msg| matches!(msg, Msg::TurnFinished(_)))
            .await
        {
            Msg::TurnFinished(outcome) => outcome,
            other => panic!("not a turn outcome: {other:?}"),
        }
    }

    /// The committed transcript of `id`.
    pub async fn session(&self, id: &SessionId) -> Session {
        self.service
            .load_authoritative_session(id)
            .await
            .expect("load session")
            .expect("session exists")
    }

    /// Quit and wait for the runner's shutdown.
    pub async fn quit(self) -> Option<SavedUnsent> {
        self.send(Effect::Quit);
        within(self.task).await.expect("runner task")
    }
}

/// Texts of the user messages of `messages`.
pub fn user_texts(messages: &[Message]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|message| match message {
            Message::User(user) => Some(user.text_content()),
            _ => None,
        })
        .collect()
}

/// Copy a directory tree.
pub fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}
