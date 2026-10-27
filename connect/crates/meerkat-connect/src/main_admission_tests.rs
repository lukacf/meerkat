//! Deterministic CLI-helper/native custody checks, not executable SIGINT tests.

use super::*;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;

use async_trait::async_trait;
use futures_util::Stream;
use meerkat::{Config, LlmClient, LlmError, LlmEvent, LlmRequest};
use meerkat_core::{Message, Provider, SessionBuildOptions};
use tokio::sync::{Notify, oneshot};

struct StalledClient {
    inner: meerkat_client::TestClient,
    calls: Arc<AtomicUsize>,
    started: Arc<Notify>,
}

#[async_trait]
impl LlmClient for StalledClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        self.inner.project_replay_messages(messages)
    }

    fn stream<'a>(
        &'a self,
        _request: &'a LlmRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.started.notify_one();
        Box::pin(futures_util::stream::pending())
    }

    fn provider(&self) -> Provider {
        Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

#[derive(Clone, Default)]
struct CapturedOutput {
    bytes: Arc<Mutex<Vec<u8>>>,
    written: Arc<Notify>,
}

impl Write for CapturedOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes.lock().expect("output capture").extend(bytes);
        self.written.notify_one();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct SubmitDrop(Arc<AtomicBool>);

impl Drop for SubmitDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

fn input_id(admission: &AcceptOutcome) -> &meerkat_core::lifecycle::InputId {
    match admission {
        AcceptOutcome::Accepted { input_id, .. } => input_id,
        AcceptOutcome::Deduplicated { existing_id, .. } => existing_id,
        _ => panic!("expected an admitted input"),
    }
}

#[tokio::test]
async fn admission_cancellation_retires_before_held_native_receipt_then_joins_exact_input() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let temp = tempfile::tempdir().expect("temporary realm");
        let calls = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(Notify::new());
        let options = SessionOptions {
            model: meerkat::CreateSessionModelResolutionRequest {
                model: Some("gpt-5.4".into()),
                ..Default::default()
            },
            build: SessionBuildOptions {
                llm_client_override: Some(meerkat::encode_llm_client_override_for_service(
                    Arc::new(StalledClient {
                        inner: meerkat_client::TestClient::for_provider(Provider::OpenAI),
                        calls: calls.clone(),
                        started: started.clone(),
                    }),
                )),
                ..Default::default()
            },
        };
        let storage = RealmOptions {
            context_root: temp.path().to_path_buf(),
            state_root: Some(temp.path().join("realms")),
            user_config_root: Some(temp.path().join("home")),
        }
        .resolve("admission-cancellation")
        .expect("resolve native realm");
        let session = ConnectSession::open_with_config(storage, Config::default(), options)
            .await
            .expect("open native session");
        let (active, active_completion) = session
            .submit("Active predecessor".into())
            .await
            .expect("active admission");
        started.notified().await;

        // An unbound real owner exposes retirement through its existing bind
        // contract. No privileged dispatch or new test-only getter is involved.
        let (approvals, _presenter) =
            LocalApprovals::create(temp.path()).expect("local approval owner");
        let (receipt_ready, observed_receipt) = oneshot::channel();
        let (release_receipt, released_receipt) = oneshot::channel();
        let (interrupt_tx, interrupt_rx) = oneshot::channel();
        let submitted = Arc::new(AtomicUsize::new(0));
        let dropped = Arc::new(AtomicBool::new(false));
        let delivered = Arc::new(AtomicBool::new(false));
        let mut output = CapturedOutput::default();
        let observed_output = output.clone();
        let submit = async {
            let _drop = SubmitDrop(dropped.clone());
            submitted.fetch_add(1, Ordering::SeqCst);
            let receipt = session.submit("Queued input".into()).await?;
            // Native admission has already returned its exact receipt. Hold
            // its delivery, rather than placing a gate before native entry.
            receipt_ready
                .send(receipt.0.clone())
                .expect("receipt observer");
            released_receipt.await.expect("release held receipt");
            delivered.store(true, Ordering::SeqCst);
            Ok(receipt)
        };
        let interrupt = async { interrupt_rx.await.expect("injected interruption") };
        tokio::pin!(interrupt);
        let work = admit_turn(
            &session,
            submit,
            interrupt.as_mut(),
            &approvals,
            &mut output,
        );
        let observation = async {
            let queued = observed_receipt.await.expect("native receipt created");
            interrupt_tx.send(Ok(())).expect("signal original helper");
            let notified = tokio::time::timeout(
                Duration::from_millis(250),
                observed_output.written.notified(),
            )
            .await
            .is_ok();
            let retired = approvals.bind_session(session.id()).is_err();
            let retained = !dropped.load(Ordering::SeqCst);
            let message = String::from_utf8(observed_output.bytes.lock().expect("output").clone())
                .expect("fixed terminal text");
            // Always release and join before asserting, including the failing
            // predecessor baseline, so the test does not abandon native work.
            release_receipt
                .send(())
                .expect("release original admission");
            (queued, notified, retired, retained, message)
        };
        let (result, (queued, notified, retired, retained, message)) =
            tokio::join!(work, observation);
        let (interrupted, completion) = match result.expect("admission helper") {
            TurnAdmission::Running {
                admission,
                completion,
            } => {
                assert_eq!(
                    input_id(&admission),
                    input_id(&queued),
                    "original receipt must be retained"
                );
                session
                    .cancel(&admission)
                    .await
                    .expect("baseline queued cleanup");
                (false, completion.try_wait_with_terminal_outcome().await)
            }
            TurnAdmission::Interrupted(completion) => (true, completion),
            TurnAdmission::NotSubmitted => panic!("native receipt already exists"),
        };
        let queued_outcome = completion.expect("queued completion");
        let waiting = active_completion.wait();
        tokio::pin!(waiting);
        let predecessor_retained = tokio::time::timeout(Duration::from_millis(100), &mut waiting)
            .await
            .is_err();
        session.cancel(&active).await.expect("active cleanup");
        let active_outcome = waiting.await.expect("active completion");
        session.close().await.expect("close native session");

        assert_ne!(
            input_id(&queued),
            input_id(&active),
            "distinct native input receipts"
        );
        assert_eq!(submitted.load(Ordering::SeqCst), 1, "no submit replay");
        assert!(
            delivered.load(Ordering::SeqCst),
            "original admission was joined"
        );
        assert!(retained, "held native admission must not be dropped");
        assert!(predecessor_retained, "do not cancel another active input");
        assert!(matches!(active_outcome, CompletionOutcome::Cancelled));
        assert!(!matches!(
            queued_outcome.outcome(),
            CompletionOutcome::Completed(_)
        ));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "queued input never dispatched"
        );
        assert!(
            notified,
            "pending cancellation must display its waiting message"
        );
        assert!(
            retired,
            "interrupt must retire authority before receipt delivery"
        );
        assert!(message.contains("Cancellation requested."), "{message:?}");
        assert!(interrupted, "joined cancellation must preserve exit intent");
    })
    .await
    .expect("bounded native admission cancellation");
}

