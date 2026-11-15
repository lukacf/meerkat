//! Typed answer of a queued-input recall
//! ([`crate::MeerkatMachine::recall_queued_input`]).
//!
//! A caller withdraws one queued input of a session, only if no run has
//! taken it yet (any queued input of the session, whoever submitted it, with
//! everything the queue coalesced into it). MeerkatMachine decides the answer
//! in one transition guarded on the
//! input's phase (`AbandonQueuedInput`): the same fact a run reads when it
//! takes the input, under the same driver lock, so exactly one of the two
//! commits. A recall never interrupts started work.

use meerkat_core::lifecycle::{InputId, RunId};

use crate::input_state::{InputLifecycleState, InputTerminalOutcome};

/// Result of [`crate::MeerkatMachine::recall_queued_input`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum QueuedInputRecall {
    /// The input was still queued. It is now terminal, durably:
    /// `Abandoned { reason: Cancelled }` (public terminal class `cancelled`).
    /// No run will take it, and its completion waiter is resolved.
    ///
    /// `attempt_count` is the machine's count of run attempts for the input
    /// (a failed attempt that returned it to the queue to be retried). `0`
    /// means no attempt was counted. It is not proof that no run ever saw
    /// the input: a steer that joined a running turn and went back to the
    /// queue because the turn discarded what it had appended counts no
    /// attempt.
    Abandoned {
        input_id: InputId,
        attempt_count: u32,
    },
    /// A run has taken the input and it has not reached a terminal. Nothing
    /// was touched: the run is not interrupted and the input stays bound to
    /// it. `phase` is `Staged`, `Applied` or `AppliedPendingConsumption`;
    /// `run_id` is the run that holds it (see
    /// [`crate::MeerkatMachine::stop_run`] to stop that run instead).
    NotQueued {
        input_id: InputId,
        phase: InputLifecycleState,
        run_id: Option<RunId>,
    },
    /// The input already reached a terminal. Nothing was touched. A second
    /// recall of a recalled input answers `Abandoned { reason: Cancelled }`
    /// here.
    AlreadyTerminal {
        input_id: InputId,
        terminal: InputTerminalOutcome,
    },
}

impl QueuedInputRecall {
    /// The input this answer is about.
    #[must_use]
    pub fn input_id(&self) -> &InputId {
        match self {
            Self::Abandoned { input_id, .. }
            | Self::NotQueued { input_id, .. }
            | Self::AlreadyTerminal { input_id, .. } => input_id,
        }
    }

    /// Whether this call withdrew the input.
    #[must_use]
    pub fn abandoned(&self) -> bool {
        matches!(self, Self::Abandoned { .. })
    }
}
