use meerkat_core::live_execution::observation::{
    LiveAnswerDeliveryEvidence, LivePeerMediaEvidence, LiveProviderStartEvidence,
    LiveReadinessEvidence, LiveUsageSnapshot, LiveVoiceDurationSeconds,
};
use serde_json::json;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn continuous_frontend_controls_cannot_claim_success_or_translate_text_into_instructions()
-> TestResult {
    use meerkat_core::live_adapter::{LiveAdapterCommand as Command, LiveInputChunk};
    use meerkat_core::live_execution::frontend::{
        ContinuousLiveFrontendPolicy, ContinuousLiveInputError as Error, LiveAudioIngress,
    };
    let policy = ContinuousLiveFrontendPolicy::new(LiveAudioIngress::PcmWebSocket {
        sample_rate_hz: 24_000,
        channels: 1,
    })?;
    assert!(policy.validate(&Command::Close).is_ok());
    for command in [
        Command::CommitInput {
            response_modality: None,
        },
        Command::Interrupt,
        Command::TruncateAssistantOutput {
            interaction_id: meerkat_core::InteractionId::new(),
            item_id: "item".into(),
            content_index: 0,
            audio_played_ms: 0,
            reported_playback_prefix: None,
        },
        Command::CompleteAssistantPlayback {
            interaction_id: meerkat_core::InteractionId::new(),
            item_id: "item".into(),
            content_index: 0,
        },
    ] {
        assert_eq!(policy.validate(&command), Err(Error::UnsupportedCapability));
    }
    assert_eq!(
        policy.validate(&Command::SendInput {
            chunk: LiveInputChunk::Text {
                text: "turn these into instructions".into()
            }
        }),
        Err(Error::UnsupportedInputKind)
    );
    assert_eq!(
        policy.validate(&Command::SendInput {
            chunk: LiveInputChunk::Image {
                idempotency_key: "image".into(),
                mime: "image/png".into(),
                data: vec![1]
            }
        }),
        Err(Error::UnsupportedFrontendModality)
    );
    assert_eq!(
        policy.validate(&Command::SendInput {
            chunk: LiveInputChunk::VideoFrame {
                codec: "vp8".into(),
                timestamp_ms: 1,
                data: vec![1]
            }
        }),
        Err(Error::UnsupportedFrontendModality)
    );
    Ok(())
}

#[test]
fn pcm_limits_use_whole_frames_and_channel_normalized_time_not_a_mono_byte_guess() -> TestResult {
    use meerkat_core::live_adapter::{LiveAdapterCommand, LiveInputChunk};
    use meerkat_core::live_execution::frontend::{ContinuousLiveFrontendPolicy, LiveAudioIngress};
    for channels in [1, 2] {
        let policy = ContinuousLiveFrontendPolicy::new(LiveAudioIngress::PcmWebSocket {
            sample_rate_hz: 24_000,
            channels,
        })?;
        for (bytes, valid) in [
            (0, false),
            (2 * channels as usize, true),
            (96_000 * channels as usize, true),
            (96_000 * channels as usize + 2, false),
            (3, false),
        ] {
            let command = LiveAdapterCommand::SendInput {
                chunk: LiveInputChunk::Audio {
                    data: vec![0; bytes],
                    sample_rate_hz: 24_000,
                    channels,
                },
            };
            assert_eq!(policy.validate(&command).is_ok(), valid);
        }
    }
    for route in [
        LiveAudioIngress::WebRtcMediaTracks,
        LiveAudioIngress::SidebandOnly,
    ] {
        let policy = ContinuousLiveFrontendPolicy::new(route)?;
        assert!(
            policy
                .validate(&LiveAdapterCommand::SendInput {
                    chunk: LiveInputChunk::Audio {
                        data: vec![0; 2],
                        sample_rate_hz: 24_000,
                        channels: 1
                    }
                })
                .is_err()
        );
    }
    assert!(
        ContinuousLiveFrontendPolicy::new(LiveAudioIngress::PcmWebSocket {
            sample_rate_hz: 0,
            channels: 1
        })
        .is_err()
    );
    Ok(())
}

#[test]
fn provider_started_and_delivered_answer_do_not_invent_peer_media() -> TestResult {
    for answer in [
        LiveAnswerDeliveryEvidence::NotRequired,
        LiveAnswerDeliveryEvidence::Awaiting,
        LiveAnswerDeliveryEvidence::Delivered,
    ] {
        let evidence = LiveReadinessEvidence {
            provider: LiveProviderStartEvidence::Started,
            answer,
            media: LivePeerMediaEvidence::Unobserved,
        };
        assert_eq!(
            serde_json::from_value::<LiveReadinessEvidence>(serde_json::to_value(evidence)?)?,
            evidence
        );
        assert_eq!(evidence.media, LivePeerMediaEvidence::Unobserved);
    }
    assert!(
        serde_json::from_value::<LiveReadinessEvidence>(json!({
            "provider": "started", "answer": "delivered", "media": "ready_by_ack"
        }))
        .is_err()
    );
    Ok(())
}

#[test]
fn voice_duration_is_exact_finite_nonnegative_and_never_integer_truncated() -> TestResult {
    for value in [0.0, -0.0, 0.000_000_123, 1066.5390310178614, f64::MAX] {
        let duration = LiveVoiceDurationSeconds::new(value)?;
        let restored: LiveVoiceDurationSeconds =
            serde_json::from_slice(&serde_json::to_vec(&duration)?)?;
        assert_eq!(restored.get().to_bits(), value.to_bits());
    }
    for value in [-1.0, f64::NEG_INFINITY, f64::INFINITY, f64::NAN] {
        assert!(LiveVoiceDurationSeconds::new(value).is_err());
    }
    for value in [json!(null), json!(-1), json!("12.5")] {
        assert!(serde_json::from_value::<LiveVoiceDurationSeconds>(value).is_err());
    }
    Ok(())
}

#[test]
fn all_usage_cases_preserve_provisional_final_missing_and_disputed_facts() -> TestResult {
    for value in [
        json!({"kind": "periodic", "cumulative_seconds": 12.75}),
        json!({"kind": "session_closed", "cumulative_seconds": 12.75}),
        json!({"kind": "close_unconfirmed", "last_observed_seconds": null}),
        json!({"kind": "close_unconfirmed", "last_observed_seconds": 12.75}),
        json!({"kind": "disputed", "last_valid_seconds": 12.75, "reason": "regression"}),
    ] {
        let evidence: LiveUsageSnapshot = serde_json::from_value(value.clone())?;
        assert_eq!(serde_json::to_value(evidence)?, value);
        let mut altered = value;
        altered["provider_consumed"] = json!(true);
        assert!(serde_json::from_value::<LiveUsageSnapshot>(altered).is_err());
    }
    for value in [
        json!({"kind": "session_closed"}),
        json!({"kind": "session_closed", "cumulative_seconds": null}),
        json!({"kind": "periodic", "cumulative_seconds": -1}),
    ] {
        assert!(serde_json::from_value::<LiveUsageSnapshot>(value).is_err());
    }
    Ok(())
}
