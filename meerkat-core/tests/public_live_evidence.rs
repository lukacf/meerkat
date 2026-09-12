use meerkat_core::live_execution::evidence::{
    LIVE_REQUEST_TEXT_MAX_BYTES, LiveContentDigest, LiveObservationInterval, LiveRequestEvidence,
    LiveRequestText,
};
use meerkat_core::live_execution::request::LiveRequestEvidenceKind;
use serde_json::json;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn diagnostics_preserve_known_unscoped_response_without_a_terminal_or_raw_body_field() -> TestResult
{
    use meerkat_core::live_execution::backend::LiveProviderDiagnostic;
    let value = json!({
        "category":"backend_advisory_error",
        "attribution":{"kind":"owned","response":{"response":"actual-r2","delegation":null}},
        "occurrences":1
    });
    let diagnostic: LiveProviderDiagnostic = serde_json::from_value(value.clone())?;
    assert_eq!(serde_json::to_value(diagnostic)?, value);
    for field in [
        "message",
        "raw_error",
        "instructions",
        "backend_snapshot",
        "terminal",
        "provider_consumed",
    ] {
        let mut altered = value.clone();
        altered[field] = json!("sentinel-private");
        assert!(serde_json::from_value::<LiveProviderDiagnostic>(altered).is_err());
    }
    Ok(())
}

#[test]
fn full_batch_digest_binds_scope_order_names_ids_and_exact_arguments() -> TestResult {
    use meerkat_core::live_execution::backend::{
        LiveBackendResponseKey, LiveCompletedFunctionView, LiveFunctionBatchDigest,
    };
    use meerkat_core::live_execution::request::LiveProviderReference;
    let response = LiveBackendResponseKey {
        response: LiveProviderReference::new("r")?,
        delegation: Some(LiveProviderReference::new("d")?),
    };
    let call = LiveCompletedFunctionView {
        call_id: "c",
        name: "invoke_meerkat",
        arguments: "{ \"request\":\"work\" }",
    };
    let digest = LiveFunctionBatchDigest::of(&response, std::iter::once(call))?;
    for changed in [
        LiveCompletedFunctionView {
            call_id: "other",
            ..call
        },
        LiveCompletedFunctionView {
            name: "other",
            ..call
        },
        LiveCompletedFunctionView {
            arguments: "{\"request\":\"work\"}",
            ..call
        },
    ] {
        assert_ne!(
            digest,
            LiveFunctionBatchDigest::of(&response, std::iter::once(changed))?
        );
    }
    let unscoped = LiveBackendResponseKey {
        delegation: None,
        ..response.clone()
    };
    assert_ne!(
        digest,
        LiveFunctionBatchDigest::of(&unscoped, std::iter::once(call))?
    );
    let another = LiveCompletedFunctionView {
        call_id: "second",
        ..call
    };
    assert_ne!(
        LiveFunctionBatchDigest::of(&response, [call, another].into_iter())?,
        LiveFunctionBatchDigest::of(&response, [another, call].into_iter())?
    );
    assert!(LiveFunctionBatchDigest::of(&response, [call, call].into_iter()).is_err());
    let empty = LiveFunctionBatchDigest::of(&response, std::iter::empty())?;
    assert_ne!(empty, digest);
    assert_eq!(
        serde_json::from_value::<LiveFunctionBatchDigest>(serde_json::to_value(digest)?)?,
        digest
    );
    Ok(())
}

#[test]
fn delegated_provenance_is_nonhuman_content_and_binds_exact_source_kind() -> TestResult {
    use meerkat_core::live_execution::evidence::DelegatedRequestProvenance;
    let base = json!({
        "request_id":uuid::Uuid::from_u128(1),
        "source":{"session_id":uuid::Uuid::from_u128(2),"channel_id":"voice",
            "source":{"kind":"client_delegation","delegation":"d"}},
        "evidence_kind":"application_snapshot",
        "request_digest":LiveRequestText::new(" request ")?.digest()
    });
    let value: DelegatedRequestProvenance = serde_json::from_value(base.clone())?;
    assert_eq!(serde_json::to_value(value)?, base);
    for field in ["final_user_transcript", "grant", "permission", "role"] {
        let mut changed = base.clone();
        changed[field] = json!("human");
        assert!(serde_json::from_value::<DelegatedRequestProvenance>(changed).is_err());
    }
    let mut changed = base;
    changed["evidence_kind"] = json!("structured_function_request");
    assert!(serde_json::from_value::<DelegatedRequestProvenance>(changed).is_err());
    Ok(())
}

