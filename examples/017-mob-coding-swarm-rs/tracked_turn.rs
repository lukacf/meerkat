use std::io::Write;
use std::time::Duration;

use meerkat_core::types::HandlingMode;
use meerkat_mob::{BoundedResultSpec, MemberHandle, MemberTurnOptions};

pub type DemoError = Box<dyn std::error::Error + Send + Sync>;

/// Observe the exact admitted turn, not the mob's unrelated lifecycle log.
pub async fn report_turn(
    member: &MemberHandle,
    prompt: &str,
    timeout: Duration,
    output: &mut impl Write,
) -> Result<(), DemoError> {
    let spec = BoundedResultSpec::new("lead-response", 16_384)?;
    let response = tokio::time::timeout(timeout, async {
        let turn = member
            .start_turn_bounded(
                prompt.to_owned(),
                HandlingMode::Queue,
                MemberTurnOptions::default(),
                None,
                spec.clone(),
            )
            .await?;
        Ok::<_, DemoError>(turn.wait_bounded(spec).await?)
    })
    .await
    .map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "timed out observing the lead turn; completion is unknown, not successful",
        )
    })??;
    let result = response.result().result();
    writeln!(
        output,
        "\nLead response ({:?}): {}",
        result.status(),
        result.text()
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use meerkat::{AgentFactory, Config, EphemeralSessionService, FactoryAgentBuilder};
    use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
    use meerkat_mob::{
        AgentIdentity, BoundedTurnFailure, BoundedTurnWaitError, MemberDeliveryReceipt, MobBuilder,
        MobDefinition, MobHandle, MobId, MobRuntimeMode, MobStorage, SpawnMemberSpec,
    };
    use tokio::sync::Notify;

    #[derive(Default)]
    struct ScriptedClient {
        entered: Notify,
        release: Notify,
        fail: bool,
        failures_emitted: AtomicUsize,
        tools: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl LlmClient for ScriptedClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(&'a self, request: &'a LlmRequest) -> meerkat_client::types::LlmStream<'a> {
            Box::pin(async_stream::stream! {
                *self.tools.lock().unwrap() =
                    request.tools.iter().map(|tool| tool.name.to_string()).collect();
                self.entered.notify_one();
                self.release.notified().await;
                if self.fail {
                    self.failures_emitted.fetch_add(1, Ordering::SeqCst);
                    yield Err(LlmError::InvalidRequest { message: "synthetic lead failure".into() });
                    return;
                }
                yield Ok(LlmEvent::TextDelta { delta: "Exact synthetic lead answer".into(), meta: None });
                yield Ok(LlmEvent::UsageUpdate {
                    usage: meerkat_core::TurnUsage::host_declared(
                        meerkat_core::Provider::Anthropic,
                        &request.model,
                        meerkat_core::Usage::default(),
                    ),
                });
                yield Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::Success { stop_reason: meerkat_core::StopReason::EndTurn },
                });
            })
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::Anthropic
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    async fn fixture(client: Arc<ScriptedClient>) -> (tempfile::TempDir, MobHandle, MemberHandle) {
        let dir = tempfile::tempdir_in(".").unwrap();
        let store_path = dir.path().join("sessions");
        std::fs::create_dir_all(&store_path).unwrap();
        let factory = AgentFactory::new(store_path).comms(true);
        let mut builder = FactoryAgentBuilder::new(factory, Config::default());
        builder.default_llm_client = Some(client);
        let service = Arc::new(EphemeralSessionService::new(builder, 4));
        let mut definition = MobDefinition::from_toml(include_str!("mob.toml")).unwrap();
        definition.id = MobId::from(format!("tracked-turn-{}", uuid::Uuid::new_v4()));
        let handle = MobBuilder::new(definition, MobStorage::in_memory())
            .with_session_service(service)
            .allow_ephemeral_sessions(true)
            .create()
            .await
            .unwrap();
        handle
            .spawn_spec(
                SpawnMemberSpec::new("lead", "lead-1")
                    .with_runtime_mode(MobRuntimeMode::TurnDriven),
            )
            .await
            .unwrap();
        let member = handle.member(&AgentIdentity::from("lead-1")).await.unwrap();
        (dir, handle, member)
    }

    fn run_test<F, Fut>(make: F)
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()>,
    {
        meerkat_runtime::host_stack::HostStackBudget::default_budget()
            .run("tracked-turn-test", move || async {
                tokio::time::timeout(Duration::from_secs(20), make())
                    .await
                    .unwrap();
            })
            .unwrap();
    }

    #[test]
    fn historical_events_cannot_complete_blocked_turn_and_answer_precedes_retirement() {
        run_test(|| {
            Box::pin(async {
                let client = Arc::new(ScriptedClient::default());
                let (_dir, handle, member) = fixture(client.clone()).await;
                assert!(!handle.poll_events(0, 50).await.unwrap().is_empty());
                let report = tokio::spawn(async move {
                    let mut output = Vec::new();
                    report_turn(
                        &member,
                        "synthetic prompt",
                        Duration::from_secs(10),
                        &mut output,
                    )
                    .await?;
                    Ok::<_, DemoError>(output)
                });
                client.entered.notified().await;
                assert!(
                    !report.is_finished(),
                    "lifecycle history is not turn completion"
                );
                assert_eq!(handle.list_members().await.len(), 1);
                client.release.notify_one();
                let output = String::from_utf8(report.await.unwrap().unwrap()).unwrap();
                assert!(output.contains("Exact synthetic lead answer"));
                assert_eq!(
                    handle.list_members().await.len(),
                    1,
                    "answer is visible before retirement"
                );

                // The reusable template must teach names in the actual model-visible catalog.
                let template: toml::Value = toml::from_str(include_str!("mob.toml")).unwrap();
                let role = template["skills"]["orchestrator"]["content"]
                    .as_str()
                    .unwrap();
                let taught = role
                    .split("Use mob tools (")
                    .nth(1)
                    .unwrap()
                    .split(')')
                    .next()
                    .unwrap();
                let tools = client.tools.lock().unwrap().clone();
                for name in taught.split(", ") {
                    assert!(
                        tools.iter().any(|tool| tool == name),
                        "unavailable operator: {name}"
                    );
                }
                assert!(!role.contains("mob.complete"));
                handle.retire_all().await.unwrap();
                assert!(handle.list_members().await.is_empty());
            })
        });
    }

    #[test]
    fn tracked_turn_failure_is_an_error_not_a_response() {
        run_test(|| {
            Box::pin(async {
                let client = Arc::new(ScriptedClient {
                    fail: true,
                    ..Default::default()
                });
                let (_dir, handle, member) = fixture(client.clone()).await;
                let expected_session = member.status().await.unwrap().current_session_id.unwrap();
                let report = tokio::spawn(async move {
                    let mut output = Vec::new();
                    let result = report_turn(
                        &member,
                        "synthetic failure",
                        Duration::from_secs(10),
                        &mut output,
                    )
                    .await;
                    (result, output)
                });
                client.entered.notified().await;
                assert!(
                    !report.is_finished(),
                    "setup succeeded; provider is still blocked"
                );
                assert_eq!(client.failures_emitted.load(Ordering::SeqCst), 0);
                client.release.notify_one();
                let (result, output) = report.await.unwrap();
                assert_eq!(client.failures_emitted.load(Ordering::SeqCst), 1);
                let error = result.unwrap_err();
                let failure = error
                    .downcast_ref::<BoundedTurnWaitError<MemberDeliveryReceipt>>()
                    .unwrap_or_else(|| panic!("expected exact-turn failure, got: {error:?}"));
                // The runtime projects the fatal provider failure as termination
                // of this admitted session, not the provider's original prose.
                let BoundedTurnFailure::RuntimeTerminated {
                    session_id, error, ..
                } = failure.failure()
                else {
                    panic!("expected runtime termination, got: {failure:?}");
                };
                assert_eq!(session_id, &expected_session);
                assert_eq!(
                    error.kind,
                    meerkat_core::TurnTerminalCauseKind::FatalFailure
                );
                assert_eq!(
                    error.outcome,
                    Some(meerkat_core::TurnTerminalOutcome::Failed)
                );
                assert!(error.terminal);
                assert!(output.is_empty());
                handle.retire_all().await.unwrap();
            })
        });
    }

    #[test]
    fn observation_deadline_is_an_explicit_error() {
        run_test(|| {
            Box::pin(async {
                let client = Arc::new(ScriptedClient::default());
                let (_dir, handle, member) = fixture(client.clone()).await;
                let mut output = Vec::new();
                let error = report_turn(
                    &member,
                    "synthetic blocked turn",
                    Duration::from_millis(50),
                    &mut output,
                )
                .await
                .unwrap_err();
                assert_eq!(
                    error
                        .downcast_ref::<std::io::Error>()
                        .unwrap_or_else(|| panic!("expected observation timeout, got: {error:?}"))
                        .kind(),
                    std::io::ErrorKind::TimedOut
                );
                assert!(output.is_empty());
                client.release.notify_one();
                handle.retire_all().await.unwrap();
            })
        });
    }
}
