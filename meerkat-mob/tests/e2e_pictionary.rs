#![cfg(all(feature = "integration-real-tests", not(target_arch = "wasm32")))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//!
//! AI Pictionary — multimodal cross-model comms stress test.
//!
//! A mob of 4 agents (3 different LLM providers) plays Pictionary:
//!
//! - **Test harness** generates images via Gemini flash-image-preview (raw reqwest)
//! - **Test harness** injects images into guessers as multimodal content
//!   via `MemberHandle::send(ContentInput::Blocks)` — tests runtime ingress with image blocks
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
//!   cargo test -p meerkat-mob --test e2e_pictionary \
//!     --features integration-real-tests -- --ignored --test-threads=1 --nocapture
//! ```

use meerkat::{AgentFactory, Config, FactoryAgentBuilder};
use meerkat_core::types::{ContentBlock, ContentInput, HandlingMode};
use meerkat_mob::{
    MeerkatId, MobBuilder, MobDefinition, MobHandle, MobId, MobMemberStatus, MobRuntimeMode,
    MobSessionService, MobStorage, Profile, ProfileName, ToolConfig,
};
use meerkat_session::EphemeralSessionService;
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
        .json(&body)
        .timeout(Duration::from_secs(60))
        .send()
        .await?;

    let status = resp.status();
    let text = resp.text().await?;
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
        comms_profile("claude-sonnet-4-5", "Artist — validates guesses"),
    );
    profiles.insert(
        ProfileName::from("guesser-a"),
        comms_profile("claude-opus-4-6", "guesser-a (Opus) — lead, literal/shapes"),
    );
    profiles.insert(
        ProfileName::from("guesser-b"),
        comms_profile(
            "gemini-3.1-pro-preview",
            "guesser-b (Gemini) — emotions/mood",
        ),
    );
    profiles.insert(
        ProfileName::from("guesser-c"),
        comms_profile("gpt-5.4", "guesser-c (GPT) — context/narrative"),
    );

    MobDefinition {
        id: MobId::from("pictionary"),
        orchestrator: None,
        profiles,
        mcp_servers: BTreeMap::new(),
        wiring: Default::default(),
        skills: BTreeMap::new(),
        backend: Default::default(),
        flows: BTreeMap::new(),
        topology: None,
        supervisor: None,
        limits: None,
        spawn_policy: None,
        event_router: None,
    }
}

async fn setup_mob()
-> Result<(MobHandle, Arc<dyn MobSessionService>, TempDir), Box<dyn std::error::Error>> {
    let temp_dir = TempDir::new()?;
    let store_path = temp_dir.path().join("sessions");
    std::fs::create_dir_all(&store_path)?;

    let factory = AgentFactory::new(&store_path).comms(true);
    let config = Config::default();
    let session_service: Arc<EphemeralSessionService<FactoryAgentBuilder>> =
        Arc::new(meerkat::build_ephemeral_service(factory, config, 16));
    let mob_service: Arc<dyn MobSessionService> = session_service.clone();

    let handle = MobBuilder::new(pictionary_definition(), MobStorage::in_memory())
        .with_session_service(mob_service.clone())
        .allow_ephemeral_sessions(true)
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
             Wait for guessers to send you their guess, then say CORRECT or WRONG.",
        ),
        (
            "guesser-a",
            "You are guesser-a (lead). When you see an image, discuss with \
             guesser-b and guesser-c, then send the consensus guess to 'artist'.",
        ),
        (
            "guesser-b",
            "You are guesser-b. When you see an image, share your emotional/mood \
             interpretation with guesser-a and guesser-c.",
        ),
        (
            "guesser-c",
            "You are guesser-c. When you see an image, share your context/narrative \
             interpretation with guesser-a and guesser-b.",
        ),
    ];

    for (name, msg) in spawns {
        handle
            .spawn(
                ProfileName::from(name),
                MeerkatId::from(name),
                Some(ContentInput::Text(msg.to_string())),
            )
            .await
            .map_err(|e| format!("spawn {name}: {e}"))?;
    }

    // Wait for all to become active
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let members = handle.list_members().await;
        let active_count = members
            .iter()
            .filter(|m| matches!(m.status, MobMemberStatus::Active { .. }))
            .count();
        if active_count == 4 {
            return Ok(());
        }
        assert!(
            Instant::now() < deadline,
            "timed out: only {active_count}/4 active"
        );
        sleep(Duration::from_millis(500)).await;
    }
}

/// Check if any member's history contains a needle.
async fn history_contains(
    handle: &MobHandle,
    service: &dyn MobSessionService,
    needle: &str,
) -> bool {
    let members = handle.list_members().await;
    for member in &members {
        let Some(session_id) = member.session_id() else {
            continue;
        };
        if let Ok(page) = service
            .read_history(
                session_id,
                meerkat_core::SessionHistoryQuery {
                    offset: 0,
                    limit: None,
                },
            )
            .await
        {
            let blob: String = page
                .messages
                .iter()
                .map(|m| serde_json::to_string(m).unwrap_or_default())
                .collect::<Vec<_>>()
                .join("\n");
            if blob.to_lowercase().contains(&needle.to_lowercase()) {
                return true;
            }
        }
    }
    false
}

