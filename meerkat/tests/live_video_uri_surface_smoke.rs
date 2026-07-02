#![allow(clippy::expect_used)]

use meerkat::{AgentFactory, Config, CreateSessionRequest, InitialTurnPolicy, SessionService};
use meerkat_core::config::SystemPromptOverride;
use meerkat_core::service::DeferredPromptPolicy;
use meerkat_core::{ContentBlock, ContentInput, VideoData};

const MODEL: &str = "gemini-3.5-flash";
const PUBLIC_VIDEO_URI: &str =
    "https://storage.googleapis.com/cloud-samples-data/generative-ai/video/pixel8.mp4";

fn gemini_api_key() -> Option<String> {
    std::env::var("RKAT_GEMINI_API_KEY")
        .or_else(|_| std::env::var("GEMINI_API_KEY"))
        .or_else(|_| std::env::var("RKAT_GOOGLE_API_KEY"))
        .or_else(|_| std::env::var("GOOGLE_API_KEY"))
        .ok()
        .filter(|key| !key.trim().is_empty())
}

#[tokio::test]
#[ignore = "live Meerkat surface smoke; requires GEMINI_API_KEY"]
async fn session_service_accepts_and_runs_gemini_video_uri_prompt()
-> Result<(), Box<dyn std::error::Error>> {
    if gemini_api_key().is_none() {
        eprintln!("skipping live surface smoke: GEMINI_API_KEY is not set");
        return Ok(());
    }

    let temp = tempfile::tempdir()?;
    let factory = AgentFactory::new(temp.path().join("sessions"));
    let service = meerkat::build_ephemeral_service(factory, Config::default(), 2);
    let result = service
        .create_session(CreateSessionRequest {
            injected_context: Vec::new(),
            model: MODEL.to_string(),
            prompt: ContentInput::Blocks(vec![
                ContentBlock::Video {
                    media_type: "video/mp4".to_string(),
                    duration_ms: 12_000,
                    data: VideoData::Uri {
                        uri: PUBLIC_VIDEO_URI.to_string(),
                    },
                },
                ContentBlock::Text {
                    text: "In one short sentence, describe what is visible in this video."
                        .to_string(),
                },
            ]),
            system_prompt: SystemPromptOverride::Inherit,
            max_tokens: Some(128),
            event_tx: None,
            initial_turn: InitialTurnPolicy::RunImmediately,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: None,
            labels: None,
        })
        .await?;

    assert!(
        !result.text.trim().is_empty(),
        "Meerkat surface returned empty text for URI video prompt"
    );
    eprintln!(
        "Meerkat surface video URI smoke response: {}",
        result.text.trim()
    );
    Ok(())
}
