#![cfg(all(feature = "integration-real-tests", not(target_arch = "wasm32")))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//!
//! AI Pictionary — multimodal cross-model comms stress test.
//!
//! A mob of 4 agents (3 different LLM providers) plays Pictionary:
//!
//! - **Test harness** generates images via Gemini flash-image-preview (raw reqwest)
//! - **Test harness** injects images into the artist as current-turn multimodal content
//!   via `MemberHandle::send(ContentInput::Blocks)`
//! - **Artist** forwards the current-turn image through the agent-facing `send_message`
//!   tool using typed `image_ref` blocks
//! - **Guesser A** (Claude Opus 4.6) — lead guesser, literal/shapes perspective
//! - **Guesser B** (Gemini 3.1 Pro) — emotions/mood perspective
//! - **Guesser C** (GPT-5.4) — context/narrative perspective
//! - Guessers discuss via peer-to-peer comms, then guesser-a sends consensus to artist
//! - **Artist** (Claude Sonnet 4.5) validates the guess
//!
//! Three rounds: easy (lighthouse), medium (loneliness), hard (déjà vu)
//!
//! ## Run
//! ```bash
//! ANTHROPIC_API_KEY=... GEMINI_API_KEY=... OPENAI_API_KEY=... \
//!   cargo test -p meerkat-mob --test smoke_mob_pictionary \
//!     --features integration-real-tests -- --ignored --nocapture
//! ```

use meerkat::{AgentFactory, Config, FactoryAgentBuilder};
use meerkat_core::types::{ContentBlock, ContentInput, HandlingMode};
use meerkat_mob::{
    AgentIdentity, MemberHandle, MobBuilder, MobDefinition, MobHandle, MobId, MobRuntimeMode,
    MobSessionService, MobStorage, Profile, ProfileBinding, ProfileName, SpawnMemberSpec,
    ToolConfig,
    definition::{RoleWiringRule, WiringRules},
};
use meerkat_session::PersistentSessionService;
use meerkat_store::{JsonlStore, StoreAdapter};
use reqwest::header::ACCEPT_ENCODING;
use std::collections::BTreeMap;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::time::{Duration, Instant, sleep};

fn first_env(vars: &[&str]) -> Option<String> {
    for name in vars {
        if let Ok(value) = std::env::var(name) {
            return Some(value);
        }
    }
    None
}

/// Generate an image using Gemini flash-image-preview. Returns (base64_data, media_type).
async fn generate_image(
    api_key: &str,
    prompt: &str,
) -> Result<(String, String), Box<dyn std::error::Error>> {
    let mut errors = Vec::new();
    for attempt in 1..=3 {
        match generate_image_once(api_key, prompt).await {
            Ok(image) => return Ok(image),
            Err(error) => {
                errors.push(format!("attempt {attempt}: {error}"));
                if attempt < 3 {
                    sleep(Duration::from_millis(500 * attempt)).await;
                }
            }
        }
    }

    Err(format!(
        "Gemini image generation failed after retries: {}",
        errors.join("; ")
    )
    .into())
}

async fn generate_image_once(
    api_key: &str,
    prompt: &str,
) -> Result<(String, String), Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/\
         gemini-3.1-flash-image-preview:generateContent?key={api_key}"
    );
    let body = serde_json::json!({
        "contents": [{ "parts": [{ "text": prompt }] }],
        "generationConfig": { "responseModalities": ["TEXT", "IMAGE"] }
    });

    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header(ACCEPT_ENCODING, "identity")
        .json(&body)
        .timeout(Duration::from_secs(120))
        .send()
        .await
        .map_err(|error| sanitize_gemini_key_error(api_key, error))?;

    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|error| sanitize_gemini_key_error(api_key, error))?;
    if !status.is_success() {
        return Err(format!("Gemini API {status}: {text}").into());
    }

    let json: serde_json::Value = serde_json::from_str(&text)?;
    let parts = json["candidates"][0]["content"]["parts"]
        .as_array()
        .ok_or("no parts in response")?;

    for part in parts {
        if let Some(d) = part.get("inlineData") {
            return Ok((
                d["data"].as_str().ok_or("no data")?.to_string(),
                d["mimeType"].as_str().unwrap_or("image/png").to_string(),
            ));
        }
    }
    Err("no image in response".into())
}

fn sanitize_gemini_key_error(api_key: &str, error: reqwest::Error) -> String {
    error.to_string().replace(api_key, "<redacted>")
}

fn comms_profile(model: &str, peer_desc: &str) -> Profile {
    Profile {
        model: model.to_string(),
        skills: vec![],
        tools: ToolConfig {
            comms: true,
            ..Default::default()
        },
        peer_description: peer_desc.to_string(),
        external_addressable: true,
        backend: None,
        runtime_mode: MobRuntimeMode::AutonomousHost,
        max_inline_peer_notifications: None,
        output_schema: None,
        provider_params: None,
    }
}

