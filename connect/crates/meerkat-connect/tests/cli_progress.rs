//! Native-event presentation with a deterministic model and fresh local stores.
//! No provider, browser, packaged MCP, installation or frontend activation.

use std::{
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use futures_util::{Stream, StreamExt};
use meerkat::{Config, LlmClient, LlmError, LlmEvent, LlmRequest};
use meerkat_connect::{
    ConnectSession, RealmOptions, SessionOptions, progress::TurnProgress, terminal::committed_text,
};
use meerkat_core::{Message, Provider, SessionBuildOptions};
use meerkat_runtime::completion::CompletionOutcome;

struct PausedModel {
    release: Arc<tokio::sync::Notify>,
    requests: Arc<AtomicUsize>,
}

#[async_trait]
impl LlmClient for PausedModel {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        let release = self.release.clone();
        let usage = meerkat_core::TurnUsage::host_declared(
            Provider::OpenAI,
            &request.model,
            meerkat_core::Usage::default(),
        );
        Box::pin(
            futures_util::stream::iter([
                Ok(LlmEvent::ReasoningDelta {
                    delta: "PRIVATE_REASONING_MUST_NOT_RENDER".into(),
                }),
                Ok(LlmEvent::TextDelta {
                    delta: "Checking service and frontend choices.".into(),
                    meta: None,
                }),
            ])
            .chain(futures_util::stream::once(async move {
                release.notified().await;
                Ok(LlmEvent::UsageUpdate { usage })
            }))
            .chain(futures_util::stream::iter([Ok(LlmEvent::Done {
                outcome: meerkat::LlmDoneOutcome::success(meerkat::StopReason::EndTurn),
            })])),
        )
    }

    fn provider(&self) -> Provider {
        Provider::OpenAI
    }
    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

async fn open(
    root: &std::path::Path,
    release: Arc<tokio::sync::Notify>,
    requests: Arc<AtomicUsize>,
) -> ConnectSession {
    let realm = RealmOptions {
        context_root: root.to_path_buf(),
        state_root: Some(root.join("state")),
        user_config_root: Some(root.join("home")),
    };
    ConnectSession::open_with_config(
        realm.resolve("progress").unwrap(),
        Config::default(),
        SessionOptions {
            model: meerkat::CreateSessionModelResolutionRequest {
                model: Some("gpt-5.4".into()),
                ..Default::default()
            },
            build: SessionBuildOptions {
                llm_client_override: Some(meerkat::encode_llm_client_override_for_service(
                    Arc::new(PausedModel { release, requests }),
                )),
                ..Default::default()
            },
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn native_assistant_text_arrives_before_completion_and_cancellation_still_joins() {
    for cancel in [false, true] {
        tokio::time::timeout(Duration::from_secs(20), async {
            let directory = tempfile::tempdir().unwrap();
            let release = Arc::new(tokio::sync::Notify::new());
            let requests = Arc::new(AtomicUsize::new(0));
            let session = open(directory.path(), release.clone(), requests.clone()).await;
            // Same ordering as the CLI: subscribe before native admission.
            let mut progress = TurnProgress::new(session.id().clone(), session.events().await.unwrap());
            let mut output = Vec::new();
            progress.begin(&mut output).unwrap();
            let (admission, completion) = session.submit("Guide my service and frontend setup".into()).await.unwrap();
            let wait = completion.wait();
            tokio::pin!(wait);
            while !String::from_utf8_lossy(&output).contains("Checking service and frontend choices.") {
                tokio::select! {
                    result = &mut wait => panic!("native completion arrived before model release: {result:?}"),
                    result = progress.next(&mut output) => result.unwrap(),
                }
            }
            assert!(!String::from_utf8_lossy(&output).contains("PRIVATE_REASONING"));
            assert!(tokio::time::timeout(Duration::from_millis(20), &mut wait).await.is_err(), "live output must precede native completion");

            // The progress view has no background writer. Main pauses its
            // polling while present_approval owns the same terminal reader.
            progress.pause_for_prompt(&mut output).unwrap();
            let paused_output = output.clone();
            if cancel {
                session.cancel(&admission).await.unwrap();
                let outcome = wait.await.unwrap();
                assert!(matches!(outcome, CompletionOutcome::Cancelled));
                assert!(committed_text(&outcome).is_err());
                assert_eq!(output, paused_output, "cancellation must not print a committed answer");
            } else {
                release.notify_one();
                let outcome = wait.await.unwrap();
                assert_eq!(output, paused_output, "native events must not print while the presenter is paused");
                let text = committed_text(&outcome).unwrap();
                assert_eq!(text, "Checking service and frontend choices.");
                progress.finish_committed(text, &mut output).unwrap();
                assert_eq!(String::from_utf8_lossy(&output).matches("Checking service and frontend choices.").count(), 1, "the committed copy must not duplicate the live answer");
                assert!(String::from_utf8_lossy(&output).ends_with("Setup turn completed.\n"));
            }
            drop(progress);
            session.close().await.unwrap();
            assert_eq!(requests.load(Ordering::SeqCst), 1, "presentation never replays a native input");
        }).await.expect("bounded native progress and cleanup");
    }
}

#[tokio::test]
async fn dropping_progress_does_not_cancel_or_complete_native_work() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let directory = tempfile::tempdir().unwrap();
        let release = Arc::new(tokio::sync::Notify::new());
        let requests = Arc::new(AtomicUsize::new(0));
        let session = open(directory.path(), release.clone(), requests.clone()).await;
        let progress = TurnProgress::new(session.id().clone(), session.events().await.unwrap());
        let (admission, completion) = session
            .submit("Explain setup choices".into())
            .await
            .unwrap();
        drop(progress);
        let wait = completion.wait();
        tokio::pin!(wait);
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut wait)
                .await
                .is_err()
        );
        release.notify_one();
        let outcome = wait.await.unwrap();
        assert_eq!(
            committed_text(&outcome).unwrap(),
            "Checking service and frontend choices."
        );
        // A stale exact receipt remains harmless after the real completion.
        session.cancel(&admission).await.unwrap();
        session.close().await.unwrap();
        assert_eq!(requests.load(Ordering::SeqCst), 1);
    })
    .await
    .expect("bounded dropped presentation cleanup");
}
