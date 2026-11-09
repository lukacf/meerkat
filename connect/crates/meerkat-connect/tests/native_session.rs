//! Local native persistence/lifecycle qualification with a deterministic model.
//! These tests make no provider, install, account or frontend readiness claim.

use std::{
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use futures_util::{Stream, StreamExt};
use meerkat::{Config, LlmClient, LlmError, LlmEvent, LlmRequest};
use meerkat_connect::{ConnectSession, RealmOptions, SessionOptions, discover_realms};
use meerkat_core::{Message, Provider, SessionBuildOptions};
use meerkat_runtime::completion::CompletionOutcome;

struct RecordingClient {
    inner: meerkat_client::TestClient,
    requests: Arc<Mutex<Vec<Vec<Message>>>>,
    stall_after: Option<usize>,
    started: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl LlmClient for RecordingClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        self.inner.project_replay_messages(messages)
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let count = {
            let mut requests = self.requests.lock().expect("request capture");
            requests.push(request.messages.clone());
            requests.len()
        };
        self.started.notify_one();
        if self.stall_after.is_some_and(|threshold| count > threshold) {
            Box::pin(futures_util::stream::pending())
        } else {
            self.inner.stream(request)
        }
    }

    fn provider(&self) -> Provider {
        Provider::OpenAI
    }
    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

fn realms(root: &std::path::Path) -> RealmOptions {
    RealmOptions {
        context_root: root.to_path_buf(),
        state_root: Some(root.join("realms")),
        user_config_root: Some(root.join("home")),
    }
}

fn options(client: Arc<dyn LlmClient>) -> SessionOptions {
    SessionOptions {
        model: meerkat::CreateSessionModelResolutionRequest {
            model: Some("gpt-5.4".into()),
            ..Default::default()
        },
        build: SessionBuildOptions {
            llm_client_override: Some(meerkat::encode_llm_client_override_for_service(client)),
            ..Default::default()
        },
    }
}

fn user_texts(messages: &[Message]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|message| match message {
            Message::User(user) => Some(user.text_content()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn complete_messages_survive_two_turns_and_native_store_reopen() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let temp = tempfile::tempdir().expect("temp root");
        let selection = realms(temp.path());
        assert!(
            discover_realms(&selection)
                .await
                .expect("empty inventory")
                .is_empty()
        );
        assert!(
            !temp.path().join("realms").exists(),
            "inventory must not create state"
        );
        let requests = Arc::new(Mutex::new(Vec::new()));
        let client = Arc::new(RecordingClient {
            inner: meerkat_client::TestClient::for_provider(Provider::OpenAI),
            requests: requests.clone(),
            stall_after: None,
            started: Arc::new(tokio::sync::Notify::new()),
        });
        let session = ConnectSession::open_with_config(
            selection.resolve("family").expect("resolve"),
            Config::default(),
            options(client),
        )
        .await
        .expect("open native session");
        let id = session.id().clone();
        let mut events = session.events().await.expect("native subscription");
        let messages = [
            "Read my latest gmails; also help my family talk through Telegram.",
            " Keep the existing host and ask me about the audience. ",
        ];
        for message in messages {
            let outcome = session
                .submit(message.into())
                .await
                .expect("admit")
                .1
                .wait()
                .await
                .expect("native completion");
            assert!(
                matches!(outcome, CompletionOutcome::Completed(_)),
                "{outcome:?}"
            );
        }
        let captured = requests.lock().expect("request capture").clone();
        assert_eq!(
            user_texts(captured.last().expect("second native model request")),
            messages
        );
        let mut text_events = 0;
        while text_events < 2 {
            let event = events.next().await.expect("native event");
            if let meerkat_core::AgentEvent::TextDelta { delta, .. } = event.payload {
                assert_eq!(delta, "ok");
                text_events += 1;
            }
        }
        drop(events);
        session.close().await.expect("native teardown");
        let inventory = discover_realms(&selection).await.expect("stored inventory");
        assert_eq!(inventory.len(), 1);
        assert_eq!(inventory[0].realm_id, "family");
        let (_, persistence) =
            meerkat::open_realm_persistence_in(&temp.path().join("realms"), "family", None, None)
                .await
                .expect("reopen native stores");
        let stored = persistence
            .session_store()
            .load(&id)
            .await
            .expect("read durable row")
            .expect("durable session");
        assert_eq!(user_texts(stored.messages()), messages);
        let (_, other) =
            meerkat::open_realm_persistence_in(&temp.path().join("realms"), "work", None, None)
                .await
                .expect("other realm");
        assert!(
            other
                .session_store()
                .load(&id)
                .await
                .expect("other realm read")
                .is_none()
        );
    })
    .await
    .expect("native lifecycle must settle within bound");
}

#[tokio::test]
async fn active_native_turn_cancels_without_model_completion_or_replay() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let temp = tempfile::tempdir().expect("temp root");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let started = Arc::new(tokio::sync::Notify::new());
        let client = Arc::new(RecordingClient {
            inner: meerkat_client::TestClient::for_provider(Provider::OpenAI),
            requests: requests.clone(),
            stall_after: Some(0),
            started: started.clone(),
        });
        let session = ConnectSession::open_with_config(
            realms(temp.path()).resolve("cancel").expect("resolve"),
            Config::default(),
            options(client),
        )
        .await
        .expect("open");
        let (admission, completion) = session
            .submit("Explain frontend host choices".into())
            .await
            .expect("admit");
        started.notified().await;
        session.cancel(&admission).await.expect("native cancel");
        let outcome = completion.wait().await.expect("cancellation completion");
        assert!(
            matches!(outcome, CompletionOutcome::Cancelled),
            "{outcome:?}"
        );
        session.close().await.expect("native teardown");
        assert_eq!(
            requests.lock().expect("request capture").len(),
            1,
            "no replay"
        );
    })
    .await
    .expect("cancellation and teardown must settle within bound");
}