async fn immediate_session(root: &std::path::Path) -> ConnectSession {
    let storage = RealmOptions {
        context_root: root.to_path_buf(),
        state_root: Some(root.join("realms")),
        user_config_root: Some(root.join("home")),
    }
    .resolve("admission-controls")
    .expect("resolve native realm");
    ConnectSession::open_with_config(
        storage,
        Config::default(),
        SessionOptions {
            model: meerkat::CreateSessionModelResolutionRequest {
                model: Some("gpt-5.4".into()),
                ..Default::default()
            },
            build: SessionBuildOptions {
                llm_client_override: Some(meerkat::encode_llm_client_override_for_service(
                    Arc::new(meerkat_client::TestClient::for_provider(Provider::OpenAI)),
                )),
                ..Default::default()
            },
        },
    )
    .await
    .expect("open native session")
}

#[tokio::test]
async fn admission_cancellation_before_first_poll_never_submits() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let temp = tempfile::tempdir().expect("temporary realm");
        let session = immediate_session(temp.path()).await;
        let (approvals, _presenter) = LocalApprovals::create(temp.path()).expect("approvals");
        let submitted = AtomicBool::new(false);
        let submit = async {
            submitted.store(true, Ordering::SeqCst);
            session.submit("Must not be submitted".into()).await
        };
        let interrupt = std::future::ready(Ok(()));
        tokio::pin!(interrupt);
        let result = admit_turn(
            &session,
            submit,
            interrupt.as_mut(),
            &approvals,
            &mut Vec::new(),
        )
        .await
        .expect("early interruption");
        let retired = approvals.bind_session(session.id()).is_err();
        let not_submitted = matches!(&result, TurnAdmission::NotSubmitted);
        if let TurnAdmission::Running {
            admission,
            completion,
        } = result
        {
            session
                .cancel(&admission)
                .await
                .expect("unexpected input cleanup");
            completion.wait().await.expect("unexpected input joined");
        }
        session.close().await.expect("close native session");
        assert!(not_submitted);
        assert!(
            !submitted.load(Ordering::SeqCst),
            "no admission poll or replay"
        );
        assert!(retired);
    })
    .await
    .expect("bounded pre-admission interruption");
}

