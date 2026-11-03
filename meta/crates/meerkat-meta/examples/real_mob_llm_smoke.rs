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
use meerkat_mob::ids::AgentIdentity;
use meerkat_mob::{MobBuilder, MobDefinition, MobStorage, ProfileName, SpawnMemberSpec};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = std::env::var("OPENAI_API_KEY")
        .map_err(|_| "Set OPENAI_API_KEY to run real_mob_llm_smoke")?;

    let model = std::env::var("RKAT_SMOKE_MODEL").unwrap_or_else(|_| "gpt-5.5".to_string());
    println!("REAL_SMOKE: using model={model}");

    let temp_dir = tempfile::tempdir()?;
    let store_path = temp_dir.path().join("sessions");
    std::fs::create_dir_all(&store_path)?;

    let factory = AgentFactory::new(&store_path).comms(true);
    let config = Config::default();
    let session_service = Arc::new(build_ephemeral_service(factory, config, 16));

    let definition = MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "real-smoke-mob"

[profiles.lead]
model = "{model}"
external_addressable = true

[profiles.lead.tools]
comms = true
"#
    ))
    .expect("parse mob definition");

    let handle = MobBuilder::new(definition, MobStorage::in_memory())
        .with_session_service(session_service)
        .allow_ephemeral_sessions(true)
        .create()
        .await?;

    let status = handle.status().await?;
    println!(
        "REAL_SMOKE: mob created id={} status={:?}",
        handle.mob_id(),
        status
    );

    Box::pin(
        handle.spawn_spec(
            SpawnMemberSpec::new(ProfileName::from("lead"), AgentIdentity::from("lead-1"))
                .with_initial_message(meerkat_core::ContentInput::Text(
                    "You are the lead. Keep responses brief and direct.".to_string(),
                )),
        ),
    )
    .await?;
    Box::pin(
        handle.spawn_spec(
            SpawnMemberSpec::new(ProfileName::from("worker"), AgentIdentity::from("worker-1"))
                .with_initial_message(meerkat_core::ContentInput::Text(
                    "You are a worker. Help the lead when asked.".to_string(),
                )),
        ),
    )
    .await?;
    handle
        .wire(
            AgentIdentity::from("lead-1"),
            AgentIdentity::from("worker-1"),
        )
        .await?;
    println!("REAL_SMOKE: spawned lead-1 + worker-1 and wired them");

    let mut agent_stream = handle
        .subscribe_agent_events(&AgentIdentity::from("lead-1"))
        .await?;
    handle
        .member(&AgentIdentity::from("lead-1"))
        .await?
        .send(
            "Give a 2-sentence plan to implement a Rust CLI smoke test and mention one task to delegate to worker-1.",
            HandlingMode::Queue,
        )
        .await?;
    println!("REAL_SMOKE: message sent to lead-1");

    let mut final_result: Option<String> = None;
    let deadline = tokio::time::Instant::now() + Duration::from_mins(2);

    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let evt = tokio::time::timeout(remaining.min(Duration::from_secs(5)), agent_stream.next())
            .await
            .ok()
            .flatten()
            .map(|envelope| envelope.payload);

        let Some(evt) = evt else {
            continue;
        };

        match evt {
            AgentEvent::TextDelta { delta, .. } => {
                print!("{delta}");
            }
            AgentEvent::TextComplete { content, .. } => {
                println!("\nREAL_SMOKE: text_complete={content}");
            }
            AgentEvent::ToolCallRequested { name, .. } => {
                println!("\nREAL_SMOKE: tool_call_requested={name}");
            }
            AgentEvent::ToolResultReceived { name, is_error, .. } => {
                println!("\nREAL_SMOKE: tool_result name={name} is_error={is_error}");
            }
            AgentEvent::InteractionComplete { result, .. } => {
                final_result = Some(result);
                println!("\nREAL_SMOKE: interaction_complete");
                break;
            }
            AgentEvent::InteractionFailed { reason, .. } => {
                // meerkat 0.7: the failure cause is typed; the display string
                // is its Display projection.
                println!("\nREAL_SMOKE: interaction_failed error={reason}");
                break;
            }
            _ => {}
        }
    }

    if final_result.is_none() {
        println!("REAL_SMOKE: no terminal result before timeout");
    } else if let Some(result) = final_result {
        println!("REAL_SMOKE: final_result={result}");
    }

    let members = handle.list_members().await;
    println!("REAL_SMOKE: members={}", members.len());
    for member in members {
        println!(
            "  - {} profile={} wired_to={:?}",
            member.agent_identity, member.role, member.wired_to
        );
    }

    handle.retire_all().await?;
    println!("REAL_SMOKE: retired all members");

    Ok(())
}
