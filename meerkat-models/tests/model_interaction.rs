use meerkat_core::Provider;
use meerkat_core::model_profile::{ModelInteractionKind, ModelProfile, project_to_profile};
use meerkat_models::capabilities_for;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn interaction_kind_owns_coarse_live_and_text_applicability() -> TestResult {
    for (kind, realtime, text) in [
        (ModelInteractionKind::Text, false, true),
        (ModelInteractionKind::TurnBasedRealtime, true, true),
        (ModelInteractionKind::ContinuousLive, true, false),
    ] {
        assert_eq!(kind.is_realtime(), realtime);
        assert_eq!(kind.supports_text_execution(), text);
        let encoded = serde_json::to_vec(&kind)?;
        assert_eq!(
            serde_json::from_slice::<ModelInteractionKind>(&encoded)?,
            kind
        );
    }
    Ok(())
}

#[test]
fn profile_projects_interaction_without_a_second_realtime_fact() -> TestResult {
    let base = capabilities_for(Provider::OpenAI, "gpt-5.5").ok_or("missing text fixture")?;
    for kind in [
        ModelInteractionKind::Text,
        ModelInteractionKind::TurnBasedRealtime,
        ModelInteractionKind::ContinuousLive,
    ] {
        let mut capabilities = *base;
        capabilities.interaction_kind = kind;
        let profile = project_to_profile(&capabilities);
        assert_eq!(profile.interaction_kind, kind);
        assert_eq!(profile.is_realtime(), capabilities.is_realtime());
        let encoded = serde_json::to_value(&profile)?;
        assert!(encoded.get("realtime").is_none());
        let decoded: ModelProfile = serde_json::from_value(encoded)?;
        assert_eq!(decoded.interaction_kind, kind);
    }
    Ok(())
}

#[test]
fn existing_private_and_realtime_rows_keep_their_turn_based_contract() -> TestResult {
    for model in ["gpt-realtime-2", "gpt-live-1-codex"] {
        let capabilities =
            capabilities_for(Provider::OpenAI, model).ok_or("missing live fixture")?;
        assert_eq!(
            capabilities.interaction_kind,
            ModelInteractionKind::TurnBasedRealtime
        );
        assert!(capabilities.is_realtime());
    }
    Ok(())
}