/// Dump the cross-agent conversation for a round.
async fn print_conversation(handle: &MobHandle, service: &dyn MobSessionService) {
    println!();
    println!("  ┌─────────────────────────────────────────────────────────");
    println!("  │ CONVERSATION");
    println!("  ├─────────────────────────────────────────────────────────");

    // Collect all messages with timestamps from all members, then sort chronologically.
    let mut all_messages: Vec<(String, String, String)> = Vec::new(); // (timestamp, speaker, text)

    let members = handle.list_members().await;
    for member in &members {
        let name = member.meerkat_id.to_string();
        let Some(session_id) = member.session_id() else {
            continue;
        };
        let Ok(page) = service
            .read_history(
                session_id,
                meerkat_core::SessionHistoryQuery {
                    offset: 0,
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
                                if tool_name == "send" {
                                    if let Ok(v) =
                                        serde_json::from_str::<serde_json::Value>(args.get())
                                    {
                                        let peer =
                                            v.get("peer").and_then(|p| p.as_str()).unwrap_or("?");
                                        let body =
                                            v.get("body").and_then(|b| b.as_str()).unwrap_or("");
                                        let kind = v
                                            .get("kind")
                                            .and_then(|k| k.as_str())
                                            .unwrap_or("message");
                                        parts.push(format!("[send {kind} → {peer}] {body}"));
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    ("said", parts.join(" "))
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

async fn wait_for(
    handle: &MobHandle,
    service: &dyn MobSessionService,
    needle: &str,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if history_contains(handle, service, needle).await {
            return true;
        }
        if Instant::now() > deadline {
            return false;
        }
        sleep(Duration::from_secs(2)).await;
    }
}

// ---------------------------------------------------------------------------
// The test
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "e2e: live API (Gemini image gen + Claude/Gemini/GPT multimodal + mob comms)"]
async fn e2e_pictionary_multimodal_comms_stress() {
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

    let (handle, service, _tmp) = setup_mob().await.expect("mob setup failed");
    spawn_and_wait(&handle).await.expect("member spawn failed");
    println!("All 4 members active.\n");

    let rounds = [
        ("lighthouse", "Round 1 — easy: concrete object"),
        ("loneliness", "Round 2 — medium: abstract concept"),
        ("the feeling of déjà vu", "Round 3 — hard: Rorschach"),
    ];

    let mut passed = 0;

    for (round_idx, (secret_word, label)) in rounds.iter().enumerate() {
        println!("--- {label}: \"{secret_word}\" ---");

        // 1. Generate image
        println!("  [1/4] Generating image...");
        let gen_prompt = format!(
            "Generate a simple artistic image representing: \"{secret_word}\". \
             No text/letters/words in the image. Visual only."
        );
        let (img_data, mime) = match generate_image(&gemini_key, &gen_prompt).await {
            Ok(r) => r,
            Err(e) => {
                if round_idx == 0 {
                    panic!("Round 1 image generation failed — cannot validate pipeline: {e}");
                }
                eprintln!("  Image gen failed: {e} — skipping round");
                continue;
            }
        };
        println!("  Image: {} bytes ({mime})", img_data.len());

        // 2. Brief the artist on the secret word
        println!("  [2/4] Briefing artist...");
        let artist = handle
            .member(&MeerkatId::from("artist"))
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

        // 3. Send image to all guessers (multimodal runtime ingress)
        println!("  [3/4] Sending image to guessers...");
        let image_content = ContentInput::Blocks(vec![
            ContentBlock::Text {
                text: format!(
                    "[Pictionary {label}] The artist drew this. Discuss with the other \
                     guessers, then guesser-a: send your consensus guess to 'artist'."
                ),
            },
            ContentBlock::Image {
                media_type: mime.clone(),
                data: img_data.clone(),
                source_path: None,
            },
        ]);

        for name in ["guesser-a", "guesser-b", "guesser-c"] {
            let member = handle.member(&MeerkatId::from(name)).await.expect(name);
            member
                .send(image_content.clone(), HandlingMode::Queue)
                .await
                .unwrap_or_else(|e| panic!("send→{name}: {e}"));
        }

        // 4. Wait for verdict
        println!("  [4/4] Waiting for discussion + guess + validation (up to 2 min)...");
        let got_correct = wait_for(
            &handle,
            service.as_ref(),
            "CORRECT",
            Duration::from_secs(120),
        )
        .await;
        let got_wrong = if !got_correct {
            wait_for(&handle, service.as_ref(), "WRONG", Duration::from_secs(5)).await
        } else {
            false
        };

        if got_correct {
            println!("  ✓ Guessers identified \"{secret_word}\"!");
            passed += 1;
        } else if got_wrong {
            println!("  ✗ Wrong guess (artist said WRONG)");
        } else {
            println!("  ✗ Timed out — no verdict from artist");
        }

        print_conversation(&handle, service.as_ref()).await;

        // Round 1 (easy) must pass to validate the pipeline works
        if round_idx == 0 && !got_correct {
            panic!(
                "Round 1 (\"{secret_word}\") must pass — \
                 validates multimodal + comms pipeline"
            );
        }
        println!();
    }

    println!("============================================================");
    println!("  RESULTS: {passed}/3 rounds correct");
    println!("  (Round 1 required; rounds 2-3 are stretch goals)");
    println!("============================================================\n");
}