struct BrokenOutput(Arc<Notify>);

impl Write for BrokenOutput {
    fn write(&mut self, _bytes: &[u8]) -> io::Result<usize> {
        self.0.notify_one();
        Err(io::Error::other("fixture output failure"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::other("fixture output failure"))
    }
}

#[tokio::test]
async fn admission_cancellation_joins_receipt_despite_signal_or_output_error() {
    tokio::time::timeout(Duration::from_secs(30), async {
        for output_error in [false, true] {
            let temp = tempfile::tempdir().expect("temporary realm");
            let calls = Arc::new(AtomicUsize::new(0));
            let started = Arc::new(Notify::new());
            let storage = RealmOptions {
                context_root: temp.path().to_path_buf(),
                state_root: Some(temp.path().join("realms")),
                user_config_root: Some(temp.path().join("home")),
            }
            .resolve("admission-io-controls")
            .expect("resolve native realm");
            let session = ConnectSession::open_with_config(
                storage,
                Config::default(),
                SessionOptions {
                    model: meerkat::CreateSessionModelResolutionRequest {
                        model: Some("gpt-5.4".into()),
                        ..Default::default()
                    },
                    build: SessionBuildOptions {
                        llm_client_override: Some(meerkat::encode_llm_client_override_for_service(
                            Arc::new(StalledClient {
                                inner: meerkat_client::TestClient::for_provider(Provider::OpenAI),
                                calls: calls.clone(),
                                started: started.clone(),
                            }),
                        )),
                        ..Default::default()
                    },
                },
            )
            .await
            .expect("open native session");
            // Keep cancellation on a queued input so this control isolates
            // local I/O errors from native in-flight run interruption outcomes.
            let (active, active_completion) = session
                .submit("Active predecessor".into())
                .await
                .expect("active admission");
            started.notified().await;
            let (approvals, _presenter) = LocalApprovals::create(temp.path()).expect("approvals");
            let captured = CapturedOutput::default();
            let mut output: Box<dyn Write> = if output_error {
                Box::new(BrokenOutput(captured.written.clone()))
            } else {
                Box::new(captured.clone())
            };
            let (ready_tx, ready_rx) = oneshot::channel();
            let (release_tx, release_rx) = oneshot::channel();
            let (signal_tx, signal_rx) = oneshot::channel();
            let delivered = AtomicBool::new(false);
            let submitted = AtomicUsize::new(0);
            let submit = async {
                submitted.fetch_add(1, Ordering::SeqCst);
                let receipt = session.submit("Retain this exact input".into()).await?;
                ready_tx.send(()).expect("native receipt observer");
                release_rx.await.expect("release receipt");
                delivered.store(true, Ordering::SeqCst);
                Ok(receipt)
            };
            let interrupt = async { signal_rx.await.expect("injected signal") };
            tokio::pin!(interrupt);
            let work = admit_turn(&session, submit, interrupt.as_mut(), &approvals, &mut output);
            let observation = async {
                ready_rx.await.expect("native receipt exists");
                signal_tx.send(if output_error {
                    Ok(())
                } else {
                    Err(io::Error::other("fixture signal failure"))
                }).expect("inject interruption");
                let reported = tokio::time::timeout(
                    Duration::from_millis(250), captured.written.notified(),
                ).await.is_ok();
                let retired = approvals.bind_session(session.id()).is_err();
                release_tx.send(()).expect("join original receipt");
                (reported, retired)
            };
            let (result, (reported, retired)) = tokio::join!(work, observation);
            let error = match result {
                Err(error) => Some(error),
                Ok(TurnAdmission::Running { admission, completion }) => {
                    session.cancel(&admission).await.expect("unexpected input cleanup");
                    completion.wait().await.expect("unexpected input joined");
                    None
                }
                Ok(TurnAdmission::Interrupted(completion)) => {
                    completion.expect("native input joined");
                    None
                }
                Ok(TurnAdmission::NotSubmitted) => None,
            };
            let waiting = active_completion.wait();
            tokio::pin!(waiting);
            let predecessor_retained = tokio::time::timeout(Duration::from_millis(100), &mut waiting)
                .await
                .is_err();
            session.cancel(&active).await.expect("active cleanup");
            let active_outcome = waiting.await.expect("active completion");
            session.close().await.expect("close after joined native work");
            let expected = if output_error { "write cancellation progress" } else { "listen for cancellation" };
            assert!(
                matches!(&error, Some(ConnectError::Native { operation, .. }) if *operation == expected),
                "expected {expected}, actual deterministic fixture error: {error:?}"
            );
            assert!(reported && retired, "report and retire before receipt delivery");
            assert!(delivered.load(Ordering::SeqCst), "do not drop admission on local I/O error");
            assert_eq!(submitted.load(Ordering::SeqCst), 1, "no retry");
            assert!(predecessor_retained, "do not cancel another active input");
            assert!(matches!(active_outcome, CompletionOutcome::Cancelled));
            assert_eq!(calls.load(Ordering::SeqCst), 1, "queued input never dispatched");
        }
    })
    .await
    .expect("bounded admission I/O controls");
}

#[tokio::test]
async fn admission_cancellation_preserves_submit_error_and_registers_before_submission() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let temp = tempfile::tempdir().expect("temporary realm");
        let session = immediate_session(temp.path()).await;
        let (approvals, _presenter) = LocalApprovals::create(temp.path()).expect("approvals");
        let registered = AtomicBool::new(false);
        let submitted = AtomicUsize::new(0);
        let interrupt = futures_util::future::poll_fn(|_| {
            registered.store(true, Ordering::SeqCst);
            std::task::Poll::<io::Result<()>>::Pending
        });
        tokio::pin!(interrupt);
        // This sentinel exercises exact error propagation, not a native
        // admission rejection. No replacement native control API is used.
        let submit = async {
            assert!(
                registered.load(Ordering::SeqCst),
                "register before admission"
            );
            submitted.fetch_add(1, Ordering::SeqCst);
            Err(ConnectError::native(
                "fixture native submission",
                io::Error::from(io::ErrorKind::PermissionDenied),
            ))
        };
        let result = admit_turn(
            &session,
            submit,
            interrupt.as_mut(),
            &approvals,
            &mut Vec::new(),
        )
        .await;
        session.close().await.expect("close without native input");
        match result {
            Err(ConnectError::Native { operation, source }) => {
                assert_eq!(operation, "fixture native submission");
                assert_eq!(
                    source
                        .downcast_ref::<io::Error>()
                        .expect("typed source")
                        .kind(),
                    io::ErrorKind::PermissionDenied
                );
            }
            other => panic!("submission error changed: {other:?}"),
        }
        assert_eq!(submitted.load(Ordering::SeqCst), 1, "no retry");
    })
    .await
    .expect("bounded admission failure control");
}

