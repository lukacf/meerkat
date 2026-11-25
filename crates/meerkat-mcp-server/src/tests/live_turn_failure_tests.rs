//! Rows for what the plain resume does with a failed turn on the live
//! session (`live_turn_failure`).
//!
//! A not-found answer does not prove that the turn did not run. The session
//! service hands the turn's reserved admission back with an error when the
//! turn did not take it, so the resume submits the prompt again only on a
//! not-found answer that came with that admission. Every other failure is
//! the resume's result, in the tool error form a failed turn already has.
#![allow(clippy::expect_used, clippy::panic)]

use crate::{LiveTurnFailure, SessionError, format_agent_result_tool, live_turn_failure};

/// Stands in for the reserved admission: the rows read only where it goes.
#[derive(Debug, PartialEq, Eq)]
struct Admission(u8);

#[test]
fn a_not_found_answer_with_the_returned_admission_rebuilds_on_it() {
    let id = meerkat::SessionId::new();

    let outcome = live_turn_failure(SessionError::NotFound { id }, Some(Admission(7)));

    let LiveTurnFailure::Rebuild(admission) = outcome else {
        panic!("a not-found answer with its admission returned must rebuild the session");
    };
    assert_eq!(
        admission,
        Admission(7),
        "the rebuild runs on the admission that came back"
    );
}

#[test]
fn a_not_found_answer_without_the_returned_admission_is_surfaced() {
    let id = meerkat::SessionId::new();

    let outcome = live_turn_failure(SessionError::NotFound { id: id.clone() }, None::<Admission>);

    let LiveTurnFailure::Surface(error) = outcome else {
        panic!(
            "a not-found answer without its admission returned must be the resume's result: \
             nothing is submitted again"
        );
    };
    assert!(
        matches!(&error, SessionError::NotFound { id: answered } if *answered == id),
        "the session service's own answer is surfaced unchanged, got {error:?}"
    );
}

#[test]
fn any_other_failure_is_surfaced_with_or_without_the_admission() {
    let id = meerkat::SessionId::new();
    let failures: [(SessionError, Option<Admission>); 3] = [
        (SessionError::Busy { id: id.clone() }, Some(Admission(7))),
        (SessionError::Busy { id: id.clone() }, None),
        (SessionError::Agent(meerkat::AgentError::Cancelled), None),
    ];

    for (failure, returned_admission) in failures {
        let expected = failure.to_string();
        let outcome = live_turn_failure(failure, returned_admission);
        let LiveTurnFailure::Surface(error) = outcome else {
            panic!("{expected}: a failure other than not-found is the resume's result");
        };
        assert_eq!(
            error.to_string(),
            expected,
            "the failure is surfaced unchanged"
        );
    }
}

/// The surfaced not-found answer reaches the tool caller in the form every
/// other failed turn has: the internal-error code, the session error's own
/// text, no data.
#[test]
fn a_surfaced_not_found_answer_is_an_ordinary_failed_turn_tool_error() {
    let id = meerkat::SessionId::new();

    let error = format_agent_result_tool(Err(SessionError::NotFound { id: id.clone() }), &id)
        .expect_err("a failed turn is a tool error");

    assert_eq!(error.code, -32603);
    assert_eq!(
        error.message,
        format!("Agent error: session not found: {id}")
    );
    assert!(error.data.is_none(), "no typed data: {:?}", error.data);
}
