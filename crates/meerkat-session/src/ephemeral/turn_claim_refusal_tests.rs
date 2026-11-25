//! Rows for the pair a refused turn claim returns (`refused_turn_claim`).
//!
//! A not-found answer does not prove that the turn did not run. The reserved
//! admission handed back with the answer is that proof, so a claim refused
//! with not-found, which took nothing, hands it back.
#![allow(clippy::expect_used, clippy::panic)]

use super::{SessionError, SessionId, refused_turn_claim};

/// Stands in for the reserved admission: the rows read only where it goes.
#[derive(Debug, PartialEq, Eq)]
struct Admission(u8);

#[test]
fn a_not_found_claim_refusal_hands_the_reserved_admission_back() {
    let id = SessionId::new();
    let mut reserved = Some(Admission(7));

    let (error, returned) =
        refused_turn_claim(SessionError::NotFound { id: id.clone() }, &mut reserved);

    assert!(
        matches!(&error, SessionError::NotFound { id: answered } if *answered == id),
        "the claim's own error is returned unchanged, got {error:?}"
    );
    assert_eq!(
        returned,
        Some(Admission(7)),
        "a claim refused with not-found took nothing: its caller gets the admission back"
    );
    assert_eq!(reserved, None, "the admission is handed back, not kept");
}

#[test]
fn a_not_found_claim_refusal_without_a_reserved_admission_returns_none() {
    let id = SessionId::new();
    let mut reserved: Option<Admission> = None;

    let (error, returned) =
        refused_turn_claim(SessionError::NotFound { id: id.clone() }, &mut reserved);

    assert!(
        matches!(&error, SessionError::NotFound { id: answered } if *answered == id),
        "the claim's own error is returned unchanged, got {error:?}"
    );
    assert_eq!(returned, None, "there was no admission to hand back");
}

#[test]
fn a_busy_claim_refusal_answers_without_the_admission() {
    let id = SessionId::new();
    let mut reserved = Some(Admission(7));

    let (error, returned) =
        refused_turn_claim(SessionError::Busy { id: id.clone() }, &mut reserved);

    assert!(
        matches!(&error, SessionError::Busy { id: answered } if *answered == id),
        "the claim's own error is returned unchanged, got {error:?}"
    );
    assert_eq!(returned, None, "a busy refusal answers as it did");
    assert_eq!(
        reserved,
        Some(Admission(7)),
        "the admission stays where it was reserved"
    );
}
