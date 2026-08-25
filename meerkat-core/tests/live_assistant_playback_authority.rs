//! Behavioral ratchets for generated assistant playback target authority.

#![allow(clippy::expect_used, clippy::panic)]

use meerkat_core::generated::session_document::{
    LiveAssistantPlaybackTerminalDisposition, LiveAssistantPlaybackTerminalObservation,
    SessionDocumentEffect, SessionDocumentKey, SessionDocumentMachineAuthority,
};

const SESSION: &str = "session-playback-authority";
const CHANNEL: &str = "channel-playback-authority";
const INTERACTION: &str = "interaction-playback-authority";
const RESPONSE: &str = "response-playback-authority";
const ITEM: &str = "item-playback-authority";
const CONTENT_INDEX: u64 = 0;

fn key() -> SessionDocumentKey {
    SessionDocumentKey::new(SESSION.to_string())
}

fn interaction_authority() -> SessionDocumentMachineAuthority {
    let mut authority = SessionDocumentMachineAuthority::new();
    authority
        .admit_live_interaction_transcript(key(), CHANNEL.to_string(), INTERACTION.to_string())
        .expect("foreground interaction is admitted first");
    authority
}

fn admit_target(authority: &mut SessionDocumentMachineAuthority) {
    authority
        .admit_live_assistant_playback_target(
            key(),
            CHANNEL.to_string(),
            INTERACTION.to_string(),
            RESPONSE.to_string(),
            ITEM.to_string(),
            CONTENT_INDEX,
        )
        .expect("exact assistant response target is admitted");
}

#[test]
fn assistant_first_target_without_foreground_interaction_is_rejected() {
    let mut authority = SessionDocumentMachineAuthority::new();
    assert!(
        authority
            .admit_live_assistant_playback_target(
                key(),
                CHANNEL.to_string(),
                INTERACTION.to_string(),
                RESPONSE.to_string(),
                ITEM.to_string(),
                CONTENT_INDEX,
            )
            .is_err(),
        "assistant output cannot mint a shadow foreground interaction"
    );
}

#[test]
fn final_then_terminal_joins_once_and_validates_exact_prefix() {
    let mut authority = interaction_authority();
    admit_target(&mut authority);

    assert!(
        authority
            .complete_live_interaction_transcript(
                key(),
                CHANNEL.to_string(),
                INTERACTION.to_string(),
            )
            .is_err(),
        "staged assistant text cannot become terminal before playback evidence"
    );
    let effects = authority
        .observe_live_assistant_playback_final(
            key(),
            CHANNEL.to_string(),
            INTERACTION.to_string(),
            RESPONSE.to_string(),
            ITEM.to_string(),
            CONTENT_INDEX,
            12,
            "authoritative-digest".to_string(),
            LiveAssistantPlaybackTerminalObservation::Unmeasured,
            0,
            String::new(),
            false,
        )
        .expect("final fact is retained while terminal evidence is absent");
    assert!(matches!(
        effects.as_slice(),
        [SessionDocumentEffect::LiveAssistantPlaybackFinalObserved { .. }]
    ));
    assert!(
        authority
            .observe_live_assistant_playback_terminal(
                key(),
                CHANNEL.to_string(),
                INTERACTION.to_string(),
                RESPONSE.to_string(),
                ITEM.to_string(),
                CONTENT_INDEX,
                LiveAssistantPlaybackTerminalObservation::ReportedPrefix,
                5,
                "prefix-digest".to_string(),
                12,
                "authoritative-digest".to_string(),
                true,
                false,
            )
            .is_err(),
        "reported text that is not an exact prefix cannot authorize canonical replacement"
    );

    let effects = authority
        .observe_live_assistant_playback_terminal(
            key(),
            CHANNEL.to_string(),
            INTERACTION.to_string(),
            RESPONSE.to_string(),
            ITEM.to_string(),
            CONTENT_INDEX,
            LiveAssistantPlaybackTerminalObservation::ReportedPrefix,
            5,
            "prefix-digest".to_string(),
            12,
            "authoritative-digest".to_string(),
            true,
            true,
        )
        .expect("exact reported prefix terminalizes the target");
    assert!(matches!(
        effects.as_slice(),
        [SessionDocumentEffect::LiveAssistantPlaybackTerminalResolved {
            disposition: LiveAssistantPlaybackTerminalDisposition::TruncateToReportedPrefix,
            canonical_chars: Some(5),
            canonical_text_digest: Some(digest),
            biological_hearing_claimed: false,
            ..
        }] if digest == "prefix-digest"
    ));
    assert!(
        authority
            .observe_live_assistant_playback_terminal(
                key(),
                CHANNEL.to_string(),
                INTERACTION.to_string(),
                RESPONSE.to_string(),
                ITEM.to_string(),
                CONTENT_INDEX,
                LiveAssistantPlaybackTerminalObservation::ReportedPrefix,
                5,
                "prefix-digest".to_string(),
                12,
                "authoritative-digest".to_string(),
                true,
                true,
            )
            .is_err(),
        "a terminal playback target cannot be resolved twice"
    );
    authority
        .complete_live_interaction_transcript(key(), CHANNEL.to_string(), INTERACTION.to_string())
        .expect("interaction completes only after terminal playback authority");
}

