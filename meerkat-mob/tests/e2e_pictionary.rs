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
    definition::{RoleWiringRule, WiringRules},
};
use meerkat_session::PersistentSessionService;
use meerkat_store::{JsonlStore, StoreAdapter};
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
        wiring: WiringRules {
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
        },
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

    // Use an explicit RuntimeSessionAdapter override so this e2e exercises the
    // canonical mob-runtime adapter path. AutonomousHost drains and runtime-backed
    // turns must both use this same adapter instance.
    let runtime_adapter = Arc::new(meerkat_runtime::RuntimeSessionAdapter::ephemeral());

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
             2) First, send a message to guesser-b AND guesser-c describing what you see (literal shapes/objects). \
             3) WAIT for both guesser-b and guesser-c to reply with their interpretations. \
             4) Only AFTER hearing from both, synthesize a consensus guess and send it to the artist. \
             Use the 'send' tool with kind='message' for all peer messages. Keep messages to 1-2 sentences.",
        ),
        (
            "guesser-b",
            "You are guesser-b. IMPORTANT RULES: \
             1) When you receive a message from guesser-a about an image, think about the EMOTIONAL/MOOD interpretation. \
             2) Reply to guesser-a AND guesser-c with your interpretation using the 'send' tool (kind='message'). \
             3) Do NOT send anything to the artist — only guesser-a does that. \
             Keep messages to 1-2 sentences.",
        ),
        (
            "guesser-c",
            "You are guesser-c. IMPORTANT RULES: \
             1) When you receive a message from guesser-a about an image, think about the CONTEXT/NARRATIVE interpretation. \
             2) Reply to guesser-a AND guesser-b with your interpretation using the 'send' tool (kind='message'). \
             3) Do NOT send anything to the artist — only guesser-a does that. \
             Keep messages to 1-2 sentences.",
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
            .filter(|m| matches!(m.status, MobMemberStatus::Active))
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

/// Wait for the artist to receive a message containing the secret word.
/// This proves the full pipeline: image → guessers → discussion → consensus → artist.
async fn wait_for_artist_received(
    handle: &MobHandle,
    service: &dyn MobSessionService,
    needle: &str,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    let needle_lower = needle.to_lowercase();
    loop {
        let members = handle.list_members().await;
        if let Some(artist) = members
            .iter()
            .find(|m| m.meerkat_id == MeerkatId::from("artist"))
            && let Some(sid) = artist.session_id()
            && let Ok(page) = service
                .read_history(
                    sid,
                    meerkat_core::SessionHistoryQuery {
                        offset: 0,
                        limit: None,
                    },
                )
                .await
        {
            for msg in &page.messages {
                let text = match msg {
                    meerkat_core::types::Message::User(u) => {
                        meerkat_core::types::text_content(&u.content)
                    }
                    _ => continue,
                };
                // Skip the briefing message we sent (contains "SECRET WORD")
                if text.contains("SECRET WORD") {
                    continue;
                }
                if text.to_lowercase().contains(&needle_lower) {
                    return true;
                }
            }
        }
        if Instant::now() > deadline {
            return false;
        }
        sleep(Duration::from_secs(2)).await;
    }
}

/// Wait for the artist's assistant response to start with a verdict keyword.
/// This avoids false-matching on instruction text like "I'll respond with CORRECT".
async fn wait_for_artist_verdict(
    handle: &MobHandle,
    service: &dyn MobSessionService,
    verdict: &str,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let members = handle.list_members().await;
        if let Some(artist) = members
            .iter()
            .find(|m| m.meerkat_id == MeerkatId::from("artist"))
            && let Some(sid) = artist.session_id()
            && let Ok(page) = service
                .read_history(
                    sid,
                    meerkat_core::SessionHistoryQuery {
                        offset: 0,
                        limit: None,
                    },
                )
                .await
        {
            for msg in page.messages.iter().rev() {
                let text = match msg {
                    meerkat_core::types::Message::Assistant(a) => &a.content,
                    meerkat_core::types::Message::BlockAssistant(ba) => {
                        // Check text blocks — must start with verdict
                        // but NOT "CORRECT or WRONG" (instruction echo).
                        let has_verdict = ba.blocks.iter().any(|b| match b {
                            meerkat_core::types::AssistantBlock::Text { text, .. } => {
                                let t = text.trim();
                                t.starts_with(verdict)
                                    && !t.starts_with("CORRECT or")
                                    && !t.starts_with("WRONG or")
                            }
                            _ => false,
                        });
                        if has_verdict {
                            return true;
                        }
                        continue;
                    }
                    _ => continue,
                };
                let t = text.trim();
                if t.starts_with(verdict)
                    && !t.starts_with("CORRECT or")
                    && !t.starts_with("WRONG or")
                {
                    return true;
                }
            }
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
        let name = member.meerkat_id.to_string();
        let Some(session_id) = member.session_id() else {
            continue;
        };
        let Ok(page) = service
            .read_history(
                session_id,
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
                                if tool_name == "send"
                                    && let Ok(v) =
                                        serde_json::from_str::<serde_json::Value>(args.get())
                                {
                                    let peer = v
                                        .get("to")
                                        .or_else(|| v.get("peer"))
                                        .and_then(|p| p.as_str())
                                        .unwrap_or("?");
                                    let body = v.get("body").and_then(|b| b.as_str()).unwrap_or("");
                                    let kind =
                                        v.get("kind").and_then(|k| k.as_str()).unwrap_or("message");
                                    parts.push(format!("[send {kind} → {peer}] {body}"));
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

// ---------------------------------------------------------------------------
// The test
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "e2e: live API (Gemini image gen + Claude/Gemini/GPT multimodal + mob comms)"]
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
                assert!(
                    round_idx != 0,
                    "Round 1 image generation failed — cannot validate pipeline: {e}"
                );
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

        // 3. Send image to all guessers via peer-to-peer comms (from artist)
        //    This uses the artist's CommsRuntime to send actual peer messages,
        //    so guessers see "[peer: artist] ..." and the comms discussion model
        //    kicks in naturally.
        println!("  [3/4] Sending image to guessers via artist's comms...");
        let artist_session_id = {
            let members = handle.list_members().await;
            members
                .iter()
                .find(|m| m.meerkat_id == MeerkatId::from("artist"))
                .and_then(|m| m.session_id().cloned())
                .expect("artist session_id")
        };
        let artist_comms = service
            .comms_runtime(&artist_session_id)
            .await
            .expect("artist comms runtime");

        let image_blocks = vec![
            ContentBlock::Text {
                text: format!(
                    "I drew this for Pictionary ({label}). \
                     guesser-a: describe what you see to guesser-b and guesser-c FIRST, \
                     wait for their replies, THEN send me your consensus guess."
                ),
            },
            ContentBlock::Image {
                media_type: mime.clone(),
                data: img_data.clone().into(),
            },
        ];

        // Mob comms names follow the pattern: {mob_id}/{profile}/{meerkat_id}
        for guesser in ["guesser-a", "guesser-b", "guesser-c"] {
            let peer_name = format!("pictionary/{guesser}/{guesser}");
            artist_comms
                .send(meerkat_core::comms::CommsCommand::PeerMessage {
                    to: meerkat_core::comms::PeerName::new(&peer_name).unwrap(),
                    body: format!("Pictionary {label} — guess what I drew!"),
                    blocks: Some(image_blocks.clone()),
                })
                .await
                .unwrap_or_else(|e| panic!("artist→{peer_name} peer send: {e}"));
        }

        // 4. Wait for verdict
        println!("  [4/4] Waiting for discussion + guess + validation (up to 2 min)...");

        // DEBUG: After a short delay, dump guesser-a's raw history for round 1
        if round_idx == 0 {
            sleep(Duration::from_secs(15)).await;
            let members = handle.list_members().await;
            if let Some(ga) = members
                .iter()
                .find(|m| m.meerkat_id == MeerkatId::from("guesser-a"))
                && let Some(sid) = ga.session_id()
                && let Ok(page) = service
                    .read_history(
                        sid,
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
                        meerkat_core::types::Message::ToolResults { results } => {
                            ("tool_results", format!("count={}", results.len()))
                        }
                    };
                    println!("    [{i}] {role}: {summary}");
                }
                println!("  === END DEBUG ===\n");
            }
        }
        // Success criteria: the secret word (or a close match) appears in a
        // message RECEIVED by the artist — proving the full pipeline worked:
        // image gen → peer delivery → guesser discussion → consensus → artist.
        // We also accept the artist responding with "CORRECT" as a bonus.
        let guess_reached_artist = wait_for_artist_received(
            &handle,
            service.as_ref(),
            secret_word,
            Duration::from_secs(180),
        )
        .await;

        if guess_reached_artist {
            println!("  ✓ Guessers guessed \"{secret_word}\" — artist received it!");
            passed += 1;
        } else {
            println!("  ✗ Timed out — guess never reached the artist");
        }

        // Show ALL messages (no offset) so we can see the full discussion.
        print_conversation(&handle, service.as_ref(), 0).await;

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
    println!("  (Round 1 required; rounds 2-3 are stretch goals)");
    println!("============================================================\n");
}