fn pictionary_definition() -> MobDefinition {
    let mut profiles = BTreeMap::new();
    profiles.insert(
        ProfileName::from("artist"),
        ProfileBinding::Inline(comms_profile(
            "claude-sonnet-4-5",
            "Artist — validates guesses",
        )),
    );
    profiles.insert(
        ProfileName::from("guesser-a"),
        ProfileBinding::Inline(comms_profile(
            "claude-opus-4-6",
            "guesser-a (Opus) — lead, literal/shapes",
        )),
    );
    profiles.insert(
        ProfileName::from("guesser-b"),
        ProfileBinding::Inline(comms_profile(
            "gemini-3.1-pro-preview",
            "guesser-b (Gemini) — emotions/mood",
        )),
    );
    profiles.insert(
        ProfileName::from("guesser-c"),
        ProfileBinding::Inline(comms_profile(
            "gpt-5.4",
            "guesser-c (GPT) — context/narrative",
        )),
    );

    let mut definition = MobDefinition::explicit(MobId::from("pictionary"));
    definition.profiles = profiles;
    definition.wiring = WiringRules {
        auto_wire_orchestrator: false,
        role_wiring: vec![
            // Full mesh: every profile can talk to every other
            RoleWiringRule {
                a: ProfileName::from("artist"),
                b: ProfileName::from("guesser-a"),
            },
            RoleWiringRule {
                a: ProfileName::from("artist"),
                b: ProfileName::from("guesser-b"),
            },
            RoleWiringRule {
                a: ProfileName::from("artist"),
                b: ProfileName::from("guesser-c"),
            },
            RoleWiringRule {
                a: ProfileName::from("guesser-a"),
                b: ProfileName::from("guesser-b"),
            },
            RoleWiringRule {
                a: ProfileName::from("guesser-a"),
                b: ProfileName::from("guesser-c"),
            },
            RoleWiringRule {
                a: ProfileName::from("guesser-b"),
                b: ProfileName::from("guesser-c"),
            },
        ],
    };
    definition
}

async fn setup_mob()
-> Result<(MobHandle, Arc<dyn MobSessionService>, TempDir), Box<dyn std::error::Error>> {
    let temp_dir = TempDir::new()?;
    let store_path = temp_dir.path().join("sessions");
    std::fs::create_dir_all(&store_path)?;

    let factory = AgentFactory::new(&store_path)
        .comms(true)
        .runtime_root(store_path.clone());
    let mut builder = FactoryAgentBuilder::new(factory, Config::default());
    let store = Arc::new(JsonlStore::new(store_path.join("sessions")));
    builder.default_session_store = Some(Arc::new(StoreAdapter::new(store.clone())));

    let store_dyn: Arc<dyn meerkat::SessionStore> = store.clone();
    let runtime_store = Arc::new(meerkat_runtime::InMemoryRuntimeStore::default());
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(meerkat_store::MemoryBlobStore::default());
    let session_service = Arc::new(PersistentSessionService::new(
        builder,
        16,
        store_dyn,
        Some(runtime_store),
        blob_store,
    ));
    let mob_service: Arc<dyn MobSessionService> = session_service.clone();

    // Use an explicit MeerkatMachine override so this e2e exercises the
    // canonical mob-runtime adapter path. AutonomousHost drains and runtime-backed
    // turns must both use this same adapter instance.
    let runtime_adapter = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());

    let handle = MobBuilder::new(pictionary_definition(), MobStorage::in_memory())
        .with_session_service(mob_service.clone())
        .with_runtime_adapter(runtime_adapter)
        .create()
        .await?;

    Ok((handle, mob_service, temp_dir))
}

async fn spawn_and_wait(handle: &MobHandle) -> Result<(), Box<dyn std::error::Error>> {
    // Spawn all 4 members with role-appropriate initial messages
    let spawns = [
        (
            "artist",
            "You are the artist in Pictionary. You will be told a secret word. \
             Wait for a guesser to send you their FINAL guess, then say CORRECT or WRONG. \
             Do NOT respond to peer_added notifications — just acknowledge silently.",
        ),
        (
            "guesser-a",
            "You are guesser-a, the LEAD guesser. IMPORTANT RULES: \
             1) When you receive an image from the artist, DO NOT guess immediately. \
             2) First, call send_message twice: once to guesser-b and once to guesser-c, describing what you see (literal shapes/objects). \
             3) WAIT for both guesser-b and guesser-c to reply with their interpretations. They may reply from the artist image directly. \
             4) Only AFTER hearing from both, synthesize a consensus guess and send it to the artist. \
             The send_message tool sends to one peer per call; it does not broadcast. \
             Use handling_mode='steer' for all peer messages. Keep messages to 1-2 sentences.",
        ),
        (
            "guesser-b",
            "You are guesser-b. IMPORTANT RULES: \
             1) When you receive either an artist image or a message from guesser-a about an image, think about the EMOTIONAL/MOOD interpretation. \
             2) Reply by calling send_message twice: once to guesser-a and once to guesser-c with your interpretation. \
             3) Do NOT send anything to the artist — only guesser-a does that. \
             The send_message tool sends to one peer per call; use handling_mode='steer'. Keep messages to 1-2 sentences.",
        ),
        (
            "guesser-c",
            "You are guesser-c. IMPORTANT RULES: \
             1) When you receive either an artist image or a message from guesser-a about an image, think about the CONTEXT/NARRATIVE interpretation. \
             2) Reply by calling send_message twice: once to guesser-a and once to guesser-b with your interpretation. \
             3) Do NOT send anything to the artist — only guesser-a does that. \
             The send_message tool sends to one peer per call; use handling_mode='steer'. Keep messages to 1-2 sentences.",
        ),
    ];

    for (name, msg) in spawns {
        handle
            .spawn_spec(
                SpawnMemberSpec::new(name, AgentIdentity::from(name))
                    .with_initial_message(ContentInput::Text(msg.to_string())),
            )
            .await
            .map_err(|e| format!("spawn {name}: {e}"))?;
    }

    // Wait for startup readiness, not kickoff completion. Four-model kickoff
    // latency is not the same contract as "members are bound and ready to
    // orchestrate the round".
    handle
        .wait_for_ready(Some(Duration::from_secs(60)))
        .await
        .map_err(|e| format!("ready wait: {e}"))?;
    Ok(())
}