#[test]
fn terminal_then_final_and_recovery_join_without_hearing_claims() {
    let mut complete = SessionDocumentMachineAuthority::new();
    complete
        .recover_live_assistant_playback_target(
            key(),
            CHANNEL.to_string(),
            INTERACTION.to_string(),
            RESPONSE.to_string(),
            ITEM.to_string(),
            CONTENT_INDEX,
        )
        .expect("durable exact target recovers correlation");
    let effects = complete
        .observe_live_assistant_playback_terminal(
            key(),
            CHANNEL.to_string(),
            INTERACTION.to_string(),
            RESPONSE.to_string(),
            ITEM.to_string(),
            CONTENT_INDEX,
            LiveAssistantPlaybackTerminalObservation::PlaybackComplete,
            0,
            String::new(),
            0,
            String::new(),
            false,
            false,
        )
        .expect("early playback terminal is retained by generated authority");
    assert!(matches!(
        effects.as_slice(),
        [SessionDocumentEffect::LiveAssistantPlaybackTerminalObserved { .. }]
    ));
    let effects = complete
        .observe_live_assistant_playback_final(
            key(),
            CHANNEL.to_string(),
            INTERACTION.to_string(),
            RESPONSE.to_string(),
            ITEM.to_string(),
            CONTENT_INDEX,
            12,
            "full-digest".to_string(),
            LiveAssistantPlaybackTerminalObservation::PlaybackComplete,
            0,
            String::new(),
            false,
        )
        .expect("late final joins the retained complete terminal");
    assert!(matches!(
        effects.as_slice(),
        [SessionDocumentEffect::LiveAssistantPlaybackTerminalResolved {
            disposition: LiveAssistantPlaybackTerminalDisposition::PlaybackComplete,
            canonical_chars: Some(12),
            canonical_text_digest: Some(digest),
            biological_hearing_claimed: false,
            ..
        }] if digest == "full-digest"
    ));

    let mut recovered = SessionDocumentMachineAuthority::new();
    recovered
        .recover_live_assistant_playback_target(
            key(),
            CHANNEL.to_string(),
            INTERACTION.to_string(),
            RESPONSE.to_string(),
            ITEM.to_string(),
            CONTENT_INDEX,
        )
        .expect("target recovers");
    recovered
        .recover_live_assistant_playback_terminal(
            key(),
            CHANNEL.to_string(),
            INTERACTION.to_string(),
            RESPONSE.to_string(),
            ITEM.to_string(),
            CONTENT_INDEX,
            LiveAssistantPlaybackTerminalObservation::ReportedPrefix,
            5,
            "prefix-digest".to_string(),
        )
        .expect("independent terminal fact recovers");
    let effects = recovered
        .observe_live_assistant_playback_final(
            key(),
            CHANNEL.to_string(),
            INTERACTION.to_string(),
            RESPONSE.to_string(),
            ITEM.to_string(),
            CONTENT_INDEX,
            12,
            "full-digest".to_string(),
            LiveAssistantPlaybackTerminalObservation::ReportedPrefix,
            5,
            "prefix-digest".to_string(),
            true,
        )
        .expect("recovered terminal joins a late final");
    assert!(matches!(
        effects.as_slice(),
        [
            SessionDocumentEffect::LiveAssistantPlaybackTerminalResolved {
                disposition: LiveAssistantPlaybackTerminalDisposition::TruncateToReportedPrefix,
                canonical_chars: Some(5),
                biological_hearing_claimed: false,
                ..
            }
        ]
    ));

    let mut unmeasured = interaction_authority();
    admit_target(&mut unmeasured);
    let effects = unmeasured
        .observe_live_assistant_playback_terminal(
            key(),
            CHANNEL.to_string(),
            INTERACTION.to_string(),
            RESPONSE.to_string(),
            ITEM.to_string(),
            CONTENT_INDEX,
            LiveAssistantPlaybackTerminalObservation::Unmeasured,
            0,
            String::new(),
            0,
            String::new(),
            false,
            false,
        )
        .expect("unmeasured explicitly abandons the exact staged target");
    assert!(matches!(
        effects.as_slice(),
        [
            SessionDocumentEffect::LiveAssistantPlaybackTerminalResolved {
                disposition: LiveAssistantPlaybackTerminalDisposition::Unmeasured,
                canonical_chars: None,
                canonical_text_digest: None,
                biological_hearing_claimed: false,
                ..
            }
        ]
    ));
}

#[test]
fn channel_close_terminalizes_exact_target_unmeasured_and_allows_replacement_turn() {
    let mut authority = interaction_authority();
    admit_target(&mut authority);

    assert!(
        authority
            .resolve_live_assistant_playback_on_channel_close(
                key(),
                CHANNEL.to_string(),
                INTERACTION.to_string(),
                RESPONSE.to_string(),
                "wrong-item".to_string(),
                CONTENT_INDEX,
            )
            .is_err(),
        "close cannot abandon a different assistant target"
    );
    let effects = authority
        .resolve_live_assistant_playback_on_channel_close(
            key(),
            CHANNEL.to_string(),
            INTERACTION.to_string(),
            RESPONSE.to_string(),
            ITEM.to_string(),
            CONTENT_INDEX,
        )
        .expect("exact channel close terminalizes pending playback");
    assert!(matches!(
        effects.as_slice(),
        [
            SessionDocumentEffect::LiveAssistantPlaybackTerminalResolved {
                disposition: LiveAssistantPlaybackTerminalDisposition::Unmeasured,
                canonical_chars: None,
                canonical_text_digest: None,
                biological_hearing_claimed: false,
                ..
            }
        ]
    ));

    authority
        .admit_live_interaction_transcript(
            key(),
            "replacement-channel".to_string(),
            "replacement-interaction".to_string(),
        )
        .expect("replacement channel admits a fresh foreground interaction");
}
