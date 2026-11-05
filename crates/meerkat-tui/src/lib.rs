//! `rkat-tui`: an interactive terminal coding harness built on the Meerkat agent engine.
//!
//! The binary hosts the agent in-process on the runtime-backed session path
//! (a `PersistentSessionService` driven by a `MeerkatMachine`): prompts are
//! runtime inputs, messages typed mid-turn are durable steers the runtime
//! writes into the conversation at the next step, and sessions persist in
//! the workspace (`.rkat/tui/`) for `-c` / `--resume`. Every tool call is
//! gated by an auto-mode approval model installed as each session's
//! `ToolDispatchAdmission` ([`approval`]).
//!
//! The public API is [`run`], [`Args`] and [`TuiError`] (with the error
//! types its variants carry). Everything else is internal; the
//! `test-support` feature makes the modules public for the crate's own
//! integration tests and adds [`harness`] (headless runner). It is not a
//! stable API.
//!
//! Module map:
//! - `cli` — command-line arguments
//! - `error` — the surface's error type and exit codes
//! - `paths` — project-root path resolution and protected-path policy
//! - `http` — bounded HTTP body reads
//! - `delivery` — the contract for messages steered into a running turn
//! - `overlay` — the `--set`/`--allow`/… command-line layer over `tui.toml`
//! - `pricing` — token prices for the cost readout
//! - `approval` — auto-mode approval engine (tiers, rules, shell analysis, classifier, escalation)
//! - `tools` — in-crate file tools
//! - `memory` — file-based notes
//! - `endpoints` — LLM endpoints, `endpoint:model` references, their projection into the realm config
//! - `models` — live model lists of every endpoint (never the catalog)
//! - `bootstrap` — realm/config/factory/service construction, provider credentials
//! - `mobs` — helpers (sub-agents) as mob members: approval gate, model binding, observation
//! - `session` — session handle: create, resume, turns, steer, interrupt
//! - `runner` — the event loop joining the session and the UI
//! - `ui` — ratatui application

/// Declare the internal modules: crate-private, public with the
/// `test-support` feature (the crate's integration tests). Without it, an
/// item or re-export only the tests use is dead code to the binary, which
/// is expected; the all-features lint lanes still check every item.
macro_rules! internal_modules {
    ($($name:ident),* $(,)?) => {
        $(
            #[cfg(feature = "test-support")]
            pub mod $name;
            #[cfg(not(feature = "test-support"))]
            #[allow(dead_code, unused_imports)]
            pub(crate) mod $name;
        )*
    };
}

internal_modules!(
    approval, bootstrap, cli, delivery, endpoints, error, http, memory, mobs, models, overlay,
    paths, pricing, runner, session, tools, ui,
);

/// The headless runner the integration tests drive (see
/// `runner::testing`). Not a stable API.
#[cfg(feature = "test-support")]
pub mod harness {
    pub use crate::runner::testing::*;
}

pub use approval::ApprovalError;
pub use bootstrap::confinement::ConfinementError;
pub use cli::Args;
pub use endpoints::{EndpointError, ModelRefError};
pub use error::TuiError;
pub use overlay::OverlayError;
pub use session::AdmissionError;
pub use ui::UiError;

/// Run the TUI with the given arguments: bootstrap the realm and the
/// runtime-backed service, open the start session (`-c`, `--resume` or a
/// fresh one; the prompt argument becomes its first turn) and drive the
/// terminal until the user quits.
///
/// # Errors
///
/// Returns a [`TuiError`] when there is no terminal, bootstrap fails, the
/// session cannot be opened, or the terminal cannot be driven.
pub async fn run(args: Args) -> Result<(), TuiError> {
    runner::run(args).await
}
