//! Rows for the session service's check that a turn's reasoning effort
//! level was applied before the turn (`unapplied_turn_reasoning_effort`).
#![allow(clippy::expect_used, clippy::panic)]

use super::{TURN_REASONING_EFFORT_NOT_APPLIED, unapplied_turn_reasoning_effort};
use meerkat_core::error::AgentError;
use meerkat_core::lifecycle::run_primitive::TurnMetadataOverride;
use meerkat_core::model_profile::capabilities::EffortLevel;

/// A turn that names no level, or the state the session is already at,
/// goes on. Any other turn is refused with a configuration error that says
/// what the turn asked for and where the session is.
#[test]
fn a_turn_level_the_session_is_not_at_is_refused() {
    use EffortLevel::{High, Low};
    use TurnMetadataOverride::{Clear, Set};

    let applied: [(
        Option<TurnMetadataOverride<EffortLevel>>,
        Option<EffortLevel>,
    ); 5] = [
        (None, None),
        (None, Some(High)),
        (Some(Set(High)), Some(High)),
        (Some(Clear), None),
        (Some(Set(Low)), Some(Low)),
    ];
    for (turn, session) in applied {
        assert!(
            unapplied_turn_reasoning_effort(turn.as_ref(), session).is_none(),
            "{turn:?} on a session at {session:?} was applied"
        );
    }

    let not_applied = [
        (Set(High), None, "'high'", "'none'"),
        (Set(High), Some(Low), "'high'", "'low'"),
        (Clear, Some(High), "'none'", "'high'"),
    ];
    for (turn, session, asked, at) in not_applied {
        let refusal = unapplied_turn_reasoning_effort(Some(&turn), session)
            .unwrap_or_else(|| panic!("{turn:?} on a session at {session:?} must be refused"));
        let AgentError::ConfigError(message) = &refusal else {
            panic!("the refusal is a configuration error: {refusal:?}");
        };
        assert!(
            message.starts_with(TURN_REASONING_EFFORT_NOT_APPLIED),
            "{message}"
        );
        assert!(message.contains(asked) && message.contains(at), "{message}");
    }
}
