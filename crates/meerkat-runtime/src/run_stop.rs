//! Typed receipt of a run-fenced Stop ([`crate::MeerkatMachine::stop_run`]).
//!
//! The receipt is built only from canonical completion deliveries and the
//! committed input terminals they observe. It never reports a contributor
//! that the machine did not bind to the stopped run.

use meerkat_core::lifecycle::{InputId, RunId};

use crate::completion::CompletionOutcome;
use crate::input_state::InputTerminalOutcome;
use crate::runtime_state::RuntimeState;

/// One input that contributed to a stopped run, with its canonical terminal.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct RunStopContributor {
    /// The contributor's input id.
    pub input_id: InputId,
    /// The generated completion delivered for this input. A batch
    /// contributor of a cancelled run receives `Cancelled`. An unretained
    /// durable join is terminalized through the runless terminal carrier, so
    /// its completion is `RuntimeTerminated` while its committed terminal
    /// below is `Abandoned { reason: Cancelled }`: read `terminal` for the
    /// lifecycle fact.
    pub outcome: CompletionOutcome,
    /// The committed input terminal. A batch contributor of a cancelled run
    /// and an unretained durable join are `Abandoned { reason: Cancelled }`;
    /// a durable join whose append survives in the kept image is `Consumed`.
    /// Read from the completion witness, then the live ledger, then the
    /// durable row of an archived input. `None` only when none of them
    /// retains the row (an ephemeral runtime that already released it).
    pub terminal: Option<InputTerminalOutcome>,
}

/// Result of [`crate::MeerkatMachine::stop_run`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum RunStopReceipt {
    /// The expected run was the current run and the stop was linearized
    /// under the session mutation gate. Every input staged for the run at
    /// that point, including durable Steer inputs that joined it at a model
    /// boundary, has reached its terminal; none of them re-entered a lane or
    /// started a successor run. Inputs admitted but never joined to the run
    /// are untouched.
    Stopped {
        run_id: RunId,
        contributors: Vec<RunStopContributor>,
    },
    /// The expected run was not the current run (already terminal, replaced
    /// by a newer run, or never current). Nothing was stopped, and no queued
    /// input or newer run was touched.
    NotCurrent {
        run_id: RunId,
        current_run_id: Option<RunId>,
    },
    /// The expected run is still bound, but generated authority refused the
    /// stop because the runtime left `Running`/`Retired` (a runtime stop or
    /// teardown owns the run and terminalizes it). Nothing was staged.
    NotStoppable { run_id: RunId, state: RuntimeState },
}