async fn wait_for_comms_mesh_ready(
    handle: &MobHandle,
    service: &dyn MobSessionService,
    timeout: Duration,
) -> bool {
    let members = ["artist", "guesser-a", "guesser-b", "guesser-c"];
    let deadline = Instant::now() + timeout;

    loop {
        let mut all_ready = true;
        for member in members {
            let Some(session_id) = handle
                .resolve_bridge_session_id(&AgentIdentity::from(member))
                .await
            else {
                all_ready = false;
                break;
            };
            let Some(comms_runtime) = service.comms_runtime(&session_id).await else {
                all_ready = false;
                break;
            };
            let peers = comms_runtime.peers().await;
            let expected = members
                .iter()
                .filter(|other| **other != member)
                .map(|other| format!("pictionary/{other}/{other}"))
                .collect::<Vec<_>>();
            if !expected
                .iter()
                .all(|name| peers.iter().any(|peer| peer.name.as_str() == name))
            {
                all_ready = false;
                break;
            }
        }

        if all_ready {
            return true;
        }
        if Instant::now() > deadline {
            for member in members {
                let Some(session_id) = handle
                    .resolve_bridge_session_id(&AgentIdentity::from(member))
                    .await
                else {
                    eprintln!("mesh diagnostics: {member}: missing bridge session id");
                    continue;
                };
                let Some(comms_runtime) = service.comms_runtime(&session_id).await else {
                    eprintln!("mesh diagnostics: {member}: missing comms runtime for {session_id}");
                    continue;
                };
                let peers = comms_runtime.peers().await;
                let names = peers
                    .iter()
                    .map(|peer| peer.name.as_str().to_string())
                    .collect::<Vec<_>>();
                eprintln!("mesh diagnostics: {member}: peers={names:?}");
            }
            return false;
        }
        sleep(Duration::from_secs(1)).await;
    }
}

