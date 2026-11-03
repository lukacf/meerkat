//! How a memory stage reads the stop of its completion.
//!
//! The distiller, the hygienist and the steward each send one bounded
//! completion per call and strict-parse its text. This is the one place that
//! decides whether a successful stream's stop reason makes that text the
//! stage's answer, so the three stages cannot drift apart. Only an ended turn
//! or a matched stop sequence does: nothing is written from a body the model
//! did not finish, however well it parses.

use meerkat_core::StopReason;

/// What a memory stage does with a successful completion's stop reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompletionStop {
    /// The model ended its turn or matched a stop sequence: the text is the
    /// stage's answer.
    Answered,
    /// The model hit the stage's output ceiling. The body is partial (often
    /// empty); parsing it as a short answer would blame the model's JSON for
    /// a truncation.
    Truncated,
    /// The model stopped before a complete answer for any other reason: a
    /// content filter, a cancelled response, a tool call no stage offers, or
    /// a stop reason Meerkat adds later.
    Incomplete(StopReason),
}

/// Why a completion whose stream ended before its terminal event is refused:
/// with no stop reason there is no answer to read.
pub(crate) const UNTERMINATED_STREAM: &str =
    "the completion stream ended before its terminal event";

/// Read a successful completion's stop reason.
pub(crate) fn completion_stop(stop_reason: StopReason) -> CompletionStop {
    match stop_reason {
        StopReason::EndTurn | StopReason::StopSequence => CompletionStop::Answered,
        StopReason::MaxTokens => CompletionStop::Truncated,
        other => CompletionStop::Incomplete(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_diagnostics::INCOMPLETE_STOPS;

    /// How each stop reason must read. The match is exhaustive on purpose: a
    /// stop reason Meerkat adds stops this from compiling until it is
    /// classified here and, when it is not an answer, listed in
    /// `INCOMPLETE_STOPS`, which every stage's tests refuse.
    fn expected(stop_reason: StopReason) -> CompletionStop {
        match stop_reason {
            StopReason::EndTurn | StopReason::StopSequence => CompletionStop::Answered,
            StopReason::MaxTokens => CompletionStop::Truncated,
            StopReason::ContentFilter
            | StopReason::ToolUse
            | StopReason::Cancelled
            | StopReason::Refusal
            | StopReason::PauseTurn
            | StopReason::ContextWindowExceeded => CompletionStop::Incomplete(stop_reason),
        }
    }

    #[test]
    fn only_an_ended_turn_or_a_stop_sequence_is_an_answer() {
        let answered_or_truncated = [
            StopReason::EndTurn,
            StopReason::StopSequence,
            StopReason::MaxTokens,
        ];
        for stop_reason in answered_or_truncated.into_iter().chain(INCOMPLETE_STOPS) {
            assert_eq!(
                completion_stop(stop_reason),
                expected(stop_reason),
                "{stop_reason:?}"
            );
        }
        for stop_reason in INCOMPLETE_STOPS {
            assert_eq!(
                expected(stop_reason),
                CompletionStop::Incomplete(stop_reason),
                "{stop_reason:?} is listed as incomplete"
            );
        }
    }
}
