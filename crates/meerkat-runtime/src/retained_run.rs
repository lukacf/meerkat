//! Typed receipt of a host's settle of the run a session retained
//! ([`crate::MeerkatMachine::settle_retained_run`]).
//!
//! A durable session that stopped inside a run (a crash or a power cut after
//! a round commit, or a graceful stop that kept the run) keeps that run
//! open, and the registration that opens the session again marks it
//! resumable. A host that must not let the run continue (an app's Recover
//! flow) holds the session's run starts, then settles the run without
//! resuming it: no model request is made. The machine decides the verdict.

use meerkat_core::lifecycle::{InputId, RunId};

use crate::input_state::InputTerminalOutcome;

/// One input bound to a settled run, with its committed terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RetainedRunSettledInput {
    /// The input's id.
    pub input_id: InputId,
    /// The committed input terminal: `Abandoned { reason: Cancelled }` for
    /// an input the settle ended. Read from the live ledger, then the
    /// durable row of an archived input; `None` only when neither retains
    /// the row (an ephemeral runtime that already released it).
    pub terminal: Option<InputTerminalOutcome>,
}

/// What [`crate::MeerkatMachine::settle_retained_run`] found and did.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RetainedRunSettlement {
    /// `run_id` was the run the session retained (marked resumable by its
    /// registration and not begun again), and it was settled without
    /// resuming: every input bound to it ended `Abandoned { reason:
    /// Cancelled }`, its provisional tail was closed as an interrupted run
    /// (every committed round kept, open calls answered as outcome-unknown,
    /// the interrupted notice appended), and no model request was made.
    Settled {
        run_id: RunId,
        inputs: Vec<RetainedRunSettledInput>,
    },
    /// No run is current and none is retained: there is nothing to settle,
    /// and nothing changed.
    NotRetained,
    /// The session is busy, and nothing changed: a run is current
    /// (`current_run_id`: the retained run resumed, or another run started),
    /// or the registration cannot settle the run it retained now (it is
    /// initializing, retiring, stopped, or draining for its unregister,
    /// whose next registration finds the run open again).
    Busy { current_run_id: Option<RunId> },
}
