//! Inert caller projections of native observations, not native admission tests.
use super::*;
use meerkat_core::{
    RunId, SessionId,
    lifecycle::InputId,
    retained_work::{RetainedContributorRef, RetainedWorkIdentity},
};

fn binding() -> NativeOriginalTask {
    let session = SessionId::new();
    NativeOriginalTask {
        retained_work: RetainedWorkIdentity::new(
            format!("rt:session:{session}"),
            RunId::new(),
            vec![RetainedContributorRef {
                input_id: InputId::new(),
                association_digest: "fixture-association".into(),
                submission_digest: "fixture-input".into(),
            }],
            Default::default(),
            None,
        ),
        origin_session_id: session,
        member: Some(MobMemberBinding {
            mob_id: "fixture-mob".into(),
            role: "original".into(),
            member: "fixture-member".into(),
        }),
    }
}

fn receipt(binding: &NativeOriginalTask) -> ContinuationReceipt {
    ContinuationReceipt {
        owner: InstalledNativeContinuation::owner(binding),
        address: "fixture-original-incarnation".into(),
        key: ContinuationKey::new("host:setup:fixture-operation").unwrap(),
        delivery_id: "fixture-native-delivery".into(),
        submission_digest: "native-owned-fixture-digest".into(),
        committed_at_ms: 20,
    }
}

fn matched(receipt: &ContinuationReceipt, status: ContinuationStatus) -> ContinuationObservation {
    ContinuationObservation::Matched {
        receipt: receipt.clone(),
        status,
    }
}

#[test]
fn only_exact_not_submitted_is_retry_safe_absence() {
    let receipt = receipt(&binding());
    assert_eq!(
        InstalledNativeContinuation::observed_status(
            &receipt.owner,
            &receipt.key,
            Ok(ContinuationObservation::NotSubmitted),
        )
        .unwrap(),
        ContinuationStatus::NotCommitted,
    );
    // A matched binding paired with an absent status is contradictory. It
    // must not turn into the reconciliation branch for exact-key retry.
    assert!(matches!(
        InstalledNativeContinuation::observed_status(
            &receipt.owner,
            &receipt.key,
            Ok(matched(&receipt, ContinuationStatus::NotCommitted)),
        ),
        Err(ContinuationError::OwnerUnavailable),
    ));
}

#[test]
fn same_key_other_content_never_becomes_this_admission() {
    let binding = binding();
    let existing = receipt(&binding);
    // The foreign receipt may already have been Applied. Its status must
    // never be queried or used after native returns ContentMismatch.
    assert!(matches!(
        InstalledNativeContinuation::observed_status(
            &existing.owner,
            &existing.key,
            Err(ContinuationObserveError::ContentMismatch {
                existing: Box::new(existing.clone()),
            }),
        ),
        Err(ContinuationError::InvalidReceipt),
    ));
    assert!(matches!(
        InstalledNativeContinuation::submit_error(ContinuationSubmitError::Conflict {
            existing: Box::new(existing),
        }),
        ContinuationError::InvalidReceipt,
    ));
}

#[test]
fn invalid_and_unavailable_native_observations_are_not_absence() {
    let receipt = receipt(&binding());
    let cases = [
        (
            ContinuationObserveError::Invalid("private detail".into()),
            true,
        ),
        (
            ContinuationObserveError::Unavailable("private detail".into()),
            false,
        ),
        (
            ContinuationObserveError::Inconsistent("private detail".into()),
            false,
        ),
    ];
    for (error, invalid) in cases {
        let error =
            InstalledNativeContinuation::observed_status(&receipt.owner, &receipt.key, Err(error))
                .unwrap_err();
        if invalid {
            assert!(matches!(&error, ContinuationError::InvalidReceipt));
        } else {
            assert!(matches!(&error, ContinuationError::OwnerUnavailable));
        }
        assert!(!error.to_string().contains("private detail"));
    }
}

