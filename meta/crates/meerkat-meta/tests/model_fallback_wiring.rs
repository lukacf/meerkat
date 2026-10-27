//! Meta carries policy into Meerkat materialization; it does not choose targets.

#![allow(clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use meerkat_client::TestClient;
use meerkat_core::config::{ModelFallbackConfig, ModelFallbackPolicy, ModelFallbackTarget};
use meerkat_core::service::{CreateSessionRequest, SessionError};
use meerkat_core::{ContentInput, HandlingMode};
use meerkat_meta::{MemberTurnOptions, SessionHook, UnifiedRuntime};
use meerkat_mob::{MobDefinition, SpawnMemberSpec};

#[derive(Default)]
struct CaptureBuild {
    policies: Mutex<Vec<Option<ModelFallbackConfig>>>,
}

#[async_trait]
impl SessionHook for CaptureBuild {
    async fn before_create(&self, request: &mut CreateSessionRequest) -> Result<(), SessionError> {
        self.policies.lock().expect("capture lock").push(
            request
                .build
                .as_ref()
                .and_then(|build| build.model_fallback.clone()),
        );
        Ok(())
    }
}

fn definition(runtime_policy: &str, profile_policy: &str) -> MobDefinition {
    let mob_id = format!("fallback-wiring-{}", uuid::Uuid::new_v4());
    MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "{mob_id}"
{runtime_policy}
[profiles.worker]
model = "gpt-5.5"
provider = "openai"
runtime_mode = "turn_driven"
external_addressable = true
[profiles.worker.tools]
comms = true
{profile_policy}
"#
    ))
    .expect("definition")
}

#[tokio::test]
async fn fallback_runtime_and_profile_policy_reach_materialized_member_turns() {
    let runtime_table = "[runtime.model_fallback]\nenabled = false\n\
        [runtime.model_fallback.policy]\ntrigger_after_attempts = 9\n";
    let profile_table = "[profiles.worker.model_fallback]\nenabled = false\n";
    for (runtime_policy, profile_policy, expected) in [
        ("", "", None),
        (
            runtime_table,
            "",
            Some(ModelFallbackConfig {
                enabled: Some(false),
                policy: ModelFallbackPolicy {
                    trigger_after_attempts: 9,
                    ..Default::default()
                },
                ..Default::default()
            }),
        ),
        (
            runtime_table,
            profile_table,
            Some(ModelFallbackConfig {
                enabled: Some(false),
                ..Default::default()
            }),
        ),
    ] {
        let capture = Arc::new(CaptureBuild::default());
        let host = meerkat::Config {
            model_fallback: ModelFallbackConfig {
                enabled: Some(true),
                chain: vec![ModelFallbackTarget {
                    model: "gpt-5.5".to_string(),
                    provider: Some(meerkat_core::Provider::OpenAI),
                    auth_binding: None,
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let runtime = Box::pin(
            UnifiedRuntime::builder()
                .definition(definition(runtime_policy, profile_policy))
                .meerkat_config(host)
                .default_llm_client(Arc::new(TestClient::default()))
                .session_hook(capture.clone())
                .build(),
        )
        .await
        .expect("runtime");
        runtime
            .spawn(SpawnMemberSpec::new("worker", "fallback-worker"))
            .await
            .expect("materialized member");
        let admission = runtime
            .start_member_turn(
                "fallback-worker",
                ContentInput::Text("fallback wiring marker".to_string()),
                HandlingMode::Queue,
                MemberTurnOptions::new(),
                None,
            )
            .await
            .expect("admitted turn");
        tokio::time::timeout(Duration::from_secs(30), admission.turn.wait())
            .await
            .expect("bounded turn")
            .expect("successful member turn");
        assert_eq!(
            capture.policies.lock().expect("capture").as_slice(),
            std::slice::from_ref(&expected),
            "the materialization must carry the whole declared table, or preserve absence"
        );
        let shutdown = runtime.shutdown().await;
        assert!(shutdown.cleanup_completed(), "{shutdown:?}");
    }
}

#[test]
fn fallback_definition_subtrees_refuse_unknown_keys_and_invalid_chains() {
    for table in [
        "[runtime.model_fallback]\nenabled = true\n",
        "[runtime.model_fallback]\nuse_catalog_default_chain = true\n",
        "[runtime.model_fallback.policy]\ncross_providre = true\n",
        "[profiles.worker.model_fallback]\nenabled = true\n",
        "[profiles.worker.model_fallback.policy]\ntrigger_after_attempts = 0\n",
    ] {
        let text = format!(
            "[mob]\nid = \"fallback-invalid\"\n[profiles.worker]\nmodel = \"gpt-5.5\"\n{table}"
        );
        if let Ok(definition) = MobDefinition::from_toml(&text) {
            let diagnostics = meerkat_mob::validate_definition(&definition);
            assert!(
                diagnostics.iter().any(|diagnostic| {
                    diagnostic.code == meerkat_mob::DiagnosticCode::InvalidModelFallback
                        && diagnostic.severity == meerkat_mob::DiagnosticSeverity::Error
                }),
                "accepted invalid policy at both parse and semantic validation: {table}"
            );
        }
    }
}
