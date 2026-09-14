use meerkat_core::live_observation::{
    LiveTranscriptDirection, LiveTranscriptObservation, LiveTranscriptRange,
};
use meerkat_core::{Message, Session, UserMessage};

#[test]
fn continuous_observation_is_not_a_user_message_or_actor_session_projection()
-> Result<(), Box<dyn std::error::Error>> {
    let mut session = Session::new();
    session.push(Message::User(UserMessage::text("ordinary committed input")));
    let actor_before = serde_json::to_vec(&session)?;
    let observation = LiveTranscriptObservation::new(
        LiveTranscriptDirection::Input,
        LiveTranscriptRange::new(1066.5390310178614, 1067.125)?,
        " \tcontinuous, not a final user turn\n",
    );
    let encoded = serde_json::to_vec(&observation)?;
    let restored: LiveTranscriptObservation = serde_json::from_slice(&encoded)?;
    assert_eq!(serde_json::to_vec(&restored)?, encoded);
    assert!(serde_json::from_slice::<UserMessage>(&encoded).is_err());
    assert!(serde_json::from_slice::<Message>(&encoded).is_err());
    assert_eq!(serde_json::to_vec(&session)?, actor_before);
    Ok(())
}

#[test]
fn continuous_text_ranges_keep_overlap_and_fraction_without_turn_finality()
-> Result<(), Box<dyn std::error::Error>> {
    let observations = [
        LiveTranscriptObservation::new(
            LiveTranscriptDirection::Input,
            LiveTranscriptRange::new(1.125, 2.5)?,
            " ",
        ),
        LiveTranscriptObservation::new(
            LiveTranscriptDirection::Input,
            LiveTranscriptRange::new(2.0, 3.25)?,
            "\n",
        ),
        LiveTranscriptObservation::new(
            LiveTranscriptDirection::Output,
            LiveTranscriptRange::new(2.0, 2.0)?,
            "",
        ),
    ];
    let bytes = serde_json::to_vec(&observations)?;
    let restored: Vec<LiveTranscriptObservation> = serde_json::from_slice(&bytes)?;
    assert_eq!(serde_json::to_vec(&restored)?, bytes);
    let mut forged = serde_json::to_value(&observations[0])?;
    forged["final"] = serde_json::json!(true);
    assert!(serde_json::from_value::<LiveTranscriptObservation>(forged).is_err());
    Ok(())
}