#[tokio::test]
async fn admission_cancellation_keeps_completed_outcome_and_recorded_exit_intent() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let temp = tempfile::tempdir().expect("temporary realm");
        let session = immediate_session(temp.path()).await;
        let (approvals, _presenter) = LocalApprovals::create(temp.path()).expect("approvals");
        let receipt = session
            .submit("Already completed input".into())
            .await
            .expect("first input");
        // A later completed native input is a deterministic barrier: the
        // original completion is already available without consuming its handle.
        let barrier = session
            .submit("Completion barrier".into())
            .await
            .expect("barrier");
        assert!(matches!(
            barrier.1.wait().await.expect("barrier completion"),
            CompletionOutcome::Completed(_)
        ));
        let (signal_tx, signal_rx) = oneshot::channel();
        let interrupt = async { signal_rx.await.expect("ready race") };
        tokio::pin!(interrupt);
        let submit = async {
            signal_tx
                .send(Ok(()))
                .expect("record interruption with ready receipt");
            Ok(receipt)
        };
        let result = admit_turn(
            &session,
            submit,
            interrupt.as_mut(),
            &approvals,
            &mut Vec::new(),
        )
        .await
        .expect("ready admission race");
        let retired = approvals.bind_session(session.id()).is_err();
        let (interrupted, completion) = match result {
            TurnAdmission::Interrupted(outcome) => (true, outcome),
            TurnAdmission::Running { completion, .. } => {
                (false, completion.try_wait_with_terminal_outcome().await)
            }
            TurnAdmission::NotSubmitted => panic!("receipt was already admitted"),
        };
        session.close().await.expect("close completed session");
        assert!(matches!(
            completion.expect("native completion").outcome(),
            CompletionOutcome::Completed(_)
        ));
        assert!(interrupted, "recorded exit intent lost");
        assert!(retired, "completion cannot restore the invocation");
    })
    .await
    .expect("bounded completion/cancellation race");
}


