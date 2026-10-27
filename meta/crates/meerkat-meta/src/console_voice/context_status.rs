//! Public-safe, receipt-free projection of shared live context preparation.

use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct VoiceContextStatusRequest {
    pub identity: String,
    pub request_id: String,
    pub channel_id: String,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct VoiceContextStatus {
    pub identity: String,
    pub request_id: String,
    pub channel_id: String,
    pub context_preparation: VoiceContextPreparation,
}

#[cfg_attr(not(feature = "openai-live"), allow(dead_code))]
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub(crate) enum VoiceContextPreparation {
    NotRequested,
    Preparing { stage: VoiceContextStage },
    ProviderAcknowledged,
    Failed { reason: VoiceContextFailure },
}

#[cfg_attr(not(feature = "openai-live"), allow(dead_code))]
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum VoiceContextStage {
    Capturing,
    Generating,
    Delivering,
}

#[cfg_attr(not(feature = "openai-live"), allow(dead_code))]
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum VoiceContextFailure {
    Capture,
    Generation,
    TimedOut,
    InputTooLarge,
    OutputTooLarge,
    Empty,
    StaleSnapshot,
    Unsupported,
    SourceRead,
    ProducerPanicked,
    DeliveryRejected,
    DeliveryAmbiguous,
    Cancelled,
    AuthorityRejected,
}

#[cfg(feature = "openai-live")]
impl From<&meerkat::surface::LiveContextPreparationStatus> for VoiceContextPreparation {
    fn from(status: &meerkat::surface::LiveContextPreparationStatus) -> Self {
        use meerkat::surface::{
            LiveContextPreparationFailure as Failure, LiveContextPreparationStage as Stage,
            LiveContextPreparationStatus as Status,
        };
        match status {
            Status::NotRequested => Self::NotRequested,
            Status::ProviderAcknowledged => Self::ProviderAcknowledged,
            Status::Preparing(stage) => Self::Preparing {
                stage: match stage {
                    Stage::Capturing => VoiceContextStage::Capturing,
                    Stage::Generating => VoiceContextStage::Generating,
                    Stage::Delivering => VoiceContextStage::Delivering,
                },
            },
            Status::Failed(failure) => Self::Failed {
                reason: match failure {
                    Failure::Capture => VoiceContextFailure::Capture,
                    Failure::Generation => VoiceContextFailure::Generation,
                    Failure::TimedOut => VoiceContextFailure::TimedOut,
                    Failure::InputTooLarge => VoiceContextFailure::InputTooLarge,
                    Failure::OutputTooLarge => VoiceContextFailure::OutputTooLarge,
                    Failure::Empty => VoiceContextFailure::Empty,
                    Failure::StaleSnapshot => VoiceContextFailure::StaleSnapshot,
                    Failure::Unsupported => VoiceContextFailure::Unsupported,
                    Failure::SourceRead => VoiceContextFailure::SourceRead,
                    Failure::ProducerPanicked => VoiceContextFailure::ProducerPanicked,
                    Failure::DeliveryRejected => VoiceContextFailure::DeliveryRejected,
                    Failure::DeliveryAmbiguous => VoiceContextFailure::DeliveryAmbiguous,
                    Failure::Cancelled => VoiceContextFailure::Cancelled,
                    Failure::AuthorityRejected => VoiceContextFailure::AuthorityRejected,
                },
            },
        }
    }
}

#[cfg(all(test, feature = "openai-live"))]
mod tests {
    use super::*;
    use meerkat::surface::{
        LiveContextPreparationFailure as Failure, LiveContextPreparationStatus as Status,
    };

    #[test]
    fn shared_context_status_fixture_matches_rust_request_and_native_projection()
    -> Result<(), Box<dyn std::error::Error>> {
        use meerkat::surface::LiveContextPreparationStage as Stage;
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/console_voice_v1.json"))?;
        assert_eq!(
            fixture["context_status_method"],
            super::super::VOICE_CONTEXT_STATUS_METHOD
        );
        let request: VoiceContextStatusRequest =
            serde_json::from_value(fixture["context_status_request"].clone())?;
        assert_eq!(
            fixture["context_status_request"],
            serde_json::json!({
                "identity": request.identity, "request_id": request.request_id, "channel_id": request.channel_id,
            })
        );
        let project = |status: Status| {
            serde_json::to_value(VoiceContextStatus {
                identity: request.identity.clone(),
                request_id: request.request_id.clone(),
                channel_id: request.channel_id.clone(),
                context_preparation: VoiceContextPreparation::from(&status),
            })
        };
        let cases = [
            ("not_requested", Status::NotRequested),
            ("capturing", Status::Preparing(Stage::Capturing)),
            ("generating", Status::Preparing(Stage::Generating)),
            ("delivering", Status::Preparing(Stage::Delivering)),
            ("provider_acknowledged", Status::ProviderAcknowledged),
            ("failed", Status::Failed(Failure::Generation)),
        ];
        assert_eq!(
            fixture["context_status_responses"]
                .as_object()
                .map(serde_json::Map::len),
            Some(cases.len())
        );
        for (name, status) in cases {
            assert_eq!(
                project(status)?,
                fixture["context_status_responses"][name],
                "{name}"
            );
        }
        let reasons = [
            Failure::Capture,
            Failure::Generation,
            Failure::TimedOut,
            Failure::InputTooLarge,
            Failure::OutputTooLarge,
            Failure::Empty,
            Failure::StaleSnapshot,
            Failure::Unsupported,
            Failure::SourceRead,
            Failure::ProducerPanicked,
            Failure::DeliveryRejected,
            Failure::DeliveryAmbiguous,
            Failure::Cancelled,
            Failure::AuthorityRejected,
        ];
        let mut projected_reasons = Vec::new();
        for reason in reasons {
            let actual = project(Status::Failed(reason))?;
            let reason = actual["context_preparation"]["reason"].clone();
            let mut expected = fixture["context_status_responses"]["failed"].clone();
            expected["context_preparation"]["reason"] = reason.clone();
            assert_eq!(actual, expected);
            projected_reasons.push(reason);
        }
        assert_eq!(
            serde_json::json!(projected_reasons),
            fixture["context_status_failure_reasons"]
        );
        Ok(())
    }

    #[test]
    fn context_phases_and_stages_have_exact_receipt_free_shapes() {
        use meerkat::surface::LiveContextPreparationStage as Stage;
        for (status, expected) in [
            (
                Status::NotRequested,
                serde_json::json!({"phase":"not_requested"}),
            ),
            (
                Status::Preparing(Stage::Capturing),
                serde_json::json!({"phase":"preparing","stage":"capturing"}),
            ),
            (
                Status::Preparing(Stage::Generating),
                serde_json::json!({"phase":"preparing","stage":"generating"}),
            ),
            (
                Status::Preparing(Stage::Delivering),
                serde_json::json!({"phase":"preparing","stage":"delivering"}),
            ),
            (
                Status::ProviderAcknowledged,
                serde_json::json!({"phase":"provider_acknowledged"}),
            ),
        ] {
            assert_eq!(
                serde_json::to_value(VoiceContextPreparation::from(&status)).ok(),
                Some(expected)
            );
        }
    }

    #[test]
    fn context_failures_project_only_the_complete_typed_allowlist() {
        for (reason, expected) in [
            (Failure::Capture, "capture"),
            (Failure::Generation, "generation"),
            (Failure::TimedOut, "timed_out"),
            (Failure::InputTooLarge, "input_too_large"),
            (Failure::OutputTooLarge, "output_too_large"),
            (Failure::Empty, "empty"),
            (Failure::StaleSnapshot, "stale_snapshot"),
            (Failure::Unsupported, "unsupported"),
            (Failure::SourceRead, "source_read"),
            (Failure::ProducerPanicked, "producer_panicked"),
            (Failure::DeliveryRejected, "delivery_rejected"),
            (Failure::DeliveryAmbiguous, "delivery_ambiguous"),
            (Failure::Cancelled, "cancelled"),
        ] {
            assert_eq!(
                serde_json::to_value(VoiceContextPreparation::from(&Status::Failed(reason))).ok(),
                Some(serde_json::json!({"phase":"failed","reason":expected})),
            );
        }
    }
}