#[test]
fn matching_status_preserves_pending_and_terminal_meanings() {
    let binding = binding();
    let receipt = receipt(&binding);
    let statuses = [
        ContinuationStatus::Pending {
            receipt: receipt.clone(),
            sequence: 1,
            admitted: None,
        },
        ContinuationStatus::Pending {
            receipt: receipt.clone(),
            sequence: 1,
            admitted: Some((binding.origin_session_id.clone(), InputId::new())),
        },
        ContinuationStatus::Applied {
            receipt: receipt.clone(),
            session: binding.origin_session_id.clone(),
            input: InputId::new(),
        },
        ContinuationStatus::Stranded {
            receipt: receipt.clone(),
            cause: meerkat::StrandedCause::OwnerRetired,
        },
        ContinuationStatus::Refused {
            receipt: receipt.clone(),
            sequence: 1,
            reason: meerkat_runtime::RuntimeDeliveryRefusalReason::AuthorityDenied,
        },
    ];
    for status in statuses {
        assert_eq!(
            InstalledNativeContinuation::observed_status(
                &receipt.owner,
                &receipt.key,
                Ok(matched(&receipt, status.clone())),
            )
            .unwrap(),
            status,
        );
    }
}

#[test]
fn mismatched_receipt_owner_key_or_status_is_refused() {
    let binding = binding();
    let expected = receipt(&binding);
    for change in ["owner", "key", "status"] {
        let mut outer = expected.clone();
        let mut inner = expected.clone();
        match change {
            "owner" => {
                outer.owner = ContinuationOwner::Session {
                    session_id: SessionId::new(),
                };
                inner = outer.clone();
            }
            "key" => {
                outer.key = ContinuationKey::new("host:setup:other-operation").unwrap();
                inner = outer.clone();
            }
            "status" => inner.submission_digest = "another-native-receipt".into(),
            _ => unreachable!(),
        }
        assert!(matches!(
            InstalledNativeContinuation::observed_status(
                &expected.owner,
                &expected.key,
                Ok(matched(
                    &outer,
                    ContinuationStatus::Applied {
                        receipt: inner,
                        session: binding.origin_session_id.clone(),
                        input: InputId::new(),
                    }
                )),
            ),
            Err(ContinuationError::InvalidReceipt),
        ));
    }
}

#[test]
fn member_admission_compares_the_original_session_not_a_current_seat() {
    let binding = binding();
    let receipt = receipt(&binding);
    assert!(
        InstalledNativeContinuation::check_applied_binding(
            &binding,
            receipt.key.as_str(),
            10,
            &receipt,
            &binding.origin_session_id,
            30,
        )
        .is_ok()
    );
    // Stable Member identity can survive a respawn. Native exact-content
    // observation does not compare its job session, so Applied must do so.
    assert!(matches!(
        InstalledNativeContinuation::check_applied_binding(
            &binding,
            receipt.key.as_str(),
            10,
            &receipt,
            &SessionId::new(),
            30,
        ),
        Err(ContinuationError::InvalidReceipt),
    ));
}

#[test]
fn historical_match_is_not_current_submission_permission() {
    let binding = binding();
    let receipt = receipt(&binding);
    let applied = ContinuationStatus::Applied {
        receipt: receipt.clone(),
        session: binding.origin_session_id.clone(),
        input: InputId::new(),
    };
    assert_eq!(
        InstalledNativeContinuation::observed_status(
            &receipt.owner,
            &receipt.key,
            Ok(matched(&receipt, applied.clone())),
        )
        .unwrap(),
        applied
    );
    // A historical match and a refusal to submit now are distinct facts. The
    // real submit remains native-owned; this fixture grants no live authority.
    for denied in [
        ContinuationSubmitError::OwnerRetired,
        ContinuationSubmitError::NoAdmissibleWorkBinding,
    ] {
        assert!(matches!(
            InstalledNativeContinuation::submit_error(denied),
            ContinuationError::Denied
        ));
    }
    assert!(matches!(
        InstalledNativeContinuation::submit_error(ContinuationSubmitError::Invalid(
            "private detail".into()
        )),
        ContinuationError::InvalidReceipt,
    ));
    assert!(matches!(
        InstalledNativeContinuation::submit_error(ContinuationSubmitError::Unavailable(
            "private detail".into()
        )),
        ContinuationError::OwnerUnavailable,
    ));
}
