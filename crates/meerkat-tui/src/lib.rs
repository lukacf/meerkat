//! `rkat-tui`: an interactive terminal coding harness built on the Meerkat agent engine.
//!
//! The binary hosts the agent in-process on the runtime-backed session path
//! and gates every tool call through an auto-mode approval model: read-only
//! and in-project edits pass without review, everything else is judged by a
//! reasoning-blind classifier, and a human is only consulted after repeated
//! blocks.
//!
//! Module map:
//! - [`cli`] — command-line arguments
//! - [`error`] — the surface's error type and exit codes
//! - [`paths`] — project-root path resolution and protected-path policy
//! - [`delivery`] — delivery states of queued and steered prompts
//! - [`overlay`] — the `--set`/`--allow`/… command-line layer over `tui.toml`
//! - [`pricing`] — token prices for the cost readout
//! - [`approval`] — auto-mode approval engine (tiers, rules, shell analysis, classifier, escalation)
//! - [`tools`] — in-crate file tools
//! - [`memory`] — file-based memory notes
//! - [`endpoints`] — LLM endpoints and `endpoint:model` references
//! - [`models`] — model catalog and live model lists
//! - [`credentials`] — provider credentials for the list-models client
//! - [`bootstrap`] — realm/config/factory/service construction
//! - [`session`] — session handle: create, resume, turns, interrupt
//! - [`runner`] — the event loop joining the session and the UI
//! - [`ui`] — ratatui application

pub mod approval;
pub mod bootstrap;
pub mod cli;
pub mod credentials;
pub mod delivery;
pub mod endpoints;
pub mod error;
pub mod memory;
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

/// Run the TUI with the given arguments.
///
/// The crate skeleton: the session path, approval gate and terminal UI land
/// in the following port phases.
///
/// # Errors
///
/// Returns a [`TuiError`] when bootstrap fails or the terminal cannot be driven.
pub async fn run(args: Args) -> Result<(), TuiError> {
    let _ = args;
    Ok(())
}
