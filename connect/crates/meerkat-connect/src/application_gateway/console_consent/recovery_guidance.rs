//! A read-only explanation of an observed withdrawal, never retry authority.
use super::*;
use meerkat_core::approval::{
    ApprovalConsentCustody, ApprovalConsentRecord, ApprovalConsentWithdrawalReason, ApprovalStatus,
};

#[derive(serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum Withdrawal {
    ContextChanged,
    NoticeInputEnded,
}

#[derive(serde::Serialize)]
struct Guidance {
    action: &'static str,
    withdrawal: Withdrawal,
    new_request_required: bool,
    new_human_approval_required: bool,
    automatic_retry: bool,
    original_liveness: &'static str,
    other_effects: &'static str,
}

fn observed(status: ApprovalStatus, consent: Option<&ApprovalConsentRecord>) -> Option<Guidance> {
    if !matches!(status, ApprovalStatus::Cancelled | ApprovalStatus::Approved) {
        return None;
    }
    let consent = consent?;
    if consent.consumed_at.is_some()
        || matches!(
            consent.custody.as_ref(),
            Some(ApprovalConsentCustody::Transferred { .. })
        )
    {
        return None;
    }
    let withdrawal = match &consent.withdrawal.as_ref()?.reason {
        ApprovalConsentWithdrawalReason::ContextChanged => Withdrawal::ContextChanged,
        ApprovalConsentWithdrawalReason::NoticeInputEnded { .. }
            if matches!(
                consent.custody.as_ref(),
                Some(ApprovalConsentCustody::Parked)
            ) =>
        {
            Withdrawal::NoticeInputEnded
        }
        _ => return None,
    };
    Some(Guidance {
        action: "review_current_state_then_request_again",
        withdrawal,
        new_request_required: true,
        new_human_approval_required: true,
        automatic_retry: false,
        original_liveness: "not_established",
        other_effects: "not_established",
    })
}

pub(super) fn read_response(result: Result<ApprovalRecord, HostLifetimeError>) -> Response {
    let record = match result {
        Ok(record) => record,
        Err(error) => return response(Err(error)),
    };
    let Some(guidance) = observed(record.status, record.consent.as_ref()) else {
        return response(Ok(record));
    };
    // The caller has already checked the actual issuer row, human, access,
    // selected route and current proof. This snapshot can become stale; its
    // explanation never grants admission or authorizes another dispatch.
    (
        StatusCode::OK,
        [
            ("cache-control", "no-store"),
            ("referrer-policy", "no-referrer"),
            ("x-content-type-options", "nosniff"),
        ],
        Json(serde_json::json!({"approval":record, "recovery":guidance})),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn withdrawn(reason: serde_json::Value) -> ApprovalConsentRecord {
        serde_json::from_value(serde_json::json!({
            "withdrawal":{"reason":reason,"withdrawn_at":"2026-10-29T00:00:00Z"},
            "custody":{"state":"parked"}
        }))
        .unwrap()
    }
    #[test]
    fn guidance_requires_exact_unspent_withdrawal_and_never_grants_retry() {
        for (reason, label) in [
            (serde_json::json!("context_changed"), "context_changed"),
            (
                serde_json::json!({"notice_input_ended":{"input_id":"private-input-not-projected"}}),
                "notice_input_ended",
            ),
        ] {
            let mut consent = withdrawn(reason);
            let value =
                serde_json::to_value(observed(ApprovalStatus::Approved, Some(&consent)).unwrap())
                    .unwrap();
            assert_eq!(value["withdrawal"], label);
            assert_eq!(value["automatic_retry"], false);
            assert_eq!(value["new_request_required"], true);
            assert_eq!(value["new_human_approval_required"], true);
            assert_eq!(value["original_liveness"], "not_established");
            assert_eq!(value["other_effects"], "not_established");
            assert!(!value.to_string().contains("private-input"));
            consent.consumed_at = Some(consent.withdrawal.as_ref().unwrap().withdrawn_at);
            assert!(observed(ApprovalStatus::Approved, Some(&consent)).is_none());
            consent.consumed_at = None;
            consent.custody = Some(ApprovalConsentCustody::Transferred {
                input_id: "moved".into(),
            });
            assert!(observed(ApprovalStatus::Approved, Some(&consent)).is_none());
        }
    }
    #[test]
    fn unrelated_or_missing_withdrawal_and_nonparked_notice_offer_no_guidance() {
        for reason in ["run_ended", "authority_changed", "expired", "abandoned"] {
            assert!(
                observed(
                    ApprovalStatus::Cancelled,
                    Some(&withdrawn(serde_json::json!(reason)))
                )
                .is_none()
            );
        }
        assert!(observed(ApprovalStatus::Cancelled, None).is_none());
        assert!(
            observed(
                ApprovalStatus::Approved,
                Some(&ApprovalConsentRecord::default())
            )
            .is_none()
        );
        let mut consent = withdrawn(serde_json::json!({"notice_input_ended":{"input_id":"ended"}}));
        consent.custody = None;
        assert!(observed(ApprovalStatus::Cancelled, Some(&consent)).is_none());
        consent = withdrawn(serde_json::json!("context_changed"));
        for status in [
            ApprovalStatus::Pending,
            ApprovalStatus::Denied,
            ApprovalStatus::Expired,
        ] {
            assert!(observed(status, Some(&consent)).is_none());
        }
        // A live-run ContextChanged withdrawal need not have been parked.
        consent.custody = None;
        assert!(observed(ApprovalStatus::Cancelled, Some(&consent)).is_some());
    }
}
