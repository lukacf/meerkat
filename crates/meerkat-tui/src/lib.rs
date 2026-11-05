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
//! Module map:
//! - [`cli`] — command-line arguments
//! - [`error`] — the surface's error type and exit codes
//! - [`paths`] — project-root path resolution and protected-path policy
//! - [`delivery`] — the contract for messages steered into a running turn
//! - [`overlay`] — the `--set`/`--allow`/… command-line layer over `tui.toml`
//! - [`pricing`] — token prices for the cost readout
//! - [`approval`] — auto-mode approval engine (tiers, rules, shell analysis, classifier, escalation)
//! - [`tools`] — in-crate file tools
//! - [`memory`] — file-based memory notes
//! - [`endpoints`] — LLM endpoints, `endpoint:model` references, their lowering into realm bindings, per-session choice sidecars
//! - [`models`] — live model lists of every endpoint (never the catalog)
//! - [`bootstrap`] — realm/config/factory/service construction, provider credentials
//! - [`mobs`] — helpers (sub-agents) as mob members: approval gate, model binding, observation
//! - [`session`] — session handle: create, resume, turns, steer, interrupt
//! - [`runner`] — the event loop joining the session and the UI
//! - [`ui`] — ratatui application

pub mod approval;
pub mod bootstrap;
pub mod cli;
pub mod delivery;
pub mod endpoints;
pub mod error;
pub mod memory;
pub mod mobs;
pub mod models;
pub mod overlay;
pub mod paths;
pub mod pricing;
pub mod runner;
pub mod session;
pub mod tools;
pub mod ui;

pub use cli::Args;
pub use error::TuiError;

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