#[tokio::test]
async fn administrative_close_busy_returns_the_same_live_owner_and_preserves_accepted_work() {
    tokio::time::timeout(Duration::from_secs(30), async {
        use meerkat_connect::session::FinalizationClose;
        let temp = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(Notify::new());
        let storage = RealmOptions { context_root:temp.path().to_owned(),
            state_root:Some(temp.path().join("realms")), user_config_root:Some(temp.path().join("home")) }
            .resolve("quiescent-close").unwrap();
        let session = ConnectSession::open_with_config(storage, Config::default(), SessionOptions {
            model:meerkat::CreateSessionModelResolutionRequest { model:Some("gpt-5.4".into()), ..Default::default() },
            build:SessionBuildOptions { llm_client_override:Some(meerkat::encode_llm_client_override_for_service(
                Arc::new(StalledClient { inner:meerkat_client::TestClient::for_provider(Provider::OpenAI),
                    calls:calls.clone(), started:started.clone() }))), ..Default::default() },
        }).await.unwrap();
        let original = session.id().clone();
        let (active, active_completion) = session.submit("Actual active input".into()).await.unwrap();
        started.notified().await;
        let (queued, queued_completion) = session.submit("Actual queued input".into()).await.unwrap();
        let active_wait = active_completion.try_wait_with_terminal_outcome();
        let queued_wait = queued_completion.try_wait_with_terminal_outcome();
        tokio::pin!(active_wait, queued_wait);
        let (retained, busy, already_cleaned) = match session.try_close_for_finalization().await {
            FinalizationClose::Busy(session) => (Some(session), true, false),
            FinalizationClose::Unconfirmed { session, .. } => (Some(session), false, false),
            FinalizationClose::Closed(closed) => {
                assert!(closed.finish_host_cleanup().await.is_ok()); (None, false, true)
            },
        };
        let mut pending = false;
        let mut same_session = false;
        let mut active_result = None;
        let mut queued_result = None;
        if let Some(session) = retained {
            same_session = session.id() == &original;
            pending = tokio::time::timeout(Duration::from_millis(100), async {
                tokio::select! {
                    result = active_wait.as_mut() => active_result = Some(result),
                    result = queued_wait.as_mut() => queued_result = Some(result),
                }
            }).await.is_err();
            // Explicit fixture cleanup, separate from the rejected automatic
            // close. Release/join both inputs before assertions can panic.
            session.cancel(&queued).await.unwrap();
            session.cancel(&active).await.unwrap();
            if queued_result.is_none() { queued_result = Some(queued_wait.await); }
            if active_result.is_none() { active_result = Some(active_wait.await); }
            session.close().await.unwrap();
        } else {
            queued_result = Some(queued_wait.await);
            active_result = Some(active_wait.await);
        }
        assert!(queued_result.is_some() && active_result.is_some(), "both actual completions were joined exactly once");
        assert!(busy && same_session && pending && !already_cleaned);
        assert_eq!(calls.load(Ordering::SeqCst), 1, "the queued input was not dispatched or abandoned by Busy");
        assert_ne!(input_id(&active), input_id(&queued));
    }).await.expect("native Busy preserves both accepted inputs and joined fixture cleanup");
}

#[tokio::test]
async fn administrative_close_releases_a_quiescent_native_owner() {
    tokio::time::timeout(Duration::from_secs(30), async {
        use meerkat_connect::session::FinalizationClose;
        let temp = tempfile::tempdir().unwrap();
        let session = immediate_session(temp.path()).await;
        let closed = match session.try_close_for_finalization().await {
            FinalizationClose::Closed(closed) => closed.finish_host_cleanup().await.is_ok(),
            FinalizationClose::Busy(session) | FinalizationClose::Unconfirmed { session, .. } => {
                session.close().await.unwrap(); false
            }
        };
        assert!(closed, "native Closed and successful host cleanup are both required");
    }).await.expect("quiescent native cleanup settles");
}