async fn wait_for_member_histories_to_settle(
    handle: &MobHandle,
    service: &dyn MobSessionService,
    timeout: Duration,
    stable_for: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    let mut previous_snapshot: Option<Vec<String>> = None;
    let mut stable_since: Option<Instant> = None;

    loop {
        let mut snapshot = Vec::new();
        let mut complete = true;

        for member in ["artist", "guesser-a", "guesser-b", "guesser-c"] {
            let Some(session_id) = handle
                .resolve_bridge_session_id(&AgentIdentity::from(member))
                .await
            else {
                complete = false;
                break;
            };
            let Ok(page) = service
                .read_history(
                    &session_id,
                    meerkat_core::SessionHistoryQuery {
                        offset: 0,
                        limit: None,
                    },
                )
                .await
            else {
                complete = false;
                break;
            };

            let last_signature = page
                .messages
                .last()
                .map(|msg| match msg {
                    meerkat_core::types::Message::System(s) => format!("system:{}", s.content),
                    meerkat_core::types::Message::SystemNotice(notice) => {
                        format!("notice:{}", notice.rendered_text())
                    }
                    meerkat_core::types::Message::User(u) => {
                        format!("user:{}", meerkat_core::types::text_content(&u.content))
                    }
                    meerkat_core::types::Message::Assistant(a) => {
                        format!("assistant:{}", a.content)
                    }
                    meerkat_core::types::Message::BlockAssistant(ba) => format!(
                        "block_assistant:{}",
                        ba.blocks
                            .iter()
                            .filter_map(|block| match block {
                                meerkat_core::types::AssistantBlock::Text { text, .. } => {
                                    Some(text.as_str())
                                }
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join("")
                    ),
                    meerkat_core::types::Message::ToolResults { results, .. } => format!(
                        "tool_results:{}",
                        results
                            .iter()
                            .map(|result| {
                                format!(
                                    "{}:{}:{}",
                                    result.tool_use_id,
                                    result.is_error,
                                    meerkat_core::types::text_content(&result.content)
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("|")
                    ),
                })
                .unwrap_or_default();
            snapshot.push(format!("{member}:{}:{last_signature}", page.messages.len()));
        }

        if complete {
            if previous_snapshot.as_ref() == Some(&snapshot) {
                let stable_since_ref = stable_since.get_or_insert_with(Instant::now);
                if stable_since_ref.elapsed() >= stable_for {
                    return true;
                }
            } else {
                previous_snapshot = Some(snapshot);
                stable_since = Some(Instant::now());
            }
        }

        if Instant::now() > deadline {
            return false;
        }
        sleep(Duration::from_secs(1)).await;
    }
}

async fn missing_guessers_for_artist_image(
    handle: &MobHandle,
    service: &dyn MobSessionService,
    recipients: &[&'static str],
) -> Vec<&'static str> {
    let mut missing = Vec::new();
    for guesser in recipients {
        let Some(session_id) = handle
            .resolve_bridge_session_id(&AgentIdentity::from(*guesser))
            .await
        else {
            missing.push(*guesser);
            continue;
        };
        let Ok(page) = service
            .read_history(
                &session_id,
                meerkat_core::SessionHistoryQuery {
                    offset: 0,
                    limit: None,
                },
            )
            .await
        else {
            missing.push(*guesser);
            continue;
        };
        let has_artist_image = page.messages.iter().any(|msg| match msg {
            meerkat_core::types::Message::User(u) => {
                let text = meerkat_core::types::text_content(&u.content);
                text.contains("[COMMS MESSAGE from pictionary/artist/artist]")
                    && meerkat_core::has_images(&u.content)
            }
            _ => false,
        });
        if !has_artist_image {
            missing.push(*guesser);
        }
    }
    missing
}

fn artist_forward_body(label: &str) -> String {
    format!("Pictionary {label} - guess what I drew!")
}

fn artist_forward_text_block(label: &str) -> String {
    format!(
        "I drew this for Pictionary ({label}). \
         guesser-a: describe what you see to guesser-b and guesser-c FIRST, \
         wait for their replies, THEN send me your consensus guess."
    )
}

fn artist_forward_instruction(label: &str, attempt: usize) -> String {
    let retry_note = if attempt == 1 {
        "This is the only task for this turn."
    } else {
        "Retry now because guesser-a's history did not yet show the image."
    };
    let body = artist_forward_body(label);
    let text = artist_forward_text_block(label);
    let blocks_example = serde_json::json!([
        { "type": "text", "text": text },
        { "type": "image_ref", "source": "current_turn", "index": 0 }
    ])
    .to_string();
    format!(
        "Forward the attached Pictionary image to guesser-a using the send_message tool. \
         {retry_note}\n\n\
         Exact steps:\n\
         1. Call peers if you need the peer_id for pictionary/guesser-a/guesser-a.\n\
         2. Call send_message with peer_id set to pictionary/guesser-a/guesser-a's peer_id, \
         handling_mode \"steer\", body \"{body}\", and blocks exactly:\n\
         {blocks_example}\n\n\
         Do not reveal the secret word, describe the image, or guess it yourself. Only forward \
         the attached image block through the comms tool."
    )
}

fn artist_forward_blocks(
    label: &str,
    media_type: &str,
    image_data: &str,
    attempt: usize,
) -> Vec<ContentBlock> {
    vec![
        ContentBlock::Text {
            text: artist_forward_instruction(label, attempt),
        },
        ContentBlock::Image {
            media_type: media_type.to_string(),
            data: image_data.to_string().into(),
        },
    ]
}

#[test]
fn artist_forward_instruction_uses_typed_current_turn_image_ref() {
    let prompt = artist_forward_instruction("Round \"quoted\"", 1);
    assert!(prompt.contains("send_message"));
    assert!(prompt.contains("\"type\":\"image_ref\""));
    assert!(prompt.contains("\"source\":\"current_turn\""));
    assert!(prompt.contains("\"index\":0"));
    assert!(!prompt.contains("current_turn:image"));

    let blocks = artist_forward_blocks("Round \"quoted\"", "image/png", "abc123", 1);
    assert_eq!(blocks.len(), 2);
    assert!(matches!(blocks.first(), Some(ContentBlock::Text { .. })));
    assert!(matches!(blocks.get(1), Some(ContentBlock::Image { .. })));
}

struct ArtistForwardImage<'a> {
    label: &'a str,
    media_type: &'a str,
    data: &'a str,
}

async fn ensure_artist_image_delivery(
    handle: &MobHandle,
    service: &dyn MobSessionService,
    artist: &MemberHandle,
    recipient: &'static str,
    image: ArtistForwardImage<'_>,
    timeout: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    let recipients = [recipient];
    let mut attempt = 1;
    let mut last_sent_at = Instant::now();
    let retry_after = Duration::from_secs(20);

    let mut last_outcome = match artist
        .send(
            ContentInput::Blocks(artist_forward_blocks(
                image.label,
                image.media_type,
                image.data,
                attempt,
            )),
            HandlingMode::Steer,
        )
        .await
    {
        Ok(receipt) => format!("initial artist turn {receipt:?}"),
        Err(err) => format!("artist turn error: {err}"),
    };

    loop {
        let missing = missing_guessers_for_artist_image(handle, service, &recipients).await;
        if missing.is_empty() {
            return Ok(());
        }
        if Instant::now() > deadline {
            let detail = missing
                .iter()
                .map(|guesser| format!("{guesser} ({last_outcome})"))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "artist image did not reach every guesser within {}s: {detail}",
                timeout.as_secs()
            ));
        }

        sleep(Duration::from_secs(2)).await;
        let now = Instant::now();
        for guesser in missing {
            if now.duration_since(last_sent_at) >= retry_after {
                attempt += 1;
                last_sent_at = now;
                match artist
                    .send(
                        ContentInput::Blocks(artist_forward_blocks(
                            image.label,
                            image.media_type,
                            image.data,
                            attempt,
                        )),
                        HandlingMode::Steer,
                    )
                    .await
                {
                    Ok(receipt) => {
                        println!("  [DEBUG] retrying artist image turn for {guesser}: {receipt:?}");
                        last_outcome = format!("retry artist turn {receipt:?}");
                    }
                    Err(err) => {
                        println!(
                            "  [DEBUG] retrying artist image turn for {guesser} failed: {err}"
                        );
                        last_outcome = format!("artist turn error: {err}");
                    }
                }
            }
        }
    }
}

fn current_round_artist_received_guess(page: &meerkat_core::SessionHistoryPage) -> bool {
    let latest_secret_idx = page.messages.iter().rposition(|msg| match msg {
        meerkat_core::types::Message::User(u) => {
            meerkat_core::types::text_content(&u.content).contains("SECRET WORD")
        }
        _ => false,
    });

    let Some(latest_secret_idx) = latest_secret_idx else {
        return false;
    };

    page.messages
        .iter()
        .skip(latest_secret_idx + 1)
        .any(|msg| match msg {
            meerkat_core::types::Message::User(u) => {
                let text = meerkat_core::types::text_content(&u.content);
                text.contains("[COMMS MESSAGE from pictionary/guesser-a/guesser-a]")
            }
            _ => false,
        })
}

fn current_round_discussion_completed(
    page: &meerkat_core::SessionHistoryPage,
    label: &str,
) -> bool {
    let first_image_idx = page.messages.iter().position(|msg| match msg {
        meerkat_core::types::Message::User(u) => {
            let text = meerkat_core::types::text_content(&u.content);
            text.contains("[COMMS MESSAGE from pictionary/artist/artist]")
                && text.contains("I drew this for Pictionary")
                && text.contains(label)
        }
        _ => false,
    });

    let Some(first_image_idx) = first_image_idx else {
        return false;
    };

    let mut heard_from_b = false;
    let mut heard_from_c = false;
    for msg in page.messages.iter().skip(first_image_idx + 1) {
        let text = match msg {
            meerkat_core::types::Message::User(u) => meerkat_core::types::text_content(&u.content),
            _ => continue,
        };
        heard_from_b |= text.contains("[COMMS MESSAGE from pictionary/guesser-b/guesser-b]");
        heard_from_c |= text.contains("[COMMS MESSAGE from pictionary/guesser-c/guesser-c]");
        if heard_from_b && heard_from_c {
            return true;
        }
    }
    false
}

#[test]
fn current_round_discussion_survives_artist_image_retry_after_first_peer_reply() {
    let label = "Round 1 — easy: concrete object";
    let page = pictionary_history_page(vec![
        "[COMMS MESSAGE from pictionary/artist/artist]\nI drew this for Pictionary (Round 1 — easy: concrete object).",
        "[COMMS MESSAGE from pictionary/guesser-b/guesser-b]\nA protective beacon.",
        "[COMMS MESSAGE from pictionary/artist/artist]\nI drew this for Pictionary (Round 1 — easy: concrete object).",
        "[COMMS MESSAGE from pictionary/guesser-c/guesser-c]\nThis feels like a lighthouse.",
    ]);

    assert!(current_round_discussion_completed(&page, label));
}

#[test]
fn current_round_discussion_ignores_previous_round_replies() {
    let page = pictionary_history_page(vec![
        "[COMMS MESSAGE from pictionary/artist/artist]\nI drew this for Pictionary (Round 1 — easy: concrete object).",
        "[COMMS MESSAGE from pictionary/guesser-b/guesser-b]\nA protective beacon.",
        "[COMMS MESSAGE from pictionary/guesser-c/guesser-c]\nThis feels like a lighthouse.",
        "[COMMS MESSAGE from pictionary/artist/artist]\nI drew this for Pictionary (Round 2 — medium: abstract concept).",
    ]);

    assert!(!current_round_discussion_completed(
        &page,
        "Round 2 — medium: abstract concept"
    ));
}

fn pictionary_history_page(texts: Vec<&str>) -> meerkat_core::SessionHistoryPage {
    let messages = texts
        .into_iter()
        .map(|text| {
            meerkat_core::types::Message::User(meerkat_core::types::UserMessage::text(text))
        })
        .collect::<Vec<_>>();
    meerkat_core::SessionHistoryPage {
        session_id: meerkat_core::types::SessionId::new(),
        message_count: messages.len(),
        offset: 0,
        limit: None,
        has_more: false,
        messages,
    }
}

/// Wait for a full round-trip: guesser-a discusses with both peers, then a guess
/// is sent to the artist for the current round.
async fn wait_for_artist_guess_after_discussion(
    handle: &MobHandle,
    service: &dyn MobSessionService,
    label: &str,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let artist_guess_received = if let Some(sid) = handle
            .resolve_bridge_session_id(&AgentIdentity::from("artist"))
            .await
            && let Ok(page) = service
                .read_history(
                    &sid,
                    meerkat_core::SessionHistoryQuery {
                        offset: 0,
                        limit: None,
                    },
                )
                .await
        {
            current_round_artist_received_guess(&page)
        } else {
            false
        };

        let discussion_complete = if let Some(sid) = handle
            .resolve_bridge_session_id(&AgentIdentity::from("guesser-a"))
            .await
            && let Ok(page) = service
                .read_history(
                    &sid,
                    meerkat_core::SessionHistoryQuery {
                        offset: 0,
                        limit: None,
                    },
                )
                .await
        {
            current_round_discussion_completed(&page, label)
        } else {
            false
        };

        if artist_guess_received && discussion_complete {
            return true;
        }
        if Instant::now() > deadline {
            return false;
        }
        sleep(Duration::from_secs(2)).await;
    }
}

/// Dump the cross-agent conversation for a round.
/// `skip_before` skips messages before this index per member (so repeated rounds don't replay old history).
async fn print_conversation(
    handle: &MobHandle,
    service: &dyn MobSessionService,
    skip_before: usize,
) {
    println!();
    println!("  ┌─────────────────────────────────────────────────────────");
    println!("  │ CONVERSATION");
    println!("  ├─────────────────────────────────────────────────────────");

    // Collect all messages with timestamps from all members, then sort chronologically.
    let mut all_messages: Vec<(String, String, String)> = Vec::new();

    let members = handle.list_members().await;
    for member in &members {
        let name = member.agent_identity.to_string();
        let Some(session_id) = handle
            .resolve_bridge_session_id(&member.agent_identity)
            .await
        else {
            continue;
        };
        let Ok(page) = service
            .read_history(
                &session_id,
                meerkat_core::SessionHistoryQuery {
                    offset: skip_before,
                    limit: None,
                },
            )
            .await
        else {
            continue;
        };

        for (msg_idx, msg) in page.messages.iter().enumerate() {
            let (role, text) = match msg {
                meerkat_core::types::Message::User(u) => {
                    // Peer messages and injected content arrive as user messages.
                    let t = meerkat_core::types::text_content(&u.content);
                    // Skip image-only messages (just show "[image]")
                    let display = if t.is_empty() && meerkat_core::has_images(&u.content) {
                        "[image received]".to_string()
                    } else {
                        t
                    };
                    ("←recv", display)
                }
                meerkat_core::types::Message::Assistant(a) => ("said", a.content.clone()),
                meerkat_core::types::Message::BlockAssistant(ba) => {
                    // Extract text + tool send calls
                    let mut parts = Vec::new();
                    for b in &ba.blocks {
                        match b {
                            meerkat_core::types::AssistantBlock::Text { text, .. } => {
                                if !text.trim().is_empty() {
                                    parts.push(text.clone());
                                }
                            }
                            meerkat_core::types::AssistantBlock::ToolUse {
                                name: tool_name,
                                args,
                                ..
                            } => {
                                if matches!(
                                    tool_name.as_str(),
                                    "send" | "send_message" | "send_request" | "send_response"
                                ) && let Ok(v) =
                                    serde_json::from_str::<serde_json::Value>(args.get())
                                {
                                    let peer = v
                                        .get("to")
                                        .or_else(|| v.get("peer"))
                                        .and_then(|p| p.as_str())
                                        .unwrap_or("?");
                                    let body = v.get("body").and_then(|b| b.as_str()).unwrap_or("");
                                    let kind = match tool_name.as_str() {
                                        "send_request" => v
                                            .get("intent")
                                            .and_then(|intent| intent.as_str())
                                            .map(|intent| format!("request:{intent}"))
                                            .unwrap_or_else(|| "request".to_string()),
                                        "send_response" => v
                                            .get("status")
                                            .and_then(|status| status.as_str())
                                            .map(|status| format!("response:{status}"))
                                            .unwrap_or_else(|| "response".to_string()),
                                        _ => v
                                            .get("kind")
                                            .and_then(|kind| kind.as_str())
                                            .unwrap_or("message")
                                            .to_string(),
                                    };
                                    parts.push(format!("[{tool_name} {kind} → {peer}] {body}"));
                                }
                            }
                            _ => {}
                        }
                    }
                    ("said", parts.join(" "))
                }
                meerkat_core::types::Message::ToolResults { results, .. } => {
                    let rendered = results
                        .iter()
                        .map(|result| {
                            format!(
                                "[tool_result {} error={}] {}",
                                result.tool_use_id,
                                result.is_error,
                                meerkat_core::types::text_content(&result.content)
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(" ");
                    ("tool", rendered)
                }
                _ => continue,
            };
            if text.is_empty() || text.len() < 3 {
                continue;
            }
            // Sort key: member name + message index for chronological ordering per member
            all_messages.push((
                format!("{name}:{msg_idx:04}"),
                format!("{name} {role}"),
                text,
            ));
        }
    }

    all_messages.sort_by(|a, b| a.0.cmp(&b.0));

    for (_, speaker, text) in &all_messages {
        // Truncate very long messages (char-boundary safe)
        let display = if text.chars().count() > 200 {
            let end: String = text.chars().take(200).collect();
            format!("{end}...")
        } else {
            text.clone()
        };
        // Indent continuation lines
        let lines: Vec<&str> = display.lines().collect();
        if let Some(first) = lines.first() {
            println!("  │ {speaker:>10}: {first}");
            for line in &lines[1..] {
                println!("  │             {line}");
            }
        }
    }
    println!("  └─────────────────────────────────────────────────────────");
    println!();
}

// ---------------------------------------------------------------------------
// The test
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "lane:e2e-smoke"]
async fn e2e_pictionary_multimodal_comms_stress() {
    // Init tracing so RUST_LOG output is visible.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();

    // Check all 3 provider keys
    if first_env(&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]).is_none() {
        eprintln!("Skipping: ANTHROPIC_API_KEY not set");
        return;
    }
    let Some(gemini_key) = first_env(&["RKAT_GEMINI_API_KEY", "GEMINI_API_KEY"]) else {
        eprintln!("Skipping: GEMINI_API_KEY not set");
        return;
    };
    if first_env(&["RKAT_OPENAI_API_KEY", "OPENAI_API_KEY"]).is_none() {
        eprintln!("Skipping: OPENAI_API_KEY not set");
        return;
    }

    println!("\n============================================================");
    println!("  AI PICTIONARY — Multimodal Cross-Model Comms Stress Test");
    println!("  Image gen: Gemini flash-image-preview (raw reqwest)");
    println!("  Artist:    Claude Sonnet 4.5 (validates guesses)");
    println!("  Guesser A: Claude Opus 4.6 (lead — literal/shapes)");
    println!("  Guesser B: Gemini 3.1 Pro (emotions/mood)");
    println!("  Guesser C: GPT-5.4 (context/narrative)");
    println!("============================================================\n");

    let t = Instant::now();
    let (handle, service, _tmp) = setup_mob().await.expect("mob setup failed");
    spawn_and_wait(&handle).await.expect("member spawn failed");
    assert!(
        wait_for_comms_mesh_ready(&handle, service.as_ref(), Duration::from_secs(60)).await,
        "public comms mesh did not converge after wait_for_ready()",
    );
    assert!(
        wait_for_member_histories_to_settle(
            &handle,
            service.as_ref(),
            Duration::from_secs(90),
            Duration::from_secs(5),
        )
        .await,
        "member histories never settled after startup",
    );
    println!(
        "All 4 members active. [setup: {:.1}s]\n",
        t.elapsed().as_secs_f64()
    );

    let rounds = [
        ("lighthouse", "Round 1 — easy: concrete object"),
        ("loneliness", "Round 2 — medium: abstract concept"),
        ("the feeling of déjà vu", "Round 3 — hard: Rorschach"),
    ];

    let test_start = Instant::now();
    let mut passed = 0;
    for (round_idx, (secret_word, label)) in rounds.iter().enumerate() {
        let round_start = Instant::now();
        println!("--- {label}: \"{secret_word}\" ---");

        // 1. Generate image
        let t = Instant::now();
        println!("  [1/4] Generating image...");
        let gen_prompt = format!(
            "Generate a simple artistic image representing: \"{secret_word}\". \
             No text/letters/words in the image. Visual only."
        );
        let (img_data, mime) = match generate_image(&gemini_key, &gen_prompt).await {
            Ok(r) => r,
            Err(e) => {
                assert!(
                    round_idx != 0,
                    "Round 1 image generation failed — cannot validate pipeline: {e}"
                );
                eprintln!("  Image gen failed: {e} — skipping round");
                continue;
            }
        };
        println!(
            "  Image: {} bytes ({mime}) [{:.1}s]",
            img_data.len(),
            t.elapsed().as_secs_f64()
        );

        // 2. Brief the artist on the secret word
        let t = Instant::now();
        println!("  [2/4] Briefing artist...");
        let artist = handle
            .member(&AgentIdentity::from("artist"))
            .await
            .expect("artist");
        artist
            .send(
                ContentInput::Text(format!(
                    "SECRET WORD this round: \"{secret_word}\". \
                     Wait for a guess from one of the guessers."
                )),
                HandlingMode::Queue,
            )
            .await
            .expect("artist brief");
        println!("  Artist briefed [{:.1}s]", t.elapsed().as_secs_f64());

        // 3. Give the image to the artist, who must forward it via send_message
        let t = Instant::now();
        println!("  [3/4] Asking artist to forward image via send_message...");
        ensure_artist_image_delivery(
            &handle,
            service.as_ref(),
            &artist,
            "guesser-a",
            ArtistForwardImage {
                label,
                media_type: &mime,
                data: &img_data,
            },
            Duration::from_secs(90),
        )
        .await
        .unwrap_or_else(|e| panic!("artist image delivery failed: {e}"));

        println!("  Image forwarded [{:.1}s]", t.elapsed().as_secs_f64());
        for guesser_name in ["guesser-a", "guesser-b", "guesser-c"] {
            let guesser_identity = AgentIdentity::from(guesser_name);
            let members = handle.list_members().await;
            let guesser_status = members
                .iter()
                .find(|m| m.agent_identity == guesser_identity)
                .map(|m| m.status);
            if let Some(sid) = handle.resolve_bridge_session_id(&guesser_identity).await
                && let Ok(page) = service
                    .read_history(
                        &sid,
                        meerkat_core::SessionHistoryQuery {
                            offset: 0,
                            limit: None,
                        },
                    )
                    .await
            {
                let has_image = page.messages.iter().any(|msg| match msg {
                    meerkat_core::types::Message::User(u) => meerkat_core::has_images(&u.content),
                    _ => false,
                });
                let has_drew = page.messages.iter().any(|msg| match msg {
                    meerkat_core::types::Message::User(u) => {
                        meerkat_core::types::text_content(&u.content).contains("drew")
                    }
                    _ => false,
                });
                println!(
                    "  [DEBUG] {guesser_name}: msgs={} has_image={has_image} has_drew_text={has_drew} status={guesser_status:?}",
                    page.messages.len(),
                );
            }
        }

        // 4. Wait for verdict
        let t = Instant::now();
        println!("  [4/4] Waiting for discussion + guess + validation (up to 3 min)...");

        // DEBUG: After a short delay, dump guesser-a's raw history for round 1
        if round_idx == 0 {
            sleep(Duration::from_secs(15)).await;
            if let Some(sid) = handle
                .resolve_bridge_session_id(&AgentIdentity::from("guesser-a"))
                .await
                && let Ok(page) = service
                    .read_history(
                        &sid,
                        meerkat_core::SessionHistoryQuery {
                            offset: 0,
                            limit: None,
                        },
                    )
                    .await
            {
                println!(
                    "\n  === DEBUG: guesser-a raw history ({} messages) ===",
                    page.messages.len()
                );
                for (i, msg) in page.messages.iter().enumerate() {
                    let (role, summary) = match msg {
                        meerkat_core::types::Message::System(s) => {
                            ("system", format!("[{}B]", s.content.len()))
                        }
                        meerkat_core::types::Message::SystemNotice(notice) => (
                            "system_notice",
                            format!("{:?}: {}", notice.kind, notice.body),
                        ),
                        meerkat_core::types::Message::User(u) => {
                            let has_img = meerkat_core::has_images(&u.content);
                            let text = meerkat_core::types::text_content(&u.content);
                            let preview: String = text.chars().take(120).collect();
                            (
                                "user",
                                format!(
                                    "blocks={} has_img={has_img} text={preview}",
                                    u.content.len()
                                ),
                            )
                        }
                        meerkat_core::types::Message::Assistant(a) => {
                            let preview: String = a.content.chars().take(120).collect();
                            (
                                "assistant",
                                format!("tools={} text={preview}", a.tool_calls.len()),
                            )
                        }
                        meerkat_core::types::Message::BlockAssistant(ba) => {
                            let text_blocks = ba
                                .blocks
                                .iter()
                                .filter(|b| {
                                    matches!(b, meerkat_core::types::AssistantBlock::Text { .. })
                                })
                                .count();
                            let tool_blocks = ba
                                .blocks
                                .iter()
                                .filter(|b| {
                                    matches!(b, meerkat_core::types::AssistantBlock::ToolUse { .. })
                                })
                                .count();
                            (
                                "block_assistant",
                                format!(
                                    "blocks={} text={text_blocks} tool_use={tool_blocks}",
                                    ba.blocks.len()
                                ),
                            )
                        }
                        meerkat_core::types::Message::ToolResults { results, .. } => {
                            ("tool_results", format!("count={}", results.len()))
                        }
                    };
                    println!("    [{i}] {role}: {summary}");
                }
                println!("  === END DEBUG ===\n");
            }
        }
        // Success criteria: guesser-a hears from both peers and then sends a
        // guess to the artist for the current round, regardless of correctness.
        let guess_reached_artist = wait_for_artist_guess_after_discussion(
            &handle,
            service.as_ref(),
            label,
            Duration::from_secs(300),
        )
        .await;

        if guess_reached_artist {
            println!(
                "  ✓ Discussion completed and a guess reached the artist [wait: {:.1}s, round: {:.1}s]",
                t.elapsed().as_secs_f64(),
                round_start.elapsed().as_secs_f64()
            );
            passed += 1;
        } else {
            println!(
                "  ✗ Timed out — no post-discussion guess reached the artist [wait: {:.1}s, round: {:.1}s]",
                t.elapsed().as_secs_f64(),
                round_start.elapsed().as_secs_f64()
            );
        }

        // Show ALL messages (no offset) so we can see the full discussion.
        let t_conv = Instant::now();
        print_conversation(&handle, service.as_ref(), 0).await;
        println!(
            "  [conversation dump: {:.1}s]",
            t_conv.elapsed().as_secs_f64()
        );

        // Round 1 (easy) must pass to validate the pipeline works
        assert!(
            round_idx != 0 || guess_reached_artist,
            "Round 1 (\"{secret_word}\") must pass — \
             validates multimodal + comms pipeline"
        );
        println!();
    }

    println!("============================================================");
    println!("  RESULTS: {passed}/3 rounds correct");
    println!("  Total time: {:.1}s", test_start.elapsed().as_secs_f64());
    println!("  (Round 1 required; rounds 2-3 are stretch goals)");
    println!("============================================================\n");
}
