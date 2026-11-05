//! Test seam: drive the runner headless, without a terminal.
//!
//! [`spawn_runner`] wires a bootstrapped [`Runtime`] the way [`super::run`]
//! does (file and note tools, the start session, the UI seeding messages)
//! and hands back the channels the UI would own: send [`Effect`]s, collect
//! [`Msg`]s and agent events. Not a stable API.

use meerkat_core::{AgentEvent, EventEnvelope, SessionId};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

pub use super::quit::{SavedUnsent, UNSENT_FILE};
use super::{AGENT_EVENT_CAPACITY, Runner, RunnerParts, Start, Template, open_start};
use crate::bootstrap::Runtime;
use crate::error::TuiError;
use crate::ui::{Effect, Msg};

/// A running runner and the UI's ends of its channels.
pub struct RunnerHarness {
    /// Effects, as the UI would send them.
    pub effects: mpsc::UnboundedSender<Effect>,
    /// Runner messages to the UI (seeded like the real UI: a resumed start
    /// session's status and transcript come first).
    pub ui_rx: mpsc::UnboundedReceiver<Msg>,
    /// Session events, as the UI would render them.
    pub agent_rx: mpsc::Receiver<EventEnvelope<AgentEvent>>,
    /// Id of the start session.
    pub first_session: SessionId,
    /// Where quitting saves unsent messages.
    pub unsent_path: std::path::PathBuf,
    /// The runner; resolves after [`Effect::Quit`] (or when `effects` is
    /// dropped) with where unsent messages were saved.
    pub task: JoinHandle<Option<SavedUnsent>>,
}

/// Spawn a runner on `runtime`, opening `start`.
///
/// # Errors
///
/// When the tools cannot be composed or the start session cannot be opened.
pub async fn spawn_runner(runtime: Runtime, start: &Start) -> Result<RunnerHarness, TuiError> {
    let (template, diff_rx) = Template::new(&runtime)?;
    let (session, transcript) = open_start(&runtime, template.options(None), start).await?;
    let first_session = session.id().clone();
    let unsent_path = runtime.scope.realm_root().join(UNSENT_FILE);
    let (agent_tx, agent_rx) = mpsc::channel(AGENT_EVENT_CAPACITY);
    let (ui_tx, ui_rx) = mpsc::unbounded_channel();
    let (effects, effect_rx) = mpsc::unbounded_channel();
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
    Ok(RunnerHarness {
        effects,
        ui_rx,
        agent_rx,
        first_session,
        unsent_path,
        task: tokio::spawn(runner.run()),
    })
}
