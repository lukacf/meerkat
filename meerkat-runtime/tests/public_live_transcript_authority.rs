//! Generated-rule tests only; durable realization is qualified separately.

use meerkat_runtime::live_ledger::transcript_authority::dsl::{
    LiveTranscriptEffect as Effect, LiveTranscriptInput as Input,
    LiveTranscriptMachineAuthority as Authority, LiveTranscriptMachineMutator as Mutator,
    LiveTranscriptMachinePreparedAuthority as Prepared,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn activated() -> Result<Prepared, Box<dyn std::error::Error>> {
    let mut owner = Authority::new().prepare_authority();
    Mutator::apply(
        &mut owner,
        Input::ActivateChannel {
            channel: "voice".into(),
            sequence: 10,
            voice_accounting: false,
            ingress_generation: 1,
            credit_records: 2,
            credit_bytes: 2048,
            maximum_record_charge: 1024,
        },
    )?;
    Ok(owner)
}

#[test]
fn generated_transcript_append_requires_contiguous_receive_identity() -> TestResult {
    let mut owner = activated()?;
    for (channel, sequence, receive_ordinal, ingress_generation) in [
        ("foreign", 11, 1, 1),
        ("voice", 10, 1, 1),
        ("voice", 11, 2, 1),
        ("voice", 11, 1, 2),
    ] {
        let before = owner.state().clone();
        assert!(
            Mutator::apply(
                &mut owner,
                Input::AppendObservation {
                    channel: channel.into(),
                    sequence,
                    receive_ordinal,
                    ingress_generation,
                }
            )
            .is_err()
        );
        assert_eq!(owner.state(), &before);
    }
    let effect = Mutator::apply(
        &mut owner,
        Input::AppendObservation {
            channel: "voice".into(),
            sequence: 11,
            receive_ordinal: 1,
            ingress_generation: 1,
        },
    )?;
    assert!(matches!(effect.effects(), [Effect::ObservationAccepted {
        channel, sequence: 11, receive_ordinal: 1,
    }] if channel == "voice"));
    assert_eq!(owner.state().durable_watermarks["voice"], 11);
    assert_eq!(owner.state().receive_ordinals["voice"], 1);
    assert!(
        Mutator::apply(
            &mut owner,
            Input::AppendObservation {
                channel: "voice".into(),
                sequence: 12,
                receive_ordinal: 1,
                ingress_generation: 1,
            }
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn generated_prefix_reservation_spends_frontier_and_preserves_gap_facts() -> TestResult {
    let mut owner = activated()?;
    Mutator::apply(
        &mut owner,
        Input::AppendObservation {
            channel: "voice".into(),
            sequence: 11,
            receive_ordinal: 1,
            ingress_generation: 1,
        },
    )?;
    let before = owner.state().clone();
    let waiting = Mutator::apply(
        &mut owner,
        Input::ReservePrefix {
            channel: "voice".into(),
            after: 10,
            through: 11,
            received_through: 3,
            ingress_generation: 1,
        },
    )?;
    assert!(
        matches!(waiting.effects(), [Effect::AwaitingObservationDurability { channel }]
        if channel == "voice")
    );
    assert_eq!(owner.state(), &before);
    Mutator::apply(
        &mut owner,
        Input::RecordKnownGap {
            channel: "voice".into(),
            sequence: 12,
            after_received: 1,
            through_received: 3,
            ingress_generation: 1,
            record_bytes: 700,
        },
    )?;
    Mutator::apply(
        &mut owner,
        Input::AppendObservation {
            channel: "voice".into(),
            sequence: 13,
            receive_ordinal: 4,
            ingress_generation: 1,
        },
    )?;
    let first = Mutator::apply(
        &mut owner,
        Input::ReservePrefix {
            channel: "voice".into(),
            after: 10,
            through: 13,
            received_through: 4,
            ingress_generation: 1,
        },
    )?;
    assert!(matches!(
        first.effects(),
        [Effect::RangeSelected {
            after: 10,
            through: 13,
            discontinuous: true,
            ..
        }]
    ));
    Mutator::apply(
        &mut owner,
        Input::AppendObservation {
            channel: "voice".into(),
            sequence: 14,
            receive_ordinal: 5,
            ingress_generation: 1,
        },
    )?;
    let second = Mutator::apply(
        &mut owner,
        Input::ReservePrefix {
            channel: "voice".into(),
            after: 13,
            through: 14,
            received_through: 5,
            ingress_generation: 1,
        },
    )?;
    assert!(matches!(
        second.effects(),
        [Effect::RangeSelected {
            after: 13,
            through: 14,
            discontinuous: false,
            ..
        }]
    ));
    for (after, through, discontinuous) in [(10, 12, true), (12, 14, false), (14, 14, false)] {
        let selected = Mutator::apply(
            &mut owner,
            Input::SelectExplicitRange {
                channel: "voice".into(),
                after,
                through,
                ingress_generation: 1,
            },
        )?;
        assert!(
            matches!(selected.effects(), [Effect::RangeSelected { discontinuous: value, .. }]
            if *value == discontinuous)
        );
        assert_eq!(owner.state().reservation_frontiers["voice"], 14);
    }
    Ok(())
}

#[test]
fn generated_transcript_control_credit_always_preserves_final_fence() -> TestResult {
    let mut owner = activated()?;
    Mutator::apply(
        &mut owner,
        Input::RecordKnownGap {
            channel: "voice".into(),
            sequence: 11,
            after_received: 0,
            through_received: 2,
            ingress_generation: 1,
            record_bytes: 1024,
        },
    )?;
    let before = owner.state().clone();
    assert!(
        Mutator::apply(
            &mut owner,
            Input::RecordKnownGap {
                channel: "voice".into(),
                sequence: 12,
                after_received: 2,
                through_received: 3,
                ingress_generation: 1,
                record_bytes: 1,
            }
        )
        .is_err()
    );
    assert_eq!(owner.state(), &before);
    Mutator::apply(
        &mut owner,
        Input::CloseIngress {
            ingress_generation: 2,
        },
    )?;
    assert!(
        Mutator::apply(
            &mut owner,
            Input::AppendObservation {
                channel: "voice".into(),
                sequence: 12,
                receive_ordinal: 3,
                ingress_generation: 1,
            }
        )
        .is_err()
    );
    let close = Mutator::apply(
        &mut owner,
        Input::CloseChannel {
            channel: "voice".into(),
            sequence: 12,
            record_bytes: 1024,
        },
    )?;
    assert!(matches!(
        close.effects(),
        [Effect::ChannelIngressClosed { sequence: 12, .. }]
    ));
    assert_eq!(owner.state().control_spent_records["voice"], 2);
    assert_eq!(owner.state().control_spent_bytes["voice"], 2048);
    assert!(owner.state().accepting_channels.is_empty());
    Ok(())
}

#[test]
fn generated_crash_fence_never_advances_received_ordinal() -> TestResult {
    let mut owner = activated()?;
    let before = owner.state().clone();
    let effect = Mutator::apply(
        &mut owner,
        Input::RecoverUnknownTail {
            channel: "voice".into(),
            sequence: 11,
            record_bytes: 800,
        },
    )?;
    assert!(matches!(
        effect.effects(),
        [Effect::UnknownTailFenced { sequence: 11, .. }]
    ));
    assert_eq!(owner.state().receive_ordinals, before.receive_ordinals);
    assert_eq!(owner.state().gap_channels[&11], "voice");
    assert!(
        Mutator::apply(
            &mut owner,
            Input::ReservePrefix {
                channel: "voice".into(),
                after: 10,
                through: 11,
                received_through: 0,
                ingress_generation: 1,
            }
        )
        .is_err()
    );
    assert!(
        Mutator::apply(
            &mut owner,
            Input::ActivateChannel {
                channel: "voice".into(),
                sequence: 12,
                voice_accounting: false,
                ingress_generation: 1,
                credit_records: 2,
                credit_bytes: 2048,
                maximum_record_charge: 1024,
            }
        )
        .is_err()
    );
    Mutator::apply(
        &mut owner,
        Input::ActivateChannel {
            channel: "replacement".into(),
            sequence: 12,
            voice_accounting: false,
            ingress_generation: 1,
            credit_records: 2,
            credit_bytes: 2048,
            maximum_record_charge: 1024,
        },
    )?;
    let selected = Mutator::apply(
        &mut owner,
        Input::ReservePrefix {
            channel: "replacement".into(),
            after: 12,
            through: 12,
            received_through: 0,
            ingress_generation: 1,
        },
    )?;
    assert!(matches!(
        selected.effects(),
        [Effect::RangeSelected {
            discontinuous: false,
            ..
        }]
    ));
    Ok(())
}