#[tokio::test]
async fn stale_cancellation_cannot_interrupt_a_newer_native_input() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let temp = tempfile::tempdir().expect("temp root");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let started = Arc::new(tokio::sync::Notify::new());
        let session = ConnectSession::open_with_config(
            realms(temp.path())
                .resolve("stale-cancel")
                .expect("resolve"),
            Config::default(),
            options(Arc::new(RecordingClient {
                inner: meerkat_client::TestClient::for_provider(Provider::OpenAI),
                requests: requests.clone(),
                stall_after: Some(1),
                started: started.clone(),
            })),
        )
        .await
        .expect("open");
        let (first, completed) = session
            .submit("First question".into())
            .await
            .expect("first admission");
        assert!(matches!(
            completed.wait().await.expect("first completion"),
            CompletionOutcome::Completed(_)
        ));
        started.notified().await;
        let (second, pending) = session
            .submit("Second question".into())
            .await
            .expect("second admission");
        started.notified().await;
        session
            .cancel(&first)
            .await
            .expect("stale exact-input cancellation");
        let waiting = pending.wait();
        tokio::pin!(waiting);
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut waiting)
                .await
                .is_err(),
            "stale cancellation stopped the newer input"
        );
        session.cancel(&second).await.expect("cancel second input");
        assert!(matches!(
            waiting.await.expect("second completion"),
            CompletionOutcome::Cancelled
        ));
        session.close().await.expect("close");
        assert_eq!(requests.lock().expect("requests").len(), 2);
    })
    .await
    .expect("bounded stale cancellation");
}

#[tokio::test]
async fn queued_input_cancellation_leaves_active_predecessor_running() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let temp = tempfile::tempdir().expect("temp root");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let started = Arc::new(tokio::sync::Notify::new());
        let session = ConnectSession::open_with_config(
            realms(temp.path())
                .resolve("queued-cancel")
                .expect("resolve"),
            Config::default(),
            options(Arc::new(RecordingClient {
                inner: meerkat_client::TestClient::for_provider(Provider::OpenAI),
                requests: requests.clone(),
                stall_after: Some(0),
                started: started.clone(),
            })),
        )
        .await
        .expect("open");
        let (active, active_completion) = session
            .submit("Active question".into())
            .await
            .expect("active admission");
        started.notified().await;
        let (queued, queued_completion) = session
            .submit("Queued question".into())
            .await
            .expect("queued admission");
        session.cancel(&queued).await.expect("cancel queued input");
        let queued_outcome = queued_completion.wait().await.expect("queued completion");
        assert!(
            !matches!(queued_outcome, CompletionOutcome::Completed(_)),
            "queued input cannot complete"
        );
        let waiting = active_completion.wait();
        tokio::pin!(waiting);
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut waiting)
                .await
                .is_err(),
            "queued cancellation stopped its predecessor"
        );
        session.cancel(&active).await.expect("cancel active input");
        assert!(matches!(
            waiting.await.expect("active completion"),
            CompletionOutcome::Cancelled
        ));
        session.close().await.expect("close");
        assert_eq!(
            requests.lock().expect("requests").len(),
            1,
            "queued input was never dispatched"
        );
    })
    .await
    .expect("bounded queued cancellation");
}

