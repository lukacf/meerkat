#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args,
    clippy::collapsible_if,
    clippy::redundant_clone,
    clippy::needless_raw_string_hashes,
    clippy::single_match,
    clippy::redundant_closure_for_method_calls,
    clippy::redundant_pattern_matching,
    clippy::ignored_unit_patterns,
    clippy::clone_on_copy,
    clippy::manual_assert,
    clippy::unwrap_in_result,
    clippy::useless_vec
)]
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use meerkat::{AgentEvent, AgentFactory, Config, build_ephemeral_service};
use meerkat_core::types::HandlingMode;
// meerkat 0.7: the MeerkatId alias was deleted; member ids are AgentIdentity.
use meerkat_mob::ids::AgentIdentity as MeerkatId;
use meerkat_mob::{MobBuilder, MobDefinition, MobStorage, ProfileName, SpawnMemberSpec};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ =
        std::env::var("OPENAI_API_KEY").map_err(|_| "Set OPENAI_API_KEY to run streaming_smoke")?;
    let model = std::env::var("RKAT_SMOKE_MODEL").unwrap_or_else(|_| "gpt-5.5".to_string());

    println!("STREAM_SMOKE: model={model}");

    let temp_dir = tempfile::tempdir()?;
    let store_path = temp_dir.path().join("sessions");
    std::fs::create_dir_all(&store_path)?;

    let factory = AgentFactory::new(&store_path).comms(true);
    let config = Config::default();
    let session_service = Arc::new(build_ephemeral_service(factory, config, 16));

    let mut definition = MobDefinition::from_toml(
        r#"
[mob]
id = "test-mob"

[profiles.lead]
model = "gpt-5.5"
external_addressable = true

[profiles.lead.tools]
comms = true
"#,
    )
    .expect("parse mob definition");
    for binding in definition.profiles.values_mut() {
        if let Some(profile) = binding.as_inline_mut() {
            profile.model = model.clone();
        }
    }

    let handle = MobBuilder::new(definition, MobStorage::in_memory())
        .with_session_service(session_service)
        .allow_ephemeral_sessions(true)
        .create()
        .await?;

    Box::pin(
        handle.spawn_spec(
            SpawnMemberSpec::new(ProfileName::from("lead"), MeerkatId::from("lead-1"))
                .with_initial_message(meerkat_core::ContentInput::Text(
                    "You are concise. 1 short paragraph max.".to_string(),
                )),
        ),
    )
    .await?;
    Box::pin(
        handle.spawn_spec(
            SpawnMemberSpec::new(ProfileName::from("worker"), MeerkatId::from("worker-1"))
                .with_initial_message(meerkat_core::ContentInput::Text(
                    "You are a helper worker.".to_string(),
                )),
        ),
    )
    .await?;
    handle
        .wire(MeerkatId::from("lead-1"), MeerkatId::from("worker-1"))
        .await?;
    println!("STREAM_SMOKE: spawned + wired lead-1 <-> worker-1");

    // 1) Per-agent session event stream
    let mut agent_stream = handle
        .subscribe_agent_events(&MeerkatId::from("lead-1"))
        .await?;
    println!("STREAM_SMOKE: subscribed to lead-1 agent events");

    // 2) Send a message (fire-and-forget)
    handle
        .member(&MeerkatId::from("lead-1"))
        .await?
        .send(
            "Give a short plan for smoke-testing a Rust CLI and delegate one concrete task to worker-1.",
            HandlingMode::Queue,
        )
        .await?;
    println!("STREAM_SMOKE: message sent to lead-1");

    // 3) Mob-wide merged attributed event stream
    // meerkat 0.7: subscribe_mob_events returns Result<MobEventRouterHandle, _>.
    let mut mob_router = handle.subscribe_mob_events().await?;
    println!("STREAM_SMOKE: subscribed to mob event router");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(35);
    let mut seen_terminal = false;
    let mut agent_count = 0_u32;
    let mut mob_count = 0_u32;

    while tokio::time::Instant::now() < deadline && !seen_terminal {
        tokio::select! {
            maybe_agent_evt = agent_stream.next() => {
                if let Some(envelope) = maybe_agent_evt {
                    agent_count += 1;
                    match &envelope.payload {
                        AgentEvent::InteractionComplete { .. } | AgentEvent::InteractionFailed { .. } => {
                            seen_terminal = true;
                        }
                        _ => {}
                    }
                    println!(
                        "AGENT_STREAM {} seq={} source={:?} payload={:?}",
                        envelope.event_id,
                        envelope.seq,
                        envelope.source,
                        envelope.payload,
                    );
                }
            }
            maybe_mob_evt = mob_router.event_rx.recv() => {
                if let Some(attr) = maybe_mob_evt {
                    mob_count += 1;
                    println!(
                        "MOB_ROUTER source={} profile={} payload={:?} seq={}",
                        attr.source,
                        attr.role,
                        attr.envelope.payload,
                        attr.envelope.seq,
                    );
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(200)) => {}
        }
    }

    println!(
        "STREAM_SMOKE: counts agent_stream={} mob_router={}",
        agent_count, mob_count
    );

    mob_router.cancel();
    handle.retire_all().await?;
    Ok(())
}