#[test]
fn exact_request_bytes_and_digest_survive_round_trip_without_normalization() -> TestResult {
    let text = LiveRequestText::new(" \nDo the thing.\t ")?;
    let encoded = serde_json::to_vec(&text)?;
    assert_eq!(serde_json::from_slice::<LiveRequestText>(&encoded)?, text);
    assert_eq!(text.as_str(), " \nDo the thing.\t ");
    assert_ne!(
        text.digest(),
        LiveRequestText::new(text.as_str().trim())?.digest()
    );
    assert_eq!(
        serde_json::from_slice::<LiveContentDigest>(&serde_json::to_vec(&text.digest())?)?,
        text.digest()
    );
    assert!(!format!("{text:?}").contains("Do the thing"));
    Ok(())
}

#[test]
fn blank_and_over_bound_requests_reject_without_truncation() -> TestResult {
    for invalid in ["", " \n\t", "\u{2003}"] {
        assert!(LiveRequestText::new(invalid).is_err());
        assert!(serde_json::from_value::<LiveRequestText>(json!(invalid)).is_err());
    }
    let maximum = "x".repeat(LIVE_REQUEST_TEXT_MAX_BYTES);
    assert_eq!(LiveRequestText::new(maximum.clone())?.as_str(), maximum);
    assert!(LiveRequestText::new(format!("{maximum}x")).is_err());
    let unicode = "\u{1f98a}".repeat(LIVE_REQUEST_TEXT_MAX_BYTES / 4);
    assert_eq!(LiveRequestText::new(unicode.clone())?.as_str(), unicode);
    assert!(LiveRequestText::new(format!("{unicode}x")).is_err());
    Ok(())
}

#[test]
fn all_evidence_cases_are_strict_and_never_encode_final_speech_or_permission() -> TestResult {
    for (value, expected_kind) in [
        (
            json!({
                "kind": "application_snapshot",
                "observations": {"after": 0, "through": 4},
                "request": " provisional request "
            }),
            LiveRequestEvidenceKind::ApplicationSnapshot,
        ),
        (
            json!({
                "kind": "structured_function_request",
                "request": "model proposed work"
            }),
            LiveRequestEvidenceKind::StructuredFunctionRequest,
        ),
    ] {
        let evidence: LiveRequestEvidence = serde_json::from_value(value.clone())?;
        assert_eq!(evidence.kind(), expected_kind);
        assert_eq!(serde_json::to_value(evidence)?, value);
        for field in ["final", "grant", "executor", "tools", "policy"] {
            let mut altered = value.clone();
            altered[field] = json!(true);
            assert!(serde_json::from_value::<LiveRequestEvidence>(altered).is_err());
        }
    }
    assert!(
        serde_json::from_value::<LiveRequestEvidence>(
            json!({"kind": "final_user_transcript", "request": "not human proof"})
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn observation_intervals_preserve_empty_refusals_and_reject_reversal() -> TestResult {
    for (after, through) in [(0, 0), (3, 7), (u64::MAX, u64::MAX)] {
        let interval = LiveObservationInterval::new(after, through)?;
        assert_eq!(interval.after(), after);
        assert_eq!(interval.through(), through);
        assert_eq!(interval.is_empty(), after == through);
        assert_eq!(
            serde_json::from_slice::<LiveObservationInterval>(&serde_json::to_vec(&interval)?)?,
            interval
        );
    }
    for invalid in [
        json!({"after": 10, "through": 9}),
        json!({"after": 0, "through": 1, "received_watermark": 99}),
    ] {
        assert!(serde_json::from_value::<LiveObservationInterval>(invalid).is_err());
    }
    Ok(())
}