#[tokio::test]
async fn cross_realm_build_override_is_rejected_before_state_creation() {
    let temp = tempfile::tempdir().expect("temp root");
    let selection = realms(temp.path());
    let mut options = SessionOptions::default();
    options.build.realm_id = Some(meerkat_core::RealmId::parse("other").expect("realm id"));
    let result = ConnectSession::open_with_config(
        selection.resolve("family").expect("resolve"),
        Config::default(),
        options,
    )
    .await;
    assert!(result.is_err());
    assert!(!temp.path().join("realms").exists());
}

struct CleanupGate {
    entered: tokio::sync::Notify,
    released: Mutex<bool>,
    changed: std::sync::Condvar,
}

impl CleanupGate {
    fn release(&self) {
        *self.released.lock().expect("cleanup gate") = true;
        self.changed.notify_all();
    }
}

struct GatedStream(Arc<CleanupGate>);

impl Stream for GatedStream {
    type Item = Result<LlmEvent, LlmError>;
    fn poll_next(
        self: Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        std::task::Poll::Pending
    }
}

impl Drop for GatedStream {
    fn drop(&mut self) {
        self.0.entered.notify_one();
        // This is only a deterministic model-transport cleanup fixture. The
        // native runtime must join it; no alternate runtime executor is used.
        // The hard bound also releases the worker on a failed test assertion.
        let released = self.0.released.lock().expect("cleanup gate");
        let _result = self
            .0
            .changed
            .wait_timeout_while(released, Duration::from_secs(10), |released| !*released)
            .expect("cleanup wait");
    }
}

struct CleanupClient {
    gate: Arc<CleanupGate>,
    started: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl LlmClient for CleanupClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }
    fn stream<'a>(
        &'a self,
        _: &'a LlmRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        self.started.notify_one();
        Box::pin(GatedStream(self.gate.clone()))
    }
    fn provider(&self) -> Provider {
        Provider::OpenAI
    }
    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn close_joins_native_cleanup_beyond_the_ordinary_unregister_caller_grace() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let temp = tempfile::tempdir().expect("temp root");
        let gate = Arc::new(CleanupGate {
            entered: tokio::sync::Notify::new(),
            released: Mutex::new(false),
            changed: std::sync::Condvar::new(),
        });
        let started = Arc::new(tokio::sync::Notify::new());
        let session = ConnectSession::open_with_config(
            realms(temp.path()).resolve("cleanup").expect("resolve"),
            Config::default(),
            options(Arc::new(CleanupClient {
                gate: gate.clone(),
                started: started.clone(),
            })),
        )
        .await
        .expect("open");
        let (_, completion) = session
            .submit("Discuss the compatible host".into())
            .await
            .expect("admit");
        started.notified().await;
        let mut closing = tokio::spawn(session.close());
        gate.entered.notified().await;
        // Published 0.8.51's ordinary unregister caller grace is two seconds.
        // Closing the process must join the native cleanup beyond that grace.
        let early = tokio::time::timeout(Duration::from_millis(2250), &mut closing).await;
        gate.release();
        assert!(
            early.is_err(),
            "close returned before native transport cleanup: {early:?}"
        );
        closing
            .await
            .expect("close task")
            .expect("terminal native teardown");
        let outcome = completion
            .wait()
            .await
            .expect("native completion after teardown");
        assert!(
            !matches!(outcome, CompletionOutcome::Completed(_)),
            "stalled model cannot complete"
        );
    })
    .await
    .expect("bounded delayed cleanup");
}
